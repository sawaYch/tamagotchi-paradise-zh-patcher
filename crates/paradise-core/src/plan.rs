use std::collections::{BTreeMap, HashMap, HashSet};

use encoding_rs::BIG5;

use crate::charset::latin_code;
use crate::charset::Charset;
use crate::convert::{Converter, Token, INSERTED};
use crate::glyphpack::GlyphPack;
use crate::strings::StringTable;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotKind {
    Reskin,
    /// A glyph the game already draws correctly, redrawn only to match the chosen font.
    Restyle,
    Overflow,
}

#[derive(Debug, Clone)]
pub struct GlyphEdit {
    pub code: u16,
    pub ch: char,
    pub kind: SlotKind,
}

#[derive(Debug, Clone)]
pub struct Unresolved {
    pub source: char,
    pub target: char,
    pub count: usize,
    pub reason: &'static str,
}

#[derive(Debug, Clone, Default)]
pub struct PlanStats {
    pub strings_total: usize,
    pub strings_changed: usize,
    pub chars_changed: usize,
    pub chars_converted: usize,
    pub reused_codes: usize,
    pub reskinned: usize,
    pub restyled: usize,
    pub overflow: usize,
    /// Shared glyphs left in the game's own style because no spare slot was free.
    pub restyle_skipped: usize,
}

#[derive(Debug, Clone)]
pub struct Plan {
    pub table: StringTable,
    pub edits: Vec<GlyphEdit>,
    pub unresolved: Vec<Unresolved>,
    pub stats: PlanStats,
}

pub struct PlanInput<'a> {
    pub cs: &'a Charset,
    pub pack: &'a GlyphPack,
    pub zh: &'a StringTable,
    pub others: &'a [&'a StringTable],
    /// Redraw characters the game already has so every glyph matches the chosen font.
    pub restyle_shared: bool,
}

fn is_traditional_encodable(c: char) -> bool {
    let mut buf = [0u8; 4];
    let s = c.encode_utf8(&mut buf);
    let (_, _, unmappable) = BIG5.encode(s);
    !unmappable
}

fn codes_used(tables: &[&StringTable]) -> HashSet<u16> {
    tables
        .iter()
        .flat_map(|t| t.strings.iter())
        .flat_map(|s| s.iter().copied())
        .collect()
}

pub fn build_plan(input: &PlanInput, conv: &Converter) -> Plan {
    let cs = input.cs;
    let zh = input.zh;

    let converted: Vec<Vec<Token>> = zh
        .strings
        .iter()
        .enumerate()
        .map(|(i, s)| conv.convert_codes(cs, i + 1, s))
        .collect();

    // target char -> source code -> occurrences
    let mut needs: BTreeMap<char, BTreeMap<u16, usize>> = BTreeMap::new();
    let mut source_counts: HashMap<u16, (usize, usize)> = HashMap::new();
    let mut direct_outputs: BTreeMap<u16, usize> = BTreeMap::new();
    for tokens in &converted {
        for t in tokens {
            let Some(target) = t.target else { continue };
            if t.code == INSERTED {
                *needs
                    .entry(target)
                    .or_default()
                    .entry(INSERTED)
                    .or_default() += 1;
                continue;
            }
            let src = cs.char_of_code(t.code).unwrap();
            let e = source_counts.entry(t.code).or_default();
            if target == src {
                e.0 += 1;
                *direct_outputs.entry(t.code).or_default() += 1;
            } else if let Some(code) = cs.code_of(target) {
                e.1 += 1;
                *direct_outputs.entry(code).or_default() += 1;
            } else {
                *needs.entry(target).or_default().entry(t.code).or_default() += 1;
            }
        }
    }

    let mut protected: HashSet<u16> = codes_used(input.others);
    protected.extend(direct_outputs.keys().copied());
    for (&target, srcs) in &needs {
        if !input.pack.contains(target) {
            protected.extend(srcs.keys().copied());
        }
    }
    let zh_used = codes_used(&[zh]);
    let all_used: HashSet<u16> = protected.union(&zh_used).copied().collect();

    let mut assigned: HashMap<char, u16> = HashMap::new();
    let mut edits: Vec<GlyphEdit> = Vec::new();
    let mut unresolved: Vec<Unresolved> = Vec::new();
    let mut claimed: HashSet<u16> = HashSet::new();

    let mut by_source: BTreeMap<u16, Vec<(char, usize)>> = BTreeMap::new();
    for (&target, srcs) in &needs {
        if !input.pack.contains(target) {
            continue;
        }
        for (&code, &n) in srcs {
            if code == INSERTED {
                continue;
            }
            by_source.entry(code).or_default().push((target, n));
        }
    }
    let mut source_order: Vec<(u16, Vec<(char, usize)>)> = by_source.into_iter().collect();
    for (_, v) in source_order.iter_mut() {
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    }
    source_order.sort_by(|a, b| {
        let ta: usize = a.1.iter().map(|x| x.1).sum();
        let tb: usize = b.1.iter().map(|x| x.1).sum();
        tb.cmp(&ta).then(a.0.cmp(&b.0))
    });

    for (code, targets) in &source_order {
        let src = cs.char_of_code(*code).unwrap();
        let eligible = !protected.contains(code)
            && !claimed.contains(code)
            && cs.is_hanzi_code(*code)
            && !is_traditional_encodable(src);
        if !eligible {
            continue;
        }
        if let Some(&(t, _)) = targets.iter().find(|(t, _)| !assigned.contains_key(t)) {
            assigned.insert(t, *code);
            claimed.insert(*code);
            edits.push(GlyphEdit {
                code: *code,
                ch: t,
                kind: SlotKind::Reskin,
            });
        }
    }

    let mut pool: Vec<u16> = Vec::new();
    for i in 0..cs.hangul_count() {
        let code = crate::charset::CODE_BASE + i as u16;
        if !all_used.contains(&code) && !claimed.contains(&code) {
            pool.push(code);
        }
    }
    for i in cs.hanzi_start()..cs.len() {
        let code = crate::charset::CODE_BASE + i as u16;
        let c = cs.char_of_code(code).unwrap();
        if !all_used.contains(&code) && !claimed.contains(&code) && !is_traditional_encodable(c) {
            pool.push(code);
        }
    }
    let mut pool_iter = pool.into_iter();

    for (&target, srcs) in &needs {
        if assigned.contains_key(&target) {
            continue;
        }
        let count: usize = srcs.values().sum();
        let first_src = srcs
            .keys()
            .find_map(|&c| cs.char_of_code(c))
            .unwrap_or(target);
        if !input.pack.contains(target) {
            unresolved.push(Unresolved {
                source: first_src,
                target,
                count,
                reason: "no glyph available",
            });
            continue;
        }
        match pool_iter.next() {
            Some(code) => {
                assigned.insert(target, code);
                edits.push(GlyphEdit {
                    code,
                    ch: target,
                    kind: SlotKind::Overflow,
                });
            }
            None => unresolved.push(Unresolved {
                source: first_src,
                target,
                count,
                reason: "no free glyph slot",
            }),
        }
    }

    let mut moved: HashMap<u16, u16> = HashMap::new();
    let mut restyle_skipped = 0;
    if input.restyle_shared {
        let used_elsewhere = codes_used(input.others);
        let mut shared: Vec<(u16, usize)> = direct_outputs
            .iter()
            .map(|(&code, &n)| (code, n))
            .filter(|&(code, _)| cs.is_hanzi_code(code))
            .collect();
        shared.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        for (code, _) in shared {
            let Some(ch) = cs.char_of_code(code).filter(|&c| input.pack.contains(c)) else {
                continue;
            };
            if !used_elsewhere.contains(&code) {
                edits.push(GlyphEdit {
                    code,
                    ch,
                    kind: SlotKind::Restyle,
                });
            } else if let Some(slot) = pool_iter.next() {
                moved.insert(code, slot);
                edits.push(GlyphEdit {
                    code: slot,
                    ch,
                    kind: SlotKind::Overflow,
                });
            } else {
                restyle_skipped += 1;
            }
        }
    }
    let remap = |c: u16| moved.get(&c).copied().unwrap_or(c);

    let mut stats = PlanStats {
        strings_total: zh.strings.len(),
        restyle_skipped,
        ..Default::default()
    };
    let mut out_strings = Vec::with_capacity(zh.strings.len());
    for (orig, tokens) in zh.strings.iter().zip(&converted) {
        let mut out = Vec::with_capacity(orig.len());
        let mut changed = false;
        for t in tokens {
            let new_code = match t.target {
                None => t.code,
                Some(target) if t.code == INSERTED => assigned
                    .get(&target)
                    .copied()
                    .unwrap_or_else(|| latin_code('?').expect("question mark")),
                Some(target) => {
                    let src = cs.char_of_code(t.code).unwrap();
                    if target == src {
                        remap(t.code)
                    } else if let Some(c) = cs.code_of(target) {
                        stats.reused_codes += 1;
                        remap(c)
                    } else if let Some(&c) = assigned.get(&target) {
                        c
                    } else {
                        t.code
                    }
                }
            };
            if new_code != t.code {
                changed = true;
                stats.chars_changed += 1;
            }
            if matches!(t.target, Some(target) if cs.char_of_code(t.code) != Some(target)) {
                stats.chars_converted += 1;
            }
            out.push(new_code);
        }
        if changed || out != *orig {
            stats.strings_changed += 1;
        }
        out_strings.push(out);
    }
    stats.reskinned = edits.iter().filter(|e| e.kind == SlotKind::Reskin).count();
    stats.restyled = edits.iter().filter(|e| e.kind == SlotKind::Restyle).count();
    stats.overflow = edits
        .iter()
        .filter(|e| e.kind == SlotKind::Overflow)
        .count();
    edits.sort_by_key(|e| e.code);

    Plan {
        table: StringTable {
            strings: out_strings,
        },
        edits,
        unresolved,
        stats,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::convert::Variant;

    fn encode(cs: &Charset, s: &str) -> Vec<u16> {
        let mut v: Vec<u16> = s.chars().map(|c| cs.code_of(c).unwrap()).collect();
        v.push(0);
        v
    }

    #[test]
    fn reskins_pure_simplified_and_reuses_shared() {
        let cs = Charset::standard();
        let pack = GlyphPack::embedded();
        let conv = Converter::new(Variant::HongKong).unwrap();
        let zh = StringTable {
            strings: vec![encode(&cs, "个们"), encode(&cs, "国")],
        };
        let plan = build_plan(
            &PlanInput {
                cs: &cs,
                pack,
                zh: &zh,
                others: &[],
                restyle_shared: false,
            },
            &conv,
        );
        assert!(plan.unresolved.is_empty());
        assert_eq!(plan.table.strings[0], zh.strings[0]);
        let chars: Vec<char> = plan.edits.iter().map(|e| e.ch).collect();
        assert!(chars.contains(&'個') && chars.contains(&'們') && chars.contains(&'國'));
        assert!(plan.edits.iter().all(|e| e.kind == SlotKind::Reskin));
    }

    fn plan_with_restyle(cs: &Charset, zh: &StringTable, others: &[&StringTable]) -> Plan {
        let conv = Converter::new(Variant::HongKong).unwrap();
        build_plan(
            &PlanInput {
                cs,
                pack: GlyphPack::embedded(),
                zh,
                others,
                restyle_shared: true,
            },
            &conv,
        )
    }

    #[test]
    fn shared_characters_are_redrawn_in_place() {
        let cs = Charset::standard();
        let zh = StringTable {
            strings: vec![encode(&cs, "定哦")],
        };
        let plan = plan_with_restyle(&cs, &zh, &[]);
        assert_eq!(plan.table.strings, zh.strings);
        let mut chars: Vec<char> = plan.edits.iter().map(|e| e.ch).collect();
        chars.sort();
        assert_eq!(chars, vec!['哦', '定']);
        assert!(plan.edits.iter().all(|e| e.kind == SlotKind::Restyle));
        assert_eq!(plan.stats.restyled, 2);
    }

    #[test]
    fn shared_characters_used_by_other_languages_move_to_spare_slots() {
        let cs = Charset::standard();
        let zh = StringTable {
            strings: vec![encode(&cs, "定哦")],
        };
        let other = StringTable {
            strings: vec![encode(&cs, "定")],
        };
        let plan = plan_with_restyle(&cs, &zh, &[&other]);
        assert_eq!(plan.table.strings[0][1], zh.strings[0][1]);
        assert_ne!(plan.table.strings[0][0], zh.strings[0][0]);
        assert!(cs.is_hangul_code(plan.table.strings[0][0]));
        let moved = plan.edits.iter().find(|e| e.ch == '定').unwrap();
        assert_eq!(moved.kind, SlotKind::Overflow);
        assert_eq!(moved.code, plan.table.strings[0][0]);
    }

    #[test]
    fn protected_source_uses_overflow_slot() {
        let cs = Charset::standard();
        let pack = GlyphPack::embedded();
        let conv = Converter::new(Variant::HongKong).unwrap();
        let zh = StringTable {
            strings: vec![encode(&cs, "个")],
        };
        let other = StringTable {
            strings: vec![encode(&cs, "个")],
        };
        let plan = build_plan(
            &PlanInput {
                cs: &cs,
                pack,
                zh: &zh,
                others: &[&other],
                restyle_shared: false,
            },
            &conv,
        );
        assert_eq!(plan.edits.len(), 1);
        assert_eq!(plan.edits[0].kind, SlotKind::Overflow);
        assert_ne!(plan.table.strings[0][0], zh.strings[0][0]);
        assert!(cs.is_hangul_code(plan.table.strings[0][0]));
    }
}

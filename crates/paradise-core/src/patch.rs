use std::collections::{BTreeMap, BTreeSet, HashMap};

use encoding_rs::BIG5;

use crate::charset::{latin_code, Charset};
use crate::convert::{Converter, Variant};
use crate::error::{Error, Result};
use crate::glyphpack::{FontChoice, GlyphPack};
use crate::mapping::Mapping;
use crate::plan::{build_plan, GlyphEdit, Plan, PlanInput, PlanStats, SlotKind, Unresolved};
use crate::rom::Rom;
use crate::runtime_lang::{self, Status};
use crate::sprite::{FontSprite, FOREGROUND};
use crate::strings::StringTable;

#[derive(Debug, Clone)]
pub struct PatchOptions {
    pub variant: Variant,
    pub font: FontChoice,
    pub runtime_language: bool,
    /// Redraw characters shared by Simplified and Traditional in the chosen font.
    pub redraw_shared: bool,
    /// Redraw the game's Latin letters, digits and `+-!?&` in the chosen font. This also
    /// changes the text of the other languages.
    pub redraw_latin: bool,
    pub mapping: Option<Mapping>,
}

impl Default for PatchOptions {
    fn default() -> Self {
        PatchOptions {
            variant: Variant::default(),
            font: FontChoice::default(),
            runtime_language: true,
            redraw_shared: true,
            redraw_latin: true,
            mapping: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeLanguage {
    Disabled,
    Added,
    AlreadyPresent,
    Unsupported,
}

#[derive(Debug, Clone)]
pub struct PatchReport {
    pub stats: PlanStats,
    pub edits: Vec<GlyphEdit>,
    pub unresolved: Vec<Unresolved>,
    pub remaining_simplified: Vec<(char, usize)>,
    pub asset_len_before: usize,
    pub asset_len_after: usize,
    pub font_grew: bool,
    pub fallback_glyphs: usize,
    /// Number of Latin glyphs redrawn in the chosen font.
    pub latin_glyphs: usize,
    /// Each mapping key with the number of times it matched.
    pub mapping_hits: Vec<(String, usize)>,
    pub runtime_language: RuntimeLanguage,
}

pub struct Patched {
    pub bytes: Vec<u8>,
    pub report: PatchReport,
}

/// Legacy firmware index of the Chinese language option (zero-based).
pub const LANGUAGE_NAME_ZH: usize = 93;

/// Find the script name on the language screen. Its string ID varies by firmware.
fn language_name_index(tables: &[StringTable], zh: usize, cs: &Charset) -> Option<usize> {
    let [Some(zhong), Some(wen), Some(jian), Some(ti)] =
        ['中', '文', '简', '体'].map(|c| cs.code_of(c))
    else {
        return None;
    };
    // Glyph codes 181 and 182 are the full-width parentheses shown in this menu.
    let name = [zhong, wen, 181, jian, ti, 182, 0];
    tables[zh].strings.iter().enumerate().find_map(|(i, row)| {
        (row == &name && tables.iter().all(|t| t.strings.get(i) == Some(row))).then_some(i)
    })
}

/// Turns "中文（简体）" into "中文（繁体）" so the converted entry names the new script.
fn relabel_traditional(name: &mut [u16], cs: &Charset, font: &FontSprite) {
    let (Some(from), Some(to)) = (cs.code_of('简'), cs.code_of('繁')) else {
        return;
    };
    if font.is_blank(Charset::glyph_of_code(to)) {
        return;
    }
    for c in name {
        if *c == from {
            *c = to;
        }
    }
}

fn already_traditional(tables: &[StringTable], zh: usize, cs: &Charset) -> bool {
    let Some(traditional) = cs.code_of('繁') else {
        return false;
    };
    tables[zh].strings.iter().enumerate().any(|(i, row)| {
        matches!(row.as_slice(), [_, _, 181, c, _, 182, 0] if *c == traditional)
            && tables.iter().all(|t| t.strings.get(i) == Some(row))
    })
}

/// Existing patches can store Traditional characters in repurposed CJK or
/// Hangul slots. Recover their identity by exact bitmap matches before redrawing.
fn restyle_converted(
    zh: &StringTable,
    cs: &Charset,
    font: &FontSprite,
    pack: &GlyphPack,
    redraw: bool,
) -> Plan {
    let mut edits = Vec::new();
    let mut stats = PlanStats {
        strings_total: zh.strings.len(),
        ..Default::default()
    };
    if redraw {
        let mut known = HashMap::<[u8; 32], BTreeSet<char>>::new();
        for choice in FontChoice::ALL {
            for variant in [Variant::HongKong, Variant::Taiwan, Variant::Standard] {
                for (ch, bitmap) in GlyphPack::for_font(choice, variant).bitmaps() {
                    known.entry(bitmap).or_default().insert(ch);
                }
            }
        }
        let codes: BTreeSet<_> = zh
            .strings
            .iter()
            .flatten()
            .copied()
            .filter(|&code| cs.is_hanzi_code(code) || cs.is_hangul_code(code))
            .collect();
        for code in codes {
            let pixels = font.glyph_pixels(Charset::glyph_of_code(code));
            let mut mask = [0u8; 32];
            for (i, &pixel) in pixels.iter().enumerate() {
                if pixel == FOREGROUND {
                    mask[i / 8] |= 0x80 >> (i % 8);
                }
            }
            let matched = known
                .get(&mask)
                .filter(|_| pixels.iter().all(|&p| p == 0 || p == FOREGROUND));
            let ch = matched.and_then(|chars| {
                cs.char_of_code(code)
                    .filter(|ch| chars.contains(ch))
                    .or_else(|| (chars.len() == 1).then(|| *chars.first().unwrap()))
            });
            if let Some(ch) = ch.filter(|&ch| pack.contains(ch)) {
                edits.push(GlyphEdit {
                    code,
                    ch,
                    kind: SlotKind::Restyle,
                });
            } else {
                stats.unidentified_glyphs += 1;
            }
        }
    }
    stats.restyled = edits.len();
    Plan {
        table: zh.clone(),
        edits,
        unresolved: Vec::new(),
        stats,
    }
}

fn simplified_only(c: char) -> bool {
    let mut buf = [0u8; 4];
    let (_, _, unmappable) = BIG5.encode(c.encode_utf8(&mut buf));
    unmappable
}

fn paramix_label_strings(count: usize) -> Vec<Vec<u16>> {
    let mut label: Vec<u16> = "PARAMIX".chars().map(|c| latin_code(c).unwrap()).collect();
    label.push(0);
    vec![label; count]
}

/// Shared Settings rows use the same zero-based string-table index in every
/// language. Check all tables before mutating any, and retain existing labels.
fn append_settings_label(tables: &mut [StringTable], labels: Vec<Vec<u16>>) -> Result<u32> {
    let count = tables
        .first()
        .ok_or_else(|| Error::Format("missing language tables".into()))?
        .strings
        .len();
    if labels.len() != tables.len() || tables.iter().any(|t| t.strings.len() != count) {
        return Err(Error::Unsupported(
            "Settings labels require equally sized language tables".into(),
        ));
    }
    if tables.iter().zip(&labels).any(|(t, label)| {
        t.strings.len() + 3 + t.strings.iter().map(|s| s.len()).sum::<usize>() + label.len()
            > u16::MAX as usize
    }) {
        return Err(Error::Capacity(
            "adding a Settings label would exceed a language table's 64K-word limit".into(),
        ));
    }
    for (table, label) in tables.iter_mut().zip(labels) {
        table.strings.push(label);
    }
    Ok(count as u32)
}

pub fn patch(rom: &Rom, opts: &PatchOptions, progress: &mut dyn FnMut(&str)) -> Result<Patched> {
    let cs = Charset::standard();
    let pack = GlyphPack::for_font(opts.font, opts.variant);
    let info = rom.info();

    progress("Loading conversion dictionaries");
    let conv =
        Converter::with_mapping(opts.variant, opts.mapping.clone().unwrap_or_default(), &cs)?;

    progress("Reading string tables");
    let mut tables = rom.all_tables()?;
    let same_len = tables
        .iter()
        .all(|t| t.strings.len() == tables[0].strings.len());
    let runtime = match (opts.runtime_language, runtime_lang::status(rom.bytes())) {
        (false, _) => RuntimeLanguage::Disabled,
        (true, Status::Applied) => RuntimeLanguage::AlreadyPresent,
        (true, Status::Original) if same_len => RuntimeLanguage::Added,
        (true, _) => RuntimeLanguage::Unsupported,
    };
    let existing = runtime_lang::settings_labels(rom.bytes());
    let mut language_label = existing.as_ref().and_then(|s| s.language);
    let mut paramix_label = existing.as_ref().and_then(|s| s.paramix);
    for index in [language_label, paramix_label].into_iter().flatten() {
        if tables.iter().any(|t| index as usize >= t.strings.len()) {
            return Err(Error::Unsupported(
                "installed Settings helper refers to a missing string label".into(),
            ));
        }
    }
    let mut settings_changed = false;
    if runtime == RuntimeLanguage::Added {
        language_label = Some(append_settings_label(
            &mut tables,
            runtime_lang::label_strings(&cs),
        )?);
        settings_changed = true;
    }
    if opts.runtime_language
        && runtime != RuntimeLanguage::Unsupported
        && paramix_label.is_none()
        && crate::paramix::requested_shortcut_stub(rom.bytes())?.is_some()
    {
        let labels = paramix_label_strings(tables.len());
        paramix_label = Some(append_settings_label(&mut tables, labels)?);
        settings_changed = true;
    }
    let mut font = rom.font()?;
    let name_index = language_name_index(&tables, info.zh_table, &cs);
    if let Some(i) = name_index {
        relabel_traditional(&mut tables[info.zh_table].strings[i], &cs, &font);
    }
    let zh = &tables[info.zh_table];
    let pending_names = crate::neon_names::pending(rom, zh.strings.len())?;
    let others: Vec<&StringTable> = tables
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != info.zh_table)
        .map(|(_, t)| t)
        .collect();

    progress("Converting text and planning glyphs");
    let repatch = font.uses_legacy_lz4() && already_traditional(&tables, info.zh_table, &cs);
    if repatch && pending_names.is_none() && opts.mapping.as_ref().is_some_and(|m| !m.is_empty()) {
        return Err(Error::Unsupported(
            "text mappings need the pre-translation ROM because an existing patch reuses glyph codes; font changes can use this ROM directly".into(),
        ));
    }
    let plan = if repatch {
        if let Some(names) = &pending_names {
            let (mut retained, mut incoming) = (zh.clone(), zh.clone());
            for i in 0..zh.strings.len() {
                if names.contains(&i) {
                    retained.strings[i] = vec![0];
                } else {
                    incoming.strings[i] = vec![0];
                }
            }
            let mut protected = others.clone();
            protected.push(&retained);
            let mut plan = build_plan(
                &PlanInput {
                    cs: &cs,
                    pack,
                    zh: &incoming,
                    others: &protected,
                    restyle_shared: true,
                },
                &conv,
            );
            let existing = restyle_converted(&retained, &cs, &font, pack, opts.redraw_shared);
            plan.stats.restyled += existing.stats.restyled;
            plan.stats.unidentified_glyphs += existing.stats.unidentified_glyphs;
            plan.edits.extend(existing.edits);
            for (i, row) in plan.table.strings.iter_mut().enumerate() {
                if !names.contains(&i) {
                    row.clone_from(&zh.strings[i]);
                }
            }
            plan
        } else {
            restyle_converted(zh, &cs, &font, pack, opts.redraw_shared)
        }
    } else {
        build_plan(
            &PlanInput {
                cs: &cs,
                pack,
                zh,
                others: &others,
                restyle_shared: opts.redraw_shared,
            },
            &conv,
        )
    };
    if !plan.table.fits_u16() {
        return Err(Error::Capacity(
            "converted string table exceeds 64K words".into(),
        ));
    }

    progress("Drawing glyphs");
    for e in &plan.edits {
        let px = pack
            .render(e.ch)
            .ok_or_else(|| Error::Format(format!("missing glyph for {}", e.ch)))?;
        font.set_glyph(Charset::glyph_of_code(e.code), &px);
    }
    let mut latin_glyphs = Vec::new();
    if opts.redraw_latin {
        let latin = GlyphPack::latin_for(opts.font);
        for c in (' '..='z').filter(|&c| c != ' ') {
            let Some(code) = latin_code(c) else { continue };
            let px = latin
                .render_latin(c)
                .ok_or_else(|| Error::Format(format!("missing Latin glyph for {c}")))?;
            let glyph = Charset::glyph_of_code(code);
            font.set_glyph(glyph, &px);
            latin_glyphs.push(glyph);
        }
    }

    // Keep all sprite addresses stable wherever a Paramix catalog may refer
    // to them. The Neon font slot is smaller, so permit growth for an oversized
    // font if the ROM has no such catalog and the archive allocator finds space.
    progress("Compressing glyphs");
    let allow_font_growth = (info.edition == Some(7) || font.uses_legacy_lz4())
        && !crate::paramix::uses_sprite_addresses(rom.bytes());
    font.compact(
        rom.sprite_blobs()[info.font_sprite].len(),
        allow_font_growth,
    )?;

    progress("Rebuilding assets");
    let mut patched = Rom::load(rom.bytes().to_vec())?;
    patched.set_table(info.zh_table, &plan.table);
    for (i, t) in tables.iter_mut().enumerate() {
        if i == info.zh_table {
            continue;
        }
        if let Some(i) = name_index {
            t.strings[i] = plan.table.strings[i].clone();
        }
        patched.set_table(i, t);
    }
    patched.set_font(&font);
    let mut bytes = patched.to_bytes()?;
    if pending_names.is_some() {
        crate::neon_names::mark_converted(&mut bytes);
    }
    if settings_changed {
        runtime_lang::apply_shared(&mut bytes, language_label, paramix_label)?;
    }
    let asset_len_after = patched.rebuilt_asset_len();

    let mut remaining: BTreeMap<char, usize> = BTreeMap::new();
    for (_, s) in plan.table.strings.iter().enumerate().filter(|(i, _)| {
        !repatch
            || pending_names
                .as_ref()
                .is_some_and(|names| names.contains(i))
    }) {
        for &c in s {
            if cs.is_hanzi_code(c) {
                let ch = cs.char_of_code(c).unwrap();
                if simplified_only(ch) {
                    *remaining.entry(ch).or_default() += 1;
                }
            }
        }
    }
    let reskinned: std::collections::HashSet<u16> = plan.edits.iter().map(|e| e.code).collect();
    let remaining_simplified: Vec<(char, usize)> = remaining
        .into_iter()
        .filter(|(c, _)| {
            cs.code_of(*c)
                .map(|code| !reskinned.contains(&code))
                .unwrap_or(true)
        })
        .collect();

    let fallback_glyphs = plan.edits.iter().filter(|e| pack.is_fallback(e.ch)).count();
    let report = PatchReport {
        stats: plan.stats,
        edits: plan.edits,
        unresolved: plan.unresolved,
        remaining_simplified,
        asset_len_before: info.asset_len,
        asset_len_after,
        fallback_glyphs,
        latin_glyphs: latin_glyphs.len(),
        mapping_hits: conv.mapping_hits(),
        font_grew: font.grew(),
        runtime_language: runtime,
    };

    progress("Verifying output");
    let mut issues = verify_with(rom.bytes(), &bytes, opts.mapping.as_ref())?;
    let allowed: std::collections::HashSet<usize> = report
        .edits
        .iter()
        .map(|e| Charset::glyph_of_code(e.code))
        .chain(latin_glyphs)
        .collect();
    let after = Rom::load(bytes.clone())?;
    let stray = changed_glyphs(rom, &after)?
        .into_iter()
        .filter(|g| !allowed.contains(g))
        .count();
    if stray > 0 {
        issues.push(format!("{stray} unplanned glyphs changed"));
    }
    if !issues.is_empty() {
        return Err(Error::Verify(issues.join("; ")));
    }
    Ok(Patched { bytes, report })
}

/// Checks that `patched` is a structurally valid ROM that differs from `original`
/// only where a patch is expected to. Returns the list of problems found.
pub fn verify(original: &[u8], patched: &[u8]) -> Result<Vec<String>> {
    verify_with(original, patched, None)
}

/// Like verify, also accepting the text changes a mapping makes to the Chinese table.
pub fn verify_with(
    original: &[u8],
    patched: &[u8],
    mapping: Option<&Mapping>,
) -> Result<Vec<String>> {
    let a = Rom::load(original.to_vec())?;
    let b = match Rom::load(patched.to_vec()) {
        Ok(b) => b,
        Err(e) => return Ok(vec![format!("patched ROM does not parse: {e}")]),
    };
    let cs = Charset::standard();
    let mut issues = Vec::new();
    let (ia, ib) = (a.info(), b.info());

    if original.len() != patched.len() {
        issues.push("ROM size changed".into());
    }
    if !ib.top_checksum_ok || !ib.strings_checksum_ok {
        issues.push("archive checksum mismatch".into());
    }
    let (ta, tb) = (a.all_tables()?, b.all_tables()?);
    let original_settings = runtime_lang::settings_labels(original);
    let patched_settings = runtime_lang::settings_labels(patched);
    let original_language = original_settings.as_ref().and_then(|s| s.language);
    let patched_language = patched_settings.as_ref().and_then(|s| s.language);
    let original_paramix = original_settings.as_ref().and_then(|s| s.paramix);
    let patched_paramix = patched_settings.as_ref().and_then(|s| s.paramix);
    if original_language.is_some() && original_language != patched_language {
        issues.push("existing Settings language row or label changed".into());
    }
    if original_paramix.is_some() && original_paramix != patched_paramix {
        issues.push("existing Settings PARAMIX row or label changed".into());
    }
    let mut appended = Vec::new();
    if original_language.is_none() {
        if let Some(index) = patched_language {
            appended.push((index, runtime_lang::label_strings(&cs)));
        }
    }
    if original_paramix.is_none() {
        if let Some(index) = patched_paramix {
            appended.push((index, paramix_label_strings(ta.len())));
        }
    }
    appended.sort_by_key(|(index, _)| *index);
    for (offset, (index, _)) in appended.iter().enumerate() {
        if *index as usize != ta[0].strings.len() + offset {
            issues.push("new Settings label indexes are not contiguous appended strings".into());
        }
    }
    for index in [patched_language, patched_paramix].into_iter().flatten() {
        if tb.iter().any(|t| index as usize >= t.strings.len()) {
            issues.push("Settings helper refers to a missing string label".into());
        }
    }
    let head = crate::rom::ASSET_BASE
        .min(original.len())
        .min(patched.len());
    let mut expected_head = original.to_vec();
    let original_pending = crate::neon_names::pending(&a, ta[ia.zh_table].strings.len())?;
    if original_pending.is_some() && crate::neon_names::converted(patched) {
        crate::neon_names::mark_converted(&mut expected_head);
    }
    if expected_head[..head] != patched[..head]
        && runtime_lang::reproduce_settings(&mut expected_head, patched).is_err()
    {
        issues.push("shared Settings patch could not be reproduced".into());
    }
    if expected_head[..head] != patched[..head] {
        issues.push("data before the asset archive changed".into());
    }
    let tail = (crate::rom::ASSET_BASE + ia.asset_len.max(ib.asset_len)).min(original.len());
    if original.len() == patched.len() && original[tail..] != patched[tail..] {
        issues.push("data after the asset archive changed".into());
    }
    if ia.zh_table != ib.zh_table || ia.font_sprite != ib.font_sprite {
        issues.push("table or font location changed".into());
    }
    if crate::paramix::uses_sprite_addresses(original)
        && a.sprite_blobs()[ia.font_sprite].len() != b.sprite_blobs()[ib.font_sprite].len()
    {
        issues.push("font extent changed with an installed Paramix catalog".into());
    }
    if a.data_pack() != b.data_pack() {
        issues.push("data pack changed".into());
    }
    let original_archive = &patched[crate::rom::ASSET_BASE..crate::rom::ASSET_BASE + ib.asset_len];
    let mut repaired = original_archive.to_vec();
    let rain_sites = crate::six_rain::relocate(original, &mut repaired)?;
    if !rain_sites.is_empty() && repaired != original_archive {
        issues.push("external SIXB rain pointers do not match the rebuilt archive".into());
    }
    for (i, (x, y)) in a.sprite_blobs().iter().zip(b.sprite_blobs()).enumerate() {
        if i != ia.font_sprite && x != y {
            let mut expected = x.clone();
            if let Some(sites) = rain_sites.get(&i) {
                for &at in sites {
                    if at + 4 <= expected.len() && at + 4 <= y.len() {
                        expected[at..at + 4].copy_from_slice(&y[at..at + 4]);
                    }
                }
            }
            if expected != *y {
                issues.push(format!("sprite {i} changed"));
            }
        }
    }
    let extra = appended.len();
    let name_index = language_name_index(&ta, ia.zh_table, &cs);
    for (i, (x, y)) in ta.iter().zip(&tb).enumerate() {
        if i == ia.zh_table {
            continue;
        }
        let n = x.strings.len();
        let same = y.strings.len() == n + extra
            && x.strings
                .iter()
                .zip(&y.strings)
                .enumerate()
                .all(|(i, (a, b))| {
                    a == b || (name_index == Some(i) && tb[ib.zh_table].strings.get(i) == Some(b))
                });
        if !same {
            issues.push(format!("string table {i} changed"));
        } else {
            for (offset, (_, labels)) in appended.iter().enumerate() {
                if y.strings[n + offset] != labels[i] {
                    issues.push(format!("string table {i} has an unexpected Settings label"));
                }
            }
        }
    }
    let (za, zb) = (&ta[ia.zh_table], &tb[ib.zh_table]);
    if zb.strings.len() != za.strings.len() + extra {
        issues.push("string count changed".into());
    } else {
        for (i, (x, y)) in za.strings.iter().zip(&zb.strings).enumerate() {
            if mapping.is_some_and(|m| m.applies_to(&cs, x, i + 1)) {
                continue;
            }
            if x.len() != y.len() {
                issues.push(format!("string {} length changed", i + 1));
                continue;
            }
            for (&p, &q) in x.iter().zip(y) {
                let glyph_code = |c: u16| cs.is_hanzi_code(c) || cs.is_hangul_code(c);
                if p != q && !(cs.is_hanzi_code(p) && glyph_code(q)) {
                    issues.push(format!("string {} changed a non-hanzi code", i + 1));
                    break;
                }
            }
        }
    }
    if zb.strings.len() == za.strings.len() + extra {
        for (offset, (_, labels)) in appended.iter().enumerate() {
            let index = za.strings.len() + offset;
            let (x, y) = (&labels[ia.zh_table], &zb.strings[index]);
            if mapping.is_some_and(|m| m.applies_to(&cs, x, index + 1)) {
                continue;
            }
            if x.len() != y.len()
                || x.iter().zip(y).any(|(&p, &q)| {
                    p != q
                        && !(cs.is_hanzi_code(p) && (cs.is_hanzi_code(q) || cs.is_hangul_code(q)))
                })
            {
                issues.push("Chinese table has an unexpected Settings label".into());
            }
        }
    }
    let fb = b.font()?;
    if fb.glyph_count() != cs.glyph_total() {
        issues.push("font glyph count changed".into());
    }
    let mut blank_used = std::collections::BTreeSet::new();
    for s in &zb.strings {
        for &c in s {
            if (cs.is_hanzi_code(c) || cs.is_hangul_code(c))
                && fb.is_blank(Charset::glyph_of_code(c))
            {
                blank_used.insert(c);
            }
        }
    }
    if !blank_used.is_empty() {
        issues.push(format!(
            "{} used hanzi codes have blank glyphs",
            blank_used.len()
        ));
    }
    Ok(issues)
}

pub fn changed_glyphs(original: &Rom, patched: &Rom) -> Result<Vec<usize>> {
    let (fa, fb) = (original.font()?, patched.font()?);
    Ok((0..fa.glyph_count().min(fb.glyph_count()))
        .filter(|&i| fa.glyph_pixels(i) != fb.glyph_pixels(i))
        .collect())
}

#[cfg(test)]
mod settings_label_tests {
    use super::*;

    #[test]
    fn existing_paramix_label_is_preserved_when_language_is_appended() {
        let mut tables = vec![
            StringTable {
                strings: vec![vec![318, 0]]
            };
            9
        ];
        let paramix = append_settings_label(&mut tables, paramix_label_strings(9)).unwrap();
        let before = tables.clone();
        let language = append_settings_label(
            &mut tables,
            runtime_lang::label_strings(&Charset::standard()),
        )
        .unwrap();
        assert_eq!((paramix, language), (1, 2));
        for (old, new) in before.iter().zip(&tables) {
            assert_eq!(old.strings, new.strings[..2]);
            assert_eq!(new.strings[paramix as usize], paramix_label_strings(9)[0]);
        }
    }

    #[test]
    fn mismatched_or_full_language_tables_remain_unchanged() {
        let mut tables = vec![
            StringTable {
                strings: vec![vec![318, 0]]
            };
            9
        ];
        tables[3].strings.push(vec![319, 0]);
        let before = tables.clone();
        assert!(append_settings_label(&mut tables, paramix_label_strings(9)).is_err());
        assert_eq!(tables, before);
        let mut tables = vec![
            StringTable {
                strings: vec![vec![0; 65531]]
            };
            9
        ];
        assert!(tables.iter().all(StringTable::fits_u16));
        let before = tables.clone();
        assert!(append_settings_label(&mut tables, paramix_label_strings(9)).is_err());
        assert_eq!(tables, before);
    }
}

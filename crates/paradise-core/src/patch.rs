use std::collections::BTreeMap;

use encoding_rs::BIG5;

use crate::charset::Charset;
use crate::convert::{Converter, Variant};
use crate::error::{Error, Result};
use crate::glyphpack::{FontChoice, GlyphPack};
use crate::mapping::Mapping;
use crate::plan::{build_plan, GlyphEdit, PlanInput, PlanStats, Unresolved};
use crate::rom::Rom;
use crate::runtime_lang::{self, Status};
use crate::sprite::FontSprite;
use crate::strings::StringTable;

#[derive(Debug, Clone)]
pub struct PatchOptions {
    pub variant: Variant,
    pub font: FontChoice,
    pub runtime_language: bool,
    /// Redraw characters shared by Simplified and Traditional in the chosen font.
    pub redraw_shared: bool,
    pub mapping: Option<Mapping>,
}

impl Default for PatchOptions {
    fn default() -> Self {
        PatchOptions {
            variant: Variant::default(),
            font: FontChoice::default(),
            runtime_language: true,
            redraw_shared: true,
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
    /// Each mapping key with the number of times it matched.
    pub mapping_hits: Vec<(String, usize)>,
    pub runtime_language: RuntimeLanguage,
}

pub struct Patched {
    pub bytes: Vec<u8>,
    pub report: PatchReport,
}

/// Index of the "中文（简体）" entry shown on the language screen. Every language table
/// carries its own copy, so the Chinese table's converted copy is mirrored into the others.
pub const LANGUAGE_NAME_ZH: usize = 93;

fn language_name_shared(tables: &[StringTable], zh: usize) -> bool {
    tables
        .iter()
        .all(|t| t.strings.get(LANGUAGE_NAME_ZH) == tables[zh].strings.get(LANGUAGE_NAME_ZH))
        && tables[zh].strings.len() > LANGUAGE_NAME_ZH
}

/// Turns "中文（简体）" into "中文（繁体）" so the converted entry names the new script.
fn relabel_traditional(zh: &mut StringTable, cs: &Charset, font: &FontSprite) {
    let (Some(from), Some(to)) = (cs.code_of('简'), cs.code_of('繁')) else {
        return;
    };
    if font.is_blank(Charset::glyph_of_code(to)) {
        return;
    }
    for c in zh.strings[LANGUAGE_NAME_ZH].iter_mut() {
        if *c == from {
            *c = to;
        }
    }
}

fn simplified_only(c: char) -> bool {
    let mut buf = [0u8; 4];
    let (_, _, unmappable) = BIG5.encode(c.encode_utf8(&mut buf));
    unmappable
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
    let label_index = tables[0].strings.len() as u32;
    if runtime == RuntimeLanguage::Added {
        for (t, label) in tables.iter_mut().zip(runtime_lang::label_strings(&cs)) {
            t.strings.push(label);
        }
    }
    let sync_name = language_name_shared(&tables, info.zh_table);
    if sync_name {
        relabel_traditional(&mut tables[info.zh_table], &cs, &rom.font()?);
    }
    let zh = &tables[info.zh_table];
    let others: Vec<&StringTable> = tables
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != info.zh_table)
        .map(|(_, t)| t)
        .collect();

    progress("Converting text and planning glyphs");
    let plan = build_plan(
        &PlanInput {
            cs: &cs,
            pack,
            zh,
            others: &others,
            restyle_shared: opts.redraw_shared,
        },
        &conv,
    );
    if !plan.table.fits_u16() {
        return Err(Error::Capacity(
            "converted string table exceeds 64K words".into(),
        ));
    }

    progress("Drawing glyphs");
    let mut font = rom.font()?;
    for e in &plan.edits {
        let px = pack
            .render(e.ch)
            .ok_or_else(|| Error::Format(format!("missing glyph for {}", e.ch)))?;
        font.set_glyph(Charset::glyph_of_code(e.code), &px);
    }

    progress("Rebuilding assets");
    let mut patched = Rom::load(rom.bytes().to_vec())?;
    patched.set_table(info.zh_table, &plan.table);
    for (i, t) in tables.iter_mut().enumerate() {
        if i == info.zh_table {
            continue;
        }
        if sync_name {
            t.strings[LANGUAGE_NAME_ZH] = plan.table.strings[LANGUAGE_NAME_ZH].clone();
        }
        patched.set_table(i, t);
    }
    patched.set_font(&font);
    let mut bytes = patched.to_bytes()?;
    if runtime == RuntimeLanguage::Added {
        runtime_lang::apply(&mut bytes, label_index)?;
    }
    let asset_len_after = patched.rebuilt_asset_len();

    let mut remaining: BTreeMap<char, usize> = BTreeMap::new();
    for s in &plan.table.strings {
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
    let added_runtime = runtime_lang::status(original) == Status::Original
        && runtime_lang::status(patched) == Status::Applied;
    let head = crate::rom::ASSET_BASE
        .min(original.len())
        .min(patched.len());
    let mut expected_head = original.to_vec();
    if added_runtime {
        let label = a.table(0)?.strings.len() as u32;
        if runtime_lang::apply(&mut expected_head, label).is_err() {
            issues.push("runtime language patch could not be reproduced".into());
        }
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
    if a.data_pack() != b.data_pack() {
        issues.push("data pack changed".into());
    }
    for (i, (x, y)) in a.sprite_blobs().iter().zip(b.sprite_blobs()).enumerate() {
        if i != ia.font_sprite && x != y {
            issues.push(format!("sprite {i} changed"));
        }
    }
    let (ta, tb) = (a.all_tables()?, b.all_tables()?);
    let extra = usize::from(added_runtime);
    let labels = runtime_lang::label_strings(&cs);
    for (i, (x, y)) in ta.iter().zip(&tb).enumerate() {
        if i == ia.zh_table {
            continue;
        }
        let n = x.strings.len();
        let mirrored = |s: &[Vec<u16>]| {
            let mut s = s.to_vec();
            if let (Some(slot), Some(z)) = (
                s.get_mut(LANGUAGE_NAME_ZH),
                tb[ib.zh_table].strings.get(LANGUAGE_NAME_ZH),
            ) {
                if ta[ia.zh_table].strings.get(LANGUAGE_NAME_ZH) == Some(&*slot) {
                    *slot = z.clone();
                }
            }
            s
        };
        let same = y.strings.len() == n + extra
            && (x.strings[..] == y.strings[..n] || mirrored(&x.strings) == y.strings[..n]);
        if !same {
            issues.push(format!("string table {i} changed"));
        } else if extra == 1 && y.strings[n] != labels[i] {
            issues.push(format!("string table {i} has an unexpected label"));
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
    if extra == 1 && runtime_lang::applied_label(patched) != Some(za.strings.len() as u32) {
        issues.push("runtime language label index does not match".into());
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

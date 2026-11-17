use paradise_core::charset::Charset;
use paradise_core::rom::Rom;

fn load() -> Option<Vec<u8>> {
    let path = std::env::var("PARADISE_ROM").ok()?;
    std::fs::read(path).ok()
}

#[test]
fn unchanged_rebuild_is_byte_identical() {
    let Some(bytes) = load() else {
        eprintln!("PARADISE_ROM not set; skipping");
        return;
    };
    let rom = Rom::load(bytes.clone()).expect("load");
    let info = rom.info();
    assert!(info.top_checksum_ok && info.strings_checksum_ok);
    assert_eq!(info.zh_table, 7);
    assert_eq!(info.font_glyphs, 9642);
    assert_eq!(rom.to_bytes().unwrap(), bytes);
}

#[test]
fn tables_and_font_roundtrip() {
    let Some(bytes) = load() else {
        return;
    };
    let rom = Rom::load(bytes).expect("load");
    for (i, f) in rom.all_tables().unwrap().iter().enumerate() {
        let raw = rom.table(i).unwrap();
        assert_eq!(&raw, f);
    }
    let font = rom.font().unwrap();
    assert_eq!(font.glyph_count(), Charset::standard().glyph_total());
    let sprite_blob = font.serialize();
    assert_eq!(
        sprite_blob.len() as u32,
        u32::from_le_bytes(sprite_blob[0..4].try_into().unwrap())
    );
    let zh = rom.table(rom.info().zh_table).unwrap();
    assert_eq!(zh.strings.len(), 824);
}

#[test]
fn patch_produces_valid_rom() {
    use paradise_core::patch::{patch, verify, PatchOptions};
    let Some(bytes) = load() else {
        return;
    };
    let rom = Rom::load(bytes.clone()).expect("load");
    let out = patch(&rom, &PatchOptions::default(), &mut |_| {}).expect("patch");
    let r = &out.report;
    eprintln!("{:?}", r.stats);
    eprintln!("unresolved: {:?}", r.unresolved);
    eprintln!("remaining simplified: {:?}", r.remaining_simplified);
    eprintln!(
        "asset {} -> {}, font grew {}",
        r.asset_len_before, r.asset_len_after, r.font_grew
    );
    assert!(verify(&bytes, &out.bytes).unwrap().is_empty());
    assert_ne!(out.bytes, bytes);
    let again = Rom::load(out.bytes.clone()).expect("reload");
    let second = patch(&again, &PatchOptions::default(), &mut |_| {}).expect("repatch");
    assert!(second.report.stats.chars_changed < r.stats.chars_changed / 4 + 50);
}

#[test]
fn runtime_language_patch_is_detected_and_applied_once() {
    use paradise_core::patch::{patch, verify, PatchOptions, RuntimeLanguage};
    use paradise_core::runtime_lang::{status, Status};
    let Some(bytes) = load() else {
        return;
    };
    assert_eq!(status(&bytes), Status::Original);
    let rom = Rom::load(bytes.clone()).expect("load");
    let out = patch(&rom, &PatchOptions::default(), &mut |_| {}).expect("patch");
    assert_eq!(out.report.runtime_language, RuntimeLanguage::Added);
    assert_eq!(status(&out.bytes), Status::Applied);
    assert!(verify(&bytes, &out.bytes).unwrap().is_empty());

    let again = Rom::load(out.bytes.clone()).expect("reload");
    let second = patch(&again, &PatchOptions::default(), &mut |_| {}).expect("repatch");
    assert_eq!(
        second.report.runtime_language,
        RuntimeLanguage::AlreadyPresent
    );
    let paramix_before =
        paradise_core::runtime_lang::settings_labels(&bytes).and_then(|s| s.paramix);
    let paramix_after =
        paradise_core::runtime_lang::settings_labels(&out.bytes).and_then(|s| s.paramix);
    let expected = rom.all_tables().unwrap()[0].strings.len()
        + 1
        + usize::from(paramix_before.is_none() && paramix_after.is_some());
    assert_eq!(again.all_tables().unwrap()[0].strings.len(), expected);
    assert_eq!(
        Rom::load(second.bytes).unwrap().all_tables().unwrap()[0]
            .strings
            .len(),
        expected
    );

    let off = PatchOptions {
        runtime_language: false,
        ..PatchOptions::default()
    };
    let plain = patch(&rom, &off, &mut |_| {}).expect("patch without");
    assert_eq!(status(&plain.bytes), Status::Original);
    assert!(verify(&bytes, &plain.bytes).unwrap().is_empty());
}

/// Exercise both upgrade directions without embedding any game assets in tests.
/// PARAMIX-only is the standalone Studio helper; Language-only models a ROM
/// translated before Studio adds the PARAMIX launch entry.
#[test]
fn settings_helpers_merge_without_duplicate_or_reindexed_labels() {
    use paradise_core::charset::latin_code;
    use paradise_core::patch::{patch, verify, PatchOptions, RuntimeLanguage};
    use paradise_core::{paramix, runtime_lang};
    let Some(bytes) = load() else { return };
    if paramix::requested_shortcut_stub(&bytes).unwrap().is_none() {
        eprintln!("no installed Paramix menu; shared Settings integration skipped");
        return;
    }
    // This test expects the source fixture before a Settings helper is added.
    if runtime_lang::settings_labels(&bytes).is_some() {
        return;
    }
    for paramix_first in [true, false] {
        let mut standalone = Rom::load(bytes.clone()).unwrap();
        let mut tables = standalone.all_tables().unwrap();
        let label_index = tables[0].strings.len() as u32;
        let labels = if paramix_first {
            let mut label: Vec<u16> = "PARAMIX".chars().map(|c| latin_code(c).unwrap()).collect();
            label.push(0);
            vec![label; tables.len()]
        } else {
            runtime_lang::label_strings(&Charset::standard())
        };
        for (i, (table, label)) in tables.iter_mut().zip(labels).enumerate() {
            table.strings.push(label);
            standalone.set_table(i, table);
        }
        let mut standalone = standalone.to_bytes().unwrap();
        runtime_lang::apply_shared(
            &mut standalone,
            (!paramix_first).then_some(label_index),
            paramix_first.then_some(label_index),
        )
        .unwrap();
        let initial = runtime_lang::settings_labels(&standalone).unwrap();
        assert_eq!(initial.paramix, paramix_first.then_some(label_index));
        assert_eq!(initial.language, (!paramix_first).then_some(label_index));
        let before = Rom::load(standalone.clone()).unwrap();
        let out = patch(&before, &PatchOptions::default(), &mut |_| {}).unwrap();
        let merged = runtime_lang::settings_labels(&out.bytes).unwrap();
        assert_eq!(
            merged.paramix,
            Some(if paramix_first {
                label_index
            } else {
                label_index + 1
            })
        );
        assert_eq!(
            merged.language,
            Some(if paramix_first {
                label_index + 1
            } else {
                label_index
            })
        );
        assert_eq!(
            out.report.runtime_language,
            if paramix_first {
                RuntimeLanguage::Added
            } else {
                RuntimeLanguage::AlreadyPresent
            }
        );
        assert!(verify(&standalone, &out.bytes).unwrap().is_empty());
        let merged_rom = Rom::load(out.bytes.clone()).unwrap();
        for table in merged_rom.all_tables().unwrap() {
            assert_eq!(table.strings.len(), label_index as usize + 2);
        }
        let second = patch(&merged_rom, &PatchOptions::default(), &mut |_| {}).unwrap();
        assert_eq!(
            runtime_lang::settings_labels(&second.bytes).unwrap(),
            merged
        );
        assert_eq!(
            Rom::load(second.bytes).unwrap().all_tables().unwrap()[0]
                .strings
                .len(),
            label_index as usize + 2
        );
        if paramix_first {
            let disabled = PatchOptions {
                runtime_language: false,
                ..PatchOptions::default()
            };
            let out = patch(&before, &disabled, &mut |_| {}).unwrap();
            assert_eq!(runtime_lang::settings_labels(&out.bytes).unwrap(), initial);
            assert_eq!(
                Rom::load(out.bytes.clone()).unwrap().all_tables().unwrap()[0]
                    .strings
                    .len(),
                label_index as usize + 1
            );
            assert!(verify(&standalone, &out.bytes).unwrap().is_empty());
        }
    }
}

#[test]
fn language_patch_respects_persisted_studio_shortcut_opt_out() {
    use paradise_core::patch::{patch, verify, PatchOptions};
    use paradise_core::{paramix, runtime_lang};
    let Some(mut bytes) = load() else { return };
    let Some(stub) = paramix::installed_stub(&bytes).unwrap() else {
        return;
    };
    if runtime_lang::settings_labels(&bytes).is_some() {
        return;
    }
    let program = u32::from_le_bytes(bytes[stub + 0x28..stub + 0x2C].try_into().unwrap());
    let flags_at = (program - 0x10000000) as usize + 0x11000 + 0x140 + 36;
    let flags = u32::from_le_bytes(bytes[flags_at..flags_at + 4].try_into().unwrap());
    bytes[flags_at..flags_at + 4].copy_from_slice(&(flags | paramix::F_NO_SHORTCUT).to_le_bytes());
    assert_eq!(paramix::installed_stub(&bytes).unwrap(), Some(stub));
    assert_eq!(paramix::requested_shortcut_stub(&bytes).unwrap(), None);
    let rom = Rom::load(bytes.clone()).unwrap();
    let count = rom.all_tables().unwrap()[0].strings.len();
    let out = patch(&rom, &PatchOptions::default(), &mut |_| {}).unwrap();
    let labels = runtime_lang::settings_labels(&out.bytes).unwrap();
    assert_eq!(labels.language, Some(count as u32));
    assert_eq!(labels.paramix, None);
    assert_eq!(
        &bytes[flags_at..flags_at + 4],
        &out.bytes[flags_at..flags_at + 4]
    );
    assert_eq!(
        Rom::load(out.bytes.clone()).unwrap().all_tables().unwrap()[0]
            .strings
            .len(),
        count + 1
    );
    assert!(verify(&bytes, &out.bytes).unwrap().is_empty());
}

#[test]
fn every_font_produces_a_valid_rom() {
    use paradise_core::glyphpack::FontChoice;
    use paradise_core::patch::{patch, verify, PatchOptions};
    let Some(bytes) = load() else {
        return;
    };
    let rom = Rom::load(bytes.clone()).expect("load");
    for font in FontChoice::ALL {
        let opts = PatchOptions {
            font,
            ..PatchOptions::default()
        };
        let out = patch(&rom, &opts, &mut |_| {}).expect("patch");
        assert!(verify(&bytes, &out.bytes).unwrap().is_empty(), "{font:?}");
        eprintln!("{font:?}: fallback glyphs {}", out.report.fallback_glyphs);
    }
}

#[test]
fn latin_glyphs_are_redrawn_only_when_requested() {
    use paradise_core::charset::latin_code;
    use paradise_core::patch::{patch, verify, PatchOptions};
    let Some(bytes) = load() else {
        return;
    };
    let rom = Rom::load(bytes.clone()).expect("load");
    let glyph = |r: &Rom, c: char| {
        let code = latin_code(c).unwrap() as usize;
        r.font().unwrap().glyph_pixels(code - 1)
    };
    let keep = PatchOptions {
        redraw_latin: false,
        ..PatchOptions::default()
    };
    let kept = patch(&rom, &keep, &mut |_| {}).expect("patch");
    assert_eq!(kept.report.latin_glyphs, 0);
    let kept_rom = Rom::load(kept.bytes).unwrap();
    assert!(glyph(&kept_rom, 'A') == glyph(&rom, 'A'));

    let out = patch(&rom, &PatchOptions::default(), &mut |_| {}).expect("patch");
    assert_eq!(out.report.latin_glyphs, 67);
    assert!(verify(&bytes, &out.bytes).unwrap().is_empty());
    let after = Rom::load(out.bytes).unwrap();
    assert!(glyph(&after, 'A') != glyph(&rom, 'A'));
}

#[test]
fn latin_codes_match_the_game_font() {
    use paradise_core::mapping::describe;
    let Some(bytes) = load() else {
        return;
    };
    let rom = Rom::load(bytes).expect("load");
    let zh = rom.table(rom.info().zh_table).unwrap();
    assert_eq!(describe(&Charset::standard(), &zh.strings[4]), "HOSPITAL");
}

#[test]
fn mapping_is_applied_and_verified() {
    use paradise_core::charset::latin_code;
    use paradise_core::mapping::Mapping;
    use paradise_core::patch::{patch, verify, verify_with, PatchOptions};
    let Some(bytes) = load() else {
        return;
    };
    let rom = Rom::load(bytes.clone()).expect("load");
    let mapping = Mapping::parse(r#"{"拓麻": "Tama", "小游戏": "小遊戲", "没有": "x"}"#).unwrap();
    let opts = PatchOptions {
        mapping: Some(mapping.clone()),
        ..PatchOptions::default()
    };
    let out = patch(&rom, &opts, &mut |_| {}).expect("patch");
    assert_eq!(
        out.report
            .mapping_hits
            .iter()
            .filter(|(_, n)| *n == 0)
            .count(),
        1
    );
    assert!(verify_with(&bytes, &out.bytes, Some(&mapping))
        .unwrap()
        .is_empty());
    assert!(!verify(&bytes, &out.bytes).unwrap().is_empty());
    let after = Rom::load(out.bytes).unwrap();
    let zh = after.table(after.info().zh_table).unwrap();
    let tama: Vec<u16> = "Tama".chars().map(|c| latin_code(c).unwrap()).collect();
    assert_eq!(zh.strings[9][..4], tama[..]);
}

#[test]
fn exported_template_round_trips() {
    use paradise_core::charset::Charset;
    use paradise_core::convert::{Converter, Variant};
    use paradise_core::mapping::{export_template, Mapping};
    use paradise_core::patch::{patch, verify_with, PatchOptions};
    let Some(bytes) = load() else {
        return;
    };
    let rom = Rom::load(bytes.clone()).expect("load");
    let cs = Charset::standard();
    let table = rom.table(rom.info().zh_table).unwrap();
    let conv = Converter::new(Variant::HongKong).unwrap();
    let json = export_template(&table, &cs, &conv, None);
    let mapping = Mapping::parse(&json).expect("template parses");
    assert!(mapping.len() > 500);

    let plain = patch(&rom, &PatchOptions::default(), &mut |_| {}).unwrap();
    let opts = PatchOptions {
        mapping: Some(mapping.clone()),
        ..PatchOptions::default()
    };
    let with = patch(&rom, &opts, &mut |_| {}).unwrap();
    assert_eq!(plain.bytes, with.bytes);
    assert!(verify_with(&bytes, &with.bytes, Some(&mapping))
        .unwrap()
        .is_empty());

    let edited = json.replacen("\"^设置$\": \"設置\"", "\"^设置$\": \"Settings\"", 1);
    if edited != json {
        let m = Mapping::parse(&edited).unwrap();
        let opts = PatchOptions {
            mapping: Some(m),
            ..PatchOptions::default()
        };
        let out = patch(&rom, &opts, &mut |_| {}).unwrap();
        assert_ne!(out.bytes, plain.bytes);
    }
}

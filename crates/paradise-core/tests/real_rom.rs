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
    let expected = rom.all_tables().unwrap()[0].strings.len() + 1;
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
    let json = export_template(&table, &cs, &conv);
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

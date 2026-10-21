//! Adds a "Language" entry as the fourth item of the Settings menu so the language can
//! be changed at any time instead of only on first boot.
//!
//! The firmware already has a language-selection scene (id 113) and reads every string
//! through the live language setting. The patch relocates the Settings menu table into
//! unused flash, appends the Language entry, and makes the language scene return to the
//! scene it was opened from (Settings) instead of continuing the first-boot flow.

use crate::charset::Charset;
use crate::error::{Error, Result};

const SETTINGS_PAGE1_TABLE: usize = 0xA4FFC;
const SETTINGS_PAGE1_PTR: usize = 0x582CC;
const SETTINGS_PAGE1_COUNT: usize = 0x582D6;
const LANGUAGE_NEXT_SCENE: usize = 0x33CFC;
const SCENE_STATE_PTR: usize = 0x138D6;

const FREE_AT: usize = 0xB6000;
const FREE_LEN: usize = 0x80;
const TABLE_AT: usize = FREE_AT;
const TRAMPOLINE_AT: usize = FREE_AT + 0x40;
const TABLE_ENTRY_LEN: usize = 12;
const NEW_ENTRY_AT: usize = TABLE_AT + 3 * TABLE_ENTRY_LEN;

const SCENE_LANGUAGE: u32 = 113;
const SCENE_SETTINGS: u8 = 55;
const SCENE_DATETIME: u8 = 114;
const SCENE_STATE_ADDR: u32 = 0x1800_1CB4;

const FLASH_ALIAS_DELTA: usize = 0x11000;
const ALIAS_BASE: u32 = 0x1000_0000;

const ORIGINAL_PAGE1: [u32; 9] = [28, 70, 0, 27, 71, 0, 26, 0, 0];

/// What the ROM's menu code currently looks like.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Original,
    Applied,
    Unsupported,
}

fn movw(rd: u16, imm: u16) -> [u8; 4] {
    let i = (imm >> 11) & 1;
    let hi = 0xF240 | (i << 10) | (imm >> 12);
    let lo = (((imm >> 8) & 7) << 12) | (rd << 8) | (imm & 0xFF);
    pack(hi, lo)
}

fn movt(rd: u16, imm: u16) -> [u8; 4] {
    let i = (imm >> 11) & 1;
    let hi = 0xF2C0 | (i << 10) | (imm >> 12);
    let lo = (((imm >> 8) & 7) << 12) | (rd << 8) | (imm & 0xFF);
    pack(hi, lo)
}

fn bl(from: usize, to: usize) -> [u8; 4] {
    let off = to as i64 - (from as i64 + 4);
    let off = off as u32;
    let s = (off >> 24) & 1;
    let i1 = (off >> 23) & 1;
    let i2 = (off >> 22) & 1;
    let j1 = !(i1 ^ s) & 1;
    let j2 = !(i2 ^ s) & 1;
    let hi = 0xF000 | (s << 10) | ((off >> 12) & 0x3FF);
    let lo = 0xD000 | (j1 << 13) | (j2 << 11) | ((off >> 1) & 0x7FF);
    pack(hi as u16, lo as u16)
}

fn pack(hi: u16, lo: u16) -> [u8; 4] {
    let (a, b) = (hi.to_le_bytes(), lo.to_le_bytes());
    [a[0], a[1], b[0], b[1]]
}

fn alias_of(flash: usize) -> u32 {
    ALIAS_BASE + (flash - FLASH_ALIAS_DELTA) as u32
}

fn trampoline() -> Vec<u8> {
    let mut t = Vec::new();
    t.extend(movw(1, SCENE_STATE_ADDR as u16));
    t.extend(movt(1, (SCENE_STATE_ADDR >> 16) as u16));
    // ldrh r2, [r1, #4]    previous scene
    t.extend([0x8A, 0x88]);
    // movs r0, #DATETIME
    t.extend([SCENE_DATETIME, 0x20]);
    // cmp r2, #SETTINGS
    t.extend([SCENE_SETTINGS, 0x2A]);
    // bne +0
    t.extend([0x00, 0xD1]);
    // movs r0, #SETTINGS
    t.extend([SCENE_SETTINGS, 0x20]);
    // strh r0, [r1, #2]    next scene
    t.extend([0x48, 0x80]);
    // bx lr
    t.extend([0x70, 0x47]);
    t
}

struct Edit {
    at: usize,
    before: Vec<u8>,
    after: Vec<u8>,
}

fn edits(label_index: u32) -> Vec<Edit> {
    let mut table = Vec::new();
    for v in ORIGINAL_PAGE1 {
        table.extend(v.to_le_bytes());
    }
    let mut new_table = table.clone();
    for v in [label_index, SCENE_LANGUAGE, 0] {
        new_table.extend(v.to_le_bytes());
    }
    let tramp = trampoline();

    let mut ptr_before = Vec::new();
    ptr_before.extend(movw(1, 0x3FFC));
    ptr_before.extend(movt(1, 0x1009));
    let table_alias = alias_of(TABLE_AT);
    let mut ptr_after = Vec::new();
    ptr_after.extend(movw(1, table_alias as u16));
    ptr_after.extend(movt(1, (table_alias >> 16) as u16));

    let mut call_before = vec![SCENE_DATETIME, 0x20];
    call_before.extend(bl(LANGUAGE_NEXT_SCENE + 2, 0x600138CC - 0x6000_0000));
    let mut call_after = bl(LANGUAGE_NEXT_SCENE, TRAMPOLINE_AT).to_vec();
    call_after.extend([0x00, 0xBF]);

    vec![
        Edit {
            at: SETTINGS_PAGE1_PTR,
            before: ptr_before,
            after: ptr_after,
        },
        Edit {
            at: SETTINGS_PAGE1_COUNT,
            before: vec![0x03, 0x21],
            after: vec![0x04, 0x21],
        },
        Edit {
            at: LANGUAGE_NEXT_SCENE,
            before: call_before,
            after: call_after,
        },
        Edit {
            at: TABLE_AT,
            before: vec![0xFF; new_table.len()],
            after: new_table,
        },
        Edit {
            at: TRAMPOLINE_AT,
            before: vec![0xFF; tramp.len()],
            after: tramp,
        },
    ]
}

fn matches(bytes: &[u8], at: usize, want: &[u8]) -> bool {
    bytes.get(at..at + want.len()) == Some(want)
}

fn original_source_ok(bytes: &[u8]) -> bool {
    let mut table = Vec::new();
    for v in ORIGINAL_PAGE1 {
        table.extend(v.to_le_bytes());
    }
    let mut state_ptr = Vec::new();
    state_ptr.extend(movw(1, SCENE_STATE_ADDR as u16));
    state_ptr.extend(movt(1, (SCENE_STATE_ADDR >> 16) as u16));
    matches(bytes, SETTINGS_PAGE1_TABLE, &table)
        && matches(bytes, SCENE_STATE_PTR, &state_ptr)
        && bytes
            .get(FREE_AT..FREE_AT + FREE_LEN)
            .is_some_and(|s| s.iter().all(|&b| b == 0xFF))
}

pub fn status(bytes: &[u8]) -> Status {
    if bytes.len() < FREE_AT + FREE_LEN {
        return Status::Unsupported;
    }
    if original_source_ok(bytes) && edits(0).iter().all(|e| matches(bytes, e.at, &e.before)) {
        return Status::Original;
    }
    if let Some(l) = bytes.get(NEW_ENTRY_AT..NEW_ENTRY_AT + 4) {
        let label = u32::from_le_bytes(l.try_into().unwrap());
        if edits(label).iter().all(|e| matches(bytes, e.at, &e.after)) {
            return Status::Applied;
        }
    }
    Status::Unsupported
}

pub fn applied_label(bytes: &[u8]) -> Option<u32> {
    (status(bytes) == Status::Applied)
        .then(|| u32::from_le_bytes(bytes[NEW_ENTRY_AT..NEW_ENTRY_AT + 4].try_into().unwrap()))
}

/// Patches the executable region. `label_index` is the zero-based string index of the
/// menu label, identical in every language table.
pub fn apply(bytes: &mut [u8], label_index: u32) -> Result<()> {
    if status(bytes) != Status::Original {
        return Err(Error::Unsupported(
            "the Settings menu code does not match the known firmware".into(),
        ));
    }
    for e in edits(label_index) {
        bytes[e.at..e.at + e.after.len()].copy_from_slice(&e.after);
    }
    Ok(())
}

fn latin(s: &str) -> Option<Vec<u16>> {
    s.chars()
        .map(|c| c.is_ascii_lowercase().then_some(c as u16 + 221))
        .collect()
}

fn glyphs(cs: &Charset, s: &str) -> Option<Vec<u16>> {
    s.chars().map(|c| cs.code_of(c)).collect()
}

/// The "Language" label for each of the nine language tables, NUL terminated.
pub fn label_strings(cs: &Charset) -> Vec<Vec<u16>> {
    let english = || latin("language").unwrap();
    let pick = |native: Option<Vec<u16>>| native.unwrap_or_else(english);
    let words: [Option<Vec<u16>>; 9] = [
        glyphs(cs, "げんご"),
        None,
        latin("langue"),
        latin("sprache"),
        latin("idioma"),
        latin("idioma"),
        latin("lingua"),
        glyphs(cs, "语言"),
        glyphs(cs, "언어"),
    ];
    words
        .into_iter()
        .map(|w| {
            let mut v = pick(w);
            v.push(0);
            v
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoders_match_known_instructions() {
        assert_eq!(movw(1, 0x3FFC).to_vec(), vec![0x43, 0xF6, 0xFC, 0x71]);
        assert_eq!(movt(1, 0x1009).to_vec(), vec![0xC1, 0xF2, 0x09, 0x01]);
        assert_eq!(movw(1, 0x1CB4).to_vec(), vec![0x41, 0xF6, 0xB4, 0x41]);
        assert_eq!(movt(1, 0x1800).to_vec(), vec![0xC1, 0xF6, 0x00, 0x01]);
    }

    #[test]
    fn labels_cover_all_languages() {
        let labels = label_strings(&Charset::standard());
        assert_eq!(labels.len(), 9);
        assert!(labels
            .iter()
            .all(|l| l.len() > 1 && *l.last().unwrap() == 0));
    }
}

//! Adds a "Language" entry as the fourth item of the Settings menu so the language can
//! be changed at any time instead of only on first boot.
//!
//! The firmware already has a language-selection scene (id 113) and reads every string
//! through the live language setting. The patch relocates the Settings menu table into
//! unused flash, appends the Language entry, and makes the language scene return to the
//! scene it was opened from (Settings) instead of continuing the first-boot flow.
//!
//! Firmware builds differ in code layout and scene numbering, so each known build has a
//! [`Profile`]. A ROM is only patched when every byte the patch touches matches one.

use crate::charset::Charset;
use crate::error::{Error, Result};

const FREE_AT: usize = 0xB6000;
const FREE_LEN: usize = 0x80;
const TABLE_AT: usize = FREE_AT;
const TRAMPOLINE_AT: usize = FREE_AT + 0x40;
const TABLE_ENTRY_LEN: usize = 12;
const NEW_ENTRY_AT: usize = TABLE_AT + 3 * TABLE_ENTRY_LEN;

const FLASH_ALIAS_DELTA: usize = 0x11000;
const ALIAS_BASE: u32 = 0x1000_0000;

/// Locations and scene numbers of one firmware build.
struct Profile {
    page1_table: usize,
    page1_original: [u32; 9],
    page1_ptr: usize,
    language_next_scene: usize,
    /// Flash offset the original call branches to (the function or a veneer).
    set_scene: usize,
    state_site: usize,
    state_addr: u32,
    scene_language: u32,
    scene_settings: u8,
    scene_datetime: u8,
    /// The scene-change function is a plain store the trampoline can inline; otherwise
    /// the trampoline tail-calls `set_scene`.
    inline_set_scene: bool,
}

impl Profile {
    fn page1_count(&self) -> usize {
        self.page1_ptr + 10
    }
}

const PROFILES: [Profile; 3] = [
    // Glacier and Tropical
    Profile {
        page1_table: 0xA4FFC,
        page1_original: [28, 70, 0, 27, 71, 0, 26, 0, 0],
        page1_ptr: 0x582CC,
        language_next_scene: 0x33CFC,
        set_scene: 0x138CC,
        state_site: 0x138D6,
        state_addr: 0x1800_1CB4,
        scene_language: 113,
        scene_settings: 55,
        scene_datetime: 114,
        inline_set_scene: true,
    },
    // Jade Forest
    Profile {
        page1_table: 0x91A68,
        page1_original: [28, 68, 0, 27, 69, 0, 26, 0, 0],
        page1_ptr: 0x525E0,
        language_next_scene: 0x30448,
        set_scene: 0x8E948,
        state_site: 0x1CCF6,
        state_addr: 0x1800_1BF4,
        scene_language: 109,
        scene_settings: 54,
        scene_datetime: 110,
        inline_set_scene: false,
    },
    // Sky, Water and Land
    Profile {
        page1_table: 0x8C7F0,
        page1_original: [28, 67, 0, 27, 68, 0, 26, 0, 0],
        page1_ptr: 0x5061C,
        language_next_scene: 0x2F104,
        set_scene: 0x8A430,
        state_site: 0x1CA8E,
        state_addr: 0x1800_1BF4,
        scene_language: 106,
        scene_settings: 53,
        scene_datetime: 107,
        inline_set_scene: false,
    },
];

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

fn branch(from: usize, to: usize, link: bool) -> [u8; 4] {
    let off = (to as i64 - (from as i64 + 4)) as u32;
    let s = (off >> 24) & 1;
    let i1 = (off >> 23) & 1;
    let i2 = (off >> 22) & 1;
    let j1 = !(i1 ^ s) & 1;
    let j2 = !(i2 ^ s) & 1;
    let hi = 0xF000 | (s << 10) | ((off >> 12) & 0x3FF);
    let kind = if link { 0xD000 } else { 0x9000 };
    let lo = kind | (j1 << 13) | (j2 << 11) | ((off >> 1) & 0x7FF);
    pack(hi as u16, lo as u16)
}

fn bl(from: usize, to: usize) -> [u8; 4] {
    branch(from, to, true)
}

fn pack(hi: u16, lo: u16) -> [u8; 4] {
    let (a, b) = (hi.to_le_bytes(), lo.to_le_bytes());
    [a[0], a[1], b[0], b[1]]
}

fn alias_of(flash: usize) -> u32 {
    ALIAS_BASE + (flash - FLASH_ALIAS_DELTA) as u32
}

fn state_pointer(p: &Profile) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend(movw(1, p.state_addr as u16));
    v.extend(movt(1, (p.state_addr >> 16) as u16));
    v
}

fn trampoline(p: &Profile) -> Vec<u8> {
    let mut t = state_pointer(p);
    // ldrh r2, [r1, #4]    previous scene
    t.extend([0x8A, 0x88]);
    // movs r0, #DATETIME
    t.extend([p.scene_datetime, 0x20]);
    // cmp r2, #SETTINGS
    t.extend([p.scene_settings, 0x2A]);
    // bne +0
    t.extend([0x00, 0xD1]);
    // movs r0, #SETTINGS
    t.extend([p.scene_settings, 0x20]);
    if p.inline_set_scene {
        // strh r0, [r1, #2]    next scene
        t.extend([0x48, 0x80]);
        // bx lr
        t.extend([0x70, 0x47]);
    } else {
        let at = TRAMPOLINE_AT + t.len();
        t.extend(branch(at, p.set_scene, false));
    }
    t
}

struct Edit {
    at: usize,
    before: Vec<u8>,
    after: Vec<u8>,
}

fn original_table(p: &Profile) -> Vec<u8> {
    p.page1_original
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect()
}

fn edits(p: &Profile, label_index: u32) -> Vec<Edit> {
    let mut new_table = original_table(p);
    for v in [label_index, p.scene_language, 0] {
        new_table.extend(v.to_le_bytes());
    }
    let tramp = trampoline(p);

    let old_alias = alias_of(p.page1_table);
    let mut ptr_before = Vec::new();
    ptr_before.extend(movw(1, old_alias as u16));
    ptr_before.extend(movt(1, (old_alias >> 16) as u16));
    let table_alias = alias_of(TABLE_AT);
    let mut ptr_after = Vec::new();
    ptr_after.extend(movw(1, table_alias as u16));
    ptr_after.extend(movt(1, (table_alias >> 16) as u16));

    let site = p.language_next_scene;
    let mut call_before = vec![p.scene_datetime, 0x20];
    call_before.extend(bl(site + 2, p.set_scene));
    let mut call_after = bl(site, TRAMPOLINE_AT).to_vec();
    call_after.extend([0x00, 0xBF]);

    vec![
        Edit {
            at: p.page1_ptr,
            before: ptr_before,
            after: ptr_after,
        },
        Edit {
            at: p.page1_count(),
            before: vec![0x03, 0x21],
            after: vec![0x04, 0x21],
        },
        Edit {
            at: site,
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

fn original_source_ok(p: &Profile, bytes: &[u8]) -> bool {
    matches(bytes, p.page1_table, &original_table(p))
        && matches(bytes, p.state_site, &state_pointer(p))
        && bytes
            .get(FREE_AT..FREE_AT + FREE_LEN)
            .is_some_and(|s| s.iter().all(|&b| b == 0xFF))
}

fn find_original(bytes: &[u8]) -> Option<&'static Profile> {
    PROFILES.iter().find(|p| {
        original_source_ok(p, bytes) && edits(p, 0).iter().all(|e| matches(bytes, e.at, &e.before))
    })
}

fn find_applied(bytes: &[u8]) -> Option<&'static Profile> {
    let label = u32::from_le_bytes(bytes.get(NEW_ENTRY_AT..NEW_ENTRY_AT + 4)?.try_into().ok()?);
    PROFILES.iter().find(|p| {
        edits(p, label)
            .iter()
            .all(|e| matches(bytes, e.at, &e.after))
    })
}

pub fn status(bytes: &[u8]) -> Status {
    if bytes.len() < FREE_AT + FREE_LEN {
        return Status::Unsupported;
    }
    if find_original(bytes).is_some() {
        Status::Original
    } else if find_applied(bytes).is_some() {
        Status::Applied
    } else {
        Status::Unsupported
    }
}

pub fn applied_label(bytes: &[u8]) -> Option<u32> {
    (status(bytes) == Status::Applied)
        .then(|| u32::from_le_bytes(bytes[NEW_ENTRY_AT..NEW_ENTRY_AT + 4].try_into().unwrap()))
}

/// Patches the executable region. `label_index` is the zero-based string index of the
/// menu label, identical in every language table.
pub fn apply(bytes: &mut [u8], label_index: u32) -> Result<()> {
    let Some(profile) = find_original(bytes) else {
        return Err(Error::Unsupported(
            "the Settings menu code does not match the known firmware".into(),
        ));
    };
    for e in edits(profile, label_index) {
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
    fn branch_encodings_match_known_instructions() {
        assert_eq!(
            branch(0xB6052, 0x8E948, false).to_vec(),
            vec![0xD8, 0xF7, 0x79, 0xBC]
        );
        assert_eq!(bl(0x30448, 0xB6040).to_vec(), vec![0x85, 0xF0, 0xFA, 0xFD]);
    }

    #[test]
    fn profiles_do_not_overlap_the_free_area() {
        for p in &PROFILES {
            assert!(p.page1_table < FREE_AT && p.page1_count() < FREE_AT);
            assert!(0x40 + trampoline(p).len() <= FREE_LEN);
        }
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

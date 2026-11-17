//! Shared optional "Language" and "PARAMIX" entries in the game's Settings menu.
//! Either works independently; merging preserves labels and the helper page.
//!
//! The firmware already has a language-selection scene (id 113) and reads every string
//! through the live language setting. The patch relocates the Settings menu table into
//! unused flash and makes Language return to Settings instead of first-boot
//! setup. A private PARAMIX table sentinel intercepts only the confirmed
//! selected-scene call and jumps to the validated direct-launch stub. Every
//! other scene tail-calls the original setter; B/C and More remain unchanged.
//!
//! Firmware builds differ in code layout and scene numbering, so each known build has a
//! [`Profile`]. A ROM is only patched when every byte the patch touches matches one.

use crate::charset::Charset;
use crate::error::{Error, Result};

const FREE_AT: usize = 0xB6000;
const FREE_LEN: usize = 0x80;
const PAGE: usize = 0x1000;
const MARKER_V1: &[u8; 8] = b"ZHLANG01";
const MARKER_V2: &[u8; 8] = b"ZHLANG02";
const PARAMIX_SCENE: u32 = 0x7FFE;
const TABLE_ENTRY_LEN: usize = 12;
const NEW_ENTRY_OFFSET: usize = 3 * TABLE_ENTRY_LEN;

const FLASH_ALIAS_DELTA: usize = 0x11000;
const ALIAS_BASE: u32 = 0x1000_0000;

/// Locations and scene numbers of one firmware build.
struct Profile {
    page1_table: usize,
    page1_original: [u32; 9],
    page1_ptr: usize,
    language_next_scene: usize,
    settings_dispatch: usize,
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
        settings_dispatch: 0x583A6,
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
        settings_dispatch: 0x526BA,
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
        settings_dispatch: 0x506F6,
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

fn trampoline(p: &Profile, base: usize) -> Vec<u8> {
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
        let at = base + 0x40 + t.len();
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

fn legacy_edits(p: &Profile, label_index: u32, base: usize) -> Vec<Edit> {
    let mut new_table = original_table(p);
    for v in [label_index, p.scene_language, 0] {
        new_table.extend(v.to_le_bytes());
    }
    let tramp = trampoline(p, base);

    let old_alias = alias_of(p.page1_table);
    let mut ptr_before = Vec::new();
    ptr_before.extend(movw(1, old_alias as u16));
    ptr_before.extend(movt(1, (old_alias >> 16) as u16));
    let table_alias = alias_of(base);
    let mut ptr_after = Vec::new();
    ptr_after.extend(movw(1, table_alias as u16));
    ptr_after.extend(movt(1, (table_alias >> 16) as u16));

    let site = p.language_next_scene;
    let mut call_before = vec![p.scene_datetime, 0x20];
    call_before.extend(bl(site + 2, p.set_scene));
    let mut call_after = bl(site, base + 0x40).to_vec();
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
            at: base,
            before: vec![0xFF; new_table.len()],
            after: new_table,
        },
        Edit {
            at: base + 0x40,
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
}

fn find_original(bytes: &[u8]) -> Option<(usize, &'static Profile)> {
    PROFILES.iter().enumerate().find(|(_, p)| {
        original_source_ok(p, bytes)
            && legacy_edits(p, 0, FREE_AT)
                .iter()
                .take(3)
                .all(|e| matches(bytes, e.at, &e.before))
    })
}

fn word(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

fn helper_sum(bytes: &[u8], base: usize) -> Option<u32> {
    Some(
        bytes
            .get(base..base + FREE_LEN)?
            .iter()
            .map(|&b| b as u32)
            .sum(),
    )
}

fn marker_v1(bytes: &[u8], base: usize) -> Option<(usize, u32)> {
    if base % PAGE != 0
        || bytes.get(base + 0x80..base + 0x88)? != MARKER_V1
        || word(bytes, base + 0x88)? != 1
        || word(bytes, base + 0x8C)? as usize != base
        || word(bytes, base + 0x98)? != PAGE as u32
        || word(bytes, base + 0x9C)? != helper_sum(bytes, base)?
        || !bytes
            .get(base + 0xA0..base + PAGE)?
            .iter()
            .all(|&b| b == 255)
    {
        return None;
    }
    let profile = word(bytes, base + 0x94)? as usize;
    if profile >= PROFILES.len() {
        return None;
    }
    Some((profile, word(bytes, base + 0x90)?))
}

// base, profile index, label, marker present. Legacy B6000 patches remain
// readable, but all new helpers are placed at the top of a safe free-code gap.
fn find_v1(bytes: &[u8]) -> Option<(usize, usize, u32, bool)> {
    for base in (0x90000..0x10F000).step_by(PAGE).rev() {
        if let Some((i, label)) = marker_v1(bytes, base) {
            let p = &PROFILES[i];
            if original_source_ok(p, bytes)
                && legacy_edits(p, label, base)
                    .iter()
                    .all(|e| matches(bytes, e.at, &e.after))
            {
                return Some((base, i, label, true));
            }
        }
    }
    // Do not mistake a corrupt new marker for the old unmarked format.
    if bytes
        .get(FREE_AT + 0x80..FREE_AT + 0xA0)?
        .iter()
        .all(|&b| b == 255)
    {
        let label = word(bytes, FREE_AT + NEW_ENTRY_OFFSET)?;
        for (i, p) in PROFILES.iter().enumerate() {
            if original_source_ok(p, bytes)
                && legacy_edits(p, label, FREE_AT)
                    .iter()
                    .all(|e| matches(bytes, e.at, &e.after))
            {
                return Some((FREE_AT, i, label, false));
            }
        }
    }
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettingsLabels {
    pub language: Option<u32>,
    pub paramix: Option<u32>,
}

#[derive(Debug, Clone, Copy)]
struct SettingsInfo {
    base: usize,
    profile: usize,
    labels: SettingsLabels,
    stub: Option<usize>,
    // 0 legacy unmarked, 1 ZHLANG01, 2 shared ZHLANG02.
    format: u8,
}

fn dispatcher(p: &Profile, base: usize, stub: usize) -> Vec<u8> {
    let mut v = movw(1, PARAMIX_SCENE as u16).to_vec();
    v.extend([0x88, 0x42, 0x01, 0xD0]); // cmp r0,r1; beq helper+6C
    v.extend(branch(base + 0x68, p.set_scene, false)); // ordinary scene: tail-call
    v.extend([0x00, 0x49, 0x08, 0x47]); // ldr r1,[pc,#0]; bx r1
    v.extend((alias_of(stub + 0x20) | 1).to_le_bytes());
    v
}

fn shared_edits(
    p: &Profile,
    base: usize,
    labels: SettingsLabels,
    stub: Option<usize>,
) -> Vec<Edit> {
    let mut v = legacy_edits(p, labels.language.unwrap_or(0), base);
    let mut table = original_table(p);
    if let Some(label) = labels.language {
        for w in [label, p.scene_language, 0] {
            table.extend(w.to_le_bytes());
        }
    }
    if let Some(label) = labels.paramix {
        for w in [label, PARAMIX_SCENE, 0] {
            table.extend(w.to_le_bytes());
        }
    }
    v[1].after[0] = 3 + labels.language.is_some() as u8 + labels.paramix.is_some() as u8;
    if labels.language.is_none() {
        v[2].after = v[2].before.clone();
    }
    v.truncate(3);
    v.push(Edit {
        at: p.settings_dispatch,
        before: bl(p.settings_dispatch, p.set_scene).to_vec(),
        after: if labels.paramix.is_some() {
            bl(p.settings_dispatch, base + 0x60).to_vec()
        } else {
            bl(p.settings_dispatch, p.set_scene).to_vec()
        },
    });
    v.push(Edit {
        at: base,
        before: vec![255; table.len()],
        after: table,
    });
    if labels.language.is_some() {
        let t = trampoline(p, base);
        v.push(Edit {
            at: base + 0x40,
            before: vec![255; t.len()],
            after: t,
        });
    }
    if let Some(stub) = stub {
        let t = dispatcher(p, base, stub);
        v.push(Edit {
            at: base + 0x60,
            before: vec![255; t.len()],
            after: t,
        });
    }
    v
}

fn shared_page(base: usize, index: usize, labels: SettingsLabels, stub: Option<usize>) -> Vec<u8> {
    let mut page = vec![255; PAGE];
    for e in shared_edits(&PROFILES[index], base, labels, stub)
        .into_iter()
        .filter(|e| e.at >= base && e.at < base + PAGE)
    {
        page[e.at - base..e.at - base + e.after.len()].copy_from_slice(&e.after);
    }
    page[0x80..0x88].copy_from_slice(MARKER_V2);
    let flags = labels.language.is_some() as u32 | ((labels.paramix.is_some() as u32) << 1);
    let sum = page[..0x80].iter().map(|&b| b as u32).sum::<u32>();
    for (at, val) in [
        (0x88, 2),
        (0x8C, base as u32),
        (0x90, labels.language.unwrap_or(u32::MAX)),
        (0x94, index as u32),
        (0x98, PAGE as u32),
        (0x9C, sum),
        (0xA0, flags),
        (0xA4, labels.paramix.unwrap_or(u32::MAX)),
        (0xA8, stub.unwrap_or(0) as u32),
    ] {
        page[at..at + 4].copy_from_slice(&val.to_le_bytes());
    }
    let sum = page[..0xAC].iter().map(|&b| b as u32).sum::<u32>();
    page[0xAC..0xB0].copy_from_slice(&sum.to_le_bytes());
    page
}

fn find_shared(bytes: &[u8]) -> Option<SettingsInfo> {
    for base in (0x90000..0x10F000).step_by(PAGE).rev() {
        if bytes.get(base + 0x80..base + 0x88) != Some(MARKER_V2.as_slice())
            || word(bytes, base + 0x88) != Some(2)
            || word(bytes, base + 0x8C) != Some(base as u32)
        {
            continue;
        }
        let index = word(bytes, base + 0x94)? as usize;
        if index >= PROFILES.len() {
            continue;
        }
        let flags = word(bytes, base + 0xA0)?;
        if flags == 0 || flags > 3 {
            continue;
        }
        let lang = word(bytes, base + 0x90)?;
        let paramix = word(bytes, base + 0xA4)?;
        let stub = word(bytes, base + 0xA8)? as usize;
        if (flags & 1 != 0) != (lang != u32::MAX)
            || (flags & 2 != 0) != (paramix != u32::MAX)
            || (flags & 2 != 0) != (stub != 0)
        {
            continue;
        }
        let labels = SettingsLabels {
            language: (flags & 1 != 0).then_some(lang),
            paramix: (flags & 2 != 0).then_some(paramix),
        };
        let stub = (stub != 0).then_some(stub);
        if stub.is_some() && crate::paramix::installed_stub(bytes).ok().flatten() != stub {
            continue;
        }
        let p = &PROFILES[index];
        if !original_source_ok(p, bytes)
            || !shared_edits(p, base, labels, stub)
                .iter()
                .all(|e| matches(bytes, e.at, &e.after))
        {
            continue;
        }
        if bytes.get(base..base + PAGE) == Some(shared_page(base, index, labels, stub).as_slice()) {
            return Some(SettingsInfo {
                base,
                profile: index,
                labels,
                stub,
                format: 2,
            });
        }
    }
    None
}

fn settings_info(bytes: &[u8]) -> Option<SettingsInfo> {
    find_shared(bytes).or_else(|| {
        find_v1(bytes).map(|(base, profile, label, marked)| SettingsInfo {
            base,
            profile,
            labels: SettingsLabels {
                language: Some(label),
                paramix: None,
            },
            stub: None,
            format: if marked { 1 } else { 0 },
        })
    })
}

pub fn settings_labels(bytes: &[u8]) -> Option<SettingsLabels> {
    settings_info(bytes).map(|i| i.labels)
}

pub fn status(bytes: &[u8]) -> Status {
    if let Some(i) = settings_info(bytes) {
        return if i.labels.language.is_some() {
            Status::Applied
        } else {
            Status::Original
        };
    }
    if find_original(bytes).is_some() {
        Status::Original
    } else {
        Status::Unsupported
    }
}

pub fn helper_page(bytes: &[u8]) -> Option<usize> {
    settings_info(bytes).map(|i| i.base)
}
pub fn applied_label(bytes: &[u8]) -> Option<u32> {
    settings_info(bytes).and_then(|i| i.labels.language)
}

fn available(bytes: &[u8], base: usize) -> Result<bool> {
    if base % PAGE != 0 || !(0x90000..0x10F000).contains(&base) {
        return Ok(false);
    }
    if !bytes
        .get(base..base + PAGE)
        .is_some_and(|page| page.iter().all(|&b| b == 255))
    {
        return Ok(false);
    }
    Ok(!crate::paramix::reserved_ranges(bytes)?
        .iter()
        .any(|r| crate::paramix::overlaps(base..base + PAGE, r)))
}

fn restore_source(bytes: &mut [u8], info: SettingsInfo) {
    let p = &PROFILES[info.profile];
    let edits = if info.format == 2 {
        shared_edits(p, info.base, info.labels, info.stub)
    } else {
        legacy_edits(p, info.labels.language.unwrap(), info.base)
    };
    for e in edits
        .into_iter()
        .filter(|e| e.at < info.base || e.at >= info.base + PAGE)
    {
        bytes[e.at..e.at + e.before.len()].copy_from_slice(&e.before);
    }
    let len = if info.format == 0 { FREE_LEN } else { PAGE };
    bytes[info.base..info.base + len].fill(255);
}

fn apply_complete(
    bytes: &mut [u8],
    labels: SettingsLabels,
    forced: Option<usize>,
    format: u8,
) -> Result<()> {
    let prior = settings_info(bytes);
    let mut working = bytes.to_vec();
    if let Some(i) = prior {
        restore_source(&mut working, i);
    }
    let Some((index, p)) = find_original(&working) else {
        return Err(Error::Unsupported(
            "the Settings menu code does not match the known firmware".into(),
        ));
    };
    if !matches(
        &working,
        p.settings_dispatch,
        &bl(p.settings_dispatch, p.set_scene),
    ) || !matches(&working, p.settings_dispatch - 2, &[0x80, 0x88])
    {
        return Err(Error::Unsupported(
            "the Settings selection dispatcher does not match the known firmware".into(),
        ));
    }
    let stub = if labels.paramix.is_some() {
        Some(crate::paramix::installed_stub(&working)?.ok_or_else(||Error::Unsupported("PARAMIX Settings entry requires an installed Menu with a validated direct-launch stub".into()))?)
    } else {
        None
    };
    let preferred = forced.or(prior.map(|i| i.base));
    let mut base = None;
    if let Some(at) = preferred {
        if available(&working, at)? {
            base = Some(at);
        } else if forced.is_some() {
            return Err(Error::Capacity(
                "the requested Settings helper page is not unallocated erased flash".into(),
            ));
        }
    }
    if base.is_none() {
        for at in (0x90000..0x10F000).step_by(PAGE).rev() {
            if available(&working, at)? {
                base = Some(at);
                break;
            }
        }
    }
    let base=base.ok_or_else(||Error::Capacity("no unallocated erased 4 KiB page for the Settings helper; Paramix save slots and application regions were preserved".into()))?;
    if format == 2 {
        for e in shared_edits(p, base, labels, stub)
            .into_iter()
            .filter(|e| e.at < base || e.at >= base + PAGE)
        {
            working[e.at..e.at + e.after.len()].copy_from_slice(&e.after);
        }
        working[base..base + PAGE].copy_from_slice(&shared_page(base, index, labels, stub));
    } else {
        let label = labels
            .language
            .ok_or_else(|| Error::Unsupported("legacy helpers only support Language".into()))?;
        for e in legacy_edits(p, label, base) {
            working[e.at..e.at + e.after.len()].copy_from_slice(&e.after);
        }
        if format == 1 {
            let sum = helper_sum(&working, base).unwrap();
            working[base + 0x80..base + 0x88].copy_from_slice(MARKER_V1);
            for (at, val) in [
                (0x88, 1),
                (0x8C, base as u32),
                (0x90, label),
                (0x94, index as u32),
                (0x98, PAGE as u32),
                (0x9C, sum),
            ] {
                working[base + at..base + at + 4].copy_from_slice(&val.to_le_bytes());
            }
        }
    }
    bytes.copy_from_slice(&working);
    Ok(())
}

/// Merge independent Language/PARAMIX rows. None preserves an existing row;
/// this API never silently removes either feature or rewrites an existing label.
pub fn apply_shared(bytes: &mut [u8], language: Option<u32>, paramix: Option<u32>) -> Result<()> {
    let prior = settings_info(bytes);
    let old = prior.map(|i| i.labels).unwrap_or(SettingsLabels {
        language: None,
        paramix: None,
    });
    for (a, b) in [(old.language, language), (old.paramix, paramix)] {
        if a.is_some() && b.is_some() && a != b {
            return Err(Error::Unsupported(
                "Settings feature already uses a different label index".into(),
            ));
        }
    }
    let labels = SettingsLabels {
        language: language.or(old.language),
        paramix: paramix.or(old.paramix),
    };
    if labels.language.is_none() && labels.paramix.is_none() {
        return Err(Error::Unsupported("no Settings feature requested".into()));
    }
    if prior.is_some_and(|i| i.labels == labels) {
        return Ok(());
    }
    apply_complete(bytes, labels, None, 2)
}

pub fn apply_at(bytes: &mut [u8], label_index: u32, base: usize) -> Result<()> {
    let old = settings_labels(bytes).unwrap_or(SettingsLabels {
        language: None,
        paramix: None,
    });
    apply_complete(
        bytes,
        SettingsLabels {
            language: Some(label_index),
            paramix: old.paramix,
        },
        Some(base),
        2,
    )
}

pub fn reproduce_settings(bytes: &mut [u8], patched: &[u8]) -> Result<()> {
    let info = settings_info(patched)
        .ok_or_else(|| Error::Verify("Settings helper metadata/code is invalid".into()))?;
    apply_complete(bytes, info.labels, Some(info.base), info.format)
}

pub fn reproduce(bytes: &mut [u8], patched: &[u8], label_index: u32) -> Result<()> {
    if applied_label(patched) != Some(label_index) {
        return Err(Error::Verify(
            "runtime language label does not match".into(),
        ));
    }
    reproduce_settings(bytes, patched)
}

pub fn apply(bytes: &mut [u8], label_index: u32) -> Result<()> {
    apply_shared(bytes, Some(label_index), None)
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

    fn original_fixture(index: usize) -> Vec<u8> {
        let mut b = vec![255; 0x1000000];
        let p = &PROFILES[index];
        for e in legacy_edits(p, 0, FREE_AT).into_iter().take(3) {
            b[e.at..e.at + e.before.len()].copy_from_slice(&e.before);
        }
        b[p.settings_dispatch - 2..p.settings_dispatch].copy_from_slice(&[0x80, 0x88]);
        b[p.settings_dispatch..p.settings_dispatch + 4]
            .copy_from_slice(&bl(p.settings_dispatch, p.set_scene));
        let table = original_table(p);
        b[p.page1_table..p.page1_table + table.len()].copy_from_slice(&table);
        let state = state_pointer(p);
        b[p.state_site..p.state_site + state.len()].copy_from_slice(&state);
        b
    }

    #[test]
    fn relocated_helpers_support_each_profile_and_replay() {
        for i in 0..PROFILES.len() {
            let original = original_fixture(i);
            let mut b = original.clone();
            apply(&mut b, 1234).unwrap();
            assert_eq!(status(&b), Status::Applied);
            assert_eq!(helper_page(&b), Some(0x10E000));
            assert_eq!(applied_label(&b), Some(1234));
            assert_eq!(settings_labels(&b).unwrap().language, Some(1234));
            let mut replay = original;
            reproduce(&mut replay, &b, 1234).unwrap();
            assert_eq!(replay, b);
            let snapshot = b.clone();
            apply(&mut b, 1234).unwrap();
            assert_eq!(snapshot, b);
            assert!(apply(&mut b, 1235).is_err());
            assert_eq!(snapshot, b);
        }
    }

    #[test]
    fn language_does_not_move_or_duplicate_the_independent_home_menu_shortcut() {
        for profile in 0..PROFILES.len() {
            let mut bytes = original_fixture(profile);
            crate::paramix::install_home_fixture(&mut bytes, profile, 0x10E000, 824);
            let original = bytes.clone();
            apply(&mut bytes, 825).unwrap();
            assert_eq!(helper_page(&bytes), Some(0x10D000));
            assert_eq!(
                settings_labels(&bytes).unwrap(),
                SettingsLabels {
                    language: Some(825),
                    paramix: None
                }
            );
            assert_eq!(&bytes[0x10E000..0x10F000], &original[0x10E000..0x10F000]);
            assert_eq!(
                crate::paramix::home_menu_helper(&bytes).unwrap(),
                crate::paramix::home_menu_helper(&original).unwrap()
            );
            assert_eq!(
                crate::paramix::requested_shortcut_stub(&bytes).unwrap(),
                None
            );
            let mut replay = original;
            reproduce_settings(&mut replay, &bytes).unwrap();
            assert_eq!(replay, bytes);
        }
    }

    #[test]
    fn paramix_empty_pool_pages_are_never_helper_space() {
        let mut b = original_fixture(0);
        crate::paramix::install_fixture(&mut b, 0x100000);
        assert_eq!(status(&b), Status::Original);
        let original = b.clone();
        assert!(apply_at(&mut b, 2345, 0xFF000).is_err());
        assert_eq!(original, b);
        apply(&mut b, 2345).unwrap();
        assert_eq!(helper_page(&b), Some(0x10E000));
        for r in crate::paramix::reserved_ranges(&original).unwrap() {
            assert_eq!(original[r.clone()], b[r]);
        }
    }

    #[test]
    fn standalone_paramix_and_language_merge_in_both_orders() {
        for index in 0..PROFILES.len() {
            let mut original = original_fixture(index);
            crate::paramix::install_fixture(&mut original, 0x100000);
            let p = &PROFILES[index];
            let mut paramix_first = original.clone();
            apply_shared(&mut paramix_first, None, Some(2000)).unwrap();
            assert_eq!(status(&paramix_first), Status::Original);
            assert_eq!(
                settings_labels(&paramix_first),
                Some(SettingsLabels {
                    language: None,
                    paramix: Some(2000)
                })
            );
            let base = helper_page(&paramix_first).unwrap();
            assert_eq!(word(&paramix_first, base + 36), Some(2000));
            assert_eq!(word(&paramix_first, base + 40), Some(PARAMIX_SCENE));
            assert_eq!(paramix_first[p.page1_count()], 4);
            assert!(matches(
                &paramix_first,
                p.language_next_scene,
                &legacy_edits(p, 0, base)[2].before
            ));
            assert!(matches(
                &paramix_first,
                p.settings_dispatch,
                &bl(p.settings_dispatch, base + 0x60)
            ));
            apply_shared(&mut paramix_first, Some(2001), None).unwrap();
            assert_eq!(helper_page(&paramix_first), Some(base));
            assert_eq!(paramix_first[p.page1_count()], 5);
            assert_eq!(word(&paramix_first, base + 36), Some(2001));
            assert_eq!(word(&paramix_first, base + 48), Some(2000));
            assert_eq!(word(&paramix_first, base + 52), Some(PARAMIX_SCENE));
            let mut language_first = original.clone();
            apply_shared(&mut language_first, Some(2001), None).unwrap();
            apply_shared(&mut language_first, None, Some(2000)).unwrap();
            assert_eq!(language_first, paramix_first);
            let mut replay = original.clone();
            reproduce_settings(&mut replay, &paramix_first).unwrap();
            assert_eq!(replay, paramix_first);
            let mut replay = original.clone();
            apply_shared(&mut replay, None, Some(2000)).unwrap();
            reproduce_settings(&mut replay, &paramix_first).unwrap();
            assert_eq!(replay, paramix_first);
            // Every ordinary entry forwards directly to the original scene
            // setter; sentinel-only path loads validated Paramix launch.
            let d = dispatcher(p, base, 0xB6000);
            assert_eq!(d.len(), 20);
            assert_eq!(&d[8..12], &branch(base + 0x68, p.set_scene, false));
            assert_eq!(&d[16..20], &(alias_of(0xB6020) | 1).to_le_bytes());
            // Both reserved pools and the game setting's original More tuple
            // are retained byte-for-byte.
            assert_eq!(&paramix_first[base..base + 36], original_table(p));
            for range in crate::paramix::reserved_ranges(&original).unwrap() {
                assert_eq!(original[range.clone()], paramix_first[range]);
            }
        }
    }

    #[test]
    fn paramix_requires_a_real_direct_launch_stub() {
        let mut b = original_fixture(0);
        let before = b.clone();
        assert!(apply_shared(&mut b, None, Some(2000)).is_err());
        assert_eq!(before, b);
        crate::paramix::install_fixture(&mut b, 0x100000);
        b[0xB6020] ^= 1;
        let before = b.clone();
        assert!(apply_shared(&mut b, None, Some(2000)).is_err());
        assert_eq!(before, b);
    }

    #[test]
    fn v1_language_page_upgrades_to_shared_without_relocating() {
        let mut b = original_fixture(0);
        crate::paramix::install_fixture(&mut b, 0x100000);
        let base = 0x10D000;
        let p = &PROFILES[0];
        for e in legacy_edits(p, 2000, base) {
            b[e.at..e.at + e.after.len()].copy_from_slice(&e.after);
        }
        let sum = helper_sum(&b, base).unwrap();
        b[base + 0x80..base + 0x88].copy_from_slice(MARKER_V1);
        for (at, v) in [
            (0x88, 1),
            (0x8C, base as u32),
            (0x90, 2000),
            (0x94, 0),
            (0x98, PAGE as u32),
            (0x9C, sum),
        ] {
            b[base + at..base + at + 4].copy_from_slice(&v.to_le_bytes());
        }
        assert_eq!(helper_page(&b), Some(base));
        apply_shared(&mut b, None, Some(2001)).unwrap();
        assert_eq!(helper_page(&b), Some(base));
        assert_eq!(b.get(base + 0x80..base + 0x88), Some(MARKER_V2.as_slice()));
        assert_eq!(
            settings_labels(&b),
            Some(SettingsLabels {
                language: Some(2000),
                paramix: Some(2001)
            })
        );
    }

    #[test]
    fn corrupt_marker_or_trampoline_is_not_an_applied_patch() {
        let mut b = original_fixture(0);
        apply(&mut b, 1234).unwrap();
        for offset in [0x40, 0x80, 0x88, 0x8C, 0x90, 0x94, 0x98, 0x9C, 0xA0] {
            let mut bad = b.clone();
            bad[0x10E000 + offset] ^= 1;
            assert_eq!(status(&bad), Status::Unsupported, "offset {offset:x}");
            assert_eq!(helper_page(&bad), None);
        }
    }

    #[test]
    fn legacy_b6000_helpers_are_recognized_and_reproduced() {
        for i in 0..PROFILES.len() {
            let original = original_fixture(i);
            let mut old = original.clone();
            for e in legacy_edits(&PROFILES[i], 1234, FREE_AT) {
                old[e.at..e.at + e.after.len()].copy_from_slice(&e.after);
            }
            assert_eq!(status(&old), Status::Applied);
            assert_eq!(helper_page(&old), Some(FREE_AT));
            let mut replay = original;
            reproduce(&mut replay, &old, 1234).unwrap();
            assert_eq!(replay, old);
        }
    }

    #[test]
    fn no_available_page_leaves_every_byte_unchanged() {
        let source = original_fixture(0);
        let mut b = source.clone();
        b[0x90000..0x10F000].fill(0);
        let p = &PROFILES[0];
        for e in legacy_edits(p, 0, FREE_AT).into_iter().take(3) {
            b[e.at..e.at + e.before.len()].copy_from_slice(&e.before);
        }
        let table = original_table(p);
        b[p.page1_table..p.page1_table + table.len()].copy_from_slice(&table);
        b[p.settings_dispatch - 2..p.settings_dispatch].copy_from_slice(&[0x80, 0x88]);
        b[p.settings_dispatch..p.settings_dispatch + 4]
            .copy_from_slice(&bl(p.settings_dispatch, p.set_scene));
        let before = b.clone();
        assert_eq!(status(&b), Status::Original);
        assert!(matches!(apply(&mut b, 1234), Err(Error::Capacity(_))));
        assert_eq!(before, b);
    }

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
            assert!(0x40 + trampoline(p, FREE_AT).len() <= FREE_LEN);
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

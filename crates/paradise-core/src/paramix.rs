//! Read-only Paramix allocation discovery. An erased slot header/pool page is
//! still allocated: it must never be reused as archive growth or helper space.
use std::ops::Range;

use crate::error::{Error, Result};

pub const PAGE: usize = 0x1000;
/// Persisted Studio preference: keep the menu installed, but do not add its
/// shortcut automatically when another tool adds a Language row.
pub const F_NO_SHORTCUT: u32 = 128;
const FELD: u32 = 0x444C4546;
const PCAT: u32 = 0x54414350;
const PEXT: u32 = 0x54584550;
const TPRV: u32 = 0x56525054;
const ALIAS: u32 = 0x10000000;
const DELTA: usize = 0x11000;
const HOME_MARKER_V1: &[u8; 8] = b"PMHOME01";
const HOME_MARKER_V2: &[u8; 8] = b"PMHOME02";

/// Independent main-menu shortcut installed by current Paramix Studio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HomeMenuHelper {
    pub version: u32,
    pub page: usize,
    pub label: u32,
    pub profile: usize,
    pub stub: usize,
}

fn thumb_mov(register: u16, value: u16, top: bool) -> [u8; 4] {
    let hi = (if top { 0xF2C0 } else { 0xF240 }) | ((value >> 11 & 1) << 10) | (value >> 12);
    let lo = ((value >> 8 & 7) << 12) | (register << 8) | (value & 255);
    [hi as u8, (hi >> 8) as u8, lo as u8, (lo >> 8) as u8]
}
fn thumb_branch(from: usize, to: usize, link: bool) -> [u8; 4] {
    let off = (to as i64 - from as i64 - 4) as u32;
    let s = off >> 24 & 1;
    let j1 = !((off >> 23 & 1) ^ s) & 1;
    let j2 = !((off >> 22 & 1) ^ s) & 1;
    let hi = 0xF000 | (s << 10) | (off >> 12 & 1023);
    let lo = (if link { 0xD000 } else { 0x9000 }) | (j1 << 13) | (j2 << 11) | (off >> 1 & 2047);
    [hi as u8, (hi >> 8) as u8, lo as u8, (lo >> 8) as u8]
}
struct HomeProfile {
    init: usize,
    select: usize,
    scene: usize,
    table: usize,
    pool: usize,
    allocator: usize,
    scenes: &'static [u32],
}

const HOME_PROFILES: [HomeProfile; 4] = [
    HomeProfile {
        init: 0x4C436,
        select: 0x4C7C2,
        scene: 0x138CC,
        table: 0xA5254,
        pool: 0x2C41C,
        allocator: 0x1B158,
        scenes: &[119, 127, 128, 126, 125],
    },
    HomeProfile {
        init: 0x47752,
        select: 0x47ADE,
        scene: 0x8E948,
        table: 0x91CC0,
        pool: 0x294D8,
        allocator: 0x19918,
        scenes: &[115, 123, 124, 122, 121],
    },
    HomeProfile {
        init: 0x4579E,
        select: 0x45B2A,
        scene: 0x8A430,
        table: 0x8CA48,
        pool: 0x28C54,
        allocator: 0x1975C,
        scenes: &[112, 120, 121, 119, 118],
    },
    HomeProfile {
        init: 0x4CB6A,
        select: 0x4CEF6,
        scene: 0x13A5C,
        table: 0xA6CD0,
        pool: 0x2D508,
        allocator: 0x1B4D4,
        scenes: &[119, 127, 128, 126],
    },
];

impl HomeProfile {
    fn original_rows(&self) -> Vec<u8> {
        self.scenes
            .iter()
            .enumerate()
            .flat_map(|(i, &scene)| {
                [34 + i as u32, scene, 0]
                    .into_iter()
                    .flat_map(u32::to_le_bytes)
            })
            .collect()
    }

    fn pool_size(&self, version: u32) -> u8 {
        if version >= 2 && self.scenes.len() == 5 {
            14
        } else {
            12
        }
    }
}

fn home_code_matches(bytes: &[u8], info: HomeMenuHelper) -> bool {
    let profile = &HOME_PROFILES[info.profile];
    let HomeProfile {
        init,
        select,
        scene,
        table,
        pool,
        allocator,
        ..
    } = *profile;
    // Six rows need thirteen widget objects (one list + two labels per row).
    // V2 grows the original twelve-slot pool to fourteen, keeping one spare.
    let count = profile.pool_size(info.version);
    if bytes.get(pool..pool + 6) != Some([count, 0x20, 0x19, 0x46, 0x1A, 0x46].as_slice())
        || bytes.get(pool + 6..pool + 10)
            != Some(thumb_branch(pool + 6, allocator, true).as_slice())
    {
        return false;
    }
    let original = profile.original_rows();
    if bytes.get(table..table + original.len()) != Some(original.as_slice())
        || bytes.get(info.page..info.page + original.len()) != Some(original.as_slice())
        || bytes.get(init..init + 4) != Some(thumb_branch(init, info.page + 0x50, true).as_slice())
        || bytes.get(select..select + 4)
            != Some(thumb_branch(select, info.page + 0x60, true).as_slice())
    {
        return false;
    }
    let pointer = ALIAS + (info.page + 0x48 - DELTA) as u32;
    let mut init_code = thumb_mov(0, pointer as u16, false).to_vec();
    init_code.extend(thumb_mov(0, (pointer >> 16) as u16, true));
    init_code.extend([0x70, 0x47]);
    let mut dispatch = thumb_mov(1, 0x7FFE, false).to_vec();
    dispatch.extend([0x88, 0x42, 1, 0xD0]);
    dispatch.extend(thumb_branch(info.page + 0x68, scene, false));
    dispatch.extend([0, 0x49, 8, 0x47]);
    dispatch.extend((ALIAS + (info.stub + 0x20 - DELTA) as u32 | 1).to_le_bytes());
    bytes.get(info.page + 0x50..info.page + 0x5A) == Some(init_code.as_slice())
        && bytes.get(info.page + 0x60..info.page + 0x74) == Some(dispatch.as_slice())
        && bytes[info.page + 0x5A..info.page + 0x60]
            .iter()
            .all(|&v| v == 255)
        && bytes[info.page + 0x74..info.page + 0x80]
            .iter()
            .all(|&v| v == 255)
}

/// Recognize the complete owned home-menu reservation. A damaged marker must
/// fail closed rather than making its erased tail available to other tools.
pub fn home_menu_helper(bytes: &[u8]) -> Result<Option<HomeMenuHelper>> {
    let mut found = None;
    for page in (0x90000..0x10F000).step_by(PAGE) {
        let marker = bytes.get(page + 0x80..page + 0x88);
        let version = if marker == Some(HOME_MARKER_V1.as_slice()) {
            1
        } else if marker == Some(HOME_MARKER_V2.as_slice()) {
            2
        } else {
            continue;
        };
        let read = |n| u32_at(bytes, page + n).ok_or_else(bad);
        let info = HomeMenuHelper {
            version,
            page,
            label: read(0x90)?,
            profile: read(0x94)? as usize,
            stub: read(0xA0)? as usize,
        };
        let sum = |end| {
            bytes[page..page + end]
                .iter()
                .map(|&v| v as u32)
                .sum::<u32>()
        };
        let Some(tail) = bytes.get(page + 0xA8..page + PAGE) else {
            return Err(bad());
        };
        let profile = HOME_PROFILES.get(info.profile).ok_or_else(bad)?;
        let paramix_row = profile.scenes.len() * 12;
        if read(0x88)? != version
            || read(0x8C)? as usize != page
            || read(0x98)? as usize != PAGE
            || read(0x9C)? != sum(0x80)
            || read(0xA4)? != sum(0xA4)
            || info.label == u32::MAX
            || !tail.iter().all(|&v| v == 255)
            || !(0x90000..0x110000).contains(&info.stub)
            || info.stub % PAGE != 0
            || read(paramix_row)? != info.label
            || read(paramix_row + 4)? != 0x7FFE
            || read(paramix_row + 8)? != 0
            || !bytes[page + paramix_row + 12..page + 0x48]
                .iter()
                .all(|&v| v == 255)
            || read(0x48)? != ALIAS + (page - DELTA) as u32
            || read(0x4C)? != profile.scenes.len() as u32 + 1
            || read(0x70)? != (ALIAS + (info.stub + 0x20 - DELTA) as u32 | 1)
            || u32_at(bytes, info.stub + 0x20) != Some(0x4C01B672)
        {
            return Err(bad());
        }
        let program =
            from_alias(u32_at(bytes, info.stub + 0x28).ok_or_else(bad)?).ok_or_else(bad)?;
        if program % PAGE != 0
            || !(0x90000..0x110000).contains(&program)
            || u32_at(bytes, program + 0x140) != Some(FELD)
            || u32_at(bytes, program + 0x140 + 32) != Some(info.stub as u32)
        {
            return Err(bad());
        }
        if !home_code_matches(bytes, info) || found.replace(info).is_some() {
            return Err(bad());
        }
    }
    Ok(found)
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn bad() -> Error {
    Error::Unsupported(
        "Paramix allocation metadata is invalid; refusing to reuse erased reserved pages".into(),
    )
}

fn add(r: &mut Vec<Range<usize>>, bytes: &[u8], at: usize, len: usize) -> Result<()> {
    let end = at.checked_add(len).ok_or_else(bad)?;
    if at % PAGE != 0 || len == 0 || end > bytes.len() {
        return Err(bad());
    }
    r.push(at..(end + PAGE - 1) / PAGE * PAGE);
    Ok(())
}

fn from_alias(v: u32) -> Option<usize> {
    (ALIAS..0x11000000)
        .contains(&v)
        .then(|| (v - ALIAS) as usize + DELTA)
}

/// Every known owned page, including empty pool pages, registered apps and the
/// ID-page safety margin. Unknown/malformed Paramix layouts fail closed.
pub fn reserved_ranges(bytes: &[u8]) -> Result<Vec<Range<usize>>> {
    let mut ranges = Vec::new();
    if let Some(home) = home_menu_helper(bytes)? {
        ranges.push(home.page..home.page + PAGE);
    }
    // Both legacy and current six-field helpers own the complete page.
    if bytes.len() >= 0x110000 {
        ranges.push(0x10F000..0x110000);
    }
    // Emulator/DOOM save space may be erased but is not spare flash. Their
    // heterogeneous allocation formats are deliberately not guessed here.
    if matches!(
        u32_at(bytes, 0xA0140),
        Some(0x43584D50 | 0x37584D50 | 0x38584D50 | 0x31584D50)
    ) {
        ranges.push(0xA0000..0x110000);
    }
    for off in (0x90000..0x110000).step_by(PAGE) {
        let c = off + 0x140;
        if u32_at(bytes, c) != Some(FELD) {
            continue;
        }
        let version = u32_at(bytes, c + 4).ok_or_else(bad)?;
        let config_size = if version >= 11 { 620 } else { 608 };
        if !(4..=11).contains(&version) || c + config_size > bytes.len() {
            return Err(bad());
        }
        let read = |n| u32_at(bytes, c + n).map(|v| v as usize).ok_or_else(bad);
        let stub = read(32)?;
        if !(0x90000..0x110000).contains(&stub) || stub % PAGE != 0 {
            return Err(bad());
        }
        let target = ALIAS + (off - DELTA) as u32;
        // Locate the installed stub's pointer rather than assuming a version's
        // literal offset (old stubs used F0/118/148, current ones differ).
        if !(0..0x400)
            .step_by(4)
            .any(|n| u32_at(bytes, stub + n) == Some(target))
        {
            return Err(bad());
        }
        add(&mut ranges, bytes, stub, PAGE)?;
        add(&mut ranges, bytes, off, 0x10000)?;
        let cat = read(272)?;
        let count = read(276)?;
        if cat != 0 {
            if count > 2048
                || cat % PAGE != 0
                || u32_at(bytes, cat) != Some(PCAT)
                || u32_at(bytes, cat + 4) != Some(count as u32)
            {
                return Err(bad());
            }
            let mut end = cat.checked_add(8 + count * 28).ok_or_else(bad)?;
            if u32_at(bytes, end) == Some(PEXT) {
                end += 4 + count * 12;
            }
            if u32_at(bytes, end) == Some(TPRV) {
                let record = u32_at(bytes, end + 8).ok_or_else(bad)? as usize;
                let n = u32_at(bytes, end + 12).ok_or_else(bad)? as usize;
                if !(24..=128).contains(&record) || n > 256 {
                    return Err(bad());
                }
                end += 24 + record * n;
            }
            add(
                &mut ranges,
                bytes,
                cat,
                end.checked_sub(cat).ok_or_else(bad)?,
            )?;
        } else if count != 0 {
            return Err(bad());
        }
        let slots = read(16)?;
        if slots > 6 {
            return Err(bad());
        }
        if slots != 0 {
            let at = read(8)?;
            let size = read(12)?;
            let second = read(364)?;
            let first_count = read(368)?;
            if size < PAGE * 2
                || size % PAGE != 0
                || size > 0x800000
                || first_count > slots
                || (second != 0 && first_count == 0)
            {
                return Err(bad());
            }
            for i in 0..slots {
                let start = if second != 0 && i >= first_count {
                    second.checked_add((i - first_count) * size)
                } else {
                    at.checked_add(i * size)
                }
                .ok_or_else(bad)?;
                add(&mut ranges, bytes, start, size)?;
            }
        }
        if read(36)? & 0x1000 != 0 {
            if slots != 3 || read(8)? != 0xD25000 || read(12)? != 0x20000
                || read(364)? != 0 || read(368)? != 0
            {
                return Err(bad());
            }
            // The recovery journal and active-slot record precede the three slots.
            add(&mut ranges, bytes, 0xD23000, 2 * PAGE)?;
        }
        // V11 adds an explicitly owned extension after the six-field assets.
        // Erased pages in it are still save capacity, never spare patch space.
        if version >= 11 {
            let at = read(612)?;
            let pages = read(616)?;
            if at != 0 || pages != 0 {
                if !(0xF00000..0xFF0000).contains(&at)
                    || at % PAGE != 0
                    || pages == 0
                    || pages > (0xFF0000 - at) / PAGE
                {
                    return Err(bad());
                }
                add(&mut ranges, bytes, at, pages * PAGE)?;
            }
        }
        let id = read(28)?;
        if id < PAGE || id % PAGE != 0 || id + PAGE > bytes.len() {
            return Err(bad());
        }
        // Conservative original allocator boundary: one page before ID, plus
        // the game's ID/live-save/auxiliary region above it.
        ranges.push(id - PAGE..bytes.len());
        let apps = if version >= 5 { read(392)? } else { 0 };
        if apps > if version >= 8 { 12 } else { 6 } {
            return Err(bad());
        }
        for i in 0..apps {
            let entry = if i < 6 {
                396 + i * 16
            } else {
                504 + (i - 6) * 16
            };
            if let Some(a) = from_alias(read(entry)? as u32) {
                add(&mut ranges, bytes, a, 0x10000)?;
            }
        }
        for n in [500, 600]
            .into_iter()
            .filter(|&n| if n == 500 { version >= 7 } else { version >= 9 })
        {
            if let Some(a) = from_alias(read(n)? as u32) {
                add(&mut ranges, bytes, a, 0x10000)?;
            }
        }
    }
    Ok(ranges)
}

pub fn overlaps(a: Range<usize>, b: &Range<usize>) -> bool {
    a.start < b.end && b.start < a.end
}

/// A validated installed Menu with the fixed direct-launch entry at stub+20.
/// Older stubs without that entry remain protected allocations, but cannot be
/// exposed as a Settings action by guessing an entry point.
pub fn installed_stub(bytes: &[u8]) -> Result<Option<usize>> {
    reserved_ranges(bytes)?;
    let mut found = None;
    for off in (0x90000..0x110000).step_by(PAGE) {
        let c = off + 0x140;
        if u32_at(bytes, c) != Some(FELD) {
            continue;
        }
        let stub = u32_at(bytes, c + 32).ok_or_else(bad)? as usize;
        if u32_at(bytes, stub + 0x20) != Some(0x4C01B672)
            || u32_at(bytes, stub + 0x28) != Some(ALIAS + (off - DELTA) as u32)
        {
            return Ok(None);
        }
        if found.replace(stub).is_some() {
            return Err(bad());
        }
    }
    Ok(found)
}

/// A legacy installed menu whose Settings shortcut has not been opted out of.
/// Current Studio puts PARAMIX in the main menu, so Language must not add a
/// duplicate Settings action when its independent home helper is present.
/// Keep `installed_stub` independent of this preference: an existing helper
/// must still be recognized and validated when a shortcut is later disabled.
pub fn requested_shortcut_stub(bytes: &[u8]) -> Result<Option<usize>> {
    if home_menu_helper(bytes)?.is_some() {
        return Ok(None);
    }
    let Some(stub) = installed_stub(bytes)? else {
        return Ok(None);
    };
    let off = from_alias(u32_at(bytes, stub + 0x28).ok_or_else(bad)?).ok_or_else(bad)?;
    let flags = u32_at(bytes, off + 0x140 + 36).ok_or_else(bad)?;
    Ok((flags & F_NO_SHORTCUT == 0).then_some(stub))
}

/// An installed menu catalog may embed absolute addresses into the sprite pack.
pub(crate) fn uses_sprite_addresses(bytes: &[u8]) -> bool {
    (0x90000..0x110000).step_by(PAGE).any(|off| {
        let config = off + 0x140;
        u32_at(bytes, config) == Some(FELD)
            && u32_at(bytes, config + 272).is_some_and(|ptr| ptr != 0)
    })
}

pub fn guard_growth(bytes: &[u8], old_end: usize, new_end: usize) -> Result<()> {
    if new_end <= old_end {
        return Ok(());
    }
    if reserved_ranges(bytes)?
        .iter()
        .any(|r| overlaps(old_end..new_end, r))
    {
        return Err(Error::Capacity("patched assets would enter a Paramix reserved page (including empty save-slot headers/pool pages); rebuild the menu layout first".into()));
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn install_fixture(bytes: &mut [u8], low_end: usize) {
    let off = 0xB7000;
    let c = off + 0x140;
    let put =
        |b: &mut [u8], n: usize, v: usize| b[n..n + 4].copy_from_slice(&(v as u32).to_le_bytes());
    put(bytes, c, FELD as usize);
    put(bytes, c + 4, 9);
    put(bytes, c + 32, 0xB6000);
    put(bytes, c + 36, 15);
    put(bytes, 0xB6148, (ALIAS + (off - DELTA) as u32) as usize);
    put(bytes, 0xB6020, 0x4C01B672);
    put(bytes, 0xB6028, (ALIAS + (off - DELTA) as u32) as usize);
    put(bytes, c + 8, 0xC7000);
    put(bytes, c + 12, (low_end - 0xC7000) / 3);
    put(bytes, c + 16, 6);
    put(bytes, c + 28, 0xD29000);
    put(bytes, c + 364, 0xCEE000);
    put(bytes, c + 368, 3);
    for n in [272, 276, 392, 500, 600] {
        put(bytes, c + n, 0);
    }
}

#[cfg(test)]
pub(crate) fn install_home_fixture(bytes: &mut [u8], profile: usize, page: usize, label: u32) {
    install_home_fixture_version(bytes, profile, page, label, 2);
}

#[cfg(test)]
fn install_home_fixture_version(
    bytes: &mut [u8],
    profile: usize,
    page: usize,
    label: u32,
    version: u32,
) {
    install_fixture(bytes, 0x100000);
    let home = &HOME_PROFILES[profile];
    let HomeProfile {
        init,
        select,
        scene,
        table,
        pool,
        allocator,
        ..
    } = *home;
    bytes[page..page + PAGE].fill(255);
    let mut words = home.original_rows();
    bytes[table..table + words.len()].copy_from_slice(&words);
    for word in [label, 0x7FFE, 0] {
        words.extend(word.to_le_bytes());
    }
    bytes[page..page + words.len()].copy_from_slice(&words);
    bytes[page + 0x48..page + 0x4C].copy_from_slice(&(ALIAS + (page - DELTA) as u32).to_le_bytes());
    bytes[page + 0x4C..page + 0x50].copy_from_slice(&(home.scenes.len() as u32 + 1).to_le_bytes());
    let pointer = ALIAS + (page + 0x48 - DELTA) as u32;
    let mut code = thumb_mov(0, pointer as u16, false).to_vec();
    code.extend(thumb_mov(0, (pointer >> 16) as u16, true));
    code.extend([0x70, 0x47]);
    bytes[page + 0x50..page + 0x50 + code.len()].copy_from_slice(&code);
    let mut code = thumb_mov(1, 0x7FFE, false).to_vec();
    code.extend([0x88, 0x42, 1, 0xD0]);
    code.extend(thumb_branch(page + 0x68, scene, false));
    code.extend([0, 0x49, 8, 0x47]);
    code.extend((ALIAS + (0xB6020 - DELTA) as u32 | 1).to_le_bytes());
    bytes[page + 0x60..page + 0x60 + code.len()].copy_from_slice(&code);
    bytes[init..init + 4].copy_from_slice(&thumb_branch(init, page + 0x50, true));
    bytes[select..select + 4].copy_from_slice(&thumb_branch(select, page + 0x60, true));
    bytes[pool..pool + 6].copy_from_slice(&[home.pool_size(version), 0x20, 0x19, 0x46, 0x1A, 0x46]);
    bytes[pool + 6..pool + 10].copy_from_slice(&thumb_branch(pool + 6, allocator, true));
    bytes[page + 0x80..page + 0x88].copy_from_slice(if version == 1 {
        HOME_MARKER_V1
    } else {
        HOME_MARKER_V2
    });
    let sum = bytes[page..page + 0x80]
        .iter()
        .map(|&v| v as u32)
        .sum::<u32>();
    for (off, value) in [
        (0x88, version),
        (0x8C, page as u32),
        (0x90, label),
        (0x94, profile as u32),
        (0x98, PAGE as u32),
        (0x9C, sum),
        (0xA0, 0xB6000),
    ] {
        bytes[page + off..page + off + 4].copy_from_slice(&value.to_le_bytes());
    }
    let sum = bytes[page..page + 0xA4]
        .iter()
        .map(|&v| v as u32)
        .sum::<u32>();
    bytes[page + 0xA4..page + 0xA8].copy_from_slice(&sum.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn home_menu_shortcut_has_an_independent_reservation_and_no_settings_duplicate() {
        for profile in 0..HOME_PROFILES.len() {
            let mut bytes = vec![255; 0x1000000];
            install_home_fixture(&mut bytes, profile, 0x10E000, 824);
            let info = home_menu_helper(&bytes).unwrap().unwrap();
            assert_eq!(info.profile, profile);
            assert_eq!(info.label, 824);
            assert_eq!(installed_stub(&bytes).unwrap(), Some(0xB6000));
            assert_eq!(requested_shortcut_stub(&bytes).unwrap(), None);
            assert!(reserved_ranges(&bytes)
                .unwrap()
                .iter()
                .any(|r| r.contains(&0x10EFFF)));
            let mut bad_code = bytes.clone();
            bad_code[0x10E060] ^= 1;
            assert!(home_menu_helper(&bad_code).is_err());
            let mut bad_tail = bytes.clone();
            bad_tail[0x10EFFF] = 0;
            assert!(reserved_ranges(&bad_tail).is_err());
        }
    }

    #[test]
    fn legacy_home_helper_requires_the_original_pool_and_v2_requires_the_expanded_pool() {
        for (profile, home) in HOME_PROFILES.iter().enumerate() {
            let pool = home.pool;
            for version in [1, 2] {
                let mut bytes = vec![255; 0x1000000];
                install_home_fixture_version(&mut bytes, profile, 0x10E000, 824, version);
                assert_eq!(home_menu_helper(&bytes).unwrap().unwrap().version, version);
                assert_eq!(requested_shortcut_stub(&bytes).unwrap(), None);
                bytes[pool] = home.pool_size(version) ^ 2;
                assert!(
                    home_menu_helper(&bytes).is_err(),
                    "profile {profile}, version {version}"
                );
                assert!(reserved_ranges(&bytes).is_err());
            }
        }
    }

    #[test]
    fn erased_slot_headers_still_bound_archive_growth() {
        let mut b = vec![255; 0x1000000];
        install_fixture(&mut b, 0x100000);
        guard_growth(&b, 0xCEDB64, 0xCEE000).unwrap();
        assert!(guard_growth(&b, 0xCEDB64, 0xCEE001).is_err());
        let r = reserved_ranges(&b).unwrap();
        assert!(r.iter().any(|r| r.contains(&0xCEE000)));
        assert!(!r.iter().any(|r| r.contains(&0x10E000)));
    }
    #[test]
    fn version_eleven_reserves_erased_pool_extension_and_rejects_bad_geometry() {
        let mut original = vec![255; 0x1000000];
        install_fixture(&mut original, 0x100000);
        assert!(reserved_ranges(&original).is_ok()); // Existing version nine.
        let c = 0xB7140;
        original[c + 4..c + 8].copy_from_slice(&11u32.to_le_bytes());
        for (at, pages, valid) in [
            (0, 0, true),
            (0xFB7000, 57, true),
            (0xFB7001, 56, false),
            (0xFB7000, 58, false),
            (0xFF0000, 1, false),
            (0xEFF000, 1, false),
            (0, 57, false),
            (0xFB7000, 0, false),
        ] {
            let mut bytes = original.clone();
            bytes[c + 612..c + 616].copy_from_slice(&(at as u32).to_le_bytes());
            bytes[c + 616..c + 620].copy_from_slice(&(pages as u32).to_le_bytes());
            let result = reserved_ranges(&bytes);
            assert_eq!(result.is_ok(), valid, "at={at:x}, pages={pages}");
            if valid && pages != 0 {
                assert!(result.unwrap().iter().any(|r| r == &(0xFB7000..0xFF0000)));
                assert!(bytes[0xFB7000..0xFF0000].iter().all(|&b| b == 255));
            }
        }
    }

    #[test]
    fn malformed_slot_size_does_not_turn_into_free_space() {
        let mut b = vec![255; 0x1000000];
        install_fixture(&mut b, 0x100000);
        b[0xB7140 + 12..0xB7140 + 16].fill(255);
        assert!(reserved_ranges(&b).is_err());
    }

    #[test]
    fn shortcut_opt_out_does_not_hide_the_installed_menu_identity() {
        let mut b = vec![255; 0x1000000];
        install_fixture(&mut b, 0x100000);
        assert_eq!(installed_stub(&b).unwrap(), Some(0xB6000));
        assert_eq!(requested_shortcut_stub(&b).unwrap(), Some(0xB6000));
        let at = 0xB7140 + 36;
        let flags = u32_at(&b, at).unwrap();
        b[at..at + 4].copy_from_slice(&(flags | F_NO_SHORTCUT).to_le_bytes());
        assert_eq!(installed_stub(&b).unwrap(), Some(0xB6000));
        assert_eq!(requested_shortcut_stub(&b).unwrap(), None);
        b[at..at + 4].copy_from_slice(&flags.to_le_bytes());
        assert_eq!(requested_shortcut_stub(&b).unwrap(), Some(0xB6000));
    }
}

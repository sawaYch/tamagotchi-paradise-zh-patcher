use std::collections::BTreeMap;

use crate::arc;
use crate::error::{Error, Result};
use crate::rom::ASSET_BASE;
use crate::sprite::SpriteHeader;

const HELPER: usize = 0x10F000;
const REGION: usize = 0xF00000;
const REGION_END: usize = 0xFF0000;
const CELLS: usize = 64;
const RECORD: usize = 4 + CELLS * 4;

fn word(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        bytes.get(at..at.checked_add(4)?)?.try_into().ok()?,
    ))
}

/// Rebase only the external cell pointers listed by the Paramix SIXB table.
/// Returns the sprite-relative locations that verification may compare after rebasing.
pub(crate) fn relocate(original: &[u8], archive: &mut [u8]) -> Result<BTreeMap<usize, Vec<usize>>> {
    let mut sites = BTreeMap::new();
    if original.get(HELPER..HELPER + 4) != Some(b"SIX1")
        || original.get(REGION..REGION + 4) != Some(b"SIXB")
    {
        return Ok(sites);
    }
    let bad = || Error::Format("invalid Paramix SIXB rain relocation table".into());
    let read = |data: &[u8], at| word(data, at).ok_or_else(bad);
    let count = read(original, REGION + 4)? as usize;
    let external_end = REGION_END.min(original.len());
    if count == 0
        || count > (external_end - REGION - 8) / RECORD
        || ASSET_BASE + archive.len() > REGION
    {
        return Err(bad());
    }
    let data_start = REGION + 8 + count * RECORD;
    let pack_start = read(archive, 36)? as usize;
    let pack_len = read(archive, 44)? as usize;
    let pack_end = pack_start.checked_add(pack_len).ok_or_else(bad)?;
    if pack_start < 64 || pack_end > archive.len() || pack_len < 4 {
        return Err(bad());
    }
    let sprite_count = read(archive, pack_start)? as usize;
    if sprite_count == 0 || sprite_count % 4 != 0 || sprite_count > pack_len {
        return Err(bad());
    }
    let sprite_count = sprite_count / 4;
    let original_count = read(original, HELPER + 16)? as usize;
    for i in 0..count {
        let record = REGION + 8 + i * RECORD;
        let index = read(original, record)? as usize;
        if index < original_count || index >= sprite_count || sites.contains_key(&index) {
            return Err(bad());
        }
        let offset = read(archive, pack_start + index * 4)? as usize;
        let end = if index + 1 == sprite_count {
            pack_len
        } else {
            read(archive, pack_start + (index + 1) * 4)? as usize
        };
        if offset < sprite_count * 4 || offset > end || end > pack_len {
            return Err(bad());
        }
        let start = pack_start + offset;
        let header = SpriteHeader::parse(&archive[start..pack_start + end]).ok_or_else(bad)?;
        let pixels = header.pixel_data_offset as usize;
        if header.num_sprites as usize != CELLS || pixels < 24 || pixels + 8 * CELLS > end - offset
        {
            return Err(bad());
        }
        let mut pointers = Vec::new();
        for cell in 0..CELLS {
            let rel = read(original, record + 4 + cell * 4)?;
            if rel == u32::MAX {
                continue;
            }
            let target = data_start.checked_add(rel as usize).ok_or_else(bad)?;
            let at = start + pixels + cell * 8;
            let len = read(archive, at + 4)? as usize;
            if len == 0 || target >= external_end || len > external_end - target {
                return Err(bad());
            }
            let offset = (target - (ASSET_BASE + start + pixels)) as u32;
            archive[at..at + 4].copy_from_slice(&offset.to_le_bytes());
            pointers.push(at - start);
        }
        sites.insert(index, pointers);
    }
    let checksum = arc::checksum(archive)?;
    archive[4..8].copy_from_slice(&checksum.to_le_bytes());
    Ok(sites)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arc::{ArcFile, Archive};
    use crate::sprite::SpritePack;

    #[test]
    fn rebases_the_rain_offset_but_preserves_local_cells() {
        fn put(b: &mut [u8], at: usize, value: u32) {
            b[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }
        let mut original = vec![0; REGION + 4096];
        original[HELPER..HELPER + 4].copy_from_slice(b"SIX1");
        original[REGION..REGION + 4].copy_from_slice(b"SIXB");
        put(&mut original, HELPER + 16, 1);
        put(&mut original, REGION + 4, 1);
        put(&mut original, REGION + 8, 1);
        original[REGION + 12..REGION + 8 + RECORD].fill(0xFF);
        put(&mut original, REGION + 12, 16);
        let target = REGION + 8 + RECORD + 16;
        let mut rain = vec![0; 24 + CELLS * 8];
        rain[6] = CELLS as u8;
        rain[20] = 24;
        put(&mut rain, 28, 17);
        put(&mut rain, 32, 0x1234);
        let archive = |pad| {
            let pack = SpritePack {
                blobs: vec![vec![0; pad], rain.clone()],
            }
            .build();
            Archive {
                files: vec![
                    ArcFile {
                        flags: 0,
                        data: vec![],
                    },
                    ArcFile {
                        flags: 0,
                        data: pack,
                    },
                    ArcFile {
                        flags: 0,
                        data: vec![],
                    },
                ],
            }
            .build()
        };
        let mut first = archive(24);
        let mut moved = archive(24 + 0x1000);
        let first_at = 64 + 8 + 24 + 24;
        let moved_at = first_at + 0x1000;
        assert_eq!(relocate(&original, &mut first).unwrap()[&1], [24]);
        relocate(&original, &mut moved).unwrap();
        assert_eq!(
            ASSET_BASE + moved_at + word(&moved, moved_at).unwrap() as usize,
            target
        );
        assert_eq!(
            word(&first, first_at).unwrap() - word(&moved, moved_at).unwrap(),
            0x1000
        );
        assert_eq!(word(&moved, moved_at + 8), Some(0x1234));
        assert!(arc::checksum_ok(&moved));
        put(&mut original, REGION + 12, u32::MAX - 1);
        assert!(relocate(&original, &mut moved).is_err());
    }
}

use std::ops::Range;

use crate::error::{fmt_err, Error, Result};
use crate::rom::Rom;

const GROWTH_PAGE: usize = 0x107000;
const STATE: usize = GROWTH_PAGE + 0xFE0;
const STATUS: usize = STATE + 12;
const NAMES: usize = 86;
const FIELD_STATE: usize = STATE + 16;
const FIELD_STATUS: usize = FIELD_STATE + 4;
const FIELDS: usize = 3;

fn word(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

/// Pending Chinese names are identified by the transplant manifest and their
/// character records, not by the visual shape of an already repurposed glyph.
pub(crate) fn pending(rom: &Rom, strings: usize) -> Result<Option<Range<usize>>> {
    let bytes = rom.bytes();
    if bytes.get(STATE..STATE + 4) != Some(b"NZN1") {
        return Ok(None);
    }
    let invalid = || Error::Format("invalid Paramix Neon name manifest".into());
    let encoded = word(bytes, STATE + 4).ok_or_else(invalid)?;
    let status = word(bytes, STATUS).ok_or_else(invalid)?;
    let first = (encoded & 0xffff) as usize;
    let end = first + NAMES;
    let field_status = if bytes.get(FIELD_STATE..FIELD_STATE + 4) == Some(b"NZF1") {
        Some(word(bytes, FIELD_STATUS).ok_or_else(invalid)?)
    } else {
        None
    };
    if !matches!(rom.info().edition, Some(5 | 6))
        || bytes.get(GROWTH_PAGE..GROWTH_PAGE + 4) != Some(b"NRGW")
        || word(bytes, STATE + 8) != Some(!encoded)
        || encoded >> 16 != NAMES as u32
        || status > 1
        || end > strings
        || field_status.is_some_and(|value| value > 1 || end + FIELDS > strings)
        || (status == 0 && field_status == Some(1))
    {
        return Err(invalid());
    }
    let data = rom.data_pack();
    let at = |i: usize| word(data, i * 4).and_then(|n| (n as usize).checked_mul(2));
    let records = data
        .get(at(11).ok_or_else(invalid)?..at(12).ok_or_else(invalid)?)
        .filter(|records| records.len() % 30 == 0)
        .ok_or_else(invalid)?;
    let mut counts = [0; 3];
    let mut used = [false; NAMES];
    for record in records.chunks_exact(30) {
        let id = u16::from_le_bytes([record[0], record[1]]);
        let field = u16::from_le_bytes([record[14], record[15]]);
        let stage = u16::from_le_bytes([record[6], record[7]]);
        if id >= 9000 || !(7..=9).contains(&field) || !(3..=5).contains(&stage) {
            continue;
        }
        let name = u16::from_le_bytes([record[4], record[5]]) as usize;
        if !(first..end).contains(&name) || used[name - first] {
            return Err(invalid());
        }
        used[name - first] = true;
        counts[field as usize - 7] += 1;
    }
    if counts != [24, 38, 24] {
        return fmt_err("Neon name manifest does not match all 86 character records");
    }
    let pending_start = if status == 0 { first } else { end };
    let pending_end = if field_status == Some(0) { end + FIELDS } else { end };
    Ok((pending_start < pending_end).then_some(pending_start..pending_end))
}

pub(crate) fn mark_converted(bytes: &mut [u8]) {
    bytes[STATUS..STATUS + 4].copy_from_slice(&1u32.to_le_bytes());
    if bytes.get(FIELD_STATE..FIELD_STATE + 4) == Some(b"NZF1") {
        bytes[FIELD_STATUS..FIELD_STATUS + 4].copy_from_slice(&1u32.to_le_bytes());
    }
}

pub(crate) fn converted(bytes: &[u8]) -> bool {
    word(bytes, STATUS) == Some(1)
}

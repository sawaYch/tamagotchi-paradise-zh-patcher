use crate::error::{fmt_err, Result};

/// One language's string table: a word-offset index followed by u16 strings.
///
/// Word 0 points at the last two index words (an unused "string 0"), word 1 is the
/// index length, and every later word is the start of the next string; the final
/// index word is the end of the data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StringTable {
    /// Strings with ids 1.., each including its terminating NUL word.
    pub strings: Vec<Vec<u16>>,
}

pub const FIRST_ID: usize = 1;

impl StringTable {
    pub fn parse(data: &[u8]) -> Result<StringTable> {
        if data.len() % 2 != 0 || data.len() < 8 {
            return fmt_err("string table size");
        }
        let words: Vec<u16> = data
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        let index_len = words[1] as usize;
        if index_len < 3 || index_len > words.len() {
            return fmt_err("string table index length");
        }
        if words[0] as usize != index_len - 2 {
            return fmt_err("string table header mismatch");
        }
        if words[index_len - 1] as usize != words.len() {
            return fmt_err("string table end offset mismatch");
        }
        let count = index_len - 2;
        let mut strings = Vec::with_capacity(count);
        for i in 1..=count {
            let (a, b) = (words[i] as usize, words[i + 1] as usize);
            if a > b || b > words.len() || a < index_len {
                return fmt_err(format!("string {i} offsets invalid"));
            }
            strings.push(words[a..b].to_vec());
        }
        let t = StringTable { strings };
        Ok(t)
    }

    pub fn serialize(&self) -> Vec<u8> {
        let count = self.strings.len();
        let index_len = count + 2;
        let total: usize = index_len + self.strings.iter().map(|s| s.len()).sum::<usize>();
        let mut words: Vec<u16> = Vec::with_capacity(total);
        words.push((index_len - 2) as u16);
        let mut cursor = index_len;
        for s in &self.strings {
            words.push(cursor as u16);
            cursor += s.len();
        }
        words.push(cursor as u16);
        for s in &self.strings {
            words.extend_from_slice(s);
        }
        let mut out = Vec::with_capacity(words.len() * 2);
        for w in words {
            out.extend_from_slice(&w.to_le_bytes());
        }
        out
    }

    pub fn fits_u16(&self) -> bool {
        self.strings.len() + 2 + self.strings.iter().map(|s| s.len()).sum::<usize>()
            <= u16::MAX as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let t = StringTable {
            strings: vec![
                vec![0x131, 0x132, 0],
                vec![0x140, 0],
                vec![0xf000, 0x150, 0],
            ],
        };
        let b = t.serialize();
        assert_eq!(StringTable::parse(&b).unwrap(), t);
        assert_eq!(StringTable::parse(&b).unwrap().serialize(), b);
    }

    #[test]
    fn rejects_truncated() {
        assert!(StringTable::parse(&[0, 0, 0]).is_err());
        assert!(StringTable::parse(&[0u8; 16]).is_err());
    }
}

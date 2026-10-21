use std::collections::HashMap;

use encoding_rs::{EUC_KR, GBK};

pub const CODE_BASE: u16 = 481;
pub const SYMBOL_GLYPHS: usize = 480;
const IDEOGRAPHIC_SPACE: char = '\u{3000}';

pub struct Charset {
    chars: Vec<char>,
    index: HashMap<char, usize>,
    hangul_end: usize,
    hanzi_start: usize,
}

fn decode_pairs(
    enc: &'static encoding_rs::Encoding,
    lead: std::ops::RangeInclusive<u8>,
    skip: impl Fn(u8, u8) -> bool,
) -> Vec<char> {
    let mut v = Vec::new();
    for a in lead {
        for b in 0xA1u8..=0xFE {
            if skip(a, b) {
                continue;
            }
            let pair = [a, b];
            let (s, _, bad) = enc.decode(&pair);
            if !bad {
                let mut it = s.chars();
                if let (Some(c), None) = (it.next(), it.next()) {
                    v.push(c);
                }
            }
        }
    }
    v.sort_unstable();
    v.dedup();
    v
}

/// Font code of a Latin character: space, digits, +, -, !, ?, &, A-Z and a-z.
pub fn latin_code(c: char) -> Option<u16> {
    Some(match c {
        ' ' => 305,
        '0'..='9' => 306 + (c as u16 - '0' as u16),
        '+' => 316,
        '-' => 317,
        'A'..='Z' => 318 + (c as u16 - 'A' as u16),
        'a'..='z' => 344 + (c as u16 - 'a' as u16),
        '!' => 187,
        '?' => 188,
        '&' => 189,
        _ => return None,
    })
}

pub fn latin_char(code: u16) -> Option<char> {
    (' '..='z').find(|&c| latin_code(c) == Some(code))
}

impl Charset {
    /// Reconstructs the CJK font charset: KS X 1001 Hangul, two spaces, Bopomofo,
    /// parenthesised numerals, then GB2312 hanzi, each block in Unicode order.
    pub fn standard() -> Charset {
        let mut chars = decode_pairs(EUC_KR, 0xB0..=0xC8, |_, _| false);
        let hangul_end = chars.len();
        chars.push(IDEOGRAPHIC_SPACE);
        chars.push(IDEOGRAPHIC_SPACE);
        chars.extend((0x3105u32..=0x3129).filter_map(char::from_u32));
        chars.extend((0x3220u32..=0x3229).filter_map(char::from_u32));
        let hanzi_start = chars.len();
        chars.extend(decode_pairs(GBK, 0xB0..=0xF7, |a, b| a == 0xD7 && b > 0xF9));
        let mut index = HashMap::with_capacity(chars.len());
        for (i, &c) in chars.iter().enumerate() {
            index.entry(c).or_insert(i);
        }
        Charset {
            chars,
            index,
            hangul_end,
            hanzi_start,
        }
    }

    pub fn len(&self) -> usize {
        self.chars.len()
    }

    pub fn is_empty(&self) -> bool {
        self.chars.is_empty()
    }

    pub fn glyph_total(&self) -> usize {
        SYMBOL_GLYPHS + self.chars.len()
    }

    pub fn hangul_count(&self) -> usize {
        self.hangul_end
    }

    pub fn hanzi_start(&self) -> usize {
        self.hanzi_start
    }

    pub fn index_of(&self, c: char) -> Option<usize> {
        self.index.get(&c).copied()
    }

    pub fn code_of(&self, c: char) -> Option<u16> {
        self.index_of(c).map(|i| CODE_BASE + i as u16)
    }

    pub fn char_of_code(&self, code: u16) -> Option<char> {
        let i = code.checked_sub(CODE_BASE)? as usize;
        self.chars.get(i).copied()
    }

    pub fn glyph_of_code(code: u16) -> usize {
        code as usize - 1
    }

    pub fn code_of_glyph(glyph: usize) -> u16 {
        glyph as u16 + 1
    }

    pub fn is_hanzi_code(&self, code: u16) -> bool {
        match (code as usize).checked_sub(CODE_BASE as usize) {
            Some(i) => i >= self.hanzi_start && i < self.chars.len(),
            None => false,
        }
    }

    pub fn is_hangul_code(&self, code: u16) -> bool {
        match (code as usize).checked_sub(CODE_BASE as usize) {
            Some(i) => i < self.hangul_end,
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_known_layout() {
        let cs = Charset::standard();
        assert_eq!(cs.hangul_count(), 2350);
        assert_eq!(cs.len(), 9162);
        assert_eq!(cs.glyph_total(), 9642);
        assert_eq!(cs.hanzi_start(), 2350 + 2 + 37 + 10);
        assert_eq!(cs.index_of('가'), Some(0));
        assert_eq!(cs.index_of('ㄅ'), Some(2352));
        assert_eq!(cs.index_of('㈠'), Some(2352 + 37));
        assert_eq!(cs.index_of('一'), Some(2399));
        assert_eq!(cs.char_of_code(481 + 2399), Some('一'));
        assert!(cs.index_of('個').is_none());
        assert!(cs.index_of('后').is_some());
    }
}

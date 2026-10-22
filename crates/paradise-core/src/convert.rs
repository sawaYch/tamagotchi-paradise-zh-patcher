use ferrous_opencc::config::BuiltinConfig;
use ferrous_opencc::OpenCC;

use std::cell::RefCell;

use crate::charset::{latin_code, Charset};
use crate::error::{Error, Result};
use crate::mapping::{view_chars, Mapping};

const PUA_BASE: u32 = 0x10_0000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Variant {
    #[default]
    HongKong,
    Taiwan,
    Standard,
}

impl Variant {
    pub fn label(self) -> &'static str {
        match self {
            Variant::HongKong => "Traditional (Hong Kong)",
            Variant::Taiwan => "Traditional (Taiwan)",
            Variant::Standard => "Traditional (OpenCC standard)",
        }
    }

    fn config(self) -> BuiltinConfig {
        match self {
            Variant::HongKong => BuiltinConfig::S2hk,
            Variant::Taiwan => BuiltinConfig::S2tw,
            Variant::Standard => BuiltinConfig::S2t,
        }
    }
}

/// One position in a converted string: the original code and, for hanzi, the target character.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token {
    pub code: u16,
    pub target: Option<char>,
}

/// Source code of a token for a character that has no code of its own yet.
pub const INSERTED: u16 = u16::MAX;

pub struct Converter {
    cc: OpenCC,
    mapping: Mapping,
    hits: RefCell<Vec<usize>>,
}

/// Variant forms OpenCC leaves alone but that are not valid in Traditional text.
fn supplement(c: char) -> char {
    match c {
        '户' => '戶',
        '温' => '溫',
        other => other,
    }
}

fn is_ideograph(c: char) -> bool {
    matches!(c as u32, 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF)
}

/// Token for a literal replacement character, or None if it cannot be shown.
fn literal_token(cs: &Charset, c: char) -> Option<Token> {
    if let Some(code) = latin_code(c) {
        return Some(Token { code, target: None });
    }
    if (PUA_BASE..=PUA_BASE + 0xFFFF).contains(&(c as u32)) {
        return Some(Token {
            code: (c as u32 - PUA_BASE) as u16,
            target: None,
        });
    }
    if let Some(code) = cs.code_of(c) {
        let target = cs.is_hanzi_code(code).then_some(c);
        return Some(Token { code, target });
    }
    is_ideograph(c).then_some(Token {
        code: INSERTED,
        target: Some(c),
    })
}

pub fn code_to_char(cs: &Charset, code: u16) -> char {
    if cs.is_hanzi_code(code) {
        cs.char_of_code(code).expect("hanzi code maps to a char")
    } else {
        char::from_u32(PUA_BASE + code as u32).expect("plane 16 code point")
    }
}

impl Converter {
    pub fn new(variant: Variant) -> Result<Converter> {
        Converter::with_mapping(variant, Mapping::default(), &Charset::standard())
    }

    /// Like `new`, with user replacements applied before conversion. Every replacement
    /// character must be Latin text, a character in the font charset, or an ideograph.
    pub fn with_mapping(variant: Variant, mapping: Mapping, cs: &Charset) -> Result<Converter> {
        let cc = OpenCC::from_config(variant.config())
            .map_err(|e| Error::Unsupported(format!("conversion dictionary: {e}")))?;
        for i in 0..mapping.len() {
            if let Some(&bad) = mapping
                .value(i)
                .iter()
                .find(|&&c| literal_token(cs, c).is_none())
            {
                return Err(Error::Mapping(format!(
                    "\"{}\": cannot show the character \"{bad}\" (use letters, digits, space, + - ! ? & or Chinese characters)",
                    mapping.key(i)
                )));
            }
        }
        let mut conv = Converter {
            cc,
            mapping: Mapping::default(),
            hits: RefCell::new(Vec::new()),
        };
        let unchanged: Vec<bool> = (0..mapping.len())
            .map(|i| conv.is_identity(cs, &mapping, i))
            .collect();
        conv.mapping = mapping.without(|i| unchanged[i]);
        conv.hits = RefCell::new(vec![0; conv.mapping.len()]);
        Ok(conv)
    }

    /// True if an entry's replacement is exactly what normal conversion produces, as in an
    /// unedited exported template; such entries are dropped so they use normal conversion.
    fn is_identity(&self, cs: &Charset, mapping: &Mapping, i: usize) -> bool {
        let mut codes = Vec::new();
        for &c in mapping.key_chars(i) {
            let code = if (PUA_BASE..=PUA_BASE + 0xFFFF).contains(&(c as u32)) {
                (c as u32 - PUA_BASE) as u16
            } else if let Some(code) = latin_code(c).or_else(|| cs.code_of(c)) {
                code
            } else {
                return false;
            };
            codes.push(code);
        }
        let out: Vec<char> = self
            .convert_plain(cs, &codes)
            .iter()
            .flat_map(|t| match t.target {
                Some(c) => vec![c],
                None => view_chars(cs, &[t.code]),
            })
            .collect();
        out == mapping.value(i)
    }

    /// How many times each mapping entry matched, as (key, count) in match-priority order.
    pub fn mapping_hits(&self) -> Vec<(String, usize)> {
        let hits = self.hits.borrow();
        (0..self.mapping.len())
            .map(|i| (self.mapping.key(i), hits[i]))
            .collect()
    }

    pub fn convert_str(&self, s: &str) -> String {
        self.cc.convert(s)
    }

    /// Converts the string of font codes with this id (as listed by `strings`). Without a
    /// mapping there is one token per input code and non-hanzi codes pass through
    /// untouched; mapped text yields its own tokens.
    pub fn convert_codes(&self, cs: &Charset, id: usize, codes: &[u16]) -> Vec<Token> {
        if self.mapping.is_empty() {
            return self.convert_plain(cs, codes);
        }
        let view = view_chars(cs, codes);
        let mut out = Vec::with_capacity(codes.len());
        let (mut start, mut i) = (0, 0);
        while i < codes.len() {
            let Some(m) = self.mapping.match_at(&view, i, id) else {
                i += 1;
                continue;
            };
            out.extend(self.convert_plain(cs, &codes[start..i]));
            out.extend(
                self.mapping
                    .value(m)
                    .iter()
                    .map(|&c| literal_token(cs, c).expect("validated")),
            );
            self.hits.borrow_mut()[m] += 1;
            i += self.mapping.key_len(m);
            start = i;
        }
        out.extend(self.convert_plain(cs, &codes[start..]));
        out
    }

    fn convert_plain(&self, cs: &Charset, codes: &[u16]) -> Vec<Token> {
        if codes.is_empty() {
            return Vec::new();
        }
        let src: String = codes.iter().map(|&c| code_to_char(cs, c)).collect();
        let out: Vec<char> = self.cc.convert(&src).chars().collect();
        let aligned = out.len() == codes.len();
        codes
            .iter()
            .enumerate()
            .map(|(i, &code)| {
                if !cs.is_hanzi_code(code) {
                    return Token { code, target: None };
                }
                let source = cs.char_of_code(code).unwrap();
                let target = if aligned {
                    out[i]
                } else {
                    let single = self.cc.convert(&source.to_string());
                    let mut it = single.chars();
                    match (it.next(), it.next()) {
                        (Some(c), None) => c,
                        _ => source,
                    }
                };
                let target = if target == source {
                    supplement(source)
                } else {
                    target
                };
                Token {
                    code,
                    target: Some(target),
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_hk_text_with_passthrough() {
        let cs = Charset::standard();
        let conv = Converter::new(Variant::HongKong).unwrap();
        let mut codes: Vec<u16> = "个们".chars().map(|c| cs.code_of(c).unwrap()).collect();
        codes.insert(1, 0x30);
        codes.push(0);
        let t = conv.convert_codes(&cs, 1, &codes);
        assert_eq!(t.len(), codes.len());
        assert_eq!(t[0].target, Some('個'));
        assert_eq!(t[1].target, None);
        assert_eq!(t[2].target, Some('們'));
        assert_eq!(t[3].target, None);
    }

    #[test]
    fn mapping_replaces_text_before_conversion() {
        let cs = Charset::standard();
        let m = Mapping::parse(r#"{"拓麻": "Tama", "游戏": "遊玩", "个": "個"}"#).unwrap();
        let conv = Converter::with_mapping(Variant::HongKong, m, &cs).unwrap();
        let mut codes: Vec<u16> = "拓麻个游戏"
            .chars()
            .map(|c| cs.code_of(c).unwrap())
            .collect();
        codes.push(0);
        let t = conv.convert_codes(&cs, 1, &codes);
        let latin: Vec<u16> = "Tama".chars().map(|c| latin_code(c).unwrap()).collect();
        assert_eq!(t.iter().take(4).map(|x| x.code).collect::<Vec<_>>(), latin);
        assert!(t.iter().take(4).all(|x| x.target.is_none()));
        assert_eq!(t[4].target, Some('個'));
        assert_eq!(t[5].target, Some('遊'));
        assert_eq!(t[5].code, INSERTED);
        assert_eq!(t.len(), 4 + 1 + 2 + 1);
        assert_eq!(
            conv.mapping_hits(),
            vec![("拓麻".to_string(), 1), ("游戏".to_string(), 1)]
        );
    }

    #[test]
    fn mapping_rejects_unshowable_text() {
        let cs = Charset::standard();
        let m = Mapping::parse(r#"{"拓麻": "Tama_"}"#).unwrap();
        assert!(Converter::with_mapping(Variant::HongKong, m, &cs).is_err());
    }

    #[test]
    fn converts_single_char() {
        let hk = Converter::new(Variant::HongKong).unwrap();
        assert_eq!(hk.convert_str("国"), "國");
    }
}

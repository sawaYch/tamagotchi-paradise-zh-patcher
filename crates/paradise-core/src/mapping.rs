//! User-supplied text replacements, applied to the Chinese string table before conversion.
//!
//! The file is a flat JSON object of source text to replacement text, for example
//! `{ "拓麻": "Tama" }`. Keys are matched against the original Simplified text; the
//! longest key wins at each position. Replacement text is used as written (it is not
//! run through the Simplified-to-Traditional conversion).
//!
//! A key starting with `^` only matches at the start of a string and one ending with `$`
//! only at its end. `<123>` stands for the raw font code 123, which is how control codes
//! and symbols are written in exported templates.

use crate::charset::{latin_char, Charset};
use crate::convert::Converter;
use crate::error::{Error, Result};
use crate::strings::StringTable;

const PUA_BASE: u32 = 0x10_0000;

#[derive(Debug, Clone)]
struct Entry {
    label: String,
    key: Vec<char>,
    value: Vec<char>,
    at_start: bool,
    at_end: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Mapping {
    entries: Vec<Entry>,
}

/// Text view of a string of font codes used for matching: hanzi and Latin codes become
/// their characters, every other code becomes a private-use placeholder.
pub fn view_chars(cs: &Charset, codes: &[u16]) -> Vec<char> {
    codes
        .iter()
        .map(|&c| {
            if cs.is_hanzi_code(c) {
                cs.char_of_code(c).expect("hanzi code maps to a char")
            } else if let Some(l) = latin_char(c) {
                l
            } else {
                char::from_u32(PUA_BASE + c as u32).expect("plane 16 code point")
            }
        })
        .collect()
}

fn render(view: &[char]) -> String {
    let mut s = String::new();
    for &c in view {
        if (c as u32) < PUA_BASE {
            s.push(c);
        } else {
            s.push_str(&format!("<{}>", c as u32 - PUA_BASE));
        }
    }
    s
}

/// Readable form of a string of font codes for listing: other codes appear as <code>.
pub fn describe(cs: &Charset, codes: &[u16]) -> String {
    let end = codes.iter().position(|&c| c == 0).unwrap_or(codes.len());
    render(&view_chars(cs, &codes[..end]))
}

/// Expands `<123>` placeholders into private-use characters for raw font codes.
fn expand(s: &str) -> Vec<char> {
    let chars: Vec<char> = s.chars().collect();
    let mut out = Vec::with_capacity(chars.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '<' {
            let digits: String = chars[i + 1..]
                .iter()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            let close = i + 1 + digits.len();
            if !digits.is_empty() && chars.get(close) == Some(&'>') {
                if let Some(c) = digits
                    .parse::<u16>()
                    .ok()
                    .and_then(|n| char::from_u32(PUA_BASE + n as u32))
                {
                    out.push(c);
                    i = close + 1;
                    continue;
                }
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

impl Mapping {
    pub fn parse(json: &str) -> Result<Mapping> {
        let value: serde_json::Value = serde_json::from_str(json)
            .map_err(|e| Error::Mapping(format!("not valid JSON: {e}")))?;
        let obj = value
            .as_object()
            .ok_or_else(|| Error::Mapping("expected an object like {\"原文\": \"替換\"}".into()))?;
        let mut pairs = Vec::with_capacity(obj.len());
        for (k, v) in obj {
            let v = v
                .as_str()
                .ok_or_else(|| Error::Mapping(format!("the value for \"{k}\" must be a string")))?;
            pairs.push((k.clone(), v.to_string()));
        }
        Mapping::from_pairs(pairs)
    }

    pub fn from_pairs(pairs: Vec<(String, String)>) -> Result<Mapping> {
        let mut entries = Vec::with_capacity(pairs.len());
        for (label, v) in pairs {
            let mut k = label.as_str();
            let at_start = k.starts_with('^');
            if at_start {
                k = &k[1..];
            }
            let at_end = k.ends_with('$');
            if at_end {
                k = &k[..k.len() - 1];
            }
            if k.is_empty() {
                return Err(Error::Mapping(format!("the key \"{label}\" is empty")));
            }
            entries.push(Entry {
                key: expand(k),
                value: expand(&v),
                label,
                at_start,
                at_end,
            });
        }
        entries.sort_by(|a, b| {
            b.key
                .len()
                .cmp(&a.key.len())
                .then_with(|| a.label.cmp(&b.label))
        });
        Ok(Mapping { entries })
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn key(&self, i: usize) -> String {
        self.entries[i].label.clone()
    }

    /// The key text without anchors, with placeholders expanded.
    pub fn key_chars(&self, i: usize) -> &[char] {
        &self.entries[i].key
    }

    pub fn value(&self, i: usize) -> &[char] {
        &self.entries[i].value
    }

    pub fn key_len(&self, i: usize) -> usize {
        self.entries[i].key.len()
    }

    /// The mapping without the entries for which `drop` returns true.
    pub fn without(&self, drop: impl Fn(usize) -> bool) -> Mapping {
        Mapping {
            entries: self
                .entries
                .iter()
                .enumerate()
                .filter(|(i, _)| !drop(*i))
                .map(|(_, e)| e.clone())
                .collect(),
        }
    }

    /// Index of the entry whose key starts at `at` in `view`, preferring the longest.
    pub fn match_at(&self, view: &[char], at: usize) -> Option<usize> {
        let end = view
            .iter()
            .position(|&c| c as u32 == PUA_BASE)
            .unwrap_or(view.len());
        self.entries.iter().position(|e| {
            (!e.at_start || at == 0)
                && view[at..].starts_with(&e.key)
                && (!e.at_end || at + e.key.len() == end)
        })
    }

    /// True if any key occurs in the string.
    pub fn applies_to(&self, cs: &Charset, codes: &[u16]) -> bool {
        let view = view_chars(cs, codes);
        (0..view.len()).any(|i| self.match_at(&view, i).is_some())
    }
}

fn json_string(s: &str) -> String {
    serde_json::to_string(s).expect("strings serialize")
}

/// A mapping file listing every distinct Chinese string, each as a whole-string key with
/// its converted text as the value. Edit values to change text; entries left as they are
/// are ignored when patching.
pub fn export_template(table: &StringTable, cs: &Charset, conv: &Converter) -> String {
    let mut seen = std::collections::HashSet::new();
    let mut lines = Vec::new();
    for s in &table.strings {
        let end = s.iter().position(|&c| c == 0).unwrap_or(s.len());
        if end == 0 {
            continue;
        }
        let codes = &s[..end];
        let source = render(&view_chars(cs, codes));
        if !seen.insert(source.clone()) {
            continue;
        }
        let converted: Vec<char> = conv
            .convert_codes(cs, codes)
            .iter()
            .flat_map(|t| match t.target {
                Some(c) => vec![c],
                None => view_chars(cs, &[t.code]),
            })
            .collect();
        lines.push(format!(
            "  {}: {}",
            json_string(&format!("^{source}$")),
            json_string(&render(&converted))
        ));
    }
    format!("{{\n{}\n}}\n", lines.join(",\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_prefers_longest_key() {
        let m = Mapping::parse(r#"{"拓麻": "Tama", "拓麻文件": "Tama files"}"#).unwrap();
        assert_eq!(m.len(), 2);
        let view: Vec<char> = "拓麻文件".chars().collect();
        let i = m.match_at(&view, 0).unwrap();
        assert_eq!(m.key(i), "拓麻文件");
    }

    #[test]
    fn rejects_bad_input() {
        assert!(Mapping::parse("[]").is_err());
        assert!(Mapping::parse(r#"{"a": 1}"#).is_err());
        assert!(Mapping::parse(r#"{"": "x"}"#).is_err());
        assert!(Mapping::parse(r#"{"^$": "x"}"#).is_err());
        assert!(Mapping::parse("{").is_err());
    }

    #[test]
    fn anchors_and_placeholders() {
        let m = Mapping::parse(r#"{"^商店$": "A", "拓麻<61440>": "B"}"#).unwrap();
        let end = char::from_u32(PUA_BASE).unwrap();
        let whole = ['商', '店', end];
        let longer = ['商', '店', '码', end];
        assert!(m.match_at(&whole, 0).is_some());
        assert!(m.match_at(&longer, 0).is_none());
        let sep = char::from_u32(PUA_BASE + 61440).unwrap();
        assert!(m.match_at(&['拓', '麻', sep], 0).is_some());
        assert_eq!(expand("a<61440>b<x>"), vec!['a', sep, 'b', '<', 'x', '>']);
    }
}

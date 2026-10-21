use std::sync::OnceLock;

use crate::convert::Variant;
use crate::error::{fmt_err, Result};
use crate::sprite::{FOREGROUND, GLYPH_DIM, GLYPH_PIXELS};

const MAGIC: &[u8; 4] = b"FPG1";
const BITMAP_BYTES: usize = 24;
const SRC_DIM: usize = 12;
const FUSION: &[u8] = include_bytes!("../assets/fusion12_zh_hk.glyphs");
const ARK_HK: &[u8] = include_bytes!("../assets/ark12_zh_hk.glyphs");
const ARK_TW: &[u8] = include_bytes!("../assets/ark12_zh_tw.glyphs");
const ARK_HANT: &[u8] = include_bytes!("../assets/ark12_zh_hant.glyphs");
const CUBIC: &[u8] = include_bytes!("../assets/cubic11.glyphs");
const OFFSET_X: usize = 2;
const OFFSET_Y: usize = 3;

/// Pixel font used to redraw glyphs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FontChoice {
    /// Fusion Pixel Font 12px (zh_hk).
    FusionPixel,
    /// Ark Pixel Font 12px, using the glyph forms of the chosen Traditional variant.
    ArkPixel,
    /// Cubic 11 (11x11), whose forms follow Taiwan conventions.
    #[default]
    Cubic11,
}

impl FontChoice {
    pub const ALL: [FontChoice; 3] = [
        FontChoice::Cubic11,
        FontChoice::FusionPixel,
        FontChoice::ArkPixel,
    ];

    pub fn label(self) -> &'static str {
        match self {
            FontChoice::FusionPixel => "Fusion Pixel Font 12px",
            FontChoice::ArkPixel => "Ark Pixel Font 12px",
            FontChoice::Cubic11 => "Cubic 11",
        }
    }

    pub fn url(self) -> &'static str {
        match self {
            FontChoice::FusionPixel => "https://github.com/TakWolf/fusion-pixel-font",
            FontChoice::ArkPixel => "https://github.com/TakWolf/ark-pixel-font",
            FontChoice::Cubic11 => "https://github.com/ACh-K/Cubic-11",
        }
    }
}

/// 12x12 CJK bitmaps (SIL OFL 1.1 fonts) keyed by BMP code point.
pub struct GlyphPack {
    codes: Vec<u16>,
    bitmaps: Vec<u8>,
    fallback: Vec<u16>,
}

impl GlyphPack {
    pub fn parse(data: &[u8]) -> Result<GlyphPack> {
        if data.len() < 8 || &data[0..4] != MAGIC {
            return fmt_err("glyph pack header");
        }
        let n = u32::from_le_bytes(data[4..8].try_into().unwrap()) as usize;
        let codes_end = 8 + n * 2;
        if data.len() != codes_end + n * BITMAP_BYTES {
            return fmt_err("glyph pack size");
        }
        let codes: Vec<u16> = data[8..codes_end]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        if codes.windows(2).any(|w| w[0] >= w[1]) {
            return fmt_err("glyph pack not sorted");
        }
        Ok(GlyphPack {
            codes,
            bitmaps: data[codes_end..].to_vec(),
            fallback: Vec::new(),
        })
    }

    /// The default font (Fusion Pixel).
    pub fn embedded() -> &'static GlyphPack {
        Self::for_font(FontChoice::FusionPixel, Variant::HongKong)
    }

    /// Pack for a font. Characters the font lacks are taken from Fusion Pixel so every
    /// character stays drawable; `is_fallback` reports which ones.
    pub fn for_font(font: FontChoice, variant: Variant) -> &'static GlyphPack {
        static PACKS: [OnceLock<GlyphPack>; 5] = [const { OnceLock::new() }; 5];
        let (slot, data) = match (font, variant) {
            (FontChoice::FusionPixel, _) => (0, FUSION),
            (FontChoice::ArkPixel, Variant::HongKong) => (1, ARK_HK),
            (FontChoice::ArkPixel, Variant::Taiwan) => (2, ARK_TW),
            (FontChoice::ArkPixel, Variant::Standard) => (3, ARK_HANT),
            (FontChoice::Cubic11, _) => (4, CUBIC),
        };
        PACKS[slot].get_or_init(|| {
            let primary = GlyphPack::parse(data).expect("embedded glyph pack is valid");
            if slot == 0 {
                primary
            } else {
                let base = GlyphPack::parse(FUSION).expect("embedded glyph pack is valid");
                primary.with_fallback(&base)
            }
        })
    }

    fn with_fallback(self, base: &GlyphPack) -> GlyphPack {
        let mut codes = Vec::new();
        let mut bitmaps = Vec::new();
        let mut fallback = Vec::new();
        let (mut i, mut j) = (0, 0);
        while i < self.codes.len() || j < base.codes.len() {
            let take_base =
                j < base.codes.len() && (i >= self.codes.len() || base.codes[j] < self.codes[i]);
            if take_base {
                codes.push(base.codes[j]);
                bitmaps.extend_from_slice(&base.bitmaps[j * BITMAP_BYTES..(j + 1) * BITMAP_BYTES]);
                fallback.push(base.codes[j]);
                j += 1;
            } else {
                if j < base.codes.len() && base.codes[j] == self.codes[i] {
                    j += 1;
                }
                codes.push(self.codes[i]);
                bitmaps.extend_from_slice(&self.bitmaps[i * BITMAP_BYTES..(i + 1) * BITMAP_BYTES]);
                i += 1;
            }
        }
        GlyphPack {
            codes,
            bitmaps,
            fallback,
        }
    }

    /// True if the glyph came from the Fusion Pixel fallback rather than the chosen font.
    pub fn is_fallback(&self, c: char) -> bool {
        u16::try_from(c as u32).is_ok_and(|cp| self.fallback.binary_search(&cp).is_ok())
    }

    pub fn len(&self) -> usize {
        self.codes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.codes.is_empty()
    }

    pub fn contains(&self, c: char) -> bool {
        self.slot(c).is_some()
    }

    fn slot(&self, c: char) -> Option<usize> {
        let cp = u16::try_from(c as u32).ok()?;
        self.codes.binary_search(&cp).ok()
    }

    pub fn bitmap(&self, c: char) -> Option<&[u8]> {
        let i = self.slot(c)?;
        Some(&self.bitmaps[i * BITMAP_BYTES..(i + 1) * BITMAP_BYTES])
    }

    /// Renders the character into a 16x16 font cell using the game's foreground index.
    pub fn render(&self, c: char) -> Option<[u8; GLYPH_PIXELS]> {
        let bm = self.bitmap(c)?;
        let mut px = [0u8; GLYPH_PIXELS];
        for y in 0..SRC_DIM {
            for x in 0..SRC_DIM {
                if bm[y * 2 + x / 8] & (0x80 >> (x % 8)) != 0 {
                    px[(y + OFFSET_Y) * GLYPH_DIM + x + OFFSET_X] = FOREGROUND;
                }
            }
        }
        Some(px)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_pack_has_common_hanzi() {
        let p = GlyphPack::embedded();
        assert!(p.len() > 15000);
        for c in "個們麼點龍體".chars() {
            assert!(p.contains(c), "{c}");
        }
        let px = p.render('一').unwrap();
        assert!(px.contains(&FOREGROUND));
        assert!(px[..OFFSET_Y * GLYPH_DIM].iter().all(|&v| v == 0));
    }

    #[test]
    fn every_font_covers_the_same_characters() {
        let base = GlyphPack::embedded().len();
        for font in FontChoice::ALL {
            for v in [Variant::HongKong, Variant::Taiwan, Variant::Standard] {
                let p = GlyphPack::for_font(font, v);
                assert!(p.len() >= base, "{font:?} {v:?}");
                assert!(p.render('體').is_some());
            }
        }
        let cubic = GlyphPack::for_font(FontChoice::Cubic11, Variant::HongKong);
        assert!(!cubic.is_fallback('體'));
        assert!(cubic.render('體') != GlyphPack::embedded().render('體'));
    }
}

use std::sync::OnceLock;

use crate::convert::Variant;
use crate::error::{fmt_err, Result};
use crate::sprite::{FOREGROUND, GLYPH_DIM, GLYPH_PIXELS};

const MAGIC_12: &[u8; 4] = b"FPG1";
const MAGIC_16: &[u8; 4] = b"FPG2";
const BITMAP_BYTES_12: usize = 24;
const BITMAP_BYTES: usize = 32;
const SRC_DIM_12: usize = 12;
const FUSION: &[u8] = include_bytes!("../assets/fusion12_zh_hk.glyphs");
const ARK_HK: &[u8] = include_bytes!("../assets/ark12_zh_hk.glyphs");
const ARK_TW: &[u8] = include_bytes!("../assets/ark12_zh_tw.glyphs");
const ARK_HANT: &[u8] = include_bytes!("../assets/ark12_zh_hant.glyphs");
const CUBIC: &[u8] = include_bytes!("../assets/cubic11.glyphs");
const MONA_HK: &[u8] = include_bytes!("../assets/mona12_zh_hk.glyphs");
const MONA_TW: &[u8] = include_bytes!("../assets/mona12_zh_tw.glyphs");
const MONA_BOLD_HK: &[u8] = include_bytes!("../assets/mona12_bold_zh_hk.glyphs");
const MONA_BOLD_TW: &[u8] = include_bytes!("../assets/mona12_bold_zh_tw.glyphs");
const ZPIX: &[u8] = include_bytes!("../assets/zpix12.glyphs");
const ROUNDPIX: &[u8] = include_bytes!("../assets/roundpix12.glyphs");
const UNIFONT: &[u8] = include_bytes!("../assets/unifont16.glyphs");
const LATIN_FUSION: &[u8] = include_bytes!("../assets/latin_fusion12.glyphs");
const LATIN_ARK: &[u8] = include_bytes!("../assets/latin_ark12.glyphs");
const LATIN_CUBIC: &[u8] = include_bytes!("../assets/latin_cubic11.glyphs");
const LATIN_MONA: &[u8] = include_bytes!("../assets/latin_mona12.glyphs");
const LATIN_MONA_BOLD: &[u8] = include_bytes!("../assets/latin_mona12_bold.glyphs");
const LATIN_ZPIX: &[u8] = include_bytes!("../assets/latin_zpix12.glyphs");
const LATIN_ROUNDPIX: &[u8] = include_bytes!("../assets/latin_roundpix12.glyphs");
const LATIN_UNIFONT: &[u8] = include_bytes!("../assets/latin_unifont16.glyphs");
const LATIN_LEFT: usize = 3;
const LATIN_ADVANCE: usize = 9;
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
    /// Mona12, using the HK or TC forms of the chosen Traditional variant.
    Mona12,
    Mona12Bold,
    /// Zpix (12px).
    Zpix,
    /// Z Labs RoundPix 12px, a rounded font.
    RoundPix12,
    /// GNU Unifont (16x16), drawn over the whole glyph cell.
    Unifont,
}

impl FontChoice {
    pub const ALL: [FontChoice; 8] = [
        FontChoice::Cubic11,
        FontChoice::FusionPixel,
        FontChoice::ArkPixel,
        FontChoice::Mona12,
        FontChoice::Mona12Bold,
        FontChoice::Zpix,
        FontChoice::RoundPix12,
        FontChoice::Unifont,
    ];

    pub fn label(self) -> &'static str {
        match self {
            FontChoice::FusionPixel => "Fusion Pixel Font 12px",
            FontChoice::ArkPixel => "Ark Pixel Font 12px",
            FontChoice::Cubic11 => "Cubic 11",
            FontChoice::Mona12 => "Mona12",
            FontChoice::Mona12Bold => "Mona12 Bold",
            FontChoice::Zpix => "Zpix",
            FontChoice::RoundPix12 => "Z Labs RoundPix 12px",
            FontChoice::Unifont => "GNU Unifont",
        }
    }

    pub fn url(self) -> &'static str {
        match self {
            FontChoice::FusionPixel => "https://github.com/TakWolf/fusion-pixel-font",
            FontChoice::ArkPixel => "https://github.com/TakWolf/ark-pixel-font",
            FontChoice::Cubic11 => "https://github.com/ACh-K/Cubic-11",
            FontChoice::Mona12 | FontChoice::Mona12Bold => {
                "https://github.com/MonadABXY/mona-font"
            }
            FontChoice::Zpix => "https://github.com/SolidZORO/zpix-pixel-font",
            FontChoice::RoundPix12 => "https://github.com/Astro-2539/ZLabs-RoundPix-12px",
            FontChoice::Unifont => "https://unifoundry.com/unifont/",
        }
    }
}

/// CJK bitmaps laid out in the 16x16 glyph cell, keyed by BMP code point.
pub struct GlyphPack {
    codes: Vec<u16>,
    bitmaps: Vec<u8>,
    fallback: Vec<u16>,
}

impl GlyphPack {
    pub fn parse(data: &[u8]) -> Result<GlyphPack> {
        if data.len() < 8 {
            return fmt_err("glyph pack header");
        }
        let src_bytes = match &data[0..4] {
            m if m == MAGIC_12 => BITMAP_BYTES_12,
            m if m == MAGIC_16 => BITMAP_BYTES,
            _ => return fmt_err("glyph pack header"),
        };
        let n = u32::from_le_bytes(data[4..8].try_into().unwrap()) as usize;
        let codes_end = 8 + n * 2;
        if data.len() != codes_end + n * src_bytes {
            return fmt_err("glyph pack size");
        }
        let codes: Vec<u16> = data[8..codes_end]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        if codes.windows(2).any(|w| w[0] >= w[1]) {
            return fmt_err("glyph pack not sorted");
        }
        let bitmaps = if src_bytes == BITMAP_BYTES_12 {
            let mut cells = vec![0u8; n * BITMAP_BYTES];
            for (src, dst) in data[codes_end..]
                .chunks_exact(BITMAP_BYTES_12)
                .zip(cells.chunks_exact_mut(BITMAP_BYTES))
            {
                for y in 0..SRC_DIM_12 {
                    let row = u16::from_be_bytes([src[y * 2], src[y * 2 + 1]]) >> OFFSET_X;
                    let at = (y + OFFSET_Y) * 2;
                    dst[at..at + 2].copy_from_slice(&row.to_be_bytes());
                }
            }
            cells
        } else {
            data[codes_end..].to_vec()
        };
        Ok(GlyphPack {
            codes,
            bitmaps,
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
        static PACKS: [OnceLock<GlyphPack>; 12] = [const { OnceLock::new() }; 12];
        let (slot, data) = match (font, variant) {
            (FontChoice::FusionPixel, _) => (0, FUSION),
            (FontChoice::ArkPixel, Variant::HongKong) => (1, ARK_HK),
            (FontChoice::ArkPixel, Variant::Taiwan) => (2, ARK_TW),
            (FontChoice::ArkPixel, Variant::Standard) => (3, ARK_HANT),
            (FontChoice::Cubic11, _) => (4, CUBIC),
            (FontChoice::Mona12, Variant::HongKong) => (5, MONA_HK),
            (FontChoice::Mona12, _) => (6, MONA_TW),
            (FontChoice::Mona12Bold, Variant::HongKong) => (7, MONA_BOLD_HK),
            (FontChoice::Mona12Bold, _) => (8, MONA_BOLD_TW),
            (FontChoice::Zpix, _) => (9, ZPIX),
            (FontChoice::RoundPix12, _) => (10, ROUNDPIX),
            (FontChoice::Unifont, _) => (11, UNIFONT),
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

    /// Pack with the digits, letters and `+-!?&` of a font.
    pub fn latin_for(font: FontChoice) -> &'static GlyphPack {
        static PACKS: [OnceLock<GlyphPack>; 8] = [const { OnceLock::new() }; 8];
        let (slot, data) = match font {
            FontChoice::FusionPixel => (0, LATIN_FUSION),
            FontChoice::ArkPixel => (1, LATIN_ARK),
            FontChoice::Cubic11 => (2, LATIN_CUBIC),
            FontChoice::Mona12 => (3, LATIN_MONA),
            FontChoice::Mona12Bold => (4, LATIN_MONA_BOLD),
            FontChoice::Zpix => (5, LATIN_ZPIX),
            FontChoice::RoundPix12 => (6, LATIN_ROUNDPIX),
            FontChoice::Unifont => (7, LATIN_UNIFONT),
        };
        PACKS[slot].get_or_init(|| GlyphPack::parse(data).expect("embedded glyph pack is valid"))
    }

    /// Like `render`, with the ink centered in the game's 9 pixel wide Latin advance, which
    /// occupies columns 3 to 11 of the glyph cell.
    pub fn render_latin(&self, c: char) -> Option<[u8; GLYPH_PIXELS]> {
        let px = self.render(c)?;
        let columns = |x: usize| (0..GLYPH_DIM).any(|y| px[y * GLYPH_DIM + x] != 0);
        let (Some(lo), Some(hi)) = (
            (0..GLYPH_DIM).find(|&x| columns(x)),
            (0..GLYPH_DIM).rev().find(|&x| columns(x)),
        ) else {
            return Some(px);
        };
        let width = hi - lo + 1;
        let left = (LATIN_LEFT + (LATIN_ADVANCE.saturating_sub(width) + 1) / 2)
            .min(GLYPH_DIM.saturating_sub(width));
        let mut out = [0u8; GLYPH_PIXELS];
        for y in 0..GLYPH_DIM {
            for x in lo..=hi {
                out[y * GLYPH_DIM + x - lo + left] = px[y * GLYPH_DIM + x];
            }
        }
        Some(out)
    }

    /// Renders the character into a 16x16 font cell using the game's foreground index.
    pub fn render(&self, c: char) -> Option<[u8; GLYPH_PIXELS]> {
        let bm = self.bitmap(c)?;
        let mut px = [0u8; GLYPH_PIXELS];
        for y in 0..GLYPH_DIM {
            for x in 0..GLYPH_DIM {
                if bm[y * 2 + x / 8] & (0x80 >> (x % 8)) != 0 {
                    px[y * GLYPH_DIM + x] = FOREGROUND;
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

    #[test]
    fn every_font_has_latin_glyphs_centered_in_the_advance() {
        let chars = || {
            (' '..='z').filter(|&c| c != ' ' && crate::charset::latin_code(c).is_some())
        };
        assert_eq!(chars().count(), 67);
        for font in FontChoice::ALL {
            let p = GlyphPack::latin_for(font);
            for c in chars() {
                let px = p.render_latin(c).unwrap_or_else(|| panic!("{font:?} {c}"));
                let cols: Vec<usize> = (0..GLYPH_DIM)
                    .filter(|&x| (0..GLYPH_DIM).any(|y| px[y * GLYPH_DIM + x] != 0))
                    .collect();
                assert!(!cols.is_empty(), "{font:?} {c}");
                let (lo, hi) = (cols[0], cols[cols.len() - 1]);
                let width = hi - lo + 1;
                if width <= LATIN_ADVANCE {
                    assert!(lo >= LATIN_LEFT && hi < LATIN_LEFT + LATIN_ADVANCE, "{font:?} {c}");
                }
            }
        }
    }

    #[test]
    fn fonts_render_distinct_glyphs_within_the_cell() {
        let fusion = GlyphPack::embedded().render('體').unwrap();
        for font in FontChoice::ALL {
            let p = GlyphPack::for_font(font, Variant::HongKong);
            for c in "體們麼點龍".chars() {
                let px = p.render(c).unwrap();
                assert!(px.contains(&FOREGROUND), "{font:?} {c}");
            }
            if !matches!(
                font,
                FontChoice::FusionPixel | FontChoice::ArkPixel | FontChoice::Mona12
            ) {
                assert!(p.render('體').unwrap() != fusion, "{font:?}");
            }
        }
        let unifont = GlyphPack::for_font(FontChoice::Unifont, Variant::HongKong);
        let px = unifont.render('體').unwrap();
        assert!(px[..GLYPH_DIM].iter().any(|&v| v == FOREGROUND));
        assert!(px[15 * GLYPH_DIM..].iter().any(|&v| v == FOREGROUND));
    }
}

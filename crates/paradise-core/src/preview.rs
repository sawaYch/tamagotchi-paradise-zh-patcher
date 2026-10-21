use crate::sprite::{FontSprite, GLYPH_DIM};
use crate::strings::StringTable;

const BACKGROUND: [u8; 4] = [28, 30, 44, 255];
const SEPARATOR: [u8; 4] = [60, 64, 90, 255];
const SHADES: [[u8; 4]; 4] = [
    BACKGROUND,
    [120, 126, 160, 255],
    [255, 255, 255, 255],
    [80, 84, 112, 255],
];
const NARROW_ADVANCE: usize = 9;
const NARROW_LIMIT: u16 = 481;
const MARGIN: usize = 4;

pub struct Image {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

impl Image {
    pub fn write_png(&self, path: &std::path::Path) -> crate::error::Result<()> {
        let file = std::io::BufWriter::new(std::fs::File::create(path)?);
        let mut enc = png::Encoder::new(file, self.width as u32, self.height as u32);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc
            .write_header()
            .map_err(|e| crate::error::Error::Format(e.to_string()))?;
        w.write_image_data(&self.rgba)
            .map_err(|e| crate::error::Error::Format(e.to_string()))?;
        Ok(())
    }
}

fn advance(code: u16) -> usize {
    if code < NARROW_LIMIT {
        NARROW_ADVANCE
    } else {
        GLYPH_DIM
    }
}

fn line_height() -> usize {
    GLYPH_DIM + 2
}

fn wrapped_rows(codes: &[u16], max_width: usize) -> usize {
    let mut rows = 1;
    let mut x = 0;
    for &c in codes.iter().take_while(|&&c| c != 0) {
        let a = advance(c);
        if x + a > max_width {
            rows += 1;
            x = 0;
        }
        x += a;
    }
    rows
}

fn draw_line(img: &mut Image, font: &FontSprite, codes: &[u16], top: usize, max_width: usize) {
    let (mut x, mut y) = (MARGIN, top);
    for &c in codes.iter().take_while(|&&c| c != 0) {
        let a = advance(c);
        if x - MARGIN + a > max_width {
            x = MARGIN;
            y += line_height();
        }
        let g = c as usize - 1;
        if g < font.glyph_count() {
            let px = font.glyph_pixels(g);
            for gy in 0..GLYPH_DIM {
                for gx in 0..GLYPH_DIM {
                    let v = px[gy * GLYPH_DIM + gx] as usize & 3;
                    if v != 0 {
                        let o = ((y + gy) * img.width + x + gx) * 4;
                        img.rgba[o..o + 4].copy_from_slice(&SHADES[v]);
                    }
                }
            }
        }
        x += a;
    }
}

/// Stacks a "before" and "after" rendering of each listed string.
pub fn render_comparison(
    before: (&FontSprite, &StringTable),
    after: (&FontSprite, &StringTable),
    ids: &[usize],
    max_width: usize,
) -> Image {
    let mut heights = Vec::new();
    for &id in ids {
        let (Some(b), Some(a)) = (before.1.strings.get(id - 1), after.1.strings.get(id - 1)) else {
            heights.push(0);
            continue;
        };
        let rows = wrapped_rows(b, max_width).max(wrapped_rows(a, max_width));
        heights.push(rows * line_height() * 2 + 6);
    }
    let width = max_width + MARGIN * 2 + GLYPH_DIM;
    let height = heights.iter().sum::<usize>().max(1) + MARGIN;
    let mut img = Image {
        width,
        height,
        rgba: BACKGROUND.repeat(width * height),
    };
    let mut y = 0;
    for (&id, &h) in ids.iter().zip(&heights) {
        if h == 0 {
            continue;
        }
        let rows = (h - 6) / (line_height() * 2);
        draw_line(
            &mut img,
            before.0,
            &before.1.strings[id - 1],
            y + 2,
            max_width,
        );
        draw_line(
            &mut img,
            after.0,
            &after.1.strings[id - 1],
            y + 2 + rows * line_height(),
            max_width,
        );
        let sep = (y + h - 2) * width * 4;
        for px in img.rgba[sep..sep + width * 4].chunks_exact_mut(4) {
            px.copy_from_slice(&SEPARATOR);
        }
        y += h;
    }
    img
}

/// Ids of strings whose codes or glyphs differ between the two ROMs.
pub fn affected_ids(
    before: &StringTable,
    after: &StringTable,
    changed_glyphs: &std::collections::HashSet<usize>,
) -> Vec<usize> {
    before
        .strings
        .iter()
        .zip(&after.strings)
        .enumerate()
        .filter(|(_, (a, b))| {
            a != b
                || b.iter()
                    .any(|&c| c != 0 && changed_glyphs.contains(&(c as usize - 1)))
        })
        .map(|(i, _)| i + 1)
        .collect()
}

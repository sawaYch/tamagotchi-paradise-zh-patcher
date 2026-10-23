//! Converts a BDF font into the compact glyph pack embedded by paradise-core: "FPG1" for 12px
//! fonts (also 11px, top-left aligned) and "FPG2" for 16px fonts.
//!
//! Usage: glyphpack-gen <input.bdf> <output.glyphs> [12|16] [latin] [ascent=N]
//!
//! By default the pack holds CJK ideographs; `latin` packs the digits, letters and `+-!?&`.
//! `ascent=N` sets the cell row of the baseline pixel row (y = 0), for fonts whose BDF ascent
//! differs from the 12px fonts' (9 for a 12 cell, 13 for a 16 cell).

use std::collections::BTreeMap;
use std::fs;

use anyhow::{bail, Context, Result};

const ROW_BYTES: usize = 2;

#[derive(Clone, Copy)]
struct Cell {
    dim: i32,
    ascent_row: i32,
    magic: &'static [u8; 4],
}

const CELL_12: Cell = Cell {
    dim: 12,
    ascent_row: 9,
    magic: b"FPG1",
};
const CELL_16: Cell = Cell {
    dim: 16,
    ascent_row: 13,
    magic: b"FPG2",
};

fn wanted(cp: u32, latin: bool) -> bool {
    if latin {
        return char::from_u32(cp).is_some_and(|c| c.is_ascii_alphanumeric() || "+-!?&".contains(c));
    }
    (0x3400..=0x4DBF).contains(&cp)
        || (0x4E00..=0x9FFF).contains(&cp)
        || (0xF900..=0xFAFF).contains(&cp)
}

fn parse(text: &str, cell: Cell, latin: bool) -> Result<BTreeMap<u16, Vec<u8>>> {
    let mut out = BTreeMap::new();
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        if !line.starts_with("STARTCHAR") {
            continue;
        }
        let mut encoding = -1i64;
        let mut bbx = (0i32, 0i32, 0i32, 0i32);
        let mut rows: Vec<u32> = Vec::new();
        let mut in_bitmap = false;
        for l in lines.by_ref() {
            if l.starts_with("ENDCHAR") {
                break;
            }
            if in_bitmap {
                rows.push(u32::from_str_radix(l.trim(), 16).context("bitmap row")?);
            } else if let Some(v) = l.strip_prefix("ENCODING ") {
                encoding = v.trim().parse()?;
            } else if let Some(v) = l.strip_prefix("BBX ") {
                let n: Vec<i32> = v
                    .split_whitespace()
                    .map(|x| x.parse())
                    .collect::<Result<_, _>>()?;
                if n.len() != 4 {
                    bail!("bad BBX: {l}");
                }
                bbx = (n[0], n[1], n[2], n[3]);
            } else if l.starts_with("BITMAP") {
                in_bitmap = true;
            }
        }
        if encoding < 0 || !wanted(encoding as u32, latin) {
            continue;
        }
        let (w, h, xo, yo) = bbx;
        if w <= 0 || h <= 0 || rows.len() != h as usize {
            continue;
        }
        let digits_bits = ((w + 7) / 8 * 8) as u32;
        let mut bitmap = vec![0u8; cell.dim as usize * ROW_BYTES];
        for (j, &bits) in rows.iter().enumerate() {
            let y = yo + h - 1 - j as i32;
            let r = cell.ascent_row - y;
            if !(0..cell.dim).contains(&r) {
                continue;
            }
            for c in 0..w {
                if bits >> (digits_bits - 1 - c as u32) & 1 == 1 {
                    let x = xo + c;
                    if (0..cell.dim).contains(&x) {
                        let idx = r as usize * ROW_BYTES + (x as usize) / 8;
                        bitmap[idx] |= 0x80 >> (x % 8);
                    }
                }
            }
        }
        out.insert(encoding as u16, bitmap);
    }
    Ok(out)
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        bail!("usage: glyphpack-gen <input.bdf> <output.glyphs> [12|16] [latin]");
    }
    let (mut cell, mut latin, mut ascent_row) = (CELL_12, false, None);
    for opt in &args[3..] {
        match opt.as_str() {
            "12" => cell = CELL_12,
            "16" => cell = CELL_16,
            "latin" => latin = true,
            other => match other.strip_prefix("ascent=") {
                Some(n) => ascent_row = Some(n.parse().context("ascent")?),
                None => bail!("unsupported option {other}"),
            },
        }
    }
    if let Some(ascent_row) = ascent_row {
        cell.ascent_row = ascent_row;
    }
    let glyphs = parse(
        &fs::read_to_string(&args[1]).context("reading BDF")?,
        cell,
        latin,
    )?;
    let mut out = Vec::new();
    out.extend_from_slice(cell.magic);
    out.extend_from_slice(&(glyphs.len() as u32).to_le_bytes());
    for cp in glyphs.keys() {
        out.extend_from_slice(&cp.to_le_bytes());
    }
    for g in glyphs.values() {
        out.extend_from_slice(g);
    }
    fs::write(&args[2], &out)?;
    println!("{} glyphs, {} bytes", glyphs.len(), out.len());
    Ok(())
}

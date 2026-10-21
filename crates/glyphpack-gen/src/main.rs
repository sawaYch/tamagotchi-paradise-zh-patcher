//! Converts a 12px (or 11px, top-left aligned) BDF font into the compact "FPG1" glyph pack embedded by paradise-core.
//!
//! Usage: glyphpack-gen <input.bdf> <output.glyphs>

use std::collections::BTreeMap;
use std::fs;

use anyhow::{bail, Context, Result};

const CELL: i32 = 12;
const ASCENT_ROW: i32 = 9;
const ROW_BYTES: usize = 2;

fn wanted(cp: u32) -> bool {
    (0x3400..=0x4DBF).contains(&cp)
        || (0x4E00..=0x9FFF).contains(&cp)
        || (0xF900..=0xFAFF).contains(&cp)
}

fn parse(text: &str) -> Result<BTreeMap<u16, [u8; CELL as usize * ROW_BYTES]>> {
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
        if encoding < 0 || !wanted(encoding as u32) {
            continue;
        }
        let (w, h, xo, yo) = bbx;
        if w <= 0 || h <= 0 || rows.len() != h as usize {
            continue;
        }
        let digits_bits = ((w + 7) / 8 * 8) as u32;
        let mut cell = [0u8; CELL as usize * ROW_BYTES];
        for (j, &bits) in rows.iter().enumerate() {
            let y = yo + h - 1 - j as i32;
            let r = ASCENT_ROW - y;
            if !(0..CELL).contains(&r) {
                continue;
            }
            for c in 0..w {
                if bits >> (digits_bits - 1 - c as u32) & 1 == 1 {
                    let x = xo + c;
                    if (0..CELL).contains(&x) {
                        let idx = r as usize * ROW_BYTES + (x as usize) / 8;
                        cell[idx] |= 0x80 >> (x % 8);
                    }
                }
            }
        }
        out.insert(encoding as u16, cell);
    }
    Ok(out)
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        bail!("usage: glyphpack-gen <input.bdf> <output.glyphs>");
    }
    let glyphs = parse(&fs::read_to_string(&args[1]).context("reading BDF")?)?;
    let mut out = Vec::new();
    out.extend_from_slice(b"FPG1");
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

use crate::error::{fmt_err, Error, Result};

pub const FLAG_COMPRESSED_BYTEWISE: u8 = 1 << 5;
pub const FLAG_ENCRYPTED: u8 = 1 << 7;
pub const XOR_KEY: u8 = 0x53;
pub const GLYPH_DIM: usize = 16;
pub const GLYPH_PIXELS: usize = GLYPH_DIM * GLYPH_DIM;
pub const GLYPH_RAW_BYTES: usize = GLYPH_PIXELS / 4;
pub const FOREGROUND: u8 = 2;

pub struct SpritePack {
    pub blobs: Vec<Vec<u8>>,
}

#[derive(Debug, Clone, Copy)]
pub struct SpriteHeader {
    pub data_length: u32,
    pub flags: u8,
    pub bpp: u8,
    pub num_sprites: u16,
    pub width: u8,
    pub height: u8,
    pub palette_offset: u16,
    pub pixel_data_offset: u16,
}

impl SpriteHeader {
    pub fn parse(blob: &[u8]) -> Option<SpriteHeader> {
        if blob.len() < 24 {
            return None;
        }
        Some(SpriteHeader {
            data_length: u32::from_le_bytes(blob[0..4].try_into().ok()?),
            flags: blob[4],
            bpp: blob[5],
            num_sprites: u16::from_le_bytes([blob[6], blob[7]]),
            width: blob[8],
            height: blob[9],
            palette_offset: u16::from_le_bytes([blob[18], blob[19]]),
            pixel_data_offset: u16::from_le_bytes([blob[20], blob[21]]),
        })
    }
}

impl SpritePack {
    pub fn parse(data: &[u8]) -> Result<SpritePack> {
        if data.len() < 4 {
            return fmt_err("sprite pack too small");
        }
        let first = u32::from_le_bytes(data[0..4].try_into().unwrap()) as usize;
        if first % 4 != 0 || first == 0 || first > data.len() {
            return fmt_err("bad sprite pack table");
        }
        let n = first / 4;
        let mut offs = Vec::with_capacity(n + 1);
        for i in 0..n {
            offs.push(u32::from_le_bytes(data[i * 4..i * 4 + 4].try_into().unwrap()) as usize);
        }
        offs.push(data.len());
        let mut blobs = Vec::with_capacity(n);
        for i in 0..n {
            if offs[i] > offs[i + 1] || offs[i + 1] > data.len() {
                return fmt_err(format!("sprite {i} offsets not ascending"));
            }
            blobs.push(data[offs[i]..offs[i + 1]].to_vec());
        }
        Ok(SpritePack { blobs })
    }

    pub fn build(&self) -> Vec<u8> {
        let n = self.blobs.len();
        let mut out = Vec::with_capacity(4 * n + self.blobs.iter().map(|b| b.len()).sum::<usize>());
        let mut cursor = 4 * n;
        for b in &self.blobs {
            out.extend_from_slice(&(cursor as u32).to_le_bytes());
            cursor += b.len();
        }
        for b in &self.blobs {
            out.extend_from_slice(b);
        }
        out
    }
}

pub fn rle_decode(src: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < src.len() {
        let c = src[i];
        i += 1;
        if c == 0 {
            break;
        }
        if c & 0x80 != 0 {
            let n = (c & 0x7f) as usize;
            let end = (i + n).min(src.len());
            out.extend_from_slice(&src[i..end]);
            i = end;
        } else {
            if i >= src.len() {
                break;
            }
            out.extend(std::iter::repeat_n(src[i], c as usize));
            i += 1;
        }
    }
    out
}

pub fn unpack_2bpp(raw: &[u8]) -> [u8; GLYPH_PIXELS] {
    let mut px = [0u8; GLYPH_PIXELS];
    for (i, p) in px.iter_mut().enumerate() {
        if let Some(&b) = raw.get(i / 4) {
            *p = (b >> ((i % 4) * 2)) & 3;
        }
    }
    px
}

pub fn pack_2bpp(px: &[u8; GLYPH_PIXELS]) -> [u8; GLYPH_RAW_BYTES] {
    let mut raw = [0u8; GLYPH_RAW_BYTES];
    for (i, &p) in px.iter().enumerate() {
        raw[i / 4] |= (p & 3) << ((i % 4) * 2);
    }
    raw
}

fn encode_literal_payload(raw: &[u8; GLYPH_RAW_BYTES]) -> Vec<u8> {
    let mut s = Vec::with_capacity(GLYPH_RAW_BYTES + 2);
    s.push(0x80 | GLYPH_RAW_BYTES as u8);
    s.extend_from_slice(raw);
    s.push(0);
    for b in s.iter_mut() {
        *b ^= XOR_KEY;
    }
    while s.len() % 4 != 0 {
        s.push(0);
    }
    s
}

/// Bytewise-compressed, XOR-encrypted glyph font (one glyph per sprite).
pub struct FontSprite {
    prefix: Vec<u8>,
    table: Vec<(u32, u32)>,
    data: Vec<u8>,
    free: Vec<(usize, usize)>,
    grew: bool,
}

impl FontSprite {
    pub fn parse(blob: &[u8]) -> Result<FontSprite> {
        let h =
            SpriteHeader::parse(blob).ok_or_else(|| Error::Format("font sprite header".into()))?;
        if h.flags & (FLAG_COMPRESSED_BYTEWISE | FLAG_ENCRYPTED)
            != (FLAG_COMPRESSED_BYTEWISE | FLAG_ENCRYPTED)
        {
            return fmt_err("font sprite is not compressed+encrypted");
        }
        if h.bpp != 1 || h.width as usize != GLYPH_DIM || h.height as usize != GLYPH_DIM {
            return fmt_err("font sprite is not 16x16 2bpp");
        }
        let pdo = h.pixel_data_offset as usize;
        let n = h.num_sprites as usize;
        let table_bytes = n * 8;
        if pdo < 24 || blob.len() < pdo + table_bytes {
            return fmt_err("font sprite truncated");
        }
        let mut table = Vec::with_capacity(n);
        for i in 0..n {
            let o = pdo + i * 8;
            let off = u32::from_le_bytes(blob[o..o + 4].try_into().unwrap());
            let len = u32::from_le_bytes(blob[o + 4..o + 8].try_into().unwrap());
            if (off as usize) < table_bytes || off as usize + len as usize > blob.len() - pdo {
                return fmt_err(format!("glyph {i} payload out of range"));
            }
            table.push((off - table_bytes as u32, len));
        }
        Ok(FontSprite {
            prefix: blob[..pdo].to_vec(),
            table,
            data: blob[pdo + table_bytes..].to_vec(),
            free: Vec::new(),
            grew: false,
        })
    }

    pub fn glyph_count(&self) -> usize {
        self.table.len()
    }

    pub fn grew(&self) -> bool {
        self.grew
    }

    pub fn data_len(&self) -> usize {
        self.data.len()
    }

    fn payload(&self, i: usize) -> &[u8] {
        let (off, len) = self.table[i];
        &self.data[off as usize..(off + len) as usize]
    }

    pub fn glyph_pixels(&self, i: usize) -> [u8; GLYPH_PIXELS] {
        let stored: Vec<u8> = self.payload(i).iter().map(|b| b ^ XOR_KEY).collect();
        let raw = rle_decode(&stored);
        unpack_2bpp(&raw)
    }

    pub fn is_blank(&self, i: usize) -> bool {
        self.glyph_pixels(i).iter().all(|&p| p == 0)
    }

    fn refs(&self, off: u32) -> usize {
        self.table.iter().filter(|e| e.0 == off).count()
    }

    fn release(&mut self, off: usize, len: usize) {
        if len == 0 {
            return;
        }
        self.free.push((off, len));
        self.free.sort_unstable();
        let mut merged: Vec<(usize, usize)> = Vec::with_capacity(self.free.len());
        for &(o, l) in &self.free {
            match merged.last_mut() {
                Some(last) if last.0 + last.1 >= o => {
                    let end = (last.0 + last.1).max(o + l);
                    last.1 = end - last.0;
                }
                _ => merged.push((o, l)),
            }
        }
        self.free = merged;
    }

    fn alloc(&mut self, len: usize) -> usize {
        if let Some(pos) = self.free.iter().position(|&(_, l)| l >= len) {
            let (o, l) = self.free[pos];
            if l == len {
                self.free.remove(pos);
            } else {
                self.free[pos] = (o + len, l - len);
            }
            return o;
        }
        let o = self.data.len();
        self.data.resize(o + len, 0);
        self.grew = true;
        o
    }

    fn drop_entry_region(&mut self, i: usize) {
        let (off, len) = self.table[i];
        if self.refs(off) == 1 {
            self.release(off as usize, len as usize);
        }
    }

    pub fn set_glyph(&mut self, i: usize, px: &[u8; GLYPH_PIXELS]) {
        self.drop_entry_region(i);
        let payload = encode_literal_payload(&pack_2bpp(px));
        let off = self.alloc(payload.len());
        self.data[off..off + payload.len()].copy_from_slice(&payload);
        self.table[i] = (off as u32, payload.len() as u32);
    }

    /// Points a glyph at an existing blank glyph's payload and releases its own storage.
    pub fn blank_glyph(&mut self, i: usize, blank_source: usize) {
        if self.table[i] == self.table[blank_source] {
            return;
        }
        self.drop_entry_region(i);
        self.table[i] = self.table[blank_source];
    }

    pub fn free_bytes(&self) -> usize {
        self.free.iter().map(|f| f.1).sum()
    }

    pub fn serialize(&self) -> Vec<u8> {
        let table_bytes = self.table.len() * 8;
        let mut out = Vec::with_capacity(self.prefix.len() + table_bytes + self.data.len());
        out.extend_from_slice(&self.prefix);
        for &(off, len) in &self.table {
            out.extend_from_slice(&(off + table_bytes as u32).to_le_bytes());
            out.extend_from_slice(&len.to_le_bytes());
        }
        out.extend_from_slice(&self.data);
        let total = out.len() as u32;
        out[0..4].copy_from_slice(&total.to_le_bytes());
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic_font(n: usize) -> Vec<u8> {
        let table_bytes = n * 8;
        let mut data = Vec::new();
        let mut table = Vec::new();
        for i in 0..n {
            let mut raw = [0u8; GLYPH_RAW_BYTES];
            raw[0] = i as u8;
            let mut payload = vec![0x80 | 64];
            payload.extend_from_slice(&raw);
            payload.push(0);
            for b in payload.iter_mut() {
                *b ^= XOR_KEY;
            }
            while payload.len() % 4 != 0 {
                payload.push(0);
            }
            table.push(((table_bytes + data.len()) as u32, payload.len() as u32));
            data.extend_from_slice(&payload);
        }
        let mut blob = vec![0u8; 24];
        blob[4] = 0xa7;
        blob[5] = 1;
        blob[6..8].copy_from_slice(&(n as u16).to_le_bytes());
        blob[8] = 16;
        blob[9] = 16;
        blob[18..20].copy_from_slice(&24u16.to_le_bytes());
        blob[20..22].copy_from_slice(&24u16.to_le_bytes());
        for (o, l) in table {
            blob.extend_from_slice(&o.to_le_bytes());
            blob.extend_from_slice(&l.to_le_bytes());
        }
        blob.extend_from_slice(&data);
        let total = blob.len() as u32;
        blob[0..4].copy_from_slice(&total.to_le_bytes());
        blob
    }

    #[test]
    fn rle_decodes_runs_and_literals() {
        assert_eq!(rle_decode(&[3, 7, 0x82, 1, 2, 0]), vec![7, 7, 7, 1, 2]);
        assert_eq!(rle_decode(&[0x40, 0, 0]).len(), 64);
    }

    #[test]
    fn pack_unpack_roundtrip() {
        let mut px = [0u8; GLYPH_PIXELS];
        for (i, p) in px.iter_mut().enumerate() {
            *p = (i % 4) as u8;
        }
        assert_eq!(unpack_2bpp(&pack_2bpp(&px)), px);
    }

    #[test]
    fn font_identity_roundtrip() {
        let blob = synthetic_font(8);
        let f = FontSprite::parse(&blob).unwrap();
        assert_eq!(f.serialize(), blob);
    }

    #[test]
    fn set_glyph_reuses_space() {
        let blob = synthetic_font(8);
        let mut f = FontSprite::parse(&blob).unwrap();
        let mut px = [0u8; GLYPH_PIXELS];
        px[5] = FOREGROUND;
        px[200] = FOREGROUND;
        f.set_glyph(3, &px);
        assert_eq!(f.glyph_pixels(3), px);
        assert!(!f.grew());
        let out = f.serialize();
        assert_eq!(out.len(), blob.len());
        let f2 = FontSprite::parse(&out).unwrap();
        assert_eq!(f2.glyph_pixels(3), px);
        assert_eq!(f2.glyph_pixels(2)[0], 2);
    }

    #[test]
    fn blank_then_reuse() {
        let blob = synthetic_font(8);
        let mut f = FontSprite::parse(&blob).unwrap();
        let mut zero = [0u8; GLYPH_PIXELS];
        zero[0] = 0;
        f.set_glyph(0, &zero);
        let before = f.free_bytes();
        f.blank_glyph(1, 0);
        assert!(f.free_bytes() > before);
        assert!(f.is_blank(1));
        let mut px = [0u8; GLYPH_PIXELS];
        px[9] = FOREGROUND;
        f.set_glyph(4, &px);
        assert!(!f.grew());
        assert_eq!(f.glyph_pixels(4), px);
    }

    #[test]
    fn sprite_pack_roundtrip() {
        let p = SpritePack {
            blobs: vec![vec![1, 2, 3, 4], vec![5; 8], vec![]],
        };
        let b = p.build();
        let q = SpritePack::parse(&b).unwrap();
        assert_eq!(q.blobs, p.blobs);
    }
}

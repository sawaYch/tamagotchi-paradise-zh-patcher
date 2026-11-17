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

/// Smallest byte-RLE stream for one glyph. A literal-only glyph takes 68
/// aligned bytes; runs (especially blank rows) usually make this much smaller.
/// Minimize the unpadded size, then retain the ROM's XOR and four-byte alignment.
fn encode_glyph_payload(raw: &[u8; GLYPH_RAW_BYTES]) -> Vec<u8> {
    let mut cost = [0usize; GLYPH_RAW_BYTES + 1];
    let mut choice = [(false, 0usize); GLYPH_RAW_BYTES];
    for i in (0..GLYPH_RAW_BYTES).rev() {
        let mut best = usize::MAX;
        let mut step = (false, 0);
        for len in 1..=GLYPH_RAW_BYTES - i {
            let candidate = 1 + len + cost[i + len];
            if candidate < best {
                best = candidate;
                step = (false, len);
            }
        }
        for len in 1..=GLYPH_RAW_BYTES - i {
            if raw[i + len - 1] != raw[i] {
                break;
            }
            let candidate = 2 + cost[i + len];
            if candidate < best {
                best = candidate;
                step = (true, len);
            }
        }
        cost[i] = best;
        choice[i] = step;
    }
    let mut s = Vec::with_capacity(cost[0] + 1);
    let mut i = 0;
    while i < GLYPH_RAW_BYTES {
        let (repeat, len) = choice[i];
        if repeat {
            s.extend_from_slice(&[len as u8, raw[i]]);
        } else {
            s.push(0x80 | len as u8);
            s.extend_from_slice(&raw[i..i + len]);
        }
        i += len;
    }
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
        let payload = encode_glyph_payload(&pack_2bpp(px));
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

    /// Re-encode and deduplicate every live glyph, then pad to an exact blob
    /// length. Keeping the original font extent preserves all later sprite
    /// addresses, including absolute references kept by installed menu tools.
    /// `serialize` itself remains an identity operation on an unedited font.
    /// Returns the minimum required size before final padding. On failure the
    /// font is unchanged; no glyphs or languages are discarded to make it fit.
    pub fn compact_to_len(&mut self, target_len: usize) -> Result<usize> {
        let table_bytes = self.table.len() * 8;
        let fixed_bytes = self.prefix.len() + table_bytes;
        let mut table = Vec::with_capacity(self.table.len());
        let mut data = Vec::new();
        let mut seen = std::collections::HashMap::<[u8; GLYPH_RAW_BYTES], (u32, u32)>::new();
        for i in 0..self.table.len() {
            let stored: Vec<u8> = self.payload(i).iter().map(|b| b ^ XOR_KEY).collect();
            let raw = rle_decode(&stored);
            let raw: [u8; GLYPH_RAW_BYTES] = raw.try_into().map_err(|r: Vec<u8>| {
                Error::Format(format!(
                    "glyph {i} decodes to {} bytes, expected {GLYPH_RAW_BYTES}",
                    r.len()
                ))
            })?;
            let entry = if let Some(&entry) = seen.get(&raw) {
                entry
            } else {
                let payload = encode_glyph_payload(&raw);
                let entry = (data.len() as u32, payload.len() as u32);
                data.extend_from_slice(&payload);
                seen.insert(raw, entry);
                entry
            };
            table.push(entry);
        }
        let minimum = fixed_bytes + data.len();
        if minimum > target_len || target_len > u32::MAX as usize {
            return Err(Error::Capacity(format!(
                "losslessly compressed font needs {minimum} bytes, but its original extent is {target_len} bytes; later sprite addresses cannot be shifted safely"
            )));
        }
        let used = data.len();
        data.resize(target_len - fixed_bytes, 0);
        self.table = table;
        self.data = data;
        self.free = if self.data.len() > used {
            vec![(used, self.data.len() - used)]
        } else {
            Vec::new()
        };
        self.grew = false;
        Ok(minimum)
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
    fn optimal_rle_handles_runs_literals_and_difficult_glyphs() {
        assert_eq!(encode_glyph_payload(&[0; GLYPH_RAW_BYTES]).len(), 4);
        let mut state = 0x1234_5678u32;
        for case in 0..256 {
            let mut raw = [0; GLYPH_RAW_BYTES];
            for (i, byte) in raw.iter_mut().enumerate() {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                *byte = match case % 4 {
                    0 => state as u8,
                    1 => (i / 8) as u8,
                    2 => {
                        if i % 7 == 0 {
                            state as u8
                        } else {
                            0
                        }
                    }
                    _ => (i % 2) as u8,
                };
            }
            let payload = encode_glyph_payload(&raw);
            assert_eq!(payload.len() % 4, 0);
            assert!(payload.len() <= 68);
            let stored: Vec<u8> = payload.iter().map(|b| b ^ XOR_KEY).collect();
            assert_eq!(rle_decode(&stored), raw);
        }
        let difficult = std::array::from_fn(|i| i as u8);
        assert_eq!(encode_glyph_payload(&difficult).len(), 68);
    }

    #[test]
    fn compaction_preserves_all_glyphs_prefix_and_original_extent() {
        let blob = synthetic_font(8);
        let mut font = FontSprite::parse(&blob).unwrap();
        let mut pixels = [0; GLYPH_PIXELS];
        for (i, p) in pixels.iter_mut().enumerate() {
            *p = ((i * 13 + i / 16) % 4) as u8;
        }
        font.set_glyph(3, &pixels);
        let expected: Vec<_> = (0..font.glyph_count())
            .map(|i| font.glyph_pixels(i))
            .collect();
        let minimum = font.compact_to_len(blob.len()).unwrap();
        assert!(minimum < blob.len());
        let out = font.serialize();
        assert_eq!(out.len(), blob.len());
        assert_eq!(&out[4..24], &blob[4..24]);
        assert!(!font.grew());
        let parsed = FontSprite::parse(&out).unwrap();
        for (i, pixels) in expected.iter().enumerate() {
            assert_eq!(parsed.glyph_pixels(i), *pixels);
        }
    }

    #[test]
    fn deduplicated_glyphs_are_copy_on_write_and_repatch_deterministically() {
        let blob = synthetic_font(8);
        let mut font = FontSprite::parse(&blob).unwrap();
        font.blank_glyph(1, 0);
        font.compact_to_len(blob.len()).unwrap();
        assert_eq!(font.table[0], font.table[1]);
        let shared = font.glyph_pixels(0);
        let raw: [u8; GLYPH_RAW_BYTES] = std::array::from_fn(|i| i as u8);
        let edited = unpack_2bpp(&raw);
        font.set_glyph(1, &edited);
        assert_eq!(font.glyph_pixels(0), shared);
        assert_eq!(font.glyph_pixels(1), edited);
        font.compact_to_len(blob.len()).unwrap();
        let once = font.serialize();
        let mut loaded = FontSprite::parse(&once).unwrap();
        loaded.set_glyph(1, &edited);
        loaded.compact_to_len(blob.len()).unwrap();
        assert_eq!(loaded.serialize(), once);
        assert_eq!(loaded.glyph_pixels(0), shared);
    }

    #[test]
    fn capacity_failure_does_not_mutate_or_drop_glyphs() {
        let blob = synthetic_font(8);
        let mut measured = FontSprite::parse(&blob).unwrap();
        let minimum = measured.compact_to_len(blob.len()).unwrap();
        let mut original = FontSprite::parse(&blob).unwrap();
        let error = original.compact_to_len(minimum - 1).unwrap_err();
        assert!(matches!(error, Error::Capacity(_)));
        assert_eq!(original.serialize(), blob);
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

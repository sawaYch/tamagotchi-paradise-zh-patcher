use crate::error::{fmt_err, Result};

pub const MAGIC: u32 = 0x3243_5241;
const HEADER_LEN: usize = 16;
const ENTRY_LEN: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArcFile {
    pub flags: u32,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Archive {
    pub files: Vec<ArcFile>,
}

fn u32_at(d: &[u8], o: usize) -> Result<u32> {
    match d.get(o..o + 4) {
        Some(b) => Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]])),
        None => fmt_err("archive truncated"),
    }
}

pub fn align4(n: usize) -> usize {
    (n + 3) & !3
}

pub fn checksum(data: &[u8]) -> Result<u32> {
    let len = u32_at(data, 8)? as usize;
    let end = HEADER_LEN + len;
    match data.get(8..end) {
        Some(body) => Ok(body.iter().fold(0u32, |a, &b| a.wrapping_add(b as u32))),
        None => fmt_err("archive length exceeds buffer"),
    }
}

pub fn total_len(data: &[u8]) -> Result<usize> {
    Ok(HEADER_LEN + u32_at(data, 8)? as usize)
}

pub fn checksum_ok(data: &[u8]) -> bool {
    match (checksum(data), u32_at(data, 4)) {
        (Ok(c), Ok(h)) => c == h,
        _ => false,
    }
}

impl Archive {
    pub fn parse(data: &[u8]) -> Result<Archive> {
        if u32_at(data, 0)? != MAGIC {
            return fmt_err("missing ARC2 magic");
        }
        let total = total_len(data)?;
        if total > data.len() {
            return fmt_err("archive length exceeds buffer");
        }
        let n = u32_at(data, 12)? as usize;
        if n > 0x10000 {
            return fmt_err("implausible file count");
        }
        let mut files = Vec::with_capacity(n);
        for i in 0..n {
            let e = HEADER_LEN + i * ENTRY_LEN;
            let flags = u32_at(data, e)?;
            let off = u32_at(data, e + 4)? as usize;
            let ulen = u32_at(data, e + 12)? as usize;
            let end = off.checked_add(ulen).filter(|&x| x <= total);
            let Some(end) = end else {
                return fmt_err(format!("entry {i} out of range"));
            };
            files.push(ArcFile {
                flags,
                data: data[off..end].to_vec(),
            });
        }
        Ok(Archive { files })
    }

    pub fn build(&self) -> Vec<u8> {
        let n = self.files.len();
        let table = HEADER_LEN + n * ENTRY_LEN;
        let mut body_len = n * ENTRY_LEN;
        for f in &self.files {
            body_len += align4(f.data.len());
        }
        let mut out = vec![0u8; HEADER_LEN + body_len];
        out[0..4].copy_from_slice(&MAGIC.to_le_bytes());
        out[8..12].copy_from_slice(&(body_len as u32).to_le_bytes());
        out[12..16].copy_from_slice(&(n as u32).to_le_bytes());
        let mut cursor = table;
        for (i, f) in self.files.iter().enumerate() {
            let e = HEADER_LEN + i * ENTRY_LEN;
            let cl = align4(f.data.len());
            out[e..e + 4].copy_from_slice(&f.flags.to_le_bytes());
            out[e + 4..e + 8].copy_from_slice(&(cursor as u32).to_le_bytes());
            out[e + 8..e + 12].copy_from_slice(&(cl as u32).to_le_bytes());
            out[e + 12..e + 16].copy_from_slice(&(f.data.len() as u32).to_le_bytes());
            out[cursor..cursor + f.data.len()].copy_from_slice(&f.data);
            cursor += cl;
        }
        let sum = out[8..].iter().fold(0u32, |a, &b| a.wrapping_add(b as u32));
        out[4..8].copy_from_slice(&sum.to_le_bytes());
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_checksum() {
        let a = Archive {
            files: vec![
                ArcFile {
                    flags: 0,
                    data: vec![1, 2, 3],
                },
                ArcFile {
                    flags: 0,
                    data: vec![9; 8],
                },
            ],
        };
        let b = a.build();
        assert!(checksum_ok(&b));
        assert_eq!(total_len(&b).unwrap(), b.len());
        assert_eq!(Archive::parse(&b).unwrap(), a);
        assert_eq!(Archive::parse(&b).unwrap().build(), b);
    }

    #[test]
    fn rejects_garbage() {
        assert!(Archive::parse(&[0u8; 32]).is_err());
        assert!(Archive::parse(&[1, 2]).is_err());
    }
}

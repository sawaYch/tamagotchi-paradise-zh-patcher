use crate::arc::{self, Archive};
use crate::charset::Charset;
use crate::error::{fmt_err, Error, Result};
use crate::sprite::{FontSprite, SpriteHeader, SpritePack};
use crate::strings::StringTable;

pub const ASSET_BASE: usize = 0x11_1000;
pub const SIGNATURE: &[u8; 8] = b"SONIXDEV";
pub const ENTRY_DATA: usize = 0;
pub const ENTRY_SPRITES: usize = 1;
pub const ENTRY_STRINGS: usize = 2;
pub const LANGUAGE_COUNT: usize = 9;
const FREE_BYTE: u8 = 0xFF;

#[derive(Debug, Clone)]
pub struct RomInfo {
    pub rom_len: usize,
    pub asset_len: usize,
    pub top_checksum_ok: bool,
    pub strings_checksum_ok: bool,
    pub sprite_count: usize,
    pub font_sprite: usize,
    pub font_glyphs: usize,
    pub zh_table: usize,
    pub zh_string_count: usize,
    pub zh_hanzi_distinct: usize,
    pub signature_ok: bool,
}

pub struct Rom {
    bytes: Vec<u8>,
    asset_len: usize,
    top: Archive,
    sprites: SpritePack,
    tables: Archive,
    info: RomInfo,
}

pub fn count_hanzi_words(table: &StringTable, cs: &Charset) -> usize {
    table
        .strings
        .iter()
        .flat_map(|s| s.iter())
        .filter(|&&c| cs.is_hanzi_code(c))
        .count()
}

pub fn find_font_sprite(sprites: &SpritePack, cs: &Charset) -> Option<usize> {
    sprites.blobs.iter().position(|b| {
        SpriteHeader::parse(b)
            .map(|h| {
                h.num_sprites as usize == cs.glyph_total()
                    && h.width == 16
                    && h.height == 16
                    && h.bpp == 1
            })
            .unwrap_or(false)
    })
}

pub fn find_zh_table(tables: &[StringTable], cs: &Charset) -> Option<usize> {
    let counts: Vec<usize> = tables.iter().map(|t| count_hanzi_words(t, cs)).collect();
    let (idx, &best) = counts.iter().enumerate().max_by_key(|(_, &c)| c)?;
    if best < 200 {
        return None;
    }
    let others_max = counts
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != idx)
        .map(|(_, &c)| c)
        .max()
        .unwrap_or(0);
    if others_max * 4 > best {
        return None;
    }
    Some(idx)
}

impl Rom {
    pub fn load(bytes: Vec<u8>) -> Result<Rom> {
        if bytes.len() < ASSET_BASE + 64 {
            return Err(Error::Unsupported(
                "file is too small to be a Paradise ROM".into(),
            ));
        }
        let region = &bytes[ASSET_BASE..];
        if region.len() < 4 || u32::from_le_bytes(region[0..4].try_into().unwrap()) != arc::MAGIC {
            return Err(Error::Unsupported(format!(
                "no ARC2 asset archive at 0x{ASSET_BASE:x}"
            )));
        }
        let asset_len = arc::total_len(region)?;
        if ASSET_BASE + asset_len > bytes.len() {
            return fmt_err("asset archive extends past end of file");
        }
        let top = Archive::parse(region)?;
        if top.files.len() != 3 {
            return Err(Error::Unsupported(format!(
                "expected 3 top-level archive entries, found {}",
                top.files.len()
            )));
        }
        let sprites = SpritePack::parse(&top.files[ENTRY_SPRITES].data)?;
        let strings_data = &top.files[ENTRY_STRINGS].data;
        let tables = Archive::parse(strings_data)?;
        if tables.files.len() != LANGUAGE_COUNT {
            return Err(Error::Unsupported(format!(
                "expected {LANGUAGE_COUNT} string tables, found {}",
                tables.files.len()
            )));
        }
        let cs = Charset::standard();
        let parsed: Vec<StringTable> = tables
            .files
            .iter()
            .map(|f| StringTable::parse(&f.data))
            .collect::<Result<_>>()?;
        let zh_table = find_zh_table(&parsed, &cs).ok_or_else(|| {
            Error::Unsupported("could not identify the Chinese string table".into())
        })?;
        let font_sprite = find_font_sprite(&sprites, &cs)
            .ok_or_else(|| Error::Unsupported("could not find the CJK font sprite".into()))?;
        let mut distinct = std::collections::HashSet::new();
        for s in &parsed[zh_table].strings {
            for &c in s {
                if cs.is_hanzi_code(c) {
                    distinct.insert(c);
                }
            }
        }
        let info = RomInfo {
            rom_len: bytes.len(),
            asset_len,
            top_checksum_ok: arc::checksum_ok(region),
            strings_checksum_ok: arc::checksum_ok(strings_data),
            sprite_count: sprites.blobs.len(),
            font_sprite,
            font_glyphs: cs.glyph_total(),
            zh_table,
            zh_string_count: parsed[zh_table].strings.len(),
            zh_hanzi_distinct: distinct.len(),
            signature_ok: bytes.starts_with(SIGNATURE),
        };
        Ok(Rom {
            bytes,
            asset_len,
            top,
            sprites,
            tables,
            info,
        })
    }

    pub fn info(&self) -> &RomInfo {
        &self.info
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn data_pack(&self) -> &[u8] {
        &self.top.files[ENTRY_DATA].data
    }

    pub fn sprite_blobs(&self) -> &[Vec<u8>] {
        &self.sprites.blobs
    }

    pub fn table(&self, idx: usize) -> Result<StringTable> {
        StringTable::parse(&self.tables.files[idx].data)
    }

    pub fn all_tables(&self) -> Result<Vec<StringTable>> {
        self.tables
            .files
            .iter()
            .map(|f| StringTable::parse(&f.data))
            .collect()
    }

    pub fn font(&self) -> Result<FontSprite> {
        FontSprite::parse(&self.sprites.blobs[self.info.font_sprite])
    }

    pub fn set_table(&mut self, idx: usize, table: &StringTable) {
        self.tables.files[idx].data = table.serialize();
    }

    pub fn set_font(&mut self, font: &FontSprite) {
        self.sprites.blobs[self.info.font_sprite] = font.serialize();
    }

    /// Rebuilds the asset archive and returns a full ROM image.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let mut top = self.top.clone();
        top.files[ENTRY_STRINGS].data = self.tables.build();
        top.files[ENTRY_SPRITES].data = self.sprites.build();
        let new_asset = top.build();
        let mut out = self.bytes.clone();
        let old_end = ASSET_BASE + self.asset_len;
        let new_end = ASSET_BASE + new_asset.len();
        if new_end > out.len() {
            return Err(Error::Capacity(
                "patched assets would exceed the ROM size".into(),
            ));
        }
        if new_end > old_end && out[old_end..new_end].iter().any(|&b| b != FREE_BYTE) {
            return Err(Error::Capacity(
                "patched assets would overwrite non-empty flash after the archive".into(),
            ));
        }
        out[ASSET_BASE..new_end].copy_from_slice(&new_asset);
        if new_end < old_end {
            out[new_end..old_end].fill(FREE_BYTE);
        }
        Ok(out)
    }

    pub fn rebuilt_asset_len(&self) -> usize {
        let mut top = self.top.clone();
        top.files[ENTRY_STRINGS].data = self.tables.build();
        top.files[ENTRY_SPRITES].data = self.sprites.build();
        top.build().len()
    }

    pub fn original_asset_len(&self) -> usize {
        self.asset_len
    }
}

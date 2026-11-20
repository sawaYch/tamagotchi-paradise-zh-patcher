use crate::arc::{self, Archive};
use crate::charset::Charset;
use crate::error::{fmt_err, Error, Result};
use crate::sprite::{FontSprite, SpriteHeader, SpritePack};
use crate::strings::StringTable;

pub const ASSET_BASE: usize = 0x11_1000;
pub(crate) const MY_LAB_BOOT_ASSET_BASE: usize = 0x5_1000;
pub(crate) const MY_LAB_ASSET_BASE: usize = 0x21_1000;
pub(crate) const MY_LAB_EDITION: u32 = 254;
pub const SIGNATURE: &[u8; 8] = b"SONIXDEV";
pub const ENTRY_DATA: usize = 0;
pub const ENTRY_SPRITES: usize = 1;
pub const ENTRY_STRINGS: usize = 2;
pub const LANGUAGE_COUNT: usize = 9;
const FREE_BYTE: u8 = 0xFF;

#[derive(Debug, Clone)]
pub struct RomInfo {
    pub rom_len: usize,
    pub edition: Option<u32>,
    pub asset_base: usize,
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

const EDITIONS: &[(&str, u32)] = &[
    ("Land", 0x7016_9E2C),
    ("Water", 0x7016_9E2C),
    ("Sky", 0x7016_9E2C),
    ("Jade Forest", 0x8496_33DB),
    ("Tropical", 0x96CB_83B8),
    ("Glacier", 0x96CB_83B8),
    ("Neon Planets", 0x9943_1749),
];

impl RomInfo {
    pub fn edition_name(&self) -> Option<&'static str> {
        if self.edition == Some(MY_LAB_EDITION) {
            return Some("My Lab");
        }
        self.edition
            .and_then(|id| EDITIONS.get(id.checked_sub(1)? as usize))
            .map(|&(name, _)| name)
    }
}

fn detect_edition(bytes: &[u8]) -> Option<u32> {
    [
        (0xD85000, 7..=7),
        (0xD29000, 4..=6),
        (0xD49000, 1..=4),
        (0xEFD000, 1..=7), // Edition page copied here by Paramix Clean ROM.
    ]
    .into_iter()
    .find_map(|(at, allowed)| {
        let page = bytes.get(at..at + 0x1000)?;
        let word = |offset| u32::from_le_bytes(page[offset..offset + 4].try_into().unwrap());
        let id = word(0);
        if !allowed.contains(&id) {
            return None;
        }
        let footer = word(0xFF8);
        if id == 4
            && matches!(at, 0xD29000 | 0xEFD000)
            && footer.wrapping_add(id) == EDITIONS[4].1
            && word(0xFFC) == !footer
        {
            // Forest and Sky on a Tropical/Glacier board share this ID page.
            return Some(5);
        }
        if id == 4 && at == 0xD29000 {
            return None;
        }
        (footer.wrapping_add(id) == EDITIONS[id as usize - 1].1 && word(0xFFC) == !footer)
            .then_some(id)
    })
}

pub(crate) fn is_my_lab(bytes: &[u8]) -> bool {
    const PRODUCT_ID: &[u8] = b"PSID_YD03_MYLAB_CHARA";
    bytes.len() == 0x0100_0000
        && bytes.starts_with(SIGNATURE)
        && bytes[0x1F8..0x1FC] == 0x5A5A_0033u32.to_le_bytes()
        && &bytes[0x186A40..0x186A48] == SIGNATURE
        && &bytes[0x18B7B3..0x18B7B3 + PRODUCT_ID.len()] == PRODUCT_ID
}

pub struct Rom {
    bytes: Vec<u8>,
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
    find_zh_table_with_minimum(tables, cs, 200)
}

fn find_zh_table_with_minimum(
    tables: &[StringTable],
    cs: &Charset,
    minimum: usize,
) -> Option<usize> {
    let counts: Vec<usize> = tables.iter().map(|t| count_hanzi_words(t, cs)).collect();
    let (idx, &best) = counts.iter().enumerate().max_by_key(|(_, &c)| c)?;
    if best < minimum {
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
        let asset_base = if is_my_lab(&bytes) {
            MY_LAB_ASSET_BASE
        } else {
            ASSET_BASE
        };
        Self::load_archive(bytes, asset_base)
    }

    /// My Lab's independent startup interface, with its own strings and font.
    pub fn startup(&self) -> Result<Option<Rom>> {
        (self.info.asset_base == MY_LAB_ASSET_BASE)
            .then(|| Self::load_my_lab_boot(self.bytes.clone()))
            .transpose()
    }

    pub(crate) fn load_my_lab_boot(bytes: Vec<u8>) -> Result<Rom> {
        if !is_my_lab(&bytes) {
            return Err(Error::Unsupported("expected a My Lab ROM".into()));
        }
        Self::load_archive(bytes, MY_LAB_BOOT_ASSET_BASE)
    }

    fn load_archive(bytes: Vec<u8>, asset_base: usize) -> Result<Rom> {
        let my_lab = asset_base != ASSET_BASE;
        let entry_count = if asset_base == MY_LAB_ASSET_BASE {
            24
        } else {
            3
        };
        let region = &bytes[asset_base..];
        if region.len() < 4 || u32::from_le_bytes(region[0..4].try_into().unwrap()) != arc::MAGIC {
            return Err(Error::Unsupported(format!(
                "no ARC2 asset archive at 0x{asset_base:x}"
            )));
        }
        let asset_len = arc::total_len(region)?;
        if asset_len > region.len() {
            return fmt_err("asset archive extends past end of file");
        }
        let top = Archive::parse(region)?;
        if top.files.len() != entry_count {
            return Err(Error::Unsupported(format!(
                "expected {entry_count} top-level archive entries, found {}",
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
        let minimum = if asset_base == MY_LAB_BOOT_ASSET_BASE {
            100
        } else {
            200
        };
        let zh_table = find_zh_table_with_minimum(&parsed, &cs, minimum).ok_or_else(|| {
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
            edition: if my_lab {
                Some(MY_LAB_EDITION)
            } else {
                detect_edition(&bytes)
            },
            asset_base,
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
            top,
            sprites,
            tables,
            info,
        })
    }

    pub fn info(&self) -> &RomInfo {
        &self.info
    }

    /// Transplanted Neon item count and whether the Chinese names were converted.
    pub fn neon_items(&self) -> Result<Option<(usize, bool)>> {
        Ok(
            crate::neon_names::item_names(self, self.info.zh_string_count)?
                .map(|(names, translated)| (names.len(), translated)),
        )
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn data_pack(&self) -> &[u8] {
        &self.top.files[ENTRY_DATA].data
    }

    pub(crate) fn other_assets(&self) -> impl Iterator<Item = &arc::ArcFile> {
        self.top
            .files
            .iter()
            .enumerate()
            .filter_map(|(i, file)| (i != ENTRY_SPRITES && i != ENTRY_STRINGS).then_some(file))
    }

    /// My Lab includes transferable toys, accessories and meals alongside its interface.
    /// The receiver uses its own font. Reserve the original codes, also used by
    /// the package tables, when allocating glyph slots for the local interface.
    pub(crate) fn extra_language_tables(&self) -> Result<Vec<StringTable>> {
        let mut tables = Vec::new();
        for entry in &self.top.files[3..] {
            for package in Archive::parse(&entry.data)?.files {
                let package = Archive::parse(&package.data)?;
                let Some(strings) = package.files.get(ENTRY_STRINGS) else {
                    return fmt_err("missing string archive in My Lab package");
                };
                for table in Archive::parse(&strings.data)?.files {
                    tables.push(StringTable::parse(&table.data)?);
                }
            }
        }
        Ok(tables)
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
        let mut new_asset = top.build();
        let mut out = self.bytes.clone();
        let base = self.info.asset_base;
        let old_end = base + self.info.asset_len;
        let new_end = base + new_asset.len();
        if new_end > out.len() {
            return Err(Error::Capacity(
                "patched assets would exceed the ROM size".into(),
            ));
        }
        let free_byte = if self.info.edition == Some(MY_LAB_EDITION) {
            0
        } else {
            crate::paramix::guard_growth(&self.bytes, old_end, new_end)?;
            FREE_BYTE
        };
        if new_end > old_end && out[old_end..new_end].iter().any(|&b| b != free_byte) {
            return Err(Error::Capacity(
                "patched assets would overwrite non-empty flash after the archive".into(),
            ));
        }
        if base == ASSET_BASE {
            crate::six_rain::relocate(&self.bytes, &mut new_asset)?;
        }
        out[base..new_end].copy_from_slice(&new_asset);
        if new_end < old_end {
            out[new_end..old_end].fill(free_byte);
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
        self.info.asset_len
    }
}

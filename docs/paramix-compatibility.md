# Local Paramix-compatible build

This build preserves compatibility with the current Paramix menu and stored
save slots. It does not require a full flash rewrite.

- Font glyphs use optimal byte RLE, XOR and deduplicated payloads. The font is
  compacted and padded back to its original blob length, preserving every later
  sprite address and Paramix's PEXT/TPRV references. If it cannot fit, patching
  fails without dropping glyphs or moving game assets.
- Paramix configuration is parsed before allocation. Erased slot headers and
  pool pages remain owned, so they cannot be reused for growing assets.
- The in-game Language setting uses a separately allocated 4 KiB page. Helpers
  have a versioned `ZHLANG02` reservation at page offset 0x80. Compatible
  Paramix builds recognize the marker and reserve the full erase page when
  reinstalling their program/catalog/save pool.
- PARAMIX is independently available from the updated Studio without a language
  conversion. Its optional shortcut is appended after the five original entries
  in the main communication/space menu, rather than in Settings. The original
  entries retain their indices and availability conditions. Its independent
  4 KiB helper uses `PMHOME02` metadata at page offset 0x80, with validated
  original table rows, exact Thumb helper code, patched call sites and checksums.
  Version 2 expands the GUI object pool from 12 to 14 entries: six menu rows
  need 13 objects. This avoids exhausting the original pool during construction.
  The legacy `PMHOME01` format remains recognized for Studio migration; the
  patcher preserves it without claiming to repair its GUI pool regression.
  The language patch reserves that page and does not add another PARAMIX row to
  Settings. Studio migrates the older Settings shortcut when rebuilding Menu.
- Legacy combined Settings helpers remain readable. Their independent Language
  and PARAMIX rows retain labels and helper locations; V1 reservations upgrade
  in place. Repeated patching reuses labels rather than appending duplicates.
- The original settings code is matched before modifying pointers/branches.
  Existing marked patches and the legacy unmarked B6000 patch are recognized.
  Verification reproduces the exact helper location, rather than allocating
  another page. Unknown Paramix layouts and occupied helper space are refused.

## Recommended workflow

1. Open the updated local Paramix Studio and Read ROM from the device.
2. Keep the desired Menu, Tama and Saves options enabled. Write new ROM with
   **Only write changes** and **Keep saves** enabled.
3. Read ROM again so the language patch starts with that layout/latest saves.
4. Use this local patcher (desktop or rebuilt web frontend), choose Taiwan and
   the preferred font, then Create a new ROM.
5. Open the resulting ROM in the updated Paramix Studio and write with both
   **Only write changes** and **Keep saves** still enabled.

The previously published online patcher does not automatically contain these
local changes. Rebuilding only the menu executable also cannot create a missing
preview index: the updated Studio must rebuild its catalog.

## Validation

`cargo test -p paradise-core -p paradise-cli` covers compression, copy-on-write
glyph aliases, fixed extents, the three runtime-language profiles, marker/code
validation, exact replay, and allocation refusals.

For user-supplied ROM integration tests:

```sh
PARADISE_ROM=/path/to/rom.bin cargo test -p paradise-core --test real_rom
```

On the supplied Tropical ROM, every offered font passes verification. With
Taiwan/Cubic11 and the Language/PARAMIX labels, font size remains 653,024 bytes.
The independent main-menu shortcut and Language helper occupy separate safe
4 KiB pages; their addresses depend on installation order and existing allocations.
The original Paramix stub at 0xB6000 and all live/stored saves are preserved.
Paramix's `test/feld_language_compat_test.js` verifies both installation orders,
repeated rebuild/patch, PEXT/TPRV checksums, fixed sprite offsets and exact save
streams. Main-menu navigation and runtime language navigation still need physical
device confirmation; no device was flashed during these tests.

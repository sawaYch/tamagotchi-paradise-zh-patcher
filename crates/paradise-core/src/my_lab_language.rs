//! Settings > Language for the My Lab v1.0.00 U2 firmware.
use crate::error::{Error, Result};

// Main-firmware XIP starts at 0x10000000 and maps flash starting at 0x111000.
const XIP: usize = 0x111000;
pub(crate) const HELPER_PAGE: usize = 0x1ff000;
const PAGE_SIZE: usize = 0x1000;
const MAGIC: &[u8; 8] = b"ZHLAB001";
const LABEL_OFFSET: usize = 8;
const MENU_OFFSET: usize = 0x10;
const CODE_OFFSET: usize = 0x40;
const LANGUAGE_SCENE: u32 = 0x66;

const SETTINGS_MENU: [u8; 32] = [
    0x47, 0, 0, 0, 0x5f, 0, 0, 0, // Sound
    0x48, 0, 0, 0, 0x60, 0, 0, 0, // Brightness
    0x49, 0, 0, 0, 0x5a, 0, 0, 0, // Date/time
    0x4a, 0, 0, 0, 0x21, 0, 0, 0, // Update
];

struct Edit {
    offset: usize,
    before: &'static [u8],
    after: &'static [u8],
}

const SETTINGS_PTR_R1: &[u8] = &[0x48, 0xf6, 0x34, 0x11, 0xc1, 0xf2, 0x07, 0x01];
const HELPER_PTR_R1: &[u8] = &[0x4e, 0xf2, 0x10, 0x01, 0xc1, 0xf2, 0x0e, 0x01];

const EDITS: &[Edit] = &[
    // The same table is used for menu creation, cursor restore and selection.
    Edit {
        offset: 0x37b44,
        before: SETTINGS_PTR_R1,
        after: HELPER_PTR_R1,
    },
    Edit {
        offset: 0x37b4e,
        before: &[0x04, 0x21],
        after: &[0x05, 0x21],
    }, // row count
    Edit {
        offset: 0x37bee,
        before: &[0x03, 0x28],
        after: &[0x04, 0x28],
    }, // last row
    Edit {
        offset: 0x37bfa,
        before: SETTINGS_PTR_R1,
        after: HELPER_PTR_R1,
    },
    Edit {
        offset: 0x37ccc,
        before: &[0x48, 0xf6, 0x34, 0x10, 0xc1, 0xf2, 0x07, 0x00],
        after: &[0x4e, 0xf2, 0x10, 0x00, 0xc1, 0xf2, 0x0e, 0x00],
    },
    Edit {
        offset: 0x29d74,
        before: &[0xf5, 0xf7, 0x0a, 0xff],
        after: &[0xc4, 0xf0, 0x7c, 0xf9], // bl 0x100ee070
    },
    Edit {
        offset: 0x29d88,
        before: &[0x00, 0x8c, 0x42, 0x1c],
        after: &[0xc4, 0xf0, 0x5a, 0xb9], // b.w 0x100ee040
    },
    Edit {
        offset: 0x29db4,
        before: &[0x02, 0x98, 0x00, 0x68],
        after: &[0xc4, 0xf0, 0x50, 0xb9], // b.w 0x100ee058
    },
];

const CODE: &[u8] = &[
    // 0x100ee040: Replay the selected index load. A cancelled list returns
    // 0xffff. When opened from Settings, skip writing a language on cancel.
    0x00, 0x8c, // ldrh r0, [r0, #0x20]
    0x42, 0x1c, // adds r2, r0, #1
    0x92, 0xb2, // uxth r2, r2
    0x2a, 0xb9, // cbnz r2, 0x100ee054
    0x08, 0x48, // ldr r0, =0x18001708
    0x80, 0x88, // ldrh r0, [r0, #4] (previous scene)
    0x32, 0x28, // cmp r0, #0x32
    0x01, 0xd1, // bne 0x100ee054
    0x00, 0xf0, 0x02, 0xb8, // b.w 0x100ee058
    0x3b, 0xf7, 0x9a, 0xbe, // b.w 0x10029d8c (native language write)
    // 0x100ee058: Only the Settings entry overrides the normal first-boot
    // destination. Entering Settings runs the firmware's normal flash commit.
    0x04, 0x48, // ldr r0, =0x18001708
    0x81, 0x88, // ldrh r1, [r0, #4]
    0x32, 0x29, // cmp r1, #0x32
    0x01, 0xd1, // bne 0x100ee064
    0x32, 0x21, // movs r1, #0x32
    0x41, 0x80, // strh r1, [r0, #2] (next scene)
    0x02, 0x98, // ldr r0, [sp, #8]
    0x00, 0x68, // ldr r0, [r0]
    0x3b, 0xf7, 0xa6, 0xbe, // b.w 0x10029db8 (native fade)
    0x08, 0x17, 0x00, 0x18, // scene state address
    // 0x100ee070: The first-run list does not enable Cancel. When entered
    // from Settings, turn the native C-button event into a cancelled result.
    0x07, 0x4a, // ldr r2, =0x18001708
    0x92, 0x88, // ldrh r2, [r2, #4]
    0x32, 0x2a, // cmp r2, #0x32
    0x09, 0xd1, // bne 0x100ee08c
    0x06, 0x4a, // ldr r2, =0x1800173c
    0x12, 0x68, // ldr r2, [r2]
    0x12, 0xf0, 0x04, 0x0f, // tst.w r2, #4
    0x04, 0xd0, // beq 0x100ee08c
    0x4f, 0xf6, 0xff, 0x72, // movw r2, #0xffff
    0x0a, 0x80, // strh r2, [r1]
    0x01, 0x20, // movs r0, #1
    0x70, 0x47, // bx lr
    0x31, 0xf7, 0x7e, 0xbd, // b.w 0x1001fb8c (native menu result)
    0x08, 0x17, 0x00, 0x18, // scene state address
    0x3c, 0x17, 0x00, 0x18, // native button events
];

fn at(bytes: &[u8], offset: usize, expected: &[u8]) -> bool {
    bytes.get(XIP + offset..XIP + offset + expected.len()) == Some(expected)
}

fn anchors(bytes: &[u8]) -> bool {
    // The table, language write and scene-transition save routine must all match.
    at(bytes, 0x78934, &SETTINGS_MENU)
        && at(
            bytes,
            0x29d8c,
            &[
                0x40, 0xf2, 0x74, 0x60, 0xc1, 0xf6, 0x00, 0x00, 0xb0, 0xf8, 0x76, 0x10, 0x02, 0xf0,
                0x0f, 0x02, 0x21, 0xf4, 0xf0, 0x51, 0x41, 0xea, 0x42, 0x21, 0xa0, 0xf8, 0x76, 0x10,
                0xb0, 0xf8, 0x76, 0x00, 0xc0, 0xf3, 0x43, 0x20, 0x17, 0xf0, 0x14, 0xfb,
            ],
        )
        && at(
            bytes,
            0x4510c,
            &[
                0x02, 0x98, 0x80, 0x88, 0x61, 0x28, 0x0e, 0xd0, 0xff, 0xe7, 0x41, 0xf2, 0x08, 0x70,
                0xc1, 0xf6, 0x00, 0x00, 0x00, 0x88, 0x10, 0x21, 0xcc, 0xf7, 0xb1, 0xfd, 0x20, 0xb1,
                0xff, 0xe7, 0x00, 0x20, 0x23, 0xf0, 0xf5, 0xfc,
            ],
        )
}

fn page(label: u32) -> [u8; PAGE_SIZE] {
    let mut page = [0xff; PAGE_SIZE];
    page[..MAGIC.len()].copy_from_slice(MAGIC);
    page[LABEL_OFFSET..LABEL_OFFSET + 4].copy_from_slice(&label.to_le_bytes());
    let end = MENU_OFFSET + SETTINGS_MENU.len();
    page[MENU_OFFSET..end].copy_from_slice(&SETTINGS_MENU);
    page[end..end + 4].copy_from_slice(&label.to_le_bytes());
    page[end + 4..end + 8].copy_from_slice(&LANGUAGE_SCENE.to_le_bytes());
    page[CODE_OFFSET..CODE_OFFSET + CODE.len()].copy_from_slice(CODE);
    page
}

pub(crate) fn is_original(bytes: &[u8]) -> bool {
    anchors(bytes)
        && EDITS.iter().all(|e| at(bytes, e.offset, e.before))
        && bytes
            .get(HELPER_PAGE..HELPER_PAGE + PAGE_SIZE)
            .is_some_and(|bytes| bytes.iter().all(|&b| b == 0xff))
}

pub(crate) fn applied_label(bytes: &[u8]) -> Option<u32> {
    let stored = bytes.get(HELPER_PAGE..HELPER_PAGE + PAGE_SIZE)?;
    if !stored.starts_with(MAGIC) {
        return None;
    }
    let label = u32::from_le_bytes(stored[LABEL_OFFSET..LABEL_OFFSET + 4].try_into().unwrap());
    (label < u16::MAX as u32
        && anchors(bytes)
        && EDITS.iter().all(|e| at(bytes, e.offset, e.after))
        && stored == page(label))
    .then_some(label)
}

pub(crate) fn apply(bytes: &mut [u8], label: u32) -> Result<()> {
    if applied_label(bytes) == Some(label) {
        return Ok(());
    }
    if label >= u16::MAX as u32 || !is_original(bytes) {
        return Err(Error::Unsupported(
            "My Lab Settings code or helper space does not match the supported build".into(),
        ));
    }
    for e in EDITS {
        let start = XIP + e.offset;
        bytes[start..start + e.after.len()].copy_from_slice(e.after);
    }
    bytes[HELPER_PAGE..HELPER_PAGE + PAGE_SIZE].copy_from_slice(&page(label));
    Ok(())
}

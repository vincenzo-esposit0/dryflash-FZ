// SPDX-License-Identifier: GPL-3.0-or-later
//! Furi crash record decoding and boot-loop detection on a synthetic firmware that does
//! what `__furi_crash_implementation` does: store a message pointer in RTC BKP4R, then reset.

use dryflash_core::loader::dfu::Element;
use dryflash_core::machine::{Machine, RunExit};

/// Thumb code at 0x0800_0100, hand-assembled (DDI 0403E A7.7):
///   ldr r0, =0x40002800 (RTC)  ; ldr r1, =msg ; str r1, [r0, #0x60] (BKP4R)
///   ldr r0, =0xE000ED0C (AIRCR); ldr r1, =0x05FA0004 ; str r1, [r0]   (SYSRESETREQ)
fn image() -> Vec<u8> {
    let mut img = vec![0u8; 0x200];
    img[0..4].copy_from_slice(&0x2003_0000u32.to_le_bytes()); // initial SP
    img[4..8].copy_from_slice(&0x0800_0101u32.to_le_bytes()); // reset vector
    // Literal loads use Align(PC, 4) = Align(address + 4, 4) as base (A7.7.43).
    let code: [u16; 8] = [
        0x4803, // 0x100 ldr r0, [pc, #12] → 0x110: 0x40002800
        0x4904, // 0x102 ldr r1, [pc, #16] → 0x114: message pointer
        0x6601, // 0x104 str r1, [r0, #0x60]
        0x4804, // 0x106 ldr r0, [pc, #16] → 0x118: 0xE000ED0C
        0x4904, // 0x108 ldr r1, [pc, #16] → 0x11C: 0x05FA0004
        0x6001, // 0x10A str r1, [r0]
        0xE7FE, // 0x10C b .
        0xBF00, // 0x10E nop
    ];
    for (i, hw) in code.iter().enumerate() {
        img[0x100 + 2 * i..0x102 + 2 * i].copy_from_slice(&hw.to_le_bytes());
    }
    let pool: [u32; 4] = [0x4000_2800, 0x0800_0180, 0xE000_ED0C, 0x05FA_0004];
    for (i, w) in pool.iter().enumerate() {
        img[0x110 + 4 * i..0x114 + 4 * i].copy_from_slice(&w.to_le_bytes());
    }
    img[0x180..0x180 + 18].copy_from_slice(b"furi_check failed\0");
    img
}

#[test]
fn furi_crash_record_is_decoded_and_boot_loop_detected() {
    let mut m = Machine::new();
    m.load_elements(
        &[Element {
            address: 0x0800_0000,
            data: image(),
        }],
        None,
    )
    .unwrap();
    m.power_on();
    // The first reset is a crash: the run stops there, and carrying on finds the loop.
    assert_eq!(m.run(10_000_000), RunExit::Crashed);
    assert_eq!(m.run(10_000_000), RunExit::BootLoop { resets: 5 });
    assert_eq!(m.crashes.len(), 1, "the same record is reported once");
    assert_eq!(m.crashes[0].message.as_deref(), Some("furi_check failed"));
}

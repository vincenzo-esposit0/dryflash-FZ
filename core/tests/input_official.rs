// SPDX-License-Identifier: GPL-3.0-or-later
//! The official firmware draws its Desktop on the emulated ST7567, turns the backlight on
//! through the LP5562, and a button press travels GPIO → EXTI → NVIC → `input_isr` → input
//! service → GUI (`view_dispatcher_input_callback`), firmware 1.5.1.
//!
//! The Desktop itself does not act on the event yet: without CPU2, `desktop_srv` blocks in
//! `furi_hal_crypto_enclave_verify` (BRINGUP_LOG M3), so this test stops at the GUI.
//!
//! Needs the pinned firmware: `node scripts/fetch-firmware.mjs official --elf`.
//! Run with `cargo test -p dryflash-core --release -- --include-ignored`.

use dryflash_core::board::Button;
use dryflash_core::cpu::Stop;
use dryflash_core::loader::{dfu, elf::Symbols};
use dryflash_core::machine::{Machine, RunExit};

const DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../firmware/cache/official");
const MS: u64 = 64_000;

#[test]
#[ignore = "needs firmware/cache/official (scripts/fetch-firmware.mjs official --elf)"]
fn desktop_is_drawn_and_a_button_reaches_the_gui() {
    let dfu_file = std::fs::read(format!("{DIR}/f7-update-1.5.1/firmware.dfu")).expect("fetch the firmware first");
    let elf = std::fs::read(format!("{DIR}/flipper-z-f7-firmware-1.5.1.elf")).expect("fetch the ELF first");
    let syms = Symbols::from_elf(&elf).unwrap();
    let mut m = Machine::new();
    m.load_dfu(&dfu::parse(&dfu_file).unwrap(), Some(0x080D_7000)).unwrap();
    m.power_on();
    assert_eq!(m.run(5_000 * MS), RunExit::Budget);

    let frame = m.display_frame();
    let dark = frame.iter().filter(|&&p| p != 0).count();
    assert!(dark > 500, "the Desktop is drawn ({dark} dark pixels)");
    assert!(m.lights().white > 0, "backlight on after boot");

    let path = ["input_isr", "furi_pubsub_publish", "view_dispatcher_input_callback"];
    let addrs: Vec<u32> = path.iter().map(|n| syms.address_of(n).unwrap()).collect();
    m.cpu.watch = addrs.clone();
    m.cpu.watch.sort_unstable();
    m.set_button(Button::Ok, true);
    let mut seen = Vec::new();
    let end = m.cpu.cycles + 100 * MS;
    while m.cpu.cycles < end && seen.len() < path.len() {
        match m.run(end - m.cpu.cycles) {
            RunExit::Stopped(Stop::Watch { pc }) if !seen.contains(&pc) => seen.push(pc),
            RunExit::Stopped(Stop::Watch { .. }) | RunExit::Budget => {}
            other => panic!("{other:?}"),
        }
    }
    assert_eq!(seen, addrs, "press reaches the GUI in order");
    m.cpu.watch.clear();
    m.set_button(Button::Ok, false);
    assert_eq!(m.run(500 * MS), RunExit::Budget);
    assert!(m.digest().unmapped.is_empty(), "unmapped: {:?}", m.digest().unmapped);
    assert_eq!(m.resets, 0);
}

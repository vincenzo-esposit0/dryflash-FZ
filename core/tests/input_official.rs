// SPDX-License-Identifier: GPL-3.0-or-later
//! The official firmware is interactive: it draws on the emulated ST7567, turns the backlight on
//! through the LP5562, shows its "No Factory Keys Found" popup (CPU2 is a FUS without keys,
//! ADR-0012), Back closes it and OK on the Desktop opens the main menu through the loader
//! (applications/services/desktop, firmware 1.5.1).
//!
//! Needs the pinned firmware: `node scripts/fetch-firmware.mjs official --elf`.
//! Run with `cargo test -p dryflash-core --release -- --include-ignored`.

use dryflash_core::board::Button;
use dryflash_core::cpu::Stop;
use dryflash_core::loader::{dfu, elf::Symbols};
use dryflash_core::machine::{Machine, RunExit};

const DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../firmware/cache/official");
const MS: u64 = 64_000;

/// Presses and releases `b`, runs `after_ms`, and returns the watched functions reached.
fn click(m: &mut Machine, b: Button, after_ms: u64) -> Vec<u32> {
    let mut seen = Vec::new();
    m.set_button(b, true);
    let mut run = |m: &mut Machine, ms: u64| {
        let end = m.cpu.cycles + ms * MS;
        while m.cpu.cycles < end {
            match m.run(end - m.cpu.cycles) {
                RunExit::Stopped(Stop::Watch { pc }) => seen.push(pc),
                RunExit::Budget => {}
                other => panic!("{other:?}"),
            }
        }
    };
    run(m, 100);
    m.set_button(b, false);
    run(m, after_ms);
    seen
}

#[test]
#[ignore = "needs firmware/cache/official (scripts/fetch-firmware.mjs official --elf)"]
fn popup_then_desktop_then_menu() {
    let dfu_file = std::fs::read(format!("{DIR}/f7-update-1.5.1/firmware.dfu")).expect("fetch the firmware first");
    let elf = std::fs::read(format!("{DIR}/flipper-z-f7-firmware-1.5.1.elf")).expect("fetch the ELF first");
    let syms = Symbols::from_elf(&elf).unwrap();
    let mut m = Machine::new();
    m.load_dfu(&dfu::parse(&dfu_file).unwrap(), Some(0x080D_7000)).unwrap();
    let at = |n: &str| syms.address_of(n).unwrap();
    let (popup_exit, show_menu) = (at("desktop_scene_secure_enclave_callback"), at("loader_show_menu"));
    m.cpu.watch = vec![popup_exit, show_menu];
    m.cpu.watch.sort_unstable();
    m.power_on();
    assert_eq!(m.run(5_000 * MS), RunExit::Budget);

    let popup = m.display_frame();
    let dark = popup.iter().filter(|&&p| p != 0).count();
    assert!(dark > 300, "a screen is drawn ({dark} dark pixels)");
    assert!(m.lights().white > 0, "backlight on after boot");

    assert_eq!(
        click(&mut m, Button::Back, 900),
        vec![popup_exit],
        "Back closes the popup"
    );
    let desktop = m.display_frame();
    assert_ne!(desktop, popup);
    assert_eq!(click(&mut m, Button::Ok, 900), vec![show_menu], "OK opens the menu");
    assert_ne!(m.display_frame(), desktop);

    assert!(m.digest().unmapped.is_empty(), "unmapped: {:?}", m.digest().unmapped);
    assert!(
        !m.digest().events.iter().any(|e| e.what.contains("fault")),
        "{:?}",
        m.digest().events
    );
    assert_eq!(m.resets, 0);
}

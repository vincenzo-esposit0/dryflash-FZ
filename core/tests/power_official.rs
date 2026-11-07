// SPDX-License-Identifier: GPL-3.0-or-later
//! The official firmware's power paths reach the charger model (firmware 1.5.1):
//!
//! - USB plugged in: the power service sees the charger charge and lights the LED red, then green
//!   when full (applications/services/power/power_service/power.c `power_check_charging_state`,
//!   `sequence_charging` / `sequence_charged` in notification_messages.c).
//! - Settings → Power → Power OFF → "Power Off": `furi_hal_power_off` sets BATFET_DIS. Without USB
//!   the system loses power; with USB the firmware halts on "Disconnect USB for safe shutdown"
//!   (`power_handle_shutdown`) and power goes when the cable comes out. Plugging USB in again
//!   powers the device up (TI SLUSC76C §9.2.10.2).
//!
//! Needs the pinned firmware: `node scripts/fetch-firmware.mjs official --elf`.
//! Run with `cargo test -p dryflash-core --release -- --include-ignored`.

use dryflash_core::board::Button;
use dryflash_core::loader::dfu;
use dryflash_core::machine::{Machine, RunExit};

const DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../firmware/cache/official");
const MS: u64 = 64_000;

fn run(m: &mut Machine, ms: u64) {
    let end = m.cpu.cycles + ms * MS;
    while m.cpu.cycles < end {
        match m.run(end - m.cpu.cycles) {
            RunExit::Budget => {}
            other => panic!("{other:?}: {:?}", m.captures.last().map(|c| &c.kind)),
        }
    }
}

fn click(m: &mut Machine, b: Button, after_ms: u64) {
    m.set_button(b, true);
    run(m, 100);
    m.set_button(b, false);
    run(m, after_ms);
}

fn booted() -> Machine {
    let dfu_file = std::fs::read(format!("{DIR}/f7-update-1.5.1/firmware.dfu")).expect("fetch the firmware first");
    let mut m = Machine::new();
    m.load_dfu(&dfu::parse(&dfu_file).unwrap(), Some(0x080D_7000)).unwrap();
    m.set_battery(80, false);
    m.power_on();
    run(&mut m, 5_000);
    click(&mut m, Button::Back, 600); // the "Need an SD card" popup
    m
}

/// Desktop → menu → Settings (Up wraps to Apps first) → Power (fourth: order 10, 20, 30, 40 in
/// applications/settings/*/application.fam) → Power OFF (third) → the dialog's right button.
fn power_off_from_menu(m: &mut Machine) {
    for b in [Button::Ok, Button::Up, Button::Up, Button::Ok] {
        click(m, b, 600);
    }
    for _ in 0..3 {
        click(m, Button::Down, 300);
    }
    click(m, Button::Ok, 600);
    click(m, Button::Down, 300);
    click(m, Button::Down, 300);
    click(m, Button::Ok, 600);
    click(m, Button::Right, 1_000);
}

fn blank(m: &Machine) -> bool {
    m.display_frame().iter().all(|&p| p == 0)
}

#[test]
#[ignore = "needs firmware/cache/official (scripts/fetch-firmware.mjs official --elf)"]
fn charging_lights_the_led() {
    let mut m = booted();
    let l = m.lights();
    assert_eq!((l.red, l.green), (0, 0), "not charging");
    m.set_battery(80, true);
    run(&mut m, 2_000);
    assert!(m.charging());
    let l = m.lights();
    assert!(l.red > 0 && l.green == 0, "charging: red, {l:?}");
    m.set_battery(100, true);
    run(&mut m, 2_000);
    let l = m.lights();
    assert!(l.green > 0 && l.red == 0, "charged: green, {l:?}");
    m.set_battery(100, false);
    run(&mut m, 2_000);
    let l = m.lights();
    assert_eq!((l.red, l.green), (0, 0), "unplugged, {l:?}");
}

#[test]
#[ignore = "needs firmware/cache/official (scripts/fetch-firmware.mjs official --elf)"]
fn power_off_without_usb_turns_the_device_off() {
    let mut m = booted();
    power_off_from_menu(&mut m);
    assert!(m.is_off());
    assert!(blank(&m));
    assert_eq!(m.lights().white, 0, "backlight off");
    assert!(
        m.captures.is_empty(),
        "not a crash: {:?}",
        m.captures.first().map(|c| &c.kind)
    );
    let cycles = m.cpu.cycles;
    run(&mut m, 1_000);
    assert!(m.cpu.cycles > cycles && m.is_off(), "time passes, nothing runs");

    // Plugging USB in powers it up and it boots again.
    m.set_battery(80, true);
    assert!(!m.is_off());
    run(&mut m, 5_000);
    assert!(!blank(&m));
    assert!(m.lights().white > 0);
}

#[test]
#[ignore = "needs firmware/cache/official (scripts/fetch-firmware.mjs official --elf)"]
fn power_off_with_usb_waits_for_the_cable() {
    let mut m = booted();
    m.set_battery(80, true);
    run(&mut m, 1_000);
    power_off_from_menu(&mut m);
    run(&mut m, 10_000);
    assert!(!m.is_off(), "USB keeps the system up");
    assert!(!blank(&m), "the firmware's own unplug screen");
    assert!(
        m.captures.is_empty(),
        "not a crash: {:?}",
        m.captures.first().map(|c| &c.kind)
    );
    m.set_battery(80, false);
    assert!(m.is_off());
    assert!(blank(&m));
}

#[test]
#[ignore = "needs firmware/cache/official (scripts/fetch-firmware.mjs official --elf)"]
fn flat_battery_powers_off_after_the_firmware_warning() {
    let mut m = booted();
    m.set_battery(0, false);
    // The gauge raises SYSDWN at 0 %; the power service shows its warning and powers off after
    // POWER_OFF_TIMEOUT_S (90 s) unless USB comes in (power.c `power_check_low_battery`).
    run(&mut m, 5_000);
    assert!(!m.is_off() && !blank(&m), "warning first");
    run(&mut m, 95_000);
    assert!(m.is_off());
}

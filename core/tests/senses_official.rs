// SPDX-License-Identifier: GPL-3.0-or-later
//! The official firmware's own sounds and vibration reach the speaker and motor models. In
//! Settings → LCD and Notifications, changing Volume plays C5 for 100 ms and switching Vibro on
//! vibrates for 100 ms (applications/settings/notification_settings/notification_settings_app.c:
//! `volume_changed` plays `sequence_note_c`, `vibro_changed` sends `sequence_single_vibro`;
//! firmware 1.5.1).
//!
//! Needs the pinned firmware: `node scripts/fetch-firmware.mjs official --elf`.
//! Run with `cargo test -p dryflash-core --release -- --include-ignored`.

use dryflash_core::board::Button;
use dryflash_core::board::senses::{Sense, SenseEvent};
use dryflash_core::cpu::Stop;
use dryflash_core::loader::{dfu, elf::Symbols};
use dryflash_core::machine::{Machine, RunExit};

const DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../firmware/cache/official");
const MS: u64 = 64_000;

fn run(m: &mut Machine, ms: u64, seen: &mut Vec<u32>) {
    let end = m.cpu.cycles + ms * MS;
    while m.cpu.cycles < end {
        match m.run(end - m.cpu.cycles) {
            RunExit::Stopped(Stop::Watch { pc }) => seen.push(pc),
            RunExit::Budget => {}
            other => panic!("{other:?}"),
        }
    }
}

/// Presses and releases `b` and runs `after_ms`; returns the watched functions reached.
fn click(m: &mut Machine, b: Button, after_ms: u64) -> Vec<u32> {
    let mut seen = Vec::new();
    m.set_button(b, true);
    run(m, 100, &mut seen);
    m.set_button(b, false);
    run(m, after_ms, &mut seen);
    seen
}

#[test]
#[ignore = "needs firmware/cache/official (scripts/fetch-firmware.mjs official --elf)"]
fn volume_beeps_and_vibro_vibrates() {
    let dfu_file = std::fs::read(format!("{DIR}/f7-update-1.5.1/firmware.dfu")).expect("fetch the firmware first");
    let elf = std::fs::read(format!("{DIR}/flipper-z-f7-firmware-1.5.1.elf")).expect("fetch the ELF first");
    let syms = Symbols::from_elf(&elf).unwrap();
    let mut m = Machine::new();
    m.load_dfu(&dfu::parse(&dfu_file).unwrap(), Some(0x080D_7000)).unwrap();
    let at = |n: &str| syms.address_of(n).unwrap_or_else(|| panic!("{n} in the ELF"));
    let (volume, vibro) = (at("volume_changed"), at("vibro_changed"));
    m.cpu.watch = vec![volume, vibro];
    m.cpu.watch.sort_unstable();
    m.power_on();
    run(&mut m, 5_000, &mut Vec::new());
    assert!(m.take_senses().is_empty(), "silent and still while booting");

    // Popup → Desktop → menu → Settings (next to last: Up wraps to Apps) → LCD and Notifications (second).
    for b in [
        Button::Back,
        Button::Ok,
        Button::Up,
        Button::Up,
        Button::Ok,
        Button::Down,
        Button::Ok,
    ] {
        assert!(click(&mut m, b, 600).is_empty());
    }
    for _ in 0..4 {
        click(&mut m, Button::Down, 300); // Contrast, Backlight, Backlight Time, LED Brightness → Volume
    }

    // Volume 100% → 95%: C5 at volume 0.95. The firmware rounds the period down to whole timer ticks
    // at 64 MHz / 500 (furi_hal_speaker_calculate_autoreload), so C5 comes out at 524.59 Hz, and the
    // compare is 0.95³ / 2 of the autoreload (furi_hal_speaker_calculate_compare).
    let arr = (128_000.0 / 523.25f32) as u32 - 1;
    let (hz, duty) = (
        128_000.0 / (arr + 1) as f32,
        (0.95f32.powi(3) * arr as f32 / 2.0) as u32 as f32 / (arr + 1) as f32,
    );
    assert_eq!(click(&mut m, Button::Left, 600), vec![volume]);
    let sound = m.take_senses();
    let [
        SenseEvent {
            at_ns: on,
            sense: Sense::Speaker(Some(tone)),
        },
        SenseEvent {
            at_ns: off,
            sense: Sense::Speaker(None),
        },
    ] = sound[..]
    else {
        panic!("one note: {sound:?}");
    };
    assert!(
        (tone.hz - hz).abs() < 0.01 && (tone.duty - duty).abs() < 1e-4,
        "{tone:?}, expected {hz} Hz, duty {duty}"
    );
    let ms = (off - on) / 1_000_000;
    assert!((95..=130).contains(&ms), "the note lasts {ms} ms");

    // Vibro On → Off (setting off: no vibration) → On (vibrates).
    click(&mut m, Button::Down, 300);
    assert_eq!(click(&mut m, Button::Left, 600), vec![vibro]);
    assert!(m.take_senses().is_empty(), "Vibro off does not vibrate");
    assert_eq!(click(&mut m, Button::Right, 600), vec![vibro]);
    let buzz = m.take_senses();
    let [
        SenseEvent {
            at_ns: on,
            sense: Sense::Vibro(true),
        },
        SenseEvent {
            at_ns: off,
            sense: Sense::Vibro(false),
        },
    ] = buzz[..]
    else {
        panic!("one buzz: {buzz:?}");
    };
    let ms = (off - on) / 1_000_000;
    assert!((95..=130).contains(&ms), "the buzz lasts {ms} ms");

    assert!(
        !m.digest().events.iter().any(|e| e.what.contains("fault")),
        "{:?}",
        m.digest().events
    );
    assert_eq!(m.resets, 0);
}

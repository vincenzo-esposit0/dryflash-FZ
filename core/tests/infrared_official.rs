// SPDX-License-Identifier: GPL-3.0-or-later
//! The official firmware sends IR through TIM1 and DMA2 (targets/f7/furi_hal/furi_hal_infrared.c,
//! firmware 1.5.1), and what the LED sends is the signal of the remote file.
//!
//! The card has the update package's resources (the Infrared app is `/ext/apps/Infrared/infrared.fap`)
//! and `/ext/infrared/Test.ir` with a parsed NEC signal and a raw one. Through the
//! firmware's own menus (Infrared → Saved Remotes → Test), each button is pressed and the
//! transmission is compared with the NEC encoder's timings (lib/infrared/encoder_decoder/nec/
//! infrared_protocol_nec_i.h) and with the raw data.
//!
//! Needs the pinned firmware: `node scripts/fetch-firmware.mjs official --elf`.
//! Run with `cargo test -p dryflash-core --release -- --include-ignored`.

use dryflash_core::board::Button::{self, *};
use dryflash_core::board::fat32;
use dryflash_core::board::infrared::IrSignal;
use dryflash_core::board::sdcard::{BlockStore, SdCard};
use dryflash_core::loader::dfu;
use dryflash_core::loader::package::{Package, install_resources};
use dryflash_core::machine::{Machine, RunExit};

const TGZ: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../firmware/cache/official/flipper-z-f7-update-1.5.1.tgz"
);
const MS: u64 = 64_000;
const BLOCKS: u32 = 2_097_152;

const REMOTE: &str = "Filetype: IR signals file
Version: 1
#
name: Power
type: parsed
protocol: NEC
address: 04 00 00 00
command: 08 00 00 00
#
name: Raw
type: raw
frequency: 40000
duty_cycle: 0.500000
data: 3000 1000 500 2000 1500
";

fn run(m: &mut Machine, ms: u64) {
    assert_eq!(
        m.run(ms * MS),
        RunExit::Budget,
        "{:?}",
        m.captures.last().map(|c| &c.kind)
    );
}

fn click(m: &mut Machine, b: Button, after_ms: u64) {
    m.set_button(b, true);
    run(m, 100);
    m.set_button(b, false);
    run(m, after_ms);
}

/// Within one carrier period: the DMA counts whole periods (furi_hal_infrared_tx_fill_buffer).
fn near(got: u32, want: u32, hz: f32) -> bool {
    got.abs_diff(want) as f32 <= 1e6 / hz + 1.0
}

/// NEC: preamble, then address, inverted address, command, inverted command, LSB first, then a stop mark.
fn nec_bits(s: &IrSignal) -> Vec<u8> {
    let t = &s.timings_us;
    assert!(
        near(t[0], 9000, s.hz) && near(t[1], 4500, s.hz),
        "preamble {:?}",
        &t[..2]
    );
    (0..4)
        .map(|byte| {
            (0..8).fold(0u8, |acc, bit| {
                let i = 2 + 2 * (8 * byte + bit);
                assert!(near(t[i], 560, s.hz), "mark {i}: {}", t[i]);
                let one = near(t[i + 1], 1690, s.hz);
                assert!(one || near(t[i + 1], 560, s.hz), "space {}: {}", i + 1, t[i + 1]);
                acc | (one as u8) << bit
            })
        })
        .collect()
}

#[test]
#[ignore = "needs firmware/cache/official (scripts/fetch-firmware.mjs official --elf)"]
fn saved_remote_buttons_send_their_signals() {
    let pkg = Package::parse(&std::fs::read(TGZ).expect("fetch the firmware first")).unwrap();
    let mut store = BlockStore::default();
    fat32::format(&mut store, BLOCKS, 1);
    install_resources(&pkg.resources().unwrap(), &mut store).unwrap();
    {
        let mut v = fat32::Volume::open(&mut store).unwrap();
        v.mkdir_all("infrared").unwrap();
        v.write_file("infrared/Test.ir", REMOTE.as_bytes()).unwrap();
        v.sync();
    }
    let mut m = Machine::new();
    m.load_dfu(&dfu::parse(pkg.firmware()).unwrap(), pkg.manifest.radio_address)
        .unwrap();
    m.bus.periphs.insert_sd(SdCard::new(BLOCKS, store));
    m.power_on();
    run(&mut m, 6_000);

    // Desktop → menu (Sub-GHz, 125 kHz RFID, NFC, Infrared: order 10–40 in applications/main/*/
    // application.fam) → Saved Remotes (third, infrared_scene_start.c) → Test.ir → first button.
    click(&mut m, Back, 800);
    click(&mut m, Ok, 800);
    for _ in 0..3 {
        click(&mut m, Down, 300);
    }
    click(&mut m, Ok, 1500);
    click(&mut m, Down, 300);
    click(&mut m, Down, 300);
    click(&mut m, Ok, 1500);
    click(&mut m, Down, 300); // past the `assets` folder
    click(&mut m, Ok, 1500);
    assert!(m.take_ir().is_empty(), "nothing sent yet");

    click(&mut m, Ok, 1000); // Power
    let sent = m.take_ir();
    assert!(!sent.is_empty(), "Power sent nothing");
    let s = &sent[0];
    assert!(
        (s.hz - 38_000.0).abs() < 50.0 && (s.duty - 0.33).abs() < 0.01,
        "{} Hz {}",
        s.hz,
        s.duty
    );
    assert!(s.timings_us.len() >= 67, "{:?}", s.timings_us);
    assert_eq!(nec_bits(s), [0x04, !0x04, 0x08, !0x08]);

    // The remote is a vertical view: Right is Down there (gui/view_port.c `view_port_input_mapping`).
    click(&mut m, Right, 300);
    click(&mut m, Ok, 1000); // Raw
    let sent = m.take_ir();
    assert!(!sent.is_empty(), "Raw sent nothing");
    let s = &sent[0];
    assert!(
        (s.hz - 40_000.0).abs() < 50.0 && (s.duty - 0.5).abs() < 0.01,
        "{} Hz {}",
        s.hz,
        s.duty
    );
    let want = [3000, 1000, 500, 2000, 1500];
    assert_eq!(s.timings_us.len(), want.len(), "{:?}", s.timings_us);
    for (g, w) in s.timings_us.iter().zip(want) {
        assert!(near(*g, w, s.hz), "{:?}", s.timings_us);
    }
    assert!(m.crashes.is_empty(), "{:?}", m.crashes);
}

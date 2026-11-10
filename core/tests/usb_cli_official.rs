// SPDX-License-Identifier: GPL-3.0-or-later
//! The official firmware's CLI over USB: the virtual computer enumerates the device
//! (targets/f7/furi_hal/furi_hal_usb.c, furi_hal_usb_cdc.c, lib/libusb_stm32 at 6ca2857, firmware
//! 1.5.1), opens its serial port with DTR, and the CLI (applications/services/cli/cli_vcp.c)
//! greets it and runs a command.
//!
//! Needs the pinned firmware: `node scripts/fetch-firmware.mjs official --elf`.
//! Run with `cargo test -p dryflash-core --release -- --include-ignored`.

use dryflash_core::board::usb_host::HostEvent;
use dryflash_core::loader::dfu;
use dryflash_core::loader::package::Package;
use dryflash_core::machine::{Machine, RunExit};

const TGZ: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../firmware/cache/official/flipper-z-f7-update-1.5.1.tgz"
);
const MS: u64 = 64_000;

fn run(m: &mut Machine, ms: u64) {
    assert_eq!(
        m.run(ms * MS),
        RunExit::Budget,
        "{:?}",
        m.captures.last().map(|c| &c.kind)
    );
}

/// Runs until the serial output contains `want`, at most `ms` milliseconds.
fn read_until(m: &mut Machine, out: &mut String, want: &str, ms: u64) -> bool {
    for _ in 0..ms / 50 {
        run(m, 50);
        out.push_str(&String::from_utf8_lossy(&m.take_usb_serial()));
        if out.contains(want) {
            return true;
        }
    }
    false
}

#[test]
#[ignore = "needs firmware/cache/official (scripts/fetch-firmware.mjs official --elf)"]
fn cli_over_usb_cdc() {
    let pkg = Package::parse(&std::fs::read(TGZ).expect("fetch the firmware first")).unwrap();
    let mut m = Machine::new();
    m.load_dfu(&dfu::parse(pkg.firmware()).unwrap(), pkg.manifest.radio_address)
        .unwrap();
    m.set_battery(80, true);
    m.power_on();
    run(&mut m, 4_000);

    let events = m.take_usb_events();
    let Some(HostEvent::Configured { product, serial }) = events.first() else {
        panic!("not enumerated: {events:?}");
    };
    // Product: the device name; serial: "flip_" + the name (furi_hal_usb_cdc.c `cdc_init`).
    assert!(!product.is_empty() && serial.starts_with("flip_"), "{product} {serial}");

    m.usb_serial_open(true);
    let mut out = String::new();
    assert!(
        read_until(
            &mut m,
            &mut out,
            "Welcome to Flipper Zero Command Line Interface!",
            3_000
        ),
        "no greeting: {out:?}"
    );
    assert!(read_until(&mut m, &mut out, ">: ", 1_000), "no prompt: {out:?}");

    out.clear();
    m.usb_serial_send(b"info device\r");
    assert!(
        read_until(&mut m, &mut out, "firmware.version              : 1.5.1", 3_000),
        "no device info: {out:?}"
    );
    assert!(
        read_until(&mut m, &mut out, ">: ", 2_000),
        "no prompt after info: {out:?}"
    );
    assert!(out.contains("hardware.model                : Flipper Zero"), "{out}");

    // Closing the port drops DTR: the shell stops; reopening greets again.
    m.usb_serial_open(false);
    run(&mut m, 500);
    m.take_usb_serial();
    m.usb_serial_open(true);
    out.clear();
    assert!(
        read_until(&mut m, &mut out, "Welcome to Flipper Zero", 3_000),
        "no greeting after reopening: {out:?}"
    );

    // Unplugging: the device sees the bus go idle and suspends.
    m.set_battery(80, false);
    run(&mut m, 500);
    assert!(m.take_usb_events().contains(&HostEvent::Disconnected));
    assert!(m.crashes.is_empty(), "{:?}", m.crashes);
}

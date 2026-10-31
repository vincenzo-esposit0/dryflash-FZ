// SPDX-License-Identifier: GPL-3.0-or-later
//! The pinned official `.dfu` boots from the reset vector through `SystemInit`, `main`, the
//! FreeRTOS scheduler start, `furi_hal_init` and `flipper_init`, then runs services for 10 s
//! without faults or resets (M1 definition of done, extended in M2).
//!
//! Needs the pinned firmware: `node scripts/fetch-firmware.mjs official --elf`.
//! Run with `cargo test -p dryflash-core --release -- --include-ignored`.

use dryflash_core::cpu::Stop;
use dryflash_core::loader::{dfu, elf::Symbols};
use dryflash_core::machine::{Machine, RunExit};

const DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../firmware/cache/official");

/// Boot path, in call order (targets/f7/src/main.c and furi_hal.c, firmware 1.5.1).
const PATH: &[&str] = &[
    "Reset_Handler",
    "SystemInit",
    "__libc_init_array",
    "main",
    "furi_hal_init_early",
    "furi_hal_clock_init_early",
    "furi_hal_rtc_init_early",
    "furi_run",
    "vTaskStartScheduler",
    "prvPortStartFirstTask",
    "furi_hal_init",
    "furi_hal_clock_init",
    "furi_hal_interrupt_init",
    "furi_hal_power_init",
    "furi_hal_bt_init",
    "flipper_init",
];

#[test]
#[ignore = "needs firmware/cache/official (scripts/fetch-firmware.mjs official --elf)"]
fn official_1_5_1_boots_to_services_and_keeps_running() {
    let dfu_file = std::fs::read(format!("{DIR}/f7-update-1.5.1/firmware.dfu")).expect("fetch the firmware first");
    let elf = std::fs::read(format!("{DIR}/flipper-z-f7-firmware-1.5.1.elf")).expect("fetch the ELF first");
    let syms = Symbols::from_elf(&elf).unwrap();
    let image = dfu::parse(&dfu_file).unwrap();

    let mut m = Machine::new();
    // CPU2 region from `Radio address` in update.fuf (ADR-0005).
    m.load_dfu(&image, Some(0x080D_7000)).unwrap();
    let addrs: Vec<u32> = PATH.iter().map(|n| syms.address_of(n).unwrap()).collect();
    m.cpu.watch = addrs.clone();
    m.cpu.watch.sort_unstable();
    m.power_on();
    assert_eq!(m.cpu.pc(), addrs[0], "reset vector must point at Reset_Handler");

    let mut reached = Vec::new();
    while reached.len() < PATH.len() {
        match m.run(1_000_000) {
            RunExit::Stopped(Stop::Watch { pc }) => reached.push(pc),
            RunExit::Budget if m.cpu.cycles < 640_000_000 => {}
            other => panic!(
                "stopped after {reached:x?} with {other:?} at {:?}",
                syms.lookup(m.cpu.pc())
            ),
        }
    }
    assert_eq!(reached, addrs, "boot path order");

    // 10 s of virtual time with services running: no unimplemented encoding, lockup, fault,
    // unmapped access or reset (CPU2 absent: the firmware takes its "C2 startup failed" path).
    m.cpu.watch.clear();
    let exit = m.run(640_000_000);
    assert!(
        !m.digest()
            .events
            .iter()
            .any(|e| e.what.contains("fault") || e.what.contains("HardFault")),
        "fault events: {:?}",
        m.digest().events
    );
    assert!(
        matches!(exit, RunExit::Budget),
        "unexpected stop {exit:?} at {:?}",
        syms.lookup(m.cpu.pc())
    );
    assert!(
        m.digest().unmapped.is_empty(),
        "unmapped accesses: {:?}",
        m.digest().unmapped
    );
    assert_eq!(m.resets, 0, "no system reset on the boot path");
}

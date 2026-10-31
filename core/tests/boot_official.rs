// SPDX-License-Identifier: GPL-3.0-or-later
//! M1 definition of done: the pinned official `.dfu` executes from the reset vector into
//! `SystemInit` (and on through `main`) without hitting an emulator gap in the CPU.
//!
//! Needs the pinned firmware: `node scripts/fetch-firmware.mjs official --elf`.
//! Run with `cargo test -p dryflash-core --release -- --include-ignored`.

use dryflash_core::cpu::Stop;
use dryflash_core::loader::{dfu, elf::Symbols};
use dryflash_core::machine::{Machine, RunExit};

const DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../firmware/cache/official");

#[test]
#[ignore = "needs firmware/cache/official (scripts/fetch-firmware.mjs official --elf)"]
fn official_1_5_1_reaches_system_init_and_main() {
    let dfu_file = std::fs::read(format!("{DIR}/f7-update-1.5.1/firmware.dfu")).expect("fetch the firmware first");
    let elf = std::fs::read(format!("{DIR}/flipper-z-f7-firmware-1.5.1.elf")).expect("fetch the ELF first");
    let syms = Symbols::from_elf(&elf).unwrap();
    let image = dfu::parse(&dfu_file).unwrap();

    let mut m = Machine::new();
    // CPU2 region from `Radio address` in update.fuf (ADR-0005).
    m.load_dfu(&image, Some(0x080D_7000)).unwrap();
    let path = [
        "Reset_Handler",
        "SystemInit",
        "__libc_init_array",
        "main",
        "furi_hal_init_early",
    ];
    let addrs: Vec<u32> = path.iter().map(|n| syms.address_of(n).unwrap()).collect();
    m.cpu.watch = addrs.clone();
    m.cpu.watch.sort_unstable();
    m.power_on();
    assert_eq!(m.cpu.pc(), addrs[0], "reset vector must point at Reset_Handler");

    let mut reached = Vec::new();
    while reached.len() < path.len() {
        match m.run(1_000_000) {
            RunExit::Stopped(Stop::Watch { pc }) => reached.push(pc),
            other => panic!("stopped after {:?} with {other:?}", reached,),
        }
    }
    assert_eq!(reached, addrs, "boot path order");

    // Keep running for 1 M cycles: the CPU must not meet an unimplemented encoding,
    // lock up or access unmapped memory on the early boot path.
    m.cpu.watch.clear();
    let exit = m.run(1_000_000);
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
}

// SPDX-License-Identifier: GPL-3.0-or-later
//! Bring-up tool: boots a DfuSe image and prints milestones, where execution ended,
//! where time was spent, and the bring-up digest (BRIEF §8).
//!
//! usage: cargo run --release -p dryflash-core --example boot -- <firmware.dfu>
//!            [--elf firmware.elf] [--cycles N] [--cpu2-start 0x080D7000] [--png screen.png] [--sd-gib N]

use dryflash_core::cpu::Stop;
use dryflash_core::loader::{dfu, elf::Symbols};
use dryflash_core::machine::{Machine, RunExit};
use std::collections::BTreeMap;

/// Functions on the f7 boot path, in call order (targets/f7/src/stm32wb55_startup.c,
/// targets/f7/src/main.c, firmware 1.5.1).
const MILESTONES: &[&str] = &[
    "Reset_Handler",
    "SystemInit",
    "__libc_init_array",
    "main",
    "furi_hal_init_early",
    "furi_hal_clock_init_early",
    "furi_hal_rtc_init_early",
    "furi_init",
    "furi_hal_init",
    "furi_hal_clock_init",
    "furi_hal_interrupt_init",
    "furi_hal_mpu_init",
    "furi_hal_power_init",
    "furi_hal_bt_init",
    "flipper_init",
    "furi_run",
    "vTaskStartScheduler",
    "prvPortStartFirstTask",
    "HardFault_Handler",
    "NMI_Handler",
    "__furi_crash_implementation",
];

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = args
        .get(1)
        .expect("usage: boot <firmware.dfu> [--elf f] [--cycles N] [--cpu2-start A]");
    let opt = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let cycles: u64 = opt("--cycles")
        .map(|v| v.parse().expect("--cycles"))
        .unwrap_or(640_000_000);
    let cpu2_start = opt("--cpu2-start").map(|v| u32::from_str_radix(v.trim_start_matches("0x"), 16).unwrap());
    let symbols = opt("--elf").map(|p| Symbols::from_elf(&std::fs::read(p).unwrap()).expect("parse ELF"));
    let sym = |pc: u32| symbols.as_ref().and_then(|s| s.lookup(pc));

    let image = dfu::parse(&std::fs::read(path).expect("read DFU")).expect("parse DFU");
    for t in &image.targets {
        println!("target {:?} alt {}", t.name.as_deref().unwrap_or("-"), t.alt_setting);
        for e in &t.elements {
            println!("  element 0x{:08X} {} bytes", e.address, e.data.len());
        }
    }

    let mut m = Machine::new();
    m.load_dfu(&image, cpu2_start).expect("load DFU");
    if let Some(gib) = opt("--sd-gib") {
        // A new card: empty FAT32, as the web app prepares it.
        let blocks = gib.parse::<u32>().expect("--sd-gib") * 2_097_152;
        let mut store = dryflash_core::board::sdcard::BlockStore::default();
        dryflash_core::board::fat32::format(&mut store, blocks, 0x4446_0001);
        m.bus
            .periphs
            .insert_sd(dryflash_core::board::sdcard::SdCard::new(blocks, store));
    }
    let mut watch: Vec<(u32, &str)> = Vec::new();
    if let Some(s) = &symbols {
        for name in MILESTONES {
            if let Some(a) = s.address_of(name) {
                watch.push((a, name));
            }
        }
    }
    m.cpu.watch = watch.iter().map(|w| w.0).collect();
    m.cpu.watch.sort_unstable();
    m.power_on();
    println!(
        "reset: SP 0x{:08X} PC 0x{:08X}{}",
        m.cpu.r[13],
        m.cpu.pc(),
        fmt_sym(sym(m.cpu.pc()))
    );

    // Run in slices, sampling the PC to see where time goes.
    let slice = 1_000_000u64;
    let start = std::time::Instant::now();
    let mut samples: BTreeMap<String, u64> = BTreeMap::new();
    let mut seen = std::collections::HashSet::new();
    let exit = loop {
        if m.cpu.cycles >= cycles {
            break RunExit::Budget;
        }
        match m.run(slice.min(cycles - m.cpu.cycles)) {
            RunExit::Budget => {
                let key = sym(m.cpu.pc()).map(|s| s.split('+').next().unwrap().to_string());
                *samples
                    .entry(key.unwrap_or_else(|| format!("0x{:08X}", m.cpu.pc())))
                    .or_default() += 1;
            }
            RunExit::Stopped(Stop::Watch { pc }) => {
                let name = watch.iter().find(|w| w.0 == pc).map(|w| w.1).unwrap_or("?");
                if seen.insert(pc) {
                    println!("  @{:>12} cycles  reached {name}", m.cpu.cycles);
                    if name == "__furi_crash_implementation" {
                        // furi_crash/furi_check pass through here: show who called.
                        let lr = m.cpu.r[14] & !1;
                        println!("                       called from 0x{lr:08X}{}", fmt_sym(sym(lr)));
                    }
                }
                if seen.len() == watch.len() {
                    m.cpu.watch.clear();
                }
            }
            other => break other,
        }
    };
    let secs = start.elapsed().as_secs_f64();
    println!(
        "exit: {exit:x?} at pc 0x{:08X}{}\n      {} cycles ({:.3} s virtual at 64 MHz), {} instructions, {:.2} s wall, {:.0} MIPS",
        m.cpu.pc(),
        fmt_sym(sym(m.cpu.pc())),
        m.cpu.cycles,
        m.cpu.cycles as f64 / 64e6,
        m.cpu.instret,
        secs,
        m.cpu.instret as f64 / secs / 1e6
    );
    println!(
        "cpu: IPSR {} CONTROL {:#x} PRIMASK {} BASEPRI {:#x} CFSR {:#010x} HFSR {:#010x} MMFAR {:#010x} BFAR {:#010x}",
        m.cpu.ipsr,
        m.cpu.control,
        m.cpu.primask as u8,
        m.cpu.basepri,
        m.cpu.sys.cfsr,
        m.cpu.sys.hfsr,
        m.cpu.sys.mmfar,
        m.cpu.sys.bfar
    );
    for (i, r) in m.cpu.r.iter().enumerate() {
        print!("r{i}={r:08x} ");
    }
    println!();
    if !samples.is_empty() {
        println!("time spent (1 M-cycle samples):");
        let mut v: Vec<_> = samples.into_iter().collect();
        v.sort_by_key(|x| std::cmp::Reverse(x.1));
        for (name, n) in v.iter().take(10) {
            println!("  {n:>6}  {name}");
        }
    }
    for c in &m.crashes {
        println!(
            "firmware crash at {:.3} s: {}",
            c.at_ns as f64 / 1e9,
            c.message.as_deref().unwrap_or("(message not decodable)")
        );
    }
    let lights = m.lights();
    println!(
        "LEDs: red {} green {} blue {} backlight {}; display generation {}",
        lights.red,
        lights.green,
        lights.blue,
        lights.white,
        m.display_generation()
    );
    if let Some(card) = &m.bus.periphs.sd {
        println!(
            "SD: {} blocks in use, unsupported commands {:?}",
            card.store.used_blocks(),
            card.unsupported
        );
    }
    if let Some(p) = opt("--png") {
        let png = dryflash_core::trace::png::lcd(&m.display_frame(), 4, lights.white > 0);
        std::fs::write(&p, png).expect("write PNG");
        println!("display written to {p}");
    }
    print!("{}", m.digest().render(&|pc| sym(pc), 12));
    let log = &m.bus.periphs.usart[0].tx;
    if !log.is_empty() {
        let text = String::from_utf8_lossy(log);
        let tail: String = text
            .chars()
            .rev()
            .take(4000)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        println!("== USART1 log ({} bytes, last 4000 chars):\n{tail}", log.len());
    }
}

fn fmt_sym(s: Option<String>) -> String {
    s.map(|s| format!(" ({s})")).unwrap_or_default()
}

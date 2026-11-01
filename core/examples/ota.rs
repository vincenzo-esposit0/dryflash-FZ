// SPDX-License-Identifier: GPL-3.0-or-later
//! Real OTA, for bring-up: quick-installs <from.tgz>, stages <to.tgz> and lets the firmware's own
//! updater do the rest, printing the firmware log. usage: ota <from.tgz> <to.tgz> [seconds] [png]
use dryflash_core::board::{
    fat32,
    sdcard::{BlockStore, SdCard},
};
use dryflash_core::loader::dfu;
use dryflash_core::loader::package::{Package, install_resources, stage_ota};
use dryflash_core::machine::Machine;

const MS: u64 = 64_000;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let from = Package::parse(&std::fs::read(&a[1]).unwrap()).unwrap();
    let to = Package::parse(&std::fs::read(&a[2]).unwrap()).unwrap();
    let secs: u64 = a.get(3).map_or(120, |s| s.parse().unwrap());
    let mut store = BlockStore::default();
    fat32::format(&mut store, 2_097_152, 1);
    install_resources(&from.resources().unwrap(), &mut store).unwrap();
    println!("staged at {}", stage_ota(&to, &mut store).unwrap());
    let mut m = Machine::new();
    m.load_dfu(&dfu::parse(from.firmware()).unwrap(), from.manifest.radio_address)
        .unwrap();
    m.bus.periphs.insert_sd(SdCard::new(2_097_152, store));
    m.power_on();
    m.run(5_000 * MS);
    println!("--- starting update at {:.1} s", m.cpu.cycles as f64 / 64e6);
    m.start_ota();
    let t = std::time::Instant::now();
    let mut printed = 0;
    let mut resets = 0;
    for _ in 0..secs * 4 {
        let exit = m.run(250 * MS);
        let log = String::from_utf8_lossy(&m.bus.periphs.usart[0].tx).to_string();
        print!("{}", log.get(printed..).unwrap_or(""));
        printed = log.len();
        if m.resets != resets {
            resets = m.resets;
            println!(
                "--- reset #{resets} at {:.1} s virtual, pc {:08x}",
                m.cpu.cycles as f64 / 64e6,
                m.cpu.pc()
            );
        }
        if !matches!(exit, dryflash_core::machine::RunExit::Budget) {
            println!("--- exit {exit:?} at pc {:08x}", m.cpu.pc());
            break;
        }
    }
    println!(
        "--- {:.1} s virtual, {:?} wall, resets {}, crashes {:?}",
        m.cpu.cycles as f64 / 64e6,
        t.elapsed(),
        m.resets,
        m.crashes
    );
    println!("unmapped {:?}", m.digest().unmapped.keys().take(5).collect::<Vec<_>>());
    for e in m.digest().events.iter().rev().take(8) {
        println!("event: {}", e.what);
    }
    if let Some(p) = a.get(4) {
        std::fs::write(p, dryflash_core::trace::png::lcd(&m.display_frame(), 4, true)).unwrap();
    }
}

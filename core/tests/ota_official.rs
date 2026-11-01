// SPDX-License-Identifier: GPL-3.0-or-later
//! Real OTA: the firmware's own updater, running from RAM, replaces an installed Unleashed with the
//! official firmware from a package staged on the card. Flash must end up equal to the package's
//! image, the resources must be the package's, and the device must boot to its Desktop.
//!
//! Needs: `node scripts/fetch-firmware.mjs official --elf`, `node scripts/fetch-firmware.mjs unleashed`.
//! Run with `cargo test -p dryflash-core --release -- --include-ignored`.

use dryflash_core::board::fat32::{self, Volume};
use dryflash_core::board::sdcard::{BlockStore, SdCard};
use dryflash_core::loader::dfu;
use dryflash_core::loader::package::{Package, install_resources, stage_ota};
use dryflash_core::machine::{Machine, RunExit};

const CACHE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../firmware/cache");
const MS: u64 = 64_000;
const BLOCKS: u32 = 2_097_152;

fn package(id: &str, name: &str) -> Package {
    let path = format!("{CACHE}/{id}/{name}");
    Package::parse(&std::fs::read(&path).unwrap_or_else(|_| panic!("fetch {path} first"))).unwrap()
}

#[test]
#[ignore = "needs firmware/cache/{official,unleashed} (scripts/fetch-firmware.mjs)"]
fn updater_replaces_the_firmware_and_resources() {
    let from = package("unleashed", "flipper-z-f7-update-unlshd-094.tgz");
    let to = package("official", "flipper-z-f7-update-1.5.1.tgz");

    let mut store = BlockStore::default();
    fat32::format(&mut store, BLOCKS, 1);
    install_resources(&from.resources().unwrap(), &mut store).unwrap();
    stage_ota(&to, &mut store).unwrap();
    let mut m = Machine::new();
    m.load_dfu(&dfu::parse(from.firmware()).unwrap(), from.manifest.radio_address)
        .unwrap();
    m.bus.periphs.insert_sd(SdCard::new(BLOCKS, store));
    m.power_on();
    assert_eq!(m.run(5_000 * MS), RunExit::Budget);

    let image = dfu::parse(to.firmware()).unwrap();
    let element = &image.targets[0].elements[0];
    let offset = (element.address - 0x0800_0000) as usize;
    let installed = |m: &Machine| m.bus.flash[offset..offset + element.data.len()] == element.data[..];
    assert!(!installed(&m), "the old firmware is in flash");

    m.start_ota();
    assert_eq!(m.run(30_000 * MS), RunExit::Budget);

    let log = String::from_utf8_lossy(&m.bus.periphs.usart[0].tx).to_string();
    assert!(log.contains("1.5.1 (RAM)"), "the updater ran from RAM: {log}");
    assert!(installed(&m), "flash holds the package's firmware");
    assert_eq!(m.crashes, vec![], "{log}");
    assert_eq!(m.resets, 3, "pre-update, updater, post-update: {log}");
    assert!(m.digest().unmapped.is_empty(), "{:?}", m.digest().unmapped);

    let last_boot = log.rsplit("Firmware version:").next().unwrap();
    assert!(last_boot.trim_start().starts_with("1.5.1"), "{last_boot}");
    assert!(last_boot.contains("Boot mode 0"), "normal boot at the end: {last_boot}");
    assert!(
        log.contains("Backing up '/int'") && log.contains("Restoring '/int'"),
        "{log}"
    );

    let system = m.bus.periphs.rtc.backup(1);
    assert_eq!((system >> 16) & 0xF, 0, "boot mode back to normal");
    assert_ne!(system & (1 << 13), 0, "Sleep Method stays Legacy after the update");

    let mut card = m.bus.periphs.remove_sd().unwrap();
    let v = Volume::open(&mut card.store).unwrap();
    let official = to.resources().unwrap();
    let manifest = official.iter().find(|e| e.path == "Manifest").unwrap();
    assert_eq!(
        v.read_file("Manifest").unwrap(),
        manifest.data,
        "resources are the package's"
    );
    let mismatched: Vec<&str> = official
        .iter()
        .filter(|e| !e.is_dir && v.read_file(&e.path).ok().as_deref() != Some(&e.data[..]))
        .map(|e| e.path.as_str())
        .collect();
    assert!(
        mismatched.is_empty(),
        "{} files differ: {:?}",
        mismatched.len(),
        &mismatched[..mismatched.len().min(5)]
    );
}

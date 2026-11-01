// SPDX-License-Identifier: GPL-3.0-or-later
//! The official firmware mounts a prepared FAT32 card with its own FatFs through SPI + DMA, writes
//! its files, and finds them again after a power cycle with the same card (firmware 1.5.1).
//!
//! Needs the pinned firmware: `node scripts/fetch-firmware.mjs official --elf`.
//! Run with `cargo test -p dryflash-core --release -- --include-ignored`.

use dryflash_core::board::fat32;
use dryflash_core::board::sdcard::{BlockStore, SdCard};
use dryflash_core::loader::dfu;
use dryflash_core::machine::{Machine, RunExit};

const DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../firmware/cache/official");
const MS: u64 = 64_000;
const BLOCKS: u32 = 2_097_152;
const NO_STATE: &str = "Failed to read \"/int/.dolphin.state\"";

fn boot(image: &dfu::Dfu, card: SdCard) -> (Machine, String) {
    let mut m = Machine::new();
    m.load_dfu(image, Some(0x080D_7000)).unwrap();
    m.bus.periphs.insert_sd(card);
    m.power_on();
    assert_eq!(m.run(5_000 * MS), RunExit::Budget);
    let log = String::from_utf8_lossy(&m.bus.periphs.usart[0].tx).to_string();
    assert_eq!(m.resets, 0);
    assert!(m.digest().unmapped.is_empty(), "{:?}", m.digest().unmapped);
    assert!(
        !m.digest().events.iter().any(|e| e.what.contains("DMA")),
        "{:?}",
        m.digest().events
    );
    (m, log)
}

#[test]
#[ignore = "needs firmware/cache/official (scripts/fetch-firmware.mjs official --elf)"]
fn card_mounts_and_files_survive_a_power_cycle() {
    let file = std::fs::read(format!("{DIR}/f7-update-1.5.1/firmware.dfu")).expect("fetch the firmware first");
    let image = dfu::parse(&file).unwrap();
    let mut store = BlockStore::default();
    fat32::format(&mut store, BLOCKS, 1);
    let formatted = store.used_blocks();

    let (mut first, log) = boot(&image, SdCard::new(BLOCKS, store));
    assert!(log.contains("card mounted"), "{log}");
    assert!(log.contains(NO_STATE), "a new card has no saved state: {log}");
    assert!(log.contains("State saved"), "{log}");
    let card = first.bus.periphs.remove_sd().unwrap();
    assert!(card.unsupported.is_empty(), "{:?}", card.unsupported);
    assert!(card.store.used_blocks() > formatted, "the firmware wrote to the card");

    let (_, log) = boot(&image, card);
    assert!(log.contains("card mounted"), "{log}");
    assert!(!log.contains(NO_STATE), "state found on the same card: {log}");
}

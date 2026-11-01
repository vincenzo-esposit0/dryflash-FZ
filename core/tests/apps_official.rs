// SPDX-License-Identifier: GPL-3.0-or-later
//! Quick install of the official update package: the firmware from the package, its resources on
//! the card. Through the firmware's own menus, the Apps browser lists the card and Snake loads
//! from `/ext/apps/Games/snake_game.fap` and draws (firmware 1.5.1).
//!
//! Needs the pinned firmware: `node scripts/fetch-firmware.mjs official --elf`.
//! Run with `cargo test -p dryflash-core --release -- --include-ignored`.

use dryflash_core::board::Button::{self, *};
use dryflash_core::board::fat32;
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

fn click(m: &mut Machine, b: Button, after_ms: u64) {
    m.set_button(b, true);
    assert_eq!(m.run(100 * MS), RunExit::Budget);
    m.set_button(b, false);
    assert_eq!(m.run(after_ms * MS), RunExit::Budget);
}

#[test]
#[ignore = "needs firmware/cache/official (scripts/fetch-firmware.mjs official --elf)"]
fn official_package_quick_install_runs_an_app_from_the_card() {
    let pkg = Package::parse(&std::fs::read(TGZ).expect("fetch the firmware first")).unwrap();
    assert_eq!(pkg.manifest.info, "1.5.1");
    assert_eq!(pkg.manifest.radio_address, Some(0x080D_7000));

    let mut store = BlockStore::default();
    fat32::format(&mut store, BLOCKS, 1);
    let entries = pkg.resources().unwrap();
    let files = install_resources(&entries, &mut store).unwrap();
    assert!(files > 1000, "resources: {files} files");
    assert!(entries.iter().any(|e| e.path == "apps/Games/snake_game.fap"));

    let mut m = Machine::new();
    m.load_dfu(&dfu::parse(pkg.firmware()).unwrap(), pkg.manifest.radio_address)
        .unwrap();
    m.bus.periphs.insert_sd(SdCard::new(BLOCKS, store));
    m.power_on();
    assert_eq!(m.run(6_000 * MS), RunExit::Budget);

    // Popup → Desktop → menu → Apps → /ext → apps → Games → first game.
    click(&mut m, Back, 800);
    click(&mut m, Ok, 1500);
    click(&mut m, Up, 400);
    click(&mut m, Ok, 1500);
    click(&mut m, Ok, 1500);
    click(&mut m, Ok, 1500);
    for _ in 0..3 {
        click(&mut m, Down, 400);
    }
    click(&mut m, Ok, 1500);
    click(&mut m, Down, 400);
    click(&mut m, Ok, 2000);

    let log = String::from_utf8_lossy(&m.bus.periphs.usart[0].tx).to_string();
    assert!(log.contains("Loading /ext/apps/Games/snake_game.fap"), "{log}");
    assert!(log.contains("Loaded in"), "{log}");
    let dark = m.display_frame().iter().filter(|&&p| p != 0).count();
    assert!(dark > 100, "the game draws ({dark} dark pixels)");
    assert!(m.crashes.is_empty(), "{:?}", m.crashes);
    assert_eq!(m.resets, 0);
    assert!(m.digest().unmapped.is_empty(), "{:?}", m.digest().unmapped);
    let card = m.bus.periphs.sd.as_ref().unwrap();
    assert!(card.unsupported.is_empty(), "{:?}", card.unsupported);
}

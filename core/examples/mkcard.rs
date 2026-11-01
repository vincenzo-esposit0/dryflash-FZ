// SPDX-License-Identifier: GPL-3.0-or-later
//! Writes a prepared card to a file, to inspect with other tools: the official package's resources
//! on a new FAT32 volume. usage: mkcard <package.tgz> <partition.img>
use dryflash_core::board::{fat32, sdcard::BlockStore};
use dryflash_core::loader::package::{Package, install_resources};
use std::io::{Seek, SeekFrom, Write};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let t = std::time::Instant::now();
    let pkg = Package::parse(&std::fs::read(&args[1]).unwrap()).unwrap();
    println!("{:?}", pkg.manifest);
    let entries = pkg.resources().unwrap();
    let bytes: usize = entries.iter().map(|e| e.data.len()).sum();
    println!(
        "{} entries, {bytes} bytes of files, parsed in {:?}",
        entries.len(),
        t.elapsed()
    );
    const BLOCKS: u32 = 2_097_152;
    const START: u32 = 2048;
    let mut store = BlockStore::default();
    fat32::format(&mut store, BLOCKS, 0x4446_0001);
    let t = std::time::Instant::now();
    let n = install_resources(&entries, &mut store).unwrap();
    println!(
        "{n} files written in {:?}, {} blocks in use",
        t.elapsed(),
        store.used_blocks()
    );
    let mut f = std::fs::File::create(&args[2]).unwrap();
    f.set_len((BLOCKS - START) as u64 * 512).unwrap();
    let mut b = [0u8; 512];
    for lba in store.take_dirty() {
        if lba >= START {
            store.read(lba, &mut b);
            f.seek(SeekFrom::Start((lba - START) as u64 * 512)).unwrap();
            f.write_all(&b).unwrap();
        }
    }
}

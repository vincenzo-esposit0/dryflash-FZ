// SPDX-License-Identifier: GPL-3.0-or-later
//! Prepares a new virtual card: an MBR with one FAT32 partition aligned at 1 MiB, as on a real SD
//! card (Microsoft FAT specification; checked by the firmware's own FatFs when it mounts).

use super::sdcard::{BLOCK_SIZE, BlockStore};

const PARTITION_START: u32 = 2048;
const RESERVED: u32 = 32;
const FATS: u32 = 2;
const SECTORS_PER_CLUSTER: u32 = 8;
const ROOT_CLUSTER: u32 = 2;
const END_OF_CHAIN: u32 = 0x0FFF_FFFF;

/// Volume layout computed by `format`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    pub fat_sectors: u32,
    pub first_data_sector: u32,
    pub clusters: u32,
}

/// Number of FAT sectors and data clusters for a partition of `sectors` sectors.
pub fn layout(sectors: u32) -> Layout {
    // A larger FAT only shrinks the data area, so the size found first is always enough.
    let mut fat_sectors = 1;
    loop {
        let data = sectors - RESERVED - FATS * fat_sectors;
        let clusters = data / SECTORS_PER_CLUSTER;
        let needed = ((clusters + 2) * 4).div_ceil(BLOCK_SIZE as u32);
        if needed <= fat_sectors {
            return Layout {
                fat_sectors,
                first_data_sector: RESERVED + FATS * fat_sectors,
                clusters,
            };
        }
        fat_sectors = needed;
    }
}

fn put16(b: &mut [u8], off: usize, v: u16) {
    b[off..off + 2].copy_from_slice(&v.to_le_bytes());
}

fn put32(b: &mut [u8], off: usize, v: u32) {
    b[off..off + 4].copy_from_slice(&v.to_le_bytes());
}

/// Writes an empty FAT32 volume on a card of `capacity_blocks` blocks.
pub fn format(store: &mut BlockStore, capacity_blocks: u32, serial: u32) -> Layout {
    let sectors = capacity_blocks - PARTITION_START;
    let l = layout(sectors);
    assert!(l.clusters >= 65525, "too small for FAT32");

    // MBR: one bootable-less partition, type 0x0C (FAT32 LBA).
    let mut mbr = [0u8; BLOCK_SIZE];
    mbr[446 + 4] = 0x0C;
    put32(&mut mbr, 446 + 8, PARTITION_START);
    put32(&mut mbr, 446 + 12, sectors);
    mbr[446 + 1..446 + 4].copy_from_slice(&[0xFE, 0xFF, 0xFF]);
    mbr[446 + 5..446 + 8].copy_from_slice(&[0xFE, 0xFF, 0xFF]);
    put16(&mut mbr, 510, 0xAA55);
    store.write(0, &mbr);

    // Boot sector (BPB for FAT32), with its backup at sector 6.
    let mut bs = [0u8; BLOCK_SIZE];
    bs[0..3].copy_from_slice(&[0xEB, 0x58, 0x90]);
    bs[3..11].copy_from_slice(b"DRYFLASH");
    put16(&mut bs, 11, BLOCK_SIZE as u16);
    bs[13] = SECTORS_PER_CLUSTER as u8;
    put16(&mut bs, 14, RESERVED as u16);
    bs[16] = FATS as u8;
    bs[21] = 0xF8;
    put16(&mut bs, 24, 63);
    put16(&mut bs, 26, 255);
    put32(&mut bs, 28, PARTITION_START);
    put32(&mut bs, 32, sectors);
    put32(&mut bs, 36, l.fat_sectors);
    put32(&mut bs, 44, ROOT_CLUSTER);
    put16(&mut bs, 48, 1); // FSInfo sector
    put16(&mut bs, 50, 6); // backup boot sector
    bs[64] = 0x80;
    bs[66] = 0x29;
    put32(&mut bs, 67, serial);
    bs[71..82].copy_from_slice(b"DRYFLASH   ");
    bs[82..90].copy_from_slice(b"FAT32   ");
    put16(&mut bs, 510, 0xAA55);
    store.write(PARTITION_START, &bs);
    store.write(PARTITION_START + 6, &bs);

    // FSInfo, with its backup at sector 7: the root directory takes one cluster.
    let mut fsi = [0u8; BLOCK_SIZE];
    put32(&mut fsi, 0, 0x4161_5252);
    put32(&mut fsi, 484, 0x6141_7272);
    put32(&mut fsi, 488, l.clusters - 1);
    put32(&mut fsi, 492, ROOT_CLUSTER + 1);
    put32(&mut fsi, 508, 0xAA55_0000);
    store.write(PARTITION_START + 1, &fsi);
    store.write(PARTITION_START + 7, &fsi);

    // Both FATs: media descriptor, end-of-chain marker and the root directory's cluster.
    let mut fat = [0u8; BLOCK_SIZE];
    put32(&mut fat, 0, 0x0FFF_FFF8);
    put32(&mut fat, 4, END_OF_CHAIN);
    put32(&mut fat, 8, END_OF_CHAIN);
    store.write(PARTITION_START + RESERVED, &fat);
    store.write(PARTITION_START + RESERVED + l.fat_sectors, &fat);
    // The root directory cluster is all zero: unwritten blocks already read as zero.
    l
}

#[cfg(test)]
mod tests {
    use super::*;

    const ONE_GIB: u32 = 2_097_152;

    #[test]
    fn one_gib_card_is_fat32_by_cluster_count() {
        let l = layout(ONE_GIB - PARTITION_START);
        assert!(l.clusters >= 65525 && l.clusters < 0x0FFF_FFF5);
        // The FAT holds every cluster, with little slack (the data area shrank after sizing it).
        assert!((l.clusters + 2) * 4 <= l.fat_sectors * 512);
        assert!((l.clusters + 2) * 4 > (l.fat_sectors - 16) * 512);
    }

    #[test]
    fn format_writes_mbr_boot_sector_and_fats() {
        let mut s = BlockStore::default();
        let l = format(&mut s, ONE_GIB, 0x1234_5678);
        let mut b = [0u8; 512];
        s.read(0, &mut b);
        assert_eq!((b[450], b[510], b[511]), (0x0C, 0x55, 0xAA));
        s.read(PARTITION_START, &mut b);
        assert_eq!(&b[82..90], b"FAT32   ");
        assert_eq!(u32::from_le_bytes(b[36..40].try_into().unwrap()), l.fat_sectors);
        for fat in [RESERVED, RESERVED + l.fat_sectors] {
            s.read(PARTITION_START + fat, &mut b);
            assert_eq!(u32::from_le_bytes(b[0..4].try_into().unwrap()), 0x0FFF_FFF8);
            assert_eq!(u32::from_le_bytes(b[8..12].try_into().unwrap()), END_OF_CHAIN);
        }
        assert!(s.used_blocks() < 10, "a formatted card stays sparse");
    }
}

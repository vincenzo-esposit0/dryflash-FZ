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

/// A mounted FAT32 volume, for the host side: installing resources and (later) the file manager.
pub struct Volume<'a> {
    store: &'a mut BlockStore,
    start: u32,
    spc: u32,
    reserved: u32,
    fats: u32,
    fat_sectors: u32,
    root: u32,
    clusters: u32,
    first_data: u32,
    next_free: u32,
    free: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u32,
    pub cluster: u32,
    /// Where the short entry sits, and how many long-name slots precede it.
    slot: (u32, u32),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FatError {
    NotFat32,
    NoSpace,
    NotFound(String),
    NotADirectory(String),
    BadName(String),
}

impl std::fmt::Display for FatError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FatError::NotFat32 => write!(f, "the card has no FAT32 volume"),
            FatError::NoSpace => write!(f, "the card is full"),
            FatError::NotFound(p) => write!(f, "{p}: not found"),
            FatError::NotADirectory(p) => write!(f, "{p}: not a directory"),
            FatError::BadName(p) => write!(f, "{p}: invalid file name"),
        }
    }
}

impl std::error::Error for FatError {}

const ATTR_DIR: u8 = 0x10;
const ATTR_VOLUME: u8 = 0x08;
const ATTR_LFN: u8 = 0x0F;
const EOC_MIN: u32 = 0x0FFF_FFF8;
/// 2020-01-01 00:00: file dates are not meaningful here and must not depend on the host clock.
const FAT_DATE: u16 = (40 << 9) | (1 << 5) | 1;

fn lfn_checksum(sfn: &[u8]) -> u8 {
    sfn.iter().fold(0u8, |s, &b| s.rotate_right(1).wrapping_add(b))
}

/// The 11-byte short name if `name` is already a valid upper-case 8.3 name.
fn plain_sfn(name: &str) -> Option<[u8; 11]> {
    let (base, ext) = name.rsplit_once('.').unwrap_or((name, ""));
    let valid = |s: &str, max: usize| {
        s.len() <= max
            && s.bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b"_-~!#$%&'()@^`{}".contains(&b))
    };
    if base.is_empty() || !valid(base, 8) || !valid(ext, 3) {
        return None;
    }
    let mut sfn = [b' '; 11];
    sfn[..base.len()].copy_from_slice(base.as_bytes());
    sfn[8..8 + ext.len()].copy_from_slice(ext.as_bytes());
    Some(sfn)
}

/// Basis of a generated short name: upper case, invalid characters replaced, `~n` appended.
fn numbered_sfn(name: &str, n: u32) -> [u8; 11] {
    let clean = |s: &str| -> Vec<u8> {
        s.bytes()
            .filter(|&b| b != b' ' && b != b'.')
            .map(|b| {
                let u = b.to_ascii_uppercase();
                if u.is_ascii_uppercase() || u.is_ascii_digit() || b"_-~!#$%&'()@^`{}".contains(&u) {
                    u
                } else {
                    b'_'
                }
            })
            .collect()
    };
    let (base, ext) = name
        .rsplit_once('.')
        .filter(|(b, _)| !b.is_empty())
        .unwrap_or((name, ""));
    let (base, ext) = (clean(base), clean(ext));
    let tail = format!("~{n}");
    let keep = base.len().min(8 - tail.len());
    let mut sfn = [b' '; 11];
    sfn[..keep].copy_from_slice(&base[..keep]);
    sfn[keep..keep + tail.len()].copy_from_slice(tail.as_bytes());
    let e = ext.len().min(3);
    sfn[8..8 + e].copy_from_slice(&ext[..e]);
    sfn
}

impl<'a> Volume<'a> {
    pub fn open(store: &'a mut BlockStore) -> Result<Volume<'a>, FatError> {
        let mut b = [0u8; BLOCK_SIZE];
        store.read(0, &mut b);
        if b[510..512] != [0x55, 0xAA] {
            return Err(FatError::NotFat32);
        }
        let start = u32::from_le_bytes(b[454..458].try_into().unwrap());
        store.read(start, &mut b);
        if b[510..512] != [0x55, 0xAA] || &b[82..87] != b"FAT32" {
            return Err(FatError::NotFat32);
        }
        let spc = b[13] as u32;
        let reserved = u16::from_le_bytes([b[14], b[15]]) as u32;
        let fats = b[16] as u32;
        let sectors = u32::from_le_bytes(b[32..36].try_into().unwrap());
        let fat_sectors = u32::from_le_bytes(b[36..40].try_into().unwrap());
        let root = u32::from_le_bytes(b[44..48].try_into().unwrap());
        if spc == 0 || fat_sectors == 0 {
            return Err(FatError::NotFat32);
        }
        let first_data = reserved + fats * fat_sectors;
        let clusters = (sectors - first_data) / spc;
        let mut v = Volume {
            store,
            start,
            spc,
            reserved,
            fats,
            fat_sectors,
            root,
            clusters,
            first_data,
            next_free: 3,
            free: 0,
        };
        v.free = v.count_free();
        Ok(v)
    }

    pub fn free_clusters(&self) -> u32 {
        self.free
    }

    /// Free clusters, from the FAT itself: the firmware may have left FSInfo stale.
    fn count_free(&self) -> u32 {
        let mut b = [0u8; BLOCK_SIZE];
        let mut free = 0;
        for c in 2..self.clusters + 2 {
            if c % 128 == 0 || c == 2 {
                self.store.read(self.start + self.reserved + c / 128, &mut b);
            }
            let o = (c % 128) as usize * 4;
            if u32::from_le_bytes(b[o..o + 4].try_into().unwrap()) & 0x0FFF_FFFF == 0 {
                free += 1;
            }
        }
        free
    }

    fn fat_get(&self, c: u32) -> u32 {
        let mut b = [0u8; BLOCK_SIZE];
        self.store.read(self.start + self.reserved + c / 128, &mut b);
        let o = (c % 128) as usize * 4;
        u32::from_le_bytes(b[o..o + 4].try_into().unwrap()) & 0x0FFF_FFFF
    }

    fn fat_set(&mut self, c: u32, v: u32) {
        let mut b = [0u8; BLOCK_SIZE];
        for f in 0..self.fats {
            let lba = self.start + self.reserved + f * self.fat_sectors + c / 128;
            self.store.read(lba, &mut b);
            let o = (c % 128) as usize * 4;
            b[o..o + 4].copy_from_slice(&v.to_le_bytes());
            self.store.write(lba, &b);
        }
    }

    fn cluster_lba(&self, c: u32) -> u32 {
        self.start + self.first_data + (c - 2) * self.spc
    }

    /// Allocates a cluster at the end of the chain ending in `prev` (0: a new chain).
    fn alloc(&mut self, prev: u32) -> Result<u32, FatError> {
        if self.free == 0 {
            return Err(FatError::NoSpace);
        }
        let end = self.clusters + 2;
        let mut c = self.next_free.clamp(2, end - 1);
        for _ in 0..self.clusters {
            if self.fat_get(c) == 0 {
                self.fat_set(c, END_OF_CHAIN);
                if prev != 0 {
                    self.fat_set(prev, c);
                }
                self.free -= 1;
                self.next_free = c + 1;
                return Ok(c);
            }
            c = if c + 1 >= end { 2 } else { c + 1 };
        }
        Err(FatError::NoSpace)
    }

    fn free_chain(&mut self, first: u32) {
        let mut c = first;
        while (2..EOC_MIN).contains(&c) {
            let next = self.fat_get(c);
            self.fat_set(c, 0);
            self.free += 1;
            self.next_free = self.next_free.min(c);
            c = next;
        }
    }

    fn zero_cluster(&mut self, c: u32) {
        let zero = [0u8; BLOCK_SIZE];
        for i in 0..self.spc {
            self.store.write(self.cluster_lba(c) + i, &zero);
        }
    }

    /// Absolute LBA and offset of each 32-byte slot of a directory, in order.
    fn dir_slots(&self, dir: u32) -> Vec<(u32, usize)> {
        let mut out = Vec::new();
        let mut c = dir;
        while (2..EOC_MIN).contains(&c) {
            for s in 0..self.spc {
                for o in 0..BLOCK_SIZE / 32 {
                    out.push((self.cluster_lba(c) + s, o * 32));
                }
            }
            c = self.fat_get(c);
        }
        out
    }

    fn read_slot(&self, (lba, off): (u32, usize)) -> [u8; 32] {
        let mut b = [0u8; BLOCK_SIZE];
        self.store.read(lba, &mut b);
        b[off..off + 32].try_into().unwrap()
    }

    fn write_slot(&mut self, (lba, off): (u32, usize), slot: &[u8; 32]) {
        let mut b = [0u8; BLOCK_SIZE];
        self.store.read(lba, &mut b);
        b[off..off + 32].copy_from_slice(slot);
        self.store.write(lba, &b);
    }

    /// Entries of a directory (`.` and `..` and the volume label left out).
    pub fn list_dir(&self, dir: u32) -> Vec<DirEntry> {
        let mut out = Vec::new();
        let mut lfn: Vec<u16> = Vec::new();
        let mut lfn_slots = 0;
        for (i, loc) in self.dir_slots(dir).into_iter().enumerate() {
            let e = self.read_slot(loc);
            match e[0] {
                0x00 => break,
                0xE5 => {
                    lfn.clear();
                    lfn_slots = 0;
                    continue;
                }
                _ => {}
            }
            if e[11] == ATTR_LFN {
                let mut part: Vec<u16> = Vec::with_capacity(13);
                for r in [1..11usize, 14..26, 28..32] {
                    part.extend(e[r].chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])));
                }
                let end = part.iter().position(|&u| u == 0 || u == 0xFFFF).unwrap_or(part.len());
                part.truncate(end);
                part.append(&mut lfn);
                lfn = part;
                lfn_slots += 1;
                continue;
            }
            if e[11] & ATTR_VOLUME == 0 && e[0] != b'.' {
                let name = if lfn.is_empty() {
                    let text = |r: std::ops::Range<usize>| String::from_utf8_lossy(&e[r]).trim_end().to_string();
                    let (b, x) = (text(0..8), text(8..11));
                    if x.is_empty() { b } else { format!("{b}.{x}") }
                } else {
                    String::from_utf16_lossy(&lfn)
                };
                out.push(DirEntry {
                    name,
                    is_dir: e[11] & ATTR_DIR != 0,
                    size: u32::from_le_bytes(e[28..32].try_into().unwrap()),
                    cluster: (u16::from_le_bytes([e[20], e[21]]) as u32) << 16
                        | u16::from_le_bytes([e[26], e[27]]) as u32,
                    slot: (i as u32, lfn_slots),
                });
            }
            lfn.clear();
            lfn_slots = 0;
        }
        out
    }

    fn find(&self, dir: u32, name: &str) -> Option<DirEntry> {
        self.list_dir(dir)
            .into_iter()
            .find(|e| e.name.eq_ignore_ascii_case(name))
    }

    /// Follows `path` from the root; `Ok(None)` for a missing last component.
    pub fn lookup(&self, path: &str) -> Result<Option<DirEntry>, FatError> {
        let mut dir = self.root;
        let mut found = None;
        let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
        for (i, part) in parts.iter().enumerate() {
            match self.find(dir, part) {
                Some(e) => {
                    if i + 1 < parts.len() && !e.is_dir {
                        return Err(FatError::NotADirectory(part.to_string()));
                    }
                    dir = e.cluster;
                    found = Some(e);
                }
                None if i + 1 == parts.len() => return Ok(None),
                None => return Err(FatError::NotFound(part.to_string())),
            }
        }
        Ok(found)
    }

    /// Creates a short entry (with long-name slots when needed) in `dir`.
    fn add_entry(&mut self, dir: u32, name: &str, attr: u8, cluster: u32, size: u32) -> Result<(), FatError> {
        if name.is_empty()
            || name.len() > 255
            || name == "."
            || name == ".."
            || name.bytes().any(|b| b < 0x20 || b"\"*:<>?|/".contains(&b))
        {
            return Err(FatError::BadName(name.to_string()));
        }
        let existing: Vec<[u8; 32]> = self.dir_slots(dir).into_iter().map(|l| self.read_slot(l)).collect();
        let taken = |sfn: &[u8; 11]| {
            existing
                .iter()
                .any(|e| e[11] != ATTR_LFN && e[0] != 0xE5 && e[0] != 0 && e[..11] == sfn[..])
        };
        let (sfn, long) = match plain_sfn(name) {
            Some(s) if !taken(&s) => (s, false),
            _ => {
                let mut n = 1;
                let mut s = numbered_sfn(name, n);
                while taken(&s) {
                    n += 1;
                    s = numbered_sfn(name, n);
                }
                (s, true)
            }
        };
        let units: Vec<u16> = name.encode_utf16().collect();
        let lfn_count = if long { units.len().div_ceil(13) } else { 0 };
        let needed = lfn_count + 1;

        // First run of free slots long enough; entries past an end marker are free too.
        let mut slots = self.dir_slots(dir);
        let mut run = 0;
        let mut at = None;
        for (i, &loc) in slots.iter().enumerate() {
            let first = self.read_slot(loc)[0];
            if first == 0 || first == 0xE5 {
                run += 1;
                if run == needed {
                    at = Some(i + 1 - needed);
                    break;
                }
            } else {
                run = 0;
            }
        }
        let at = match at {
            Some(a) => a,
            None => {
                let mut last = dir;
                loop {
                    let n = self.fat_get(last);
                    if !(2..EOC_MIN).contains(&n) {
                        break;
                    }
                    last = n;
                }
                let new = self.alloc(last)?;
                self.zero_cluster(new);
                slots = self.dir_slots(dir);
                slots.len() - (self.spc as usize * BLOCK_SIZE / 32) - run
            }
        };
        let check = lfn_checksum(&sfn);
        for k in 0..lfn_count {
            // Stored last part first; sequence numbers count down to 1.
            let seq = lfn_count - k;
            let mut e = [0xFFu8; 32];
            let chunk: Vec<u16> = units.iter().copied().skip((seq - 1) * 13).take(13).collect();
            let unit = |i: usize| -> u16 {
                match chunk.get(i) {
                    Some(&u) => u,
                    None if i == chunk.len() => 0,
                    None => 0xFFFF,
                }
            };
            let positions: [usize; 13] = [1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30];
            for (i, &p) in positions.iter().enumerate() {
                e[p..p + 2].copy_from_slice(&unit(i).to_le_bytes());
            }
            e[0] = seq as u8 | if k == 0 { 0x40 } else { 0 };
            e[11] = ATTR_LFN;
            e[12] = 0;
            e[13] = check;
            e[26] = 0;
            e[27] = 0;
            self.write_slot(slots[at + k], &e);
        }
        let mut e = [0u8; 32];
        e[..11].copy_from_slice(&sfn);
        e[11] = attr;
        e[16..18].copy_from_slice(&FAT_DATE.to_le_bytes());
        e[18..20].copy_from_slice(&FAT_DATE.to_le_bytes());
        e[20..22].copy_from_slice(&((cluster >> 16) as u16).to_le_bytes());
        e[24..26].copy_from_slice(&FAT_DATE.to_le_bytes());
        e[26..28].copy_from_slice(&(cluster as u16).to_le_bytes());
        e[28..32].copy_from_slice(&size.to_le_bytes());
        self.write_slot(slots[at + lfn_count], &e);
        Ok(())
    }

    /// Creates the directory and its parents if missing; returns its first cluster.
    pub fn mkdir_all(&mut self, path: &str) -> Result<u32, FatError> {
        let mut dir = self.root;
        for part in path.split('/').filter(|p| !p.is_empty()) {
            dir = match self.find(dir, part) {
                Some(e) if e.is_dir => e.cluster,
                Some(_) => return Err(FatError::NotADirectory(part.to_string())),
                None => {
                    let c = self.alloc(0)?;
                    self.zero_cluster(c);
                    let dotdot = if dir == self.root { 0 } else { dir };
                    let mut dots = [0u8; BLOCK_SIZE];
                    for (i, (n, target)) in [(&b".          "[..], c), (&b"..         "[..], dotdot)]
                        .into_iter()
                        .enumerate()
                    {
                        let e = &mut dots[i * 32..(i + 1) * 32];
                        e[..11].copy_from_slice(n);
                        e[11] = ATTR_DIR;
                        e[16..18].copy_from_slice(&FAT_DATE.to_le_bytes());
                        e[24..26].copy_from_slice(&FAT_DATE.to_le_bytes());
                        e[20..22].copy_from_slice(&((target >> 16) as u16).to_le_bytes());
                        e[26..28].copy_from_slice(&(target as u16).to_le_bytes());
                    }
                    let lba = self.cluster_lba(c);
                    self.store.write(lba, &dots);
                    self.add_entry(dir, part, ATTR_DIR, c, 0)?;
                    c
                }
            };
        }
        Ok(dir)
    }

    /// Creates or replaces a file, creating its parent directories.
    pub fn write_file(&mut self, path: &str, data: &[u8]) -> Result<(), FatError> {
        let (parent, name) = path
            .trim_matches('/')
            .rsplit_once('/')
            .unwrap_or(("", path.trim_matches('/')));
        let dir = self.mkdir_all(parent)?;
        if let Some(old) = self.find(dir, name) {
            if old.is_dir {
                return Err(FatError::NotADirectory(name.to_string()));
            }
            self.remove_entry(dir, &old);
        }
        let mut first = 0;
        let mut prev = 0;
        let cluster_bytes = self.spc as usize * BLOCK_SIZE;
        let mut block = [0u8; BLOCK_SIZE];
        for chunk in data.chunks(cluster_bytes) {
            let c = self.alloc(prev)?;
            if first == 0 {
                first = c;
            }
            prev = c;
            for (s, part) in chunk.chunks(BLOCK_SIZE).enumerate() {
                block.fill(0);
                block[..part.len()].copy_from_slice(part);
                let lba = self.cluster_lba(c) + s as u32;
                self.store.write(lba, &block);
            }
        }
        self.add_entry(dir, name, 0x20, first, data.len() as u32)
    }

    fn remove_entry(&mut self, dir: u32, e: &DirEntry) {
        let slots = self.dir_slots(dir);
        let (short, lfn) = (e.slot.0 as usize, e.slot.1 as usize);
        for &loc in &slots[short - lfn..=short] {
            let mut s = self.read_slot(loc);
            s[0] = 0xE5;
            self.write_slot(loc, &s);
        }
        self.free_chain(e.cluster);
    }

    /// Reads a whole file.
    pub fn read_file(&self, path: &str) -> Result<Vec<u8>, FatError> {
        let e = self.lookup(path)?.ok_or_else(|| FatError::NotFound(path.to_string()))?;
        let mut out = Vec::with_capacity(e.size as usize);
        let mut b = [0u8; BLOCK_SIZE];
        let mut c = e.cluster;
        while (2..EOC_MIN).contains(&c) && out.len() < e.size as usize {
            for s in 0..self.spc {
                self.store.read(self.cluster_lba(c) + s, &mut b);
                let n = (e.size as usize - out.len()).min(BLOCK_SIZE);
                out.extend_from_slice(&b[..n]);
                if out.len() == e.size as usize {
                    break;
                }
            }
            c = self.fat_get(c);
        }
        Ok(out)
    }

    /// Root directory cluster.
    pub fn root(&self) -> u32 {
        self.root
    }

    /// Writes the free-cluster count and allocation hint back to FSInfo (and its backup).
    pub fn sync(&mut self) {
        let mut b = [0u8; BLOCK_SIZE];
        for s in [1, 7] {
            self.store.read(self.start + s, &mut b);
            put32(&mut b, 488, self.free);
            put32(&mut b, 492, self.next_free);
            self.store.write(self.start + s, &b);
        }
    }
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

    fn volume_store() -> BlockStore {
        let mut s = BlockStore::default();
        format(&mut s, ONE_GIB, 7);
        s
    }

    #[test]
    fn files_with_long_names_round_trip() {
        let mut s = volume_store();
        let mut v = Volume::open(&mut s).unwrap();
        let free = v.free_clusters();
        assert_eq!(free, v.clusters - 1);
        let big: Vec<u8> = (0..20_000u32).map(|i| i as u8).collect();
        v.write_file("apps/Games/Snake game with a long name.fap", &big)
            .unwrap();
        v.write_file("Manifest", b"V:0\n").unwrap();
        v.write_file("README.TXT", b"x").unwrap();
        v.write_file("empty.bin", b"").unwrap();
        assert_eq!(v.read_file("apps/games/snake GAME with a long name.fap").unwrap(), big);
        assert_eq!(v.read_file("manifest").unwrap(), b"V:0\n");
        assert_eq!(v.read_file("empty.bin").unwrap(), b"");
        let names: Vec<String> = v.list_dir(v.root()).into_iter().map(|e| e.name).collect();
        assert_eq!(names, ["apps", "Manifest", "README.TXT", "empty.bin"]);
        // 20 000 bytes = 5 clusters, plus 2 directories, plus Manifest and README.
        assert_eq!(free - v.free_clusters(), 5 + 2 + 2);
    }

    #[test]
    fn replacing_a_file_frees_its_clusters() {
        let mut s = volume_store();
        let mut v = Volume::open(&mut s).unwrap();
        v.write_file("a long name.bin", &[1; 9000]).unwrap();
        let free = v.free_clusters();
        v.write_file("a long name.bin", &[2; 100]).unwrap();
        assert_eq!(v.free_clusters(), free + 2);
        assert_eq!(v.read_file("a long name.bin").unwrap(), [2; 100]);
        assert_eq!(v.list_dir(v.root()).len(), 1);
    }

    #[test]
    fn a_directory_grows_past_one_cluster() {
        let mut s = volume_store();
        let mut v = Volume::open(&mut s).unwrap();
        for i in 0..300 {
            v.write_file(&format!("many/file number {i}.txt"), b"x").unwrap();
        }
        assert_eq!(v.list_dir(v.lookup("many").unwrap().unwrap().cluster).len(), 300);
        assert_eq!(v.read_file("many/file number 299.txt").unwrap(), b"x");
        let shorts: std::collections::HashSet<_> = (1..=300).map(|n| numbered_sfn("file number 1.txt", n)).collect();
        assert_eq!(shorts.len(), 300, "numbered short names stay distinct");
    }

    #[test]
    fn invalid_names_and_a_card_without_fat32() {
        let mut s = volume_store();
        let mut v = Volume::open(&mut s).unwrap();
        assert!(matches!(v.write_file("bad:name", b"x"), Err(FatError::BadName(_))));
        v.write_file("f", b"x").unwrap();
        assert!(matches!(v.write_file("f/g", b"x"), Err(FatError::NotADirectory(_))));
        let mut blank = BlockStore::default();
        assert!(matches!(Volume::open(&mut blank), Err(FatError::NotFat32)));
    }
}

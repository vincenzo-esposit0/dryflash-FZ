// SPDX-License-Identifier: GPL-3.0-or-later
//! microSD card in SPI mode (SD Physical Layer Simplified Specification, chapter "SPI Mode";
//! driver: targets/f7/furi_hal/furi_hal_sd.c, firmware 1.5.1). An SDHC card, CRC not checked.
//!
//! Data lives in a sparse block store: blocks never written read as zero and take no memory, so a
//! large card costs only what is used. The host persists `take_dirty` blocks (ADR-0013).

use std::collections::{BTreeSet, HashMap, VecDeque};

pub const BLOCK_SIZE: usize = 512;

/// Sparse block device, shared by the card model and the FAT32 tools.
#[derive(Clone, Debug, Default)]
pub struct BlockStore {
    blocks: HashMap<u32, Box<[u8; BLOCK_SIZE]>>,
    dirty: BTreeSet<u32>,
}

impl BlockStore {
    pub fn read(&self, lba: u32, out: &mut [u8]) {
        match self.blocks.get(&lba) {
            Some(b) => out.copy_from_slice(&b[..]),
            None => out.fill(0),
        }
    }

    pub fn write(&mut self, lba: u32, data: &[u8]) {
        if data.iter().all(|&b| b == 0) {
            self.blocks.remove(&lba);
        } else {
            let mut b = Box::new([0u8; BLOCK_SIZE]);
            b.copy_from_slice(data);
            self.blocks.insert(lba, b);
        }
        self.dirty.insert(lba);
    }

    /// Non-zero blocks held in memory.
    pub fn used_blocks(&self) -> usize {
        self.blocks.len()
    }

    /// Blocks written since the previous call, ascending.
    pub fn take_dirty(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.dirty).into_iter().collect()
    }
}

#[derive(Clone, Debug)]
enum Mode {
    Command,
    /// CMD18: blocks are queued as the host clocks them out, until CMD12.
    ReadStream {
        next: u32,
    },
    /// CMD24/CMD25: waiting for a data token.
    WaitToken {
        block: u32,
        multi: bool,
    },
    /// Receiving 512 data bytes and 2 CRC bytes.
    Data {
        block: u32,
        multi: bool,
        got: usize,
    },
}

// R1 bits.
const R1_IDLE: u8 = 0x01;
const R1_ILLEGAL: u8 = 0x04;
const R1_PARAM: u8 = 0x40;
const TOKEN_START: u8 = 0xFE;
const TOKEN_START_MULTI: u8 = 0xFC;
const TOKEN_STOP_MULTI: u8 = 0xFD;
/// Data accepted (xxx0_0101), then the card is not busy.
const DATA_ACCEPTED: u8 = 0xE5;

#[derive(Clone, Debug)]
pub struct SdCard {
    pub store: BlockStore,
    capacity_blocks: u32,
    mode: Mode,
    cmd: [u8; 6],
    cmd_len: usize,
    out: VecDeque<u8>,
    idle: bool,
    app_cmd: bool,
    /// ACMD41 calls answered so far since CMD0: the first one reports "still initialising".
    acmd41_seen: bool,
    buf: Box<[u8; BLOCK_SIZE]>,
    /// Commands the model does not implement, reported in the digest by the owner.
    pub unsupported: Vec<u8>,
}

impl SdCard {
    /// A card of `capacity_blocks` × 512 bytes; capacity must be a multiple of 512 KiB (CSD v2).
    pub fn new(capacity_blocks: u32, store: BlockStore) -> Self {
        assert!(capacity_blocks >= 1024 && capacity_blocks.is_multiple_of(1024));
        SdCard {
            store,
            capacity_blocks,
            mode: Mode::Command,
            cmd: [0; 6],
            cmd_len: 0,
            out: VecDeque::new(),
            idle: true,
            app_cmd: false,
            acmd41_seen: false,
            buf: Box::new([0; BLOCK_SIZE]),
            unsupported: Vec::new(),
        }
    }

    pub fn capacity_blocks(&self) -> u32 {
        self.capacity_blocks
    }

    /// Chip select went high: the card releases the bus and abandons any answer in progress.
    pub fn deselect(&mut self) {
        self.out.clear();
        self.cmd_len = 0;
        if matches!(self.mode, Mode::ReadStream { .. }) {
            self.mode = Mode::Command;
        }
    }

    /// Exchanges one byte while the card is selected.
    pub fn transfer(&mut self, mosi: u8) -> u8 {
        match self.mode.clone() {
            Mode::Data { block, multi, got } => {
                if got < BLOCK_SIZE {
                    self.buf[got] = mosi;
                }
                let got = got + 1;
                if got == BLOCK_SIZE + 2 {
                    let data = self.buf.clone();
                    self.store.write(block, &data[..]);
                    self.out.push_back(DATA_ACCEPTED);
                    self.mode = if multi {
                        Mode::WaitToken {
                            block: block + 1,
                            multi,
                        }
                    } else {
                        Mode::Command
                    };
                } else {
                    self.mode = Mode::Data { block, multi, got };
                }
                return 0xFF;
            }
            Mode::WaitToken { block, multi } => {
                let start = if multi { TOKEN_START_MULTI } else { TOKEN_START };
                if mosi == start {
                    if block < self.capacity_blocks {
                        self.mode = Mode::Data { block, multi, got: 0 };
                    } else {
                        self.mode = Mode::Command;
                    }
                } else if multi && mosi == TOKEN_STOP_MULTI {
                    self.mode = Mode::Command;
                }
                return self.out.pop_front().unwrap_or(0xFF);
            }
            Mode::Command | Mode::ReadStream { .. } => {}
        }
        if self.cmd_len > 0 || mosi & 0xC0 == 0x40 {
            self.cmd[self.cmd_len] = mosi;
            self.cmd_len += 1;
            if self.cmd_len == 6 {
                self.cmd_len = 0;
                self.execute();
            }
            return 0xFF;
        }
        if self.out.is_empty()
            && let Mode::ReadStream { next } = self.mode
        {
            if next < self.capacity_blocks {
                self.queue_block(next);
                self.mode = Mode::ReadStream { next: next + 1 };
            } else {
                self.mode = Mode::Command;
            }
        }
        self.out.pop_front().unwrap_or(0xFF)
    }

    fn r1(&self) -> u8 {
        if self.idle { R1_IDLE } else { 0 }
    }

    fn queue_block(&mut self, lba: u32) {
        let mut b = [0u8; BLOCK_SIZE];
        self.store.read(lba, &mut b);
        self.out.push_back(TOKEN_START);
        self.out.extend(b);
        self.out.extend([0xFF, 0xFF]); // CRC, not checked by the driver
    }

    fn queue_register(&mut self, r: [u8; 16]) {
        self.out.push_back(TOKEN_START);
        self.out.extend(r);
        self.out.extend([0xFF, 0xFF]);
    }

    fn execute(&mut self) {
        let index = self.cmd[0] & 0x3F;
        let arg = u32::from_be_bytes([self.cmd[1], self.cmd[2], self.cmd[3], self.cmd[4]]);
        let app = std::mem::take(&mut self.app_cmd);
        self.out.clear();
        self.out.push_back(0xFF); // NCR: one byte before the answer
        match index {
            0 => {
                self.idle = true;
                self.acmd41_seen = false;
                self.mode = Mode::Command;
                self.out.push_back(R1_IDLE);
            }
            8 => {
                let echo = arg.to_be_bytes();
                self.out.extend([self.r1(), 0, 0, echo[2] & 0x0F, echo[3]]);
            }
            9 => {
                self.out.push_back(self.r1());
                self.queue_register(self.csd());
            }
            10 => {
                self.out.push_back(self.r1());
                self.queue_register(cid());
            }
            12 => {
                self.mode = Mode::Command;
                self.out.push_back(self.r1());
            }
            13 => self.out.extend([self.r1(), 0]),
            16 => self.out.push_back(if arg as usize == BLOCK_SIZE {
                self.r1()
            } else {
                self.r1() | R1_PARAM
            }),
            17 if arg < self.capacity_blocks => {
                self.out.push_back(self.r1());
                self.queue_block(arg);
            }
            18 if arg < self.capacity_blocks => {
                self.out.push_back(self.r1());
                self.mode = Mode::ReadStream { next: arg };
            }
            24 | 25 if arg < self.capacity_blocks => {
                self.out.push_back(self.r1());
                self.mode = Mode::WaitToken {
                    block: arg,
                    multi: index == 25,
                };
            }
            17 | 18 | 24 | 25 => self.out.push_back(self.r1() | R1_PARAM),
            41 if app => {
                if self.acmd41_seen {
                    self.idle = false;
                }
                self.acmd41_seen = true;
                self.out.push_back(self.r1());
            }
            55 => {
                self.app_cmd = true;
                self.out.push_back(self.r1());
            }
            // OCR: powered up, CCS = 1 (block-addressed SDHC), 2.7–3.6 V.
            58 => self.out.extend([self.r1(), 0xC0, 0xFF, 0x80, 0x00]),
            _ => {
                if !self.unsupported.contains(&index) {
                    self.unsupported.push(index);
                }
                self.out.push_back(self.r1() | R1_ILLEGAL);
            }
        }
    }

    /// CSD version 2.0: capacity = (C_SIZE + 1) × 512 KiB.
    fn csd(&self) -> [u8; 16] {
        let c = self.capacity_blocks / 1024 - 1;
        [
            0x40, // CSD_STRUCTURE = 1
            0x0E,
            0x00,
            0x32, // TRAN_SPEED: 25 MHz
            0x5B,
            0x59, // CCC, READ_BL_LEN = 9
            0x00,
            (c >> 16) as u8 & 0x3F,
            (c >> 8) as u8,
            c as u8,
            0x7F,
            0x80, // ERASE_BLK_EN, SECTOR_SIZE
            0x0A,
            0x40, // R2W_FACTOR, WRITE_BL_LEN = 9
            0x00,
            0x01,
        ]
    }
}

/// CID of the virtual card (simulated identity).
fn cid() -> [u8; 16] {
    let mut c = [0u8; 16];
    c[0] = 0x00; // no manufacturer: a virtual card
    c[1..3].copy_from_slice(b"DF");
    c[3..8].copy_from_slice(b"DRYSD");
    c[8] = 0x10; // product revision 1.0
    c[9..13].copy_from_slice(&1u32.to_be_bytes());
    c[14] = 0x01;
    c[15] = 0x01;
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card() -> SdCard {
        SdCard::new(2048, BlockStore::default())
    }

    /// Sends a command frame and returns the first answer byte other than 0xFF.
    fn cmd(c: &mut SdCard, index: u8, arg: u32) -> u8 {
        c.transfer(0x40 | index);
        for b in arg.to_be_bytes() {
            c.transfer(b);
        }
        c.transfer(0x01);
        for _ in 0..8 {
            let r = c.transfer(0xFF);
            if r != 0xFF {
                return r;
            }
        }
        0xFF
    }

    fn init(c: &mut SdCard) {
        assert_eq!(cmd(c, 0, 0), 1);
        assert_eq!(cmd(c, 8, 0x1AA), 1);
        assert_eq!(
            [c.transfer(0xFF), c.transfer(0xFF), c.transfer(0xFF), c.transfer(0xFF)],
            [0, 0, 1, 0xAA]
        );
        let mut r = 1;
        for _ in 0..3 {
            c.deselect();
            cmd(c, 55, 0);
            c.deselect();
            r = cmd(c, 41, 0x4000_0000);
        }
        assert_eq!(r, 0, "ready after ACMD41 retries");
        c.deselect();
        assert_eq!(cmd(c, 58, 0), 0);
        assert_eq!(c.transfer(0xFF) & 0x40, 0x40, "CCS: block addressed");
        c.deselect();
    }

    #[test]
    fn write_then_read_a_block() {
        let mut c = card();
        init(&mut c);
        assert_eq!(cmd(&mut c, 16, 512), 0);
        c.deselect();
        assert_eq!(cmd(&mut c, 24, 7), 0);
        c.transfer(0xFF);
        c.transfer(0xFF);
        c.transfer(TOKEN_START);
        for i in 0..512 {
            c.transfer(i as u8);
        }
        c.transfer(0xFF);
        c.transfer(0xFF);
        assert_eq!(c.transfer(0xFF) & 0x1F, 0x05, "data accepted");
        assert_eq!(c.transfer(0xFF), 0xFF, "not busy");
        c.deselect();
        assert_eq!(c.store.take_dirty(), vec![7]);

        assert_eq!(cmd(&mut c, 17, 7), 0);
        while c.transfer(0xFF) != TOKEN_START {}
        for i in 0..512 {
            assert_eq!(c.transfer(0xFF), i as u8);
        }
    }

    #[test]
    fn unwritten_blocks_read_as_zero_and_take_no_memory() {
        let mut c = card();
        init(&mut c);
        assert_eq!(cmd(&mut c, 17, 1000), 0);
        while c.transfer(0xFF) != TOKEN_START {}
        assert!((0..512).all(|_| c.transfer(0xFF) == 0));
        assert_eq!(c.store.used_blocks(), 0);
    }

    #[test]
    fn multi_block_read_and_write() {
        let mut c = card();
        init(&mut c);
        assert_eq!(cmd(&mut c, 25, 10), 0);
        for fill in [0xAAu8, 0xBB] {
            c.transfer(TOKEN_START_MULTI);
            for _ in 0..514 {
                c.transfer(fill);
            }
            assert_eq!(c.transfer(0xFF) & 0x1F, 5);
        }
        c.transfer(TOKEN_STOP_MULTI);
        c.deselect();
        assert_eq!(cmd(&mut c, 18, 10), 0);
        for fill in [0xAAu8, 0xBB] {
            while c.transfer(0xFF) != TOKEN_START {}
            assert!((0..512).all(|_| c.transfer(0xFF) == fill));
            c.transfer(0xFF);
            c.transfer(0xFF);
        }
        assert_eq!(cmd(&mut c, 12, 0), 0);
    }

    #[test]
    fn out_of_range_address_and_unknown_command() {
        let mut c = card();
        init(&mut c);
        assert_eq!(cmd(&mut c, 17, 2048), R1_PARAM);
        c.deselect();
        assert_eq!(cmd(&mut c, 42, 0), R1_ILLEGAL);
        assert_eq!(c.unsupported, vec![42]);
    }

    #[test]
    fn csd_encodes_the_capacity() {
        let c = SdCard::new(2_097_152, BlockStore::default());
        let r = c.csd();
        let c_size = ((r[7] as u32 & 0x3F) << 16) | ((r[8] as u32) << 8) | r[9] as u32;
        assert_eq!((c_size + 1) as u64 * 512 * 1024, 1 << 30);
    }
}

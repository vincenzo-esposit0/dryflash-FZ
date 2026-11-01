// SPDX-License-Identifier: GPL-3.0-or-later
//! FLASH controller (RM0434 "Embedded flash memory"; offsets from the SVD; driver:
//! targets/f7/furi_hal/furi_hal_flash.c). Unlock sequences, page erase, 64-bit programming with
//! the sequence and "already programmed" errors, option bytes. Operations finish at once (BSY never
//! shows). The memory itself belongs to `SystemBus`, which carries out `FlashOp`.
//! Sector protection (WRP, PCROP, secure area) is not enforced.

use super::regfile::RegFile;
use super::svd;

const KEYR: u32 = svd::FLASH.offset_of("KEYR");
const OPTKEYR: u32 = svd::FLASH.offset_of("OPTKEYR");
const SR: u32 = svd::FLASH.offset_of("SR");
const CR: u32 = svd::FLASH.offset_of("CR");

const KEY1: u32 = 0x4567_0123;
const KEY2: u32 = 0xCDEF_89AB;
const OPT_KEY1: u32 = 0x0819_2A3B;
const OPT_KEY2: u32 = 0x4C5D_6E7F;

const CR_PG: u32 = 1 << 0;
const CR_PER: u32 = 1 << 1;
const CR_PNB_SHIFT: u32 = 3;
const CR_PNB_MASK: u32 = 0xFF << CR_PNB_SHIFT;
const CR_STRT: u32 = 1 << 16;
const CR_OPTSTRT: u32 = 1 << 17;
const CR_FSTPG: u32 = 1 << 18;
const CR_EOPIE: u32 = 1 << 24;
const CR_OBL_LAUNCH: u32 = 1 << 27;
const CR_OPTLOCK: u32 = 1 << 30;
const CR_LOCK: u32 = 1 << 31;

const SR_EOP: u32 = 1 << 0;
const SR_PROGERR: u32 = 1 << 3;
const SR_PGAERR: u32 = 1 << 5;
const SR_SIZERR: u32 = 1 << 6;

pub const PAGE_SIZE: usize = 4096;
pub const OPTION_BYTE_BASE: u32 = 0x1FFF_8000;
pub const OPTION_BYTE_SIZE: u32 = 0x80;

/// Register loaded from each option-byte pair (`furi_hal_flash_ob_reg_map`; the last two are the
/// secure-flash and CPU2 words, read-only through this path).
const OB_REGS: [(usize, &str); 10] = [
    (0, "OPTR"),
    (1, "PCROP1ASR"),
    (2, "PCROP1AER"),
    (3, "WRP1AR"),
    (4, "WRP1BR"),
    (5, "PCROP1BSR"),
    (6, "PCROP1BER"),
    (13, "IPCCBR"),
    (14, "SFR"),
    (15, "SRRVR"),
];

/// The `OB reference` bits shared by all pinned update packages (update.fuf): what the updater
/// expects of a production unit. Pairs of value and complement; bits outside the packages' compare
/// mask are 0, and each complement is derived.
const OB_VALUES: [u32; 16] = [
    0x0D8F_79AA,
    0x0000_01FF,
    0x8000_0000,
    0x0000_00FF,
    0x0000_00FF,
    0x0000_01FF,
    0,
    0,
    0,
    0,
    0,
    0,
    0,
    0,           // IPCCBR: the mailbox tables at the start of SRAM2A
    0x0000_10D7, // SFSA 0xD7: the secure area starts at 0x080D7000 ("Radio address")
    0x964B_5C00,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlashOp {
    ErasePage(usize),
    /// OBL_LAUNCH: option bytes reload through a system reset.
    Reload,
}

#[derive(Clone, Debug)]
pub struct Flash {
    pub regs: RegFile,
    key_stage: u8,
    opt_stage: u8,
    /// First word of a 64-bit program operation.
    half: Option<(u32, u32)>,
    pub ob: [u8; OPTION_BYTE_SIZE as usize],
    pending: Option<FlashOp>,
}

impl Flash {
    pub fn new() -> Self {
        let mut ob = [0u8; OPTION_BYTE_SIZE as usize];
        for (i, v) in OB_VALUES.iter().enumerate() {
            ob[i * 8..i * 8 + 4].copy_from_slice(&v.to_le_bytes());
            ob[i * 8 + 4..i * 8 + 8].copy_from_slice(&(!v).to_le_bytes());
        }
        let mut f = Flash {
            regs: RegFile::new(&svd::FLASH),
            key_stage: 0,
            opt_stage: 0,
            half: None,
            ob,
            pending: None,
        };
        f.reset();
        f
    }

    fn ob_value(&self, i: usize) -> u32 {
        u32::from_le_bytes(self.ob[i * 8..i * 8 + 4].try_into().unwrap())
    }

    pub fn reset(&mut self) {
        self.regs.reset();
        self.key_stage = 0;
        self.opt_stage = 0;
        self.half = None;
        self.pending = None;
        for (i, name) in OB_REGS {
            self.regs.set(svd::FLASH.offset_of(name), self.ob_value(i));
        }
    }

    pub fn write(&mut self, off: u32, v: u32) -> bool {
        match off {
            KEYR => {
                self.key_stage = match (self.key_stage, v) {
                    (0, KEY1) => 1,
                    (1, KEY2) => {
                        self.regs.set_bits(CR, CR_LOCK, false);
                        0
                    }
                    _ => 0,
                };
                true
            }
            OPTKEYR => {
                self.opt_stage = match (self.opt_stage, v) {
                    (0, OPT_KEY1) => 1,
                    (1, OPT_KEY2) => {
                        self.regs.set_bits(CR, CR_OPTLOCK, false);
                        0
                    }
                    _ => 0,
                };
                true
            }
            SR => {
                // Status bits are cleared by writing 1.
                let sr = self.regs.get(SR);
                self.regs.set(SR, sr & !v);
                true
            }
            CR => {
                self.write_cr(v);
                true
            }
            _ => self.regs.write(off, v),
        }
    }

    fn write_cr(&mut self, v: u32) {
        let cr = self.regs.get(CR);
        if cr & CR_LOCK != 0 {
            return; // ignored until the unlock sequence
        }
        let mut next = v & !(CR_STRT | CR_OPTSTRT | CR_OBL_LAUNCH);
        // LOCK and OPTLOCK can be set, never cleared, by a write.
        next |= cr & (CR_LOCK | CR_OPTLOCK);
        if v & CR_STRT != 0 && v & CR_PER != 0 {
            self.pending = Some(FlashOp::ErasePage(((v & CR_PNB_MASK) >> CR_PNB_SHIFT) as usize));
            self.end_of_operation(v);
        }
        if cr & CR_OPTLOCK == 0 {
            if v & CR_OPTSTRT != 0 {
                self.store_option_bytes();
                self.end_of_operation(v);
            }
            if v & CR_OBL_LAUNCH != 0 {
                self.pending = Some(FlashOp::Reload);
            }
        }
        self.regs.set(CR, next);
    }

    fn end_of_operation(&mut self, cr: u32) {
        if cr & CR_EOPIE != 0 {
            self.regs.set_bits(SR, SR_EOP, true);
        }
    }

    /// OPTSTRT: the option registers go to option-byte memory, with their complements.
    fn store_option_bytes(&mut self) {
        for (i, name) in OB_REGS {
            let v = self.regs.get(svd::FLASH.offset_of(name));
            self.ob[i * 8..i * 8 + 4].copy_from_slice(&v.to_le_bytes());
            self.ob[i * 8 + 4..i * 8 + 8].copy_from_slice(&(!v).to_le_bytes());
        }
    }

    pub fn take_op(&mut self) -> Option<FlashOp> {
        self.pending.take()
    }

    /// True while a CPU write to flash memory is a programming step.
    pub fn programming(&self) -> bool {
        let cr = self.regs.get(CR);
        cr & CR_LOCK == 0 && cr & (CR_PG | CR_FSTPG) != 0
    }

    /// A word written to flash in programming mode. Returns the 64-bit write to perform, as
    /// (aligned address, value), once both words have arrived.
    pub fn program_word(&mut self, addr: u32, v: u32, word_size: bool) -> Option<(u32, u64)> {
        if !word_size {
            self.regs.set_bits(SR, SR_SIZERR, true);
            self.half = None;
            return None;
        }
        match self.half.take() {
            None if addr & 7 == 0 => {
                self.half = Some((addr, v));
                None
            }
            Some((first, low)) if addr == first + 4 => {
                self.end_of_operation(self.regs.get(CR));
                Some((first, low as u64 | (v as u64) << 32))
            }
            _ => {
                self.regs.set_bits(SR, SR_PGAERR, true);
                None
            }
        }
    }

    /// The target of a 64-bit write was not erased, and the value is not all zeros.
    pub fn program_error(&mut self) {
        self.regs.set_bits(SR, SR_PROGERR, true);
    }
}

impl Default for Flash {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unlocked() -> Flash {
        let mut f = Flash::new();
        f.write(KEYR, KEY1);
        f.write(KEYR, KEY2);
        assert_eq!(f.regs.get(CR) & CR_LOCK, 0);
        f
    }

    #[test]
    fn unlock_needs_the_exact_sequence_and_relocks() {
        let mut f = Flash::new();
        f.write(KEYR, KEY2);
        f.write(KEYR, KEY1);
        assert_ne!(f.regs.get(CR) & CR_LOCK, 0);
        f.write(CR, CR_PER | CR_STRT);
        assert_eq!(f.take_op(), None, "ignored while locked");
        let mut f = unlocked();
        f.write(CR, CR_LOCK);
        assert_ne!(f.regs.get(CR) & CR_LOCK, 0);
    }

    #[test]
    fn erase_reports_the_page_and_strt_clears_itself() {
        let mut f = unlocked();
        f.write(CR, CR_PER | (42 << CR_PNB_SHIFT) | CR_STRT);
        assert_eq!(f.take_op(), Some(FlashOp::ErasePage(42)));
        assert_eq!(f.regs.get(CR) & CR_STRT, 0);
    }

    #[test]
    fn programming_takes_two_aligned_words() {
        let mut f = unlocked();
        f.write(CR, CR_PG);
        assert!(f.programming());
        assert_eq!(f.program_word(0x0800_0008, 1, true), None);
        assert_eq!(f.program_word(0x0800_000C, 2, true), Some((0x0800_0008, 0x2_0000_0001)));
        assert_eq!(f.program_word(0x0800_0004, 1, true), None);
        assert_ne!(f.regs.get(SR) & SR_PGAERR, 0, "misaligned start");
        f.write(SR, SR_PGAERR);
        assert_eq!(f.regs.get(SR), 0, "cleared by writing 1");
    }

    #[test]
    fn option_bytes_match_what_the_packages_expect() {
        // update.fuf: OB reference and compare mask of the first and last pairs.
        let f = Flash::new();
        assert_eq!(f.ob_value(0) & 0x0F8F_7FFF, 0x0D8F_79AA & 0x0F8F_7FFF);
        assert_eq!(f.ob_value(14) & 0x0000_11FF, 0x10D7 & 0x11FF);
        let comp = u32::from_le_bytes(f.ob[4..8].try_into().unwrap());
        assert_eq!(comp, !f.ob_value(0));
    }
}

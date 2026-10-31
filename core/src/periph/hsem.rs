// SPDX-License-Identifier: GPL-3.0-or-later
//! Hardware semaphore (RM0434 "HSEM"; offsets from the SVD, core IDs from stm32wb55xx.h:
//! HSEM_CR_COREID_CPU1 = 0x4 << 8, HSEM_CR_COREID_CPU2 = 0x8 << 8).
//!
//! 32 semaphores. 2-step lock: write Rx with LOCK|COREID|PROCID. 1-step lock: read RLRx,
//! which locks a free semaphore for the reading core (PROCID 0). Unlock: write Rx with LOCK = 0
//! and the owner's COREID/PROCID; freeing a semaphore raises the "free" interrupt for the
//! cores that enabled it (C1IER). CPU2 never takes a semaphore while it is not emulated.

use super::regfile::RegFile;
use super::svd;

const RLR0: u32 = svd::HSEM.offset_of("RLR0");
const C1IER: u32 = svd::HSEM.offset_of("C1IER0");
const C1ICR: u32 = svd::HSEM.offset_of("C1ICR");
const C1ISR: u32 = svd::HSEM.offset_of("C1ISR");
const C1MISR: u32 = svd::HSEM.offset_of("C1MISR");
const CR: u32 = svd::HSEM.offset_of("CR");
const KEYR: u32 = svd::HSEM.offset_of("KEYR");

const LOCK: u32 = 1 << 31;
pub const COREID_CPU1: u32 = 0x4 << 8;
const ID_MASK: u32 = 0xFFF;

#[derive(Clone, Debug)]
pub struct Hsem {
    pub regs: RegFile,
    /// Semaphore state: 0 = free, otherwise LOCK|COREID|PROCID.
    sem: [u32; 32],
}

impl Default for Hsem {
    fn default() -> Self {
        Self::new()
    }
}

impl Hsem {
    pub fn new() -> Self {
        Hsem {
            regs: RegFile::new(&svd::HSEM),
            sem: [0; 32],
        }
    }

    pub fn reset(&mut self) {
        self.regs.reset();
        self.sem = [0; 32];
    }

    pub fn irq(&self) -> bool {
        self.regs.get(C1ISR) & self.regs.get(C1IER) != 0
    }

    fn free(&mut self, n: usize) {
        self.sem[n] = 0;
        self.regs.set_bits(C1ISR, 1 << n, true);
    }

    pub fn read(&mut self, off: u32) -> Option<u32> {
        match off {
            0x00..=0x7C => Some(self.sem[(off / 4) as usize]),
            o if (RLR0..RLR0 + 0x80).contains(&o) => {
                let n = ((o - RLR0) / 4) as usize;
                if self.sem[n] == 0 {
                    self.sem[n] = LOCK | COREID_CPU1;
                }
                Some(self.sem[n])
            }
            C1MISR => Some(self.regs.get(C1ISR) & self.regs.get(C1IER)),
            C1ISR => Some(self.regs.get(C1ISR)),
            _ => self.regs.read(off),
        }
    }

    pub fn write(&mut self, off: u32, v: u32) -> bool {
        match off {
            0x00..=0x7C => {
                let n = (off / 4) as usize;
                if v & LOCK != 0 {
                    // 2-step lock by CPU1 (COREID in the written value is the bus master's).
                    if self.sem[n] == 0 {
                        self.sem[n] = LOCK | COREID_CPU1 | (v & 0xFF);
                    }
                } else if self.sem[n] & ID_MASK == (COREID_CPU1 | (v & 0xFF)) {
                    self.free(n);
                }
                true
            }
            o if (RLR0..RLR0 + 0x80).contains(&o) => true, // RLR is read-only.
            C1ICR => {
                let isr = self.regs.get(C1ISR);
                self.regs.set(C1ISR, isr & !v);
                true
            }
            CR => {
                // Clear all semaphores of COREID when KEY matches KEYR.
                if v >> 16 == self.regs.get(KEYR) >> 16 {
                    for n in 0..32 {
                        if self.sem[n] != 0 && self.sem[n] & 0xF00 == v & 0xF00 {
                            self.free(n);
                        }
                    }
                }
                true
            }
            _ => self.regs.write(off, v),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_step_lock_as_furi_hal_bt_init_uses_it() {
        let mut h = Hsem::new();
        // LL_HSEM_1StepLock: success iff RLR reads LOCK | LL_HSEM_COREID.
        assert_eq!(h.read(RLR0 + 4 * 5), Some(LOCK | COREID_CPU1));
        // A second 1-step lock still reads the same owner (already held by CPU1).
        assert_eq!(h.read(RLR0 + 4 * 5), Some(LOCK | COREID_CPU1));
        // Release with matching COREID frees it and flags the free event.
        h.write(C1IER, 1 << 5);
        h.write(4 * 5, COREID_CPU1);
        assert_eq!(h.read(4 * 5), Some(0));
        assert!(h.irq());
    }
}

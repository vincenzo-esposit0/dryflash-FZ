// SPDX-License-Identifier: GPL-3.0-or-later
//! Extended interrupt/event controller, CPU1 side (RM0434 "EXTI"; registers from the SVD).
//!
//! Lines 0–15 are the GPIO lines selected by SYSCFG_EXTICRx; they reach the NVIC through
//! EXTI0..EXTI4, EXTI9_5 and EXTI15_10. Pending bits (PR1/PR2) are write-1-to-clear.
//! Edge detection from GPIO levels is wired with the board inputs (M3).

use super::irq;
use super::regfile::RegFile;
use super::svd;

const SWIER1: u32 = svd::EXTI.offset_of("SWIER1");
const PR1: u32 = svd::EXTI.offset_of("PR1");
const SWIER2: u32 = svd::EXTI.offset_of("SWIER2");
const PR2: u32 = svd::EXTI.offset_of("PR2");
const C1IMR1: u32 = svd::EXTI.offset_of("C1IMR1");
const RTSR1: u32 = svd::EXTI.offset_of("RTSR1");
const FTSR1: u32 = svd::EXTI.offset_of("FTSR1");

#[derive(Clone, Debug)]
pub struct Exti {
    pub regs: RegFile,
}

impl Default for Exti {
    fn default() -> Self {
        Self::new()
    }
}

impl Exti {
    pub fn new() -> Self {
        Exti {
            regs: RegFile::new(&svd::EXTI),
        }
    }

    pub fn reset(&mut self) {
        self.regs.reset();
    }

    pub fn write(&mut self, off: u32, v: u32) -> bool {
        match off {
            PR1 | PR2 => {
                let p = self.regs.get(off);
                self.regs.set(off, p & !v);
                true
            }
            SWIER1 => {
                // A software trigger sets the pending bit of enabled lines.
                let pend = v & self.regs.get(C1IMR1);
                self.regs.set_bits(PR1, pend, true);
                true
            }
            SWIER2 => {
                self.regs.set_bits(PR2, v, true);
                true
            }
            _ => self.regs.write(off, v),
        }
    }

    /// Edge on configurable line `line` (0–15 from GPIO): latches PR1 if the edge is
    /// enabled in RTSR1/FTSR1 and the line is unmasked for CPU1.
    pub fn edge(&mut self, line: u32, rising: bool) {
        let trig = self.regs.get(if rising { RTSR1 } else { FTSR1 });
        if trig & (1 << line) != 0 && self.regs.get(C1IMR1) & (1 << line) != 0 {
            self.regs.set_bits(PR1, 1 << line, true);
        }
    }

    /// NVIC lines asserted by pending, unmasked EXTI lines 0–15.
    pub fn irq_lines(&self) -> u64 {
        let p = self.regs.get(PR1) & self.regs.get(C1IMR1) & 0xFFFF;
        let mut out = 0u64;
        for line in 0..16 {
            if p & (1 << line) != 0 {
                let n = match line {
                    0 => irq::EXTI0,
                    1 => irq::EXTI1,
                    2 => irq::EXTI2,
                    3 => irq::EXTI3,
                    4 => irq::EXTI4,
                    5..=9 => irq::EXTI9_5,
                    _ => irq::EXTI15_10,
                };
                out |= 1 << n;
            }
        }
        out
    }
}

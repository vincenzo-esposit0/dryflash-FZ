// SPDX-License-Identifier: GPL-3.0-or-later
//! GPIO port (RM0434 "General-purpose I/Os"; registers from the SVD).
//!
//! IDR is computed: output pins read back ODR; input pins read the level driven by a board
//! device if any, otherwise their pull-up/pull-down, otherwise 0 (floating). Board devices
//! (buttons, chip selects, …) drive pins through [`Gpio::drive`] from M3 on.

use super::regfile::{PeriphDesc, RegFile};
use super::svd;

const MODER: u32 = svd::GPIOA.offset_of("MODER");
const PUPDR: u32 = svd::GPIOA.offset_of("PUPDR");
const IDR: u32 = svd::GPIOA.offset_of("IDR");
const ODR: u32 = svd::GPIOA.offset_of("ODR");
const BSRR: u32 = svd::GPIOA.offset_of("BSRR");
const BRR: u32 = svd::GPIOA.offset_of("BRR");

#[derive(Clone, Debug)]
pub struct Gpio {
    pub regs: RegFile,
    /// Pins driven by a board device, and their levels.
    pub ext_driven: u16,
    pub ext_level: u16,
}

impl Gpio {
    pub fn new(desc: &'static PeriphDesc) -> Self {
        Gpio {
            regs: RegFile::new(desc),
            ext_driven: 0,
            ext_level: 0,
        }
    }

    pub fn reset(&mut self) {
        self.regs.reset();
    }

    /// Pins configured as general-purpose outputs (MODER = 01) or alternate function
    /// (10, driven by the peripheral; read back as ODR for lack of a better source).
    fn output_mask(&self) -> u16 {
        let moder = self.regs.get(MODER);
        (0..16)
            .filter(|p| matches!((moder >> (2 * p)) & 3, 1 | 2))
            .fold(0, |m, p| m | 1 << p)
    }

    /// Current pin levels as seen on IDR.
    pub fn levels(&self) -> u16 {
        let out = self.output_mask();
        let pupdr = self.regs.get(PUPDR);
        let mut v = (self.regs.get(ODR) as u16) & out;
        for p in 0..16 {
            if out & (1 << p) != 0 {
                continue;
            }
            let level = if self.ext_driven & (1 << p) != 0 {
                self.ext_level & (1 << p) != 0
            } else {
                (pupdr >> (2 * p)) & 3 == 1
            };
            if level {
                v |= 1 << p;
            }
        }
        v
    }

    /// The pin is an output driven low. A chip select left in analog mode floats high on the
    /// board (pull-ups), while `levels` would read it as 0.
    pub fn driven_low(&self, pin: u8) -> bool {
        self.output_mask() & (1 << pin) != 0 && self.regs.get(ODR) & (1 << pin) == 0
    }

    /// A board device drives (or releases, with `None`) pin `pin`.
    pub fn drive(&mut self, pin: u8, level: Option<bool>) {
        let m = 1u16 << pin;
        match level {
            Some(l) => {
                self.ext_driven |= m;
                if l { self.ext_level |= m } else { self.ext_level &= !m }
            }
            None => self.ext_driven &= !m,
        }
    }

    pub fn read(&mut self, off: u32) -> Option<u32> {
        if off == IDR {
            return Some(self.levels() as u32);
        }
        self.regs.read(off)
    }

    pub fn write(&mut self, off: u32, v: u32) -> bool {
        match off {
            BSRR => {
                // Set bits win over reset bits (BS has priority, RM0434 GPIOx_BSRR).
                let odr = self.regs.get(ODR);
                self.regs.set(ODR, ((odr & !(v >> 16)) | (v & 0xFFFF)) & 0xFFFF);
                true
            }
            BRR => {
                let odr = self.regs.get(ODR);
                self.regs.set(ODR, odr & !(v & 0xFFFF));
                true
            }
            _ => self.regs.write(off, v),
        }
    }
}

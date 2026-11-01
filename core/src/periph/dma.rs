// SPDX-License-Identifier: GPL-3.0-or-later
//! DMA1/DMA2 register model (RM0434 "DMA"; offsets from the SVD). Interrupt and flag behaviour
//! live here; the transfers are run by `bus::dma_service`, which can reach memory. Transfers are
//! instantaneous, like the SPI bus they serve.

use super::regfile::{PeriphDesc, RegFile};
use super::svd;

const ISR: u32 = svd::DMA1.offset_of("ISR");
const IFCR: u32 = svd::DMA1.offset_of("IFCR");
const CCR1: u32 = svd::DMA1.offset_of("CCR1");
/// Channel register stride.
const STRIDE: u32 = 20;

pub const CCR_EN: u32 = 1 << 0;
pub const CCR_TCIE: u32 = 1 << 1;
pub const CCR_DIR: u32 = 1 << 4;
pub const CCR_MINC: u32 = 1 << 7;

pub const CHANNELS: usize = 7;

#[derive(Clone, Debug)]
pub struct Dma {
    pub regs: RegFile,
}

impl Dma {
    pub fn new(desc: &'static PeriphDesc) -> Self {
        Dma {
            regs: RegFile::new(desc),
        }
    }

    pub fn reset(&mut self) {
        self.regs.reset();
    }

    pub fn write(&mut self, off: u32, v: u32) -> bool {
        if off == IFCR {
            // Clearing the global flag of a channel clears all its flags.
            let mut clear = v;
            for ch in 0..CHANNELS {
                if v & (1 << (4 * ch)) != 0 {
                    clear |= 0xF << (4 * ch);
                }
            }
            let isr = self.regs.get(ISR);
            self.regs.set(ISR, isr & !clear);
            return true;
        }
        self.regs.write(off, v)
    }

    pub fn ccr(&self, ch: usize) -> u32 {
        self.regs.get(CCR1 + STRIDE * ch as u32)
    }

    pub fn cndtr(&self, ch: usize) -> u32 {
        self.regs.get(CCR1 + STRIDE * ch as u32 + 4) & 0xFFFF
    }

    pub fn cmar(&self, ch: usize) -> u32 {
        self.regs.get(CCR1 + STRIDE * ch as u32 + 12)
    }

    /// The channel finished: CNDTR = 0, TCIF and GIF set (half transfer passed too).
    pub fn complete(&mut self, ch: usize) {
        self.regs.set(CCR1 + STRIDE * ch as u32 + 4, 0);
        let isr = self.regs.get(ISR);
        self.regs.set(ISR, isr | (0x7 << (4 * ch)));
    }

    /// Bit mask of channels asserting their interrupt (only TCIE is raised by the model).
    pub fn irq_channels(&self) -> u32 {
        let isr = self.regs.get(ISR);
        (0..CHANNELS)
            .filter(|&ch| self.ccr(ch) & CCR_TCIE != 0 && isr & (2 << (4 * ch)) != 0)
            .fold(0, |m, ch| m | (1 << ch))
    }
}

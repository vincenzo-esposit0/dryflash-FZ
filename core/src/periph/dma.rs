// SPDX-License-Identifier: GPL-3.0-or-later
//! DMA1/DMA2 register model (RM0434 "DMA"; offsets from the SVD). Interrupt and flag behaviour
//! live here; the transfers are run by `bus::dma_service`, which can reach memory. SPI transfers are
//! instantaneous, like the SPI bus they serve; timer requests move one item per request.

use super::regfile::{PeriphDesc, RegFile};
use super::svd;

const ISR: u32 = svd::DMA1.offset_of("ISR");
const IFCR: u32 = svd::DMA1.offset_of("IFCR");
const CCR1: u32 = svd::DMA1.offset_of("CCR1");
/// Channel register stride.
const STRIDE: u32 = 20;

pub const CCR_EN: u32 = 1 << 0;
pub const CCR_TCIE: u32 = 1 << 1;
pub const CCR_HTIE: u32 = 1 << 2;
pub const CCR_TEIE: u32 = 1 << 3;
pub const CCR_DIR: u32 = 1 << 4;
pub const CCR_MINC: u32 = 1 << 7;

pub const CHANNELS: usize = 7;

#[derive(Clone, Debug)]
pub struct Dma {
    pub regs: RegFile,
    /// CNDTR as last written per channel: the size of the transfer, for the half-transfer flag
    /// and the memory address the next item comes from.
    programmed: [u32; CHANNELS],
}

impl Dma {
    pub fn new(desc: &'static PeriphDesc) -> Self {
        Dma {
            regs: RegFile::new(desc),
            programmed: [0; CHANNELS],
        }
    }

    pub fn reset(&mut self) {
        self.regs.reset();
        self.programmed = [0; CHANNELS];
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
        let ch = (off.wrapping_sub(CCR1) / STRIDE) as usize;
        let h = self.regs.write(off, v);
        if ch < CHANNELS && off == CCR1 + STRIDE * ch as u32 + 4 {
            self.programmed[ch] = self.cndtr(ch);
        }
        h
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

    pub fn cpar(&self, ch: usize) -> u32 {
        self.regs.get(CCR1 + STRIDE * ch as u32 + 8)
    }

    /// Items already moved since CNDTR was written.
    pub fn transferred(&self, ch: usize) -> u32 {
        self.programmed[ch].saturating_sub(self.cndtr(ch))
    }

    /// One item moved: CNDTR counts down; HTIF at half the transfer, TCIF at the end (RM0434 DMA
    /// "Channel data transfers": half transfer when half the data has been transferred).
    pub fn item_done(&mut self, ch: usize) {
        let left = self.cndtr(ch).saturating_sub(1);
        let at = CCR1 + STRIDE * ch as u32 + 4;
        let old = self.regs.get(at);
        self.regs.set(at, (old & !0xFFFF) | left);
        let done = self.programmed[ch] - left;
        let mut flags = 0;
        if done == self.programmed[ch] / 2 {
            flags |= 0b0101; // HTIF, GIF
        }
        if left == 0 {
            flags |= 0b0011; // TCIF, GIF
        }
        let isr = self.regs.get(ISR);
        self.regs.set(ISR, isr | (flags << (4 * ch)));
    }

    /// The channel finished: CNDTR = 0, TCIF and GIF set (half transfer passed too).
    pub fn complete(&mut self, ch: usize) {
        self.regs.set(CCR1 + STRIDE * ch as u32 + 4, 0);
        let isr = self.regs.get(ISR);
        self.regs.set(ISR, isr | (0x7 << (4 * ch)));
    }

    /// Bit mask of channels asserting their interrupt: a flag (TCIF, HTIF, TEIF) with its enable
    /// (TCIE, HTIE, TEIE) at the same bit position. The model never raises TEIF.
    pub fn irq_channels(&self) -> u32 {
        let isr = self.regs.get(ISR);
        let enables = CCR_TCIE | CCR_HTIE | CCR_TEIE;
        (0..CHANNELS)
            .filter(|&ch| (isr >> (4 * ch)) & self.ccr(ch) & enables != 0)
            .fold(0, |m, ch| m | (1 << ch))
    }
}

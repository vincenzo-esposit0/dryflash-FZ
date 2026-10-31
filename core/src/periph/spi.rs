// SPDX-License-Identifier: GPL-3.0-or-later
//! SPI controller, master mode with the 32-bit RX/TX FIFOs (RM0434 "Serial peripheral
//! interface"; registers and bit positions from the SVD).
//!
//! Frames are exchanged instantly: a write to DR sends each frame (8-bit frames; a 16-bit
//! access with DS ≤ 8 bits packs two) to the selected device and queues the reply in the
//! RX FIFO (4 bytes; overflow sets OVR and drops data, as on hardware). With no device the
//! MISO line reads 0xFF. Device selection by chip-select GPIOs is wired in M3.

use super::regfile::{PeriphDesc, RegFile};
use super::svd;
use crate::cpu::Size;
use std::collections::VecDeque;

const CR1: u32 = svd::SPI1.offset_of("CR1");
const SR: u32 = svd::SPI1.offset_of("SR");
const DR: u32 = svd::SPI1.offset_of("DR");

const SR_RXNE: u32 = 1 << 0;
const SR_TXE: u32 = 1 << 1;
const SR_OVR: u32 = 1 << 6;
const CR2: u32 = svd::SPI1.offset_of("CR2");
const RX_FIFO_BYTES: usize = 4;

/// A device on an SPI bus: one byte in, one byte out.
pub trait SpiDevice: std::fmt::Debug {
    fn transfer(&mut self, mosi: u8) -> u8;
}

#[derive(Debug)]
pub struct Spi {
    pub regs: RegFile,
    rx: VecDeque<u8>,
    /// Currently selected device (by chip select), if any.
    pub device: Option<Box<dyn SpiDevice>>,
    /// Set by a DR read so that the next SR read clears OVR (RM0434 clearing sequence).
    ovr_clear_armed: bool,
}

impl Spi {
    pub fn new(desc: &'static PeriphDesc) -> Self {
        Spi {
            regs: RegFile::new(desc),
            rx: VecDeque::new(),
            device: None,
            ovr_clear_armed: false,
        }
    }

    pub fn reset(&mut self) {
        self.regs.reset();
        self.rx.clear();
    }

    fn enabled(&self) -> bool {
        self.regs.get(CR1) & (1 << 6) != 0
    }

    /// RXNE follows FRXTH: 1 byte (FRXTH = 1) or 2 bytes (FRXTH = 0).
    fn sr(&self) -> u32 {
        let threshold = if self.regs.get(CR2) & (1 << 12) != 0 { 1 } else { 2 };
        let level = match self.rx.len() {
            0 => 0,
            1 => 1,
            2 => 2,
            _ => 3,
        };
        let mut v = self.regs.get(SR) & !(SR_RXNE | (3 << 9) | (3 << 11) | (1 << 7));
        v |= SR_TXE | (level << 9);
        if self.rx.len() >= threshold {
            v |= SR_RXNE;
        }
        v
    }

    pub fn read(&mut self, off: u32, size: Size) -> Option<u32> {
        match off {
            SR => {
                let v = self.sr();
                if self.ovr_clear_armed {
                    self.regs.set_bits(SR, SR_OVR, false);
                    self.ovr_clear_armed = false;
                }
                Some(v)
            }
            DR => {
                self.ovr_clear_armed = true;
                let n = if size == Size::Byte { 1 } else { 2 };
                let mut v = 0;
                for i in 0..n {
                    if let Some(b) = self.rx.pop_front() {
                        v |= (b as u32) << (8 * i);
                    }
                }
                Some(v)
            }
            _ => self.regs.read(off),
        }
    }

    pub fn write(&mut self, off: u32, v: u32, size: Size) -> bool {
        if off != DR {
            return self.regs.write(off, v);
        }
        if !self.enabled() {
            return true;
        }
        let frames = if size == Size::Byte { 1 } else { 2 };
        for i in 0..frames {
            let mosi = (v >> (8 * i)) as u8;
            let miso = self.device.as_mut().map(|d| d.transfer(mosi)).unwrap_or(0xFF);
            if self.rx.len() < RX_FIFO_BYTES {
                self.rx.push_back(miso);
            } else {
                self.regs.set_bits(SR, SR_OVR, true);
            }
        }
        true
    }
}

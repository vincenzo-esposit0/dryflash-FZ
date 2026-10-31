// SPDX-License-Identifier: GPL-3.0-or-later
//! I2C controller, master mode (RM0434 "Inter-integrated circuit interface"; registers and
//! bit positions from the SVD).
//!
//! Transfers complete instantly in virtual time (no bus timing). The controller follows
//! the CR2 transfer model the firmware uses (targets/f7/furi_hal/furi_hal_i2c.c): START with
//! SADD/RD_WRN/NBYTES, TXIS/RXNE per byte, then STOPF (AUTOEND), TC (software end) or TCR
//! (RELOAD). An address with no device on the bus is NACKed: NACKF, then an automatic STOP.

use super::regfile::{PeriphDesc, RegFile};
use super::svd;

const CR1: u32 = svd::I2C1.offset_of("CR1");
const CR2: u32 = svd::I2C1.offset_of("CR2");
const ISR: u32 = svd::I2C1.offset_of("ISR");
const ICR: u32 = svd::I2C1.offset_of("ICR");
const RXDR: u32 = svd::I2C1.offset_of("RXDR");
const TXDR: u32 = svd::I2C1.offset_of("TXDR");

const TXE: u32 = 1 << 0;
const TXIS: u32 = 1 << 1;
const RXNE: u32 = 1 << 2;
const NACKF: u32 = 1 << 4;
const STOPF: u32 = 1 << 5;
const TC: u32 = 1 << 6;
const TCR: u32 = 1 << 7;
const BUSY: u32 = 1 << 15;
const DIR: u32 = 1 << 16;

const CR2_RD_WRN: u32 = 1 << 10;
const CR2_ADD10: u32 = 1 << 11;
const CR2_START: u32 = 1 << 13;
const CR2_STOP: u32 = 1 << 14;
const CR2_RELOAD: u32 = 1 << 24;
const CR2_AUTOEND: u32 = 1 << 25;

/// A device on an I2C bus (7-bit addressing).
pub trait I2cDevice: std::fmt::Debug {
    fn address(&self) -> u8;
    /// START (or repeated START) addressed to this device; return false to NACK.
    fn start(&mut self, read: bool) -> bool;
    /// Byte written by the master; return false to NACK it.
    fn write(&mut self, byte: u8) -> bool;
    /// Byte requested by the master.
    fn read(&mut self) -> u8;
    fn stop(&mut self) {}
}

#[derive(Debug)]
pub struct I2c {
    pub regs: RegFile,
    pub devices: Vec<Box<dyn I2cDevice>>,
    /// Device index of the ongoing transfer.
    current: Option<usize>,
    remaining: u32,
    read: bool,
}

impl I2c {
    pub fn new(desc: &'static PeriphDesc) -> Self {
        I2c {
            regs: RegFile::new(desc),
            devices: Vec::new(),
            current: None,
            remaining: 0,
            read: false,
        }
    }

    pub fn reset(&mut self) {
        self.regs.reset();
        self.end_transfer();
    }

    fn end_transfer(&mut self) {
        if let Some(i) = self.current.take() {
            self.devices[i].stop();
        }
        self.remaining = 0;
    }

    fn isr(&mut self, set: u32, clear: u32) {
        let v = self.regs.get(ISR);
        self.regs.set(ISR, (v | set) & !clear);
    }

    pub fn read(&mut self, off: u32) -> Option<u32> {
        if off == RXDR {
            let v = self.regs.get(RXDR);
            self.isr(0, RXNE);
            if self.remaining > 0 {
                self.remaining -= 1;
                if self.remaining > 0 {
                    self.fetch();
                } else {
                    self.finish_chunk();
                }
            }
            return Some(v & 0xFF);
        }
        self.regs.read(off)
    }

    pub fn write(&mut self, off: u32, v: u32) -> bool {
        match off {
            CR1 => {
                self.regs.write(CR1, v);
                if v & 1 == 0 {
                    // PE = 0: software reset of the communication flags and state.
                    self.isr(0, TXIS | RXNE | NACKF | STOPF | TC | TCR | BUSY);
                    self.isr(TXE, 0);
                    self.end_transfer();
                }
                true
            }
            ICR => {
                // ADDRCF, NACKCF, STOPCF, BERRCF, ARLOCF, OVRCF, PECCF, TIMOUTCF, ALERTCF clear
                // the ISR bits at the same positions.
                self.isr(0, v & 0x3F38);
                true
            }
            TXDR => {
                self.regs.set(TXDR, v & 0xFF);
                if self.regs.get(ISR) & TXIS != 0 && !self.read && self.remaining > 0 {
                    self.isr(0, TXIS | TXE);
                    let acked = self.current.map(|i| self.devices[i].write(v as u8)).unwrap_or(false);
                    self.remaining -= 1;
                    self.isr(TXE, 0);
                    if !acked {
                        self.nack();
                    } else if self.remaining > 0 {
                        self.isr(TXIS, 0);
                    } else {
                        self.finish_chunk();
                    }
                }
                true
            }
            CR2 => {
                self.regs.write(CR2, v);
                let cr2 = self.regs.get(CR2);
                if v & CR2_STOP != 0 && self.regs.get(ISR) & (TC | TCR) != 0 {
                    // Software STOP after a TC (SOFTEND).
                    self.stop();
                } else if v & CR2_START != 0 {
                    self.start(cr2);
                } else if self.regs.get(ISR) & TCR != 0 {
                    // Reload: a new NBYTES continues the transfer without START.
                    self.isr(0, TCR);
                    self.remaining = (cr2 >> 16) & 0xFF;
                    self.continue_chunk();
                }
                // START and STOP are cleared by hardware.
                self.regs.set_bits(CR2, CR2_START | CR2_STOP, false);
                true
            }
            _ => self.regs.write(off, v),
        }
    }

    fn start(&mut self, cr2: u32) {
        let read = cr2 & CR2_RD_WRN != 0;
        let addr = if cr2 & CR2_ADD10 != 0 {
            None
        } else {
            Some(((cr2 >> 1) & 0x7F) as u8)
        };
        let dev = addr.and_then(|a| self.devices.iter().position(|d| d.address() == a));
        // A repeated START to another device ends the previous one.
        if self.current != dev {
            self.end_transfer();
        }
        self.read = read;
        self.remaining = (cr2 >> 16) & 0xFF;
        self.isr(
            BUSY | if read { DIR } else { 0 },
            TC | TCR | NACKF | STOPF | if read { 0 } else { DIR },
        );
        match dev {
            Some(i) if self.devices[i].start(read) => {
                self.current = Some(i);
                self.continue_chunk();
            }
            _ => self.nack(),
        }
    }

    fn continue_chunk(&mut self) {
        if self.remaining == 0 {
            self.finish_chunk();
        } else if self.read {
            self.fetch();
        } else {
            self.isr(TXIS | TXE, 0);
        }
    }

    fn fetch(&mut self) {
        let b = self.current.map(|i| self.devices[i].read()).unwrap_or(0xFF);
        self.regs.set(RXDR, b as u32);
        self.isr(RXNE, 0);
    }

    /// NBYTES transferred: RELOAD → TCR, AUTOEND → STOP, otherwise TC (software end).
    fn finish_chunk(&mut self) {
        let cr2 = self.regs.get(CR2);
        if cr2 & CR2_RELOAD != 0 {
            self.isr(TCR, TXIS);
        } else if cr2 & CR2_AUTOEND != 0 {
            self.stop();
        } else {
            self.isr(TC, TXIS);
        }
    }

    fn nack(&mut self) {
        // In master mode a NACK makes the controller send STOP automatically.
        self.isr(NACKF, TXIS | RXNE);
        self.stop();
    }

    fn stop(&mut self) {
        self.end_transfer();
        self.isr(STOPF | TXE, BUSY | TXIS | TC | TCR);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Default)]
    struct Echo {
        last: u8,
    }
    impl I2cDevice for Echo {
        fn address(&self) -> u8 {
            0x30
        }
        fn start(&mut self, _read: bool) -> bool {
            true
        }
        fn write(&mut self, b: u8) -> bool {
            self.last = b;
            true
        }
        fn read(&mut self) -> u8 {
            self.last.wrapping_add(1)
        }
    }

    #[test]
    fn absent_device_is_nacked_with_stop() {
        let mut i2c = I2c::new(&svd::I2C1);
        i2c.write(CR1, 1);
        i2c.write(CR2, (0x55 << 1) | (1 << 16) | CR2_AUTOEND | CR2_START);
        let isr = i2c.read(ISR).unwrap();
        assert_eq!(isr & (NACKF | STOPF | TC | BUSY), NACKF | STOPF);
    }

    #[test]
    fn write_then_read_with_autoend() {
        let mut i2c = I2c::new(&svd::I2C1);
        i2c.devices.push(Box::new(Echo::default()));
        i2c.write(CR1, 1);
        i2c.write(CR2, (0x30 << 1) | (1 << 16) | CR2_START);
        assert_ne!(i2c.read(ISR).unwrap() & TXIS, 0);
        i2c.write(TXDR, 0x41);
        assert_ne!(i2c.read(ISR).unwrap() & TC, 0, "software end: TC");
        i2c.write(CR2, (0x30 << 1) | (1 << 16) | CR2_RD_WRN | CR2_AUTOEND | CR2_START);
        assert_ne!(i2c.read(ISR).unwrap() & RXNE, 0);
        assert_eq!(i2c.read(RXDR).unwrap(), 0x42);
        assert_eq!(i2c.read(ISR).unwrap() & (STOPF | BUSY), STOPF);
    }
}

// SPDX-License-Identifier: GPL-3.0-or-later
//! TI BQ25896 charger on I2C1 (7-bit address 0x6B; `BQ25896_ADDRESS` 0xD6 in
//! lib/drivers/bq25896_reg.h, firmware 1.5.1).
//!
//! A register file REG00–REG14 with auto-increment, as lib/drivers/bq25896.c reads and writes
//! it. REG_RST (REG14 bit 7) clears the registers. Reset values are not taken from the
//! datasheet (not available here): registers start at zero except the charge/VBUS status in
//! REG0B, derived from [`super::Battery`]. Charging physics is not simulated.

use super::Battery;
use crate::periph::i2c::I2cDevice;

const REGS: usize = 0x15;

#[derive(Debug)]
pub struct Bq25896 {
    pub battery: Battery,
    regs: [u8; REGS],
    ptr: u8,
    expect_ptr: bool,
}

impl Default for Bq25896 {
    fn default() -> Self {
        Self::new()
    }
}

impl Bq25896 {
    pub fn new() -> Self {
        Bq25896 {
            battery: Battery::default(),
            regs: [0; REGS],
            ptr: 0,
            expect_ptr: false,
        }
    }

    fn status(&self) -> u8 {
        // REG0B: VBUS_STAT[7:5] (0b001 USB host when connected), CHRG_STAT[4:3]
        // (0b10 fast charging), PG_STAT[2] (power good).
        let b = &self.battery;
        let mut v = 0;
        if b.usb_connected {
            v |= 0b001 << 5 | 1 << 2;
        }
        if b.charging {
            v |= 0b10 << 3;
        }
        v
    }
}

impl I2cDevice for Bq25896 {
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn address(&self) -> u8 {
        0x6B
    }

    fn start(&mut self, read: bool) -> bool {
        self.expect_ptr = !read;
        true
    }

    fn write(&mut self, byte: u8) -> bool {
        if self.expect_ptr {
            self.ptr = byte;
            self.expect_ptr = false;
            return true;
        }
        if (self.ptr as usize) < REGS {
            if self.ptr == 0x14 && byte & 0x80 != 0 {
                self.regs = [0; REGS]; // REG_RST: register reset, bit self-clears.
            } else {
                self.regs[self.ptr as usize] = byte;
            }
        }
        self.ptr = self.ptr.wrapping_add(1);
        true
    }

    fn read(&mut self) -> u8 {
        let v = match self.ptr {
            0x0B => self.status(),
            p if (p as usize) < REGS => self.regs[p as usize],
            _ => 0,
        };
        self.ptr = self.ptr.wrapping_add(1);
        v
    }
}

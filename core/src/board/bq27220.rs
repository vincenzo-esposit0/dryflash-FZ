// SPDX-License-Identifier: GPL-3.0-or-later
//! TI BQ27220 fuel gauge on I2C1 (7-bit address 0x55; `BQ27220_ADDRESS` 0xAA in
//! lib/drivers/bq27220_reg.h, firmware 1.5.1).
//!
//! Modelled as the firmware's driver (lib/drivers/bq27220.c) uses it: standard word commands,
//! CONTROL subcommands (also accepted through 0x3E for values below 0x4000), the MAC/data
//! memory interface (0x3E select + 0x40 data + 0x60 checksum/length), security states
//! (sealed → unsealed with UnsealKey1/2, → full access with FullAccessKey twice) and CFG
//! UPDATE mode. The data memory starts empty like an unprovisioned chip, so the firmware
//! runs its own provisioning on first boot. Gauging itself is not simulated: readings come
//! from [`super::Battery`].

use super::Battery;
use crate::periph::i2c::I2cDevice;
use std::collections::BTreeMap;

const OP_SEC_SEALED: u8 = 0b11;
const OP_SEC_UNSEALED: u8 = 0b10;
const OP_SEC_FULL: u8 = 0b01;

#[derive(Debug)]
pub struct Bq27220 {
    pub battery: Battery,
    ptr: u8,
    /// Bytes written in the current transaction after the register pointer.
    written: Vec<u8>,
    writing: bool,
    /// Next written byte is the register pointer (first byte of a write transaction).
    expect_ptr: bool,
    sec: u8,
    initcomp: bool,
    cfgupdate: bool,
    /// Previous CONTROL value, for the two-word key sequences.
    last_control: u16,
    /// MAC data buffer (0x40..0x5F).
    mac: [u8; 32],
    /// Pending data-memory write selected through 0x3E: address and data.
    pending: Option<(u16, Vec<u8>)>,
    /// Data memory, by address (big-endian parameter bytes, as the driver writes them).
    pub dm: BTreeMap<u16, u8>,
}

impl Default for Bq27220 {
    fn default() -> Self {
        Self::new()
    }
}

impl Bq27220 {
    pub fn new() -> Self {
        Bq27220 {
            battery: Battery::default(),
            ptr: 0,
            written: Vec::new(),
            writing: false,
            expect_ptr: false,
            sec: OP_SEC_SEALED,
            initcomp: true,
            cfgupdate: false,
            last_control: 0,
            mac: [0; 32],
            pending: None,
            dm: BTreeMap::new(),
        }
    }

    fn control(&mut self, cmd: u16) {
        match cmd {
            0x0001 => self.set_mac(&0x0220u16.to_le_bytes()), // DEVICE_NUMBER
            0x0002 => self.set_mac(&0x0103u16.to_le_bytes()), // FW_VERSION (value not from a datasheet)
            0x0030 => self.sec = OP_SEC_SEALED,               // SEALED
            0x0041 => {
                // RESET: re-initialises immediately.
                self.cfgupdate = false;
                self.initcomp = true;
            }
            0x0056 => self.set_mac(&[0x00, 0x00]), // GAUGING_STATUS
            0x0090 => self.cfgupdate = true,       // ENTER_CFG_UPDATE
            0x0091 | 0x0092 => self.cfgupdate = false,
            0x3672 if self.last_control == 0x0414 && self.sec == OP_SEC_SEALED => self.sec = OP_SEC_UNSEALED,
            0xFFFF if self.last_control == 0xFFFF && self.sec == OP_SEC_UNSEALED => self.sec = OP_SEC_FULL,
            _ => {}
        }
        self.last_control = cmd;
    }

    fn set_mac(&mut self, data: &[u8]) {
        self.mac = [0; 32];
        self.mac[..data.len()].copy_from_slice(data);
    }

    fn word(&self, cmd: u8) -> u16 {
        let b = &self.battery;
        let fcc: u16 = 2100;
        match cmd {
            0x00 => 0,                                          // CONTROL_STATUS: BATT_ID 0
            0x06 => ((b.temperature_c + 273.15) * 10.0) as u16, // 0.1 K
            0x08 => b.voltage_mv,
            // BatteryStatus (lib/drivers/bq27220.h): DSG, SYSDWN, BATTPRES. The real gauge raises
            // SYSDWN below a data-memory voltage the firmware does not set; here it is 0 %.
            0x0A => (1 << 3) | (!b.charging as u16) | ((b.state_of_charge == 0) as u16) << 1,
            0x0C | 0x14 => b.current_ma as u16,
            0x10 => (fcc as u32 * b.state_of_charge as u32 / 100) as u16,
            0x12 | 0x3C => fcc,
            0x16 | 0x18 => 0xFFFF, // time to empty/full: n/a
            0x28 => ((b.temperature_c + 273.15) * 10.0) as u16,
            0x2C => b.state_of_charge as u16,
            0x2E => 100,
            0x3A => {
                let lo = (self.sec as u16) << 1 | (self.initcomp as u16) << 5;
                lo | (self.cfgupdate as u16) << 10
            }
            _ => 0,
        }
    }

    fn reg_byte(&self, addr: u8) -> u8 {
        if (0x40..0x60).contains(&addr) {
            return self.mac[(addr - 0x40) as usize];
        }
        let w = self.word(addr & !1);
        if addr & 1 == 0 { w as u8 } else { (w >> 8) as u8 }
    }

    fn end_write(&mut self) {
        let data = std::mem::take(&mut self.written);
        match self.ptr {
            0x00 if data.len() >= 2 => self.control(u16::from_le_bytes([data[0], data[1]])),
            0x3E if data.len() >= 2 => {
                let addr = u16::from_le_bytes([data[0], data[1]]);
                if addr < 0x4000 {
                    self.control(addr);
                } else if data.len() == 2 {
                    // Select a data-memory address for reading through 0x40.
                    let bytes: Vec<u8> = (0..32).map(|i| *self.dm.get(&(addr + i)).unwrap_or(&0)).collect();
                    self.set_mac(&bytes);
                    self.pending = None;
                } else {
                    self.pending = Some((addr, data[2..].to_vec()));
                }
            }
            0x60 if data.len() >= 2 => {
                // Commit a pending data-memory write when checksum and length match.
                if let Some((addr, bytes)) = self.pending.take() {
                    let sum = addr
                        .to_le_bytes()
                        .iter()
                        .chain(bytes.iter())
                        .fold(0u8, |a, b| a.wrapping_add(*b));
                    let ok = data[0] == 0xFF - sum && data[1] as usize == bytes.len() + 4;
                    if ok && self.sec != OP_SEC_SEALED {
                        for (i, b) in bytes.iter().enumerate() {
                            self.dm.insert(addr + i as u16, *b);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

impl I2cDevice for Bq27220 {
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn address(&self) -> u8 {
        0x55
    }

    fn start(&mut self, read: bool) -> bool {
        if self.writing {
            self.end_write();
        }
        self.writing = !read;
        self.expect_ptr = !read;
        self.written.clear();
        true
    }

    fn write(&mut self, byte: u8) -> bool {
        if self.expect_ptr {
            self.ptr = byte;
            self.expect_ptr = false;
        } else {
            self.written.push(byte);
        }
        true
    }

    fn read(&mut self) -> u8 {
        let b = self.reg_byte(self.ptr);
        self.ptr = self.ptr.wrapping_add(1);
        b
    }

    fn stop(&mut self) {
        if self.writing {
            self.end_write();
        }
        self.writing = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tx(g: &mut Bq27220, bytes: &[u8]) {
        g.start(false);
        for b in bytes {
            g.write(*b);
        }
        g.stop();
    }

    fn rx(g: &mut Bq27220, reg: u8, n: usize) -> Vec<u8> {
        tx(g, &[reg]);
        g.start(true);
        let v = (0..n).map(|_| g.read()).collect();
        g.stop();
        v
    }

    #[test]
    fn device_number_unseal_and_dm_write_as_the_driver_does() {
        let mut g = Bq27220::new();
        tx(&mut g, &[0x00, 0x01, 0x00]); // Control_DEVICE_NUMBER
        assert_eq!(rx(&mut g, 0x40, 2), vec![0x20, 0x02]);
        assert_eq!(rx(&mut g, 0x3A, 2)[0] >> 1 & 3, OP_SEC_SEALED);
        tx(&mut g, &[0x00, 0x14, 0x04]); // UnsealKey1
        tx(&mut g, &[0x00, 0x72, 0x36]); // UnsealKey2
        assert_eq!(rx(&mut g, 0x3A, 2)[0] >> 1 & 3, OP_SEC_UNSEALED);
        // DM write: 0x3E <addr lo, hi, data…>, then 0x60 <checksum, length>.
        let (addr, data) = (0x929Bu16, [0x12u8, 0x34]);
        tx(&mut g, &[0x3E, addr as u8, (addr >> 8) as u8, data[0], data[1]]);
        let sum = [addr as u8, (addr >> 8) as u8, data[0], data[1]]
            .iter()
            .fold(0u8, |a, b| a.wrapping_add(*b));
        tx(&mut g, &[0x60, 0xFF - sum, 6]);
        tx(&mut g, &[0x3E, addr as u8, (addr >> 8) as u8]);
        assert_eq!(rx(&mut g, 0x40, 2), data.to_vec());
    }
}

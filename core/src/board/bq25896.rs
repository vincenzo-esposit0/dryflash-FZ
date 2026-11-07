// SPDX-License-Identifier: GPL-3.0-or-later
//! TI BQ25896 charger on I2C1 (7-bit address 0x6B; `BQ25896_ADDRESS` 0xD6 in
//! lib/drivers/bq25896_reg.h, firmware 1.5.1).
//!
//! Register file REG00–REG14 with auto-increment, as lib/drivers/bq25896.c reads and writes it.
//! Reset values, the status register, the ADC registers and ship mode follow TI's datasheet
//! SLUSC76C (bq25896, rev. May 2018), §9.4 "Register Maps" and §9.2.10 "BATFET Control".
//!
//! What is simulated: the status the firmware reads (VBUS, charging, done), ADC readings derived
//! from [`super::Battery`], CHG_CONFIG (charging off while the firmware suppresses it, as Sub-GHz
//! does) and BATFET_DIS (ship mode: with no input source the system loses power). Not simulated:
//! charging physics, the I2C watchdog, faults, OTG boost output, input current optimisation.

use super::Battery;
use crate::periph::i2c::I2cDevice;

const REGS: usize = 0x15;

/// Values after reset, REG00–REG14 (SLUSC76C §9.4.1–9.4.21, the bit rows of each register figure).
/// REG0B and REG0C read live status; REG0E–REG12 are ADC results computed on read.
const RESET: [u8; REGS] = [
    0x08, 0x06, 0x11, 0x1A, 0x20, 0x13, 0x5E, 0x9D, 0x03, 0x44, 0x73, 0x00, 0x00, 0x12, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x02,
];

const REG03_CHG_CONFIG: u8 = 1 << 4;
const REG09_BATFET_DIS: u8 = 1 << 5;
const REG14_REG_RST: u8 = 1 << 7;

/// The voltage the page's USB supply gives (a USB host port, 5 V nominal).
const VBUS_MV: u16 = 5000;

/// REG0B CHRG_STAT (lib/drivers/bq25896_reg.h `ChrgStat`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChargeState {
    No = 0b00,
    Fast = 0b10,
    Done = 0b11,
}

#[derive(Debug)]
pub struct Bq25896 {
    /// What the page sets: charge, voltage, temperature, USB plugged in. `charging` and
    /// `current_ma` are outputs, set by `Periphs::sync_power` from [`Self::charge_state`].
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
            regs: RESET,
            ptr: 0,
            expect_ptr: false,
        }
    }

    /// Charging status from the input source, CHG_CONFIG and the charge level. Termination is
    /// taken as reached at 100 %.
    pub fn charge_state(&self) -> ChargeState {
        if !self.battery.usb_connected || self.regs[0x03] & REG03_CHG_CONFIG == 0 {
            ChargeState::No
        } else if self.battery.state_of_charge >= 100 {
            ChargeState::Done
        } else {
            ChargeState::Fast
        }
    }

    /// BATFET forced off by the host (ship mode, §9.2.10.1).
    pub fn ship_mode(&self) -> bool {
        self.regs[0x09] & REG09_BATFET_DIS != 0
    }

    /// The battery is disconnected from the system and nothing else supplies it.
    pub fn power_cut(&self) -> bool {
        self.ship_mode() && !self.battery.usb_connected
    }

    /// Leaves ship mode, as plugging in an adapter or the QON button does (§9.2.10.2).
    pub fn exit_ship_mode(&mut self) {
        self.regs[0x09] &= !REG09_BATFET_DIS;
    }

    /// REG0B: VBUS_STAT[7:5], CHRG_STAT[4:3], PG_STAT[2], bit 1 reserved and always 1 (§9.4.12).
    fn status(&self) -> u8 {
        let mut v = 1 << 1;
        if self.battery.usb_connected {
            v |= 0b001 << 5 | 1 << 2; // USB host (SDP), power good.
        }
        v | (self.charge_state() as u8) << 3
    }

    fn adc(&self, reg: u8) -> u8 {
        let b = &self.battery;
        // Steps and offsets from §9.4.15–9.4.19, the inverse of the formulas in lib/drivers/bq25896.c.
        let step = |v: i32, offset: i32, lsb: i32| ((v - offset) / lsb).clamp(0, 127) as u8;
        let vbat = b.voltage_mv as i32;
        match reg {
            0x0E => step(vbat, 2304, 20),
            // VSYS follows the battery when it supplies the system; with an input source the
            // converter keeps it at least at SYS_MIN (3.5 V after reset), approximated here.
            0x0F => step(if b.usb_connected { vbat.max(3500) } else { vbat }, 2304, 20),
            // TS as a percentage of REGN for the thermistor network the firmware assumes:
            // furi_hal_power_get_battery_temperature computes T = (71 − TS%) / 0.6.
            0x10 => step(((71.0 - 0.6 * b.temperature_c) * 1000.0) as i32, 21000, 465),
            0x11 if b.usb_connected => 0x80 | step(VBUS_MV as i32, 2600, 100),
            0x11 => 0,
            0x12 if self.charge_state() == ChargeState::Fast => step(b.current_ma.max(0) as i32, 0, 50),
            0x12 => 0,
            _ => 0,
        }
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
        match self.ptr {
            // REG_RST: every register back to its reset value, BATFET_DIS included; the bit self-clears.
            0x14 if byte & REG14_REG_RST != 0 => self.regs = RESET,
            0x0B..=0x0C | 0x0E..=0x14 => {} // read only
            p if (p as usize) < REGS => self.regs[p as usize] = byte,
            _ => {}
        }
        self.ptr = self.ptr.wrapping_add(1);
        true
    }

    fn read(&mut self) -> u8 {
        let v = match self.ptr {
            0x0B => self.status(),
            0x0C => 0, // no faults
            p @ 0x0E..=0x12 => self.adc(p),
            p if (p as usize) < REGS => self.regs[p as usize],
            _ => 0xFF, // "Register read beyond REG14 (0x14) returns 0xFF" (§9.2.16)
        };
        self.ptr = self.ptr.wrapping_add(1);
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(c: &mut Bq25896, reg: u8) -> u8 {
        c.start(false);
        c.write(reg);
        c.start(true);
        c.read()
    }

    fn write(c: &mut Bq25896, reg: u8, v: u8) {
        c.start(false);
        c.write(reg);
        c.write(v);
    }

    /// The firmware's conversions (lib/drivers/bq25896.c).
    fn vbus_mv(r: u8) -> u32 {
        if r & 0x80 != 0 {
            (r & 0x7F) as u32 * 100 + 2600
        } else {
            0
        }
    }

    #[test]
    fn readings_as_the_firmware_decodes_them() {
        let mut c = Bq25896::new();
        c.battery.voltage_mv = 3950;
        c.battery.temperature_c = 25.0;
        assert_eq!(vbus_mv(read(&mut c, 0x11)), 0);
        assert_eq!(read(&mut c, 0x0E) as u32 * 20 + 2304, 3944); // truncated to a 20 mV step
        let ts = read(&mut c, 0x10) as u32 * 465 + 21000;
        let t = (71.0 - ts as f32 / 1000.0) / 0.6;
        assert!((t - 25.0).abs() < 1.0, "{t}");
        c.battery.usb_connected = true;
        c.battery.current_ma = 500;
        assert_eq!(vbus_mv(read(&mut c, 0x11)), 5000);
        assert_eq!(read(&mut c, 0x12) as u32 * 50, 500);
        assert_eq!(read(&mut c, 0x15), 0xFF);
    }

    #[test]
    fn charging_follows_usb_chg_config_and_charge() {
        let mut c = Bq25896::new();
        assert_eq!(read(&mut c, 0x0B), 0b0000_0010);
        c.battery.usb_connected = true;
        assert_eq!(c.charge_state(), ChargeState::Fast);
        assert_eq!(read(&mut c, 0x0B), 0b0011_0110);
        write(&mut c, 0x03, RESET[3] & !REG03_CHG_CONFIG); // bq25896_disable_charging
        assert_eq!(c.charge_state(), ChargeState::No);
        write(&mut c, 0x03, RESET[3]);
        c.battery.state_of_charge = 100;
        assert_eq!(c.charge_state(), ChargeState::Done);
    }

    #[test]
    fn ship_mode_cuts_power_without_usb() {
        let mut c = Bq25896::new();
        write(&mut c, 0x09, RESET[9] | REG09_BATFET_DIS); // bq25896_poweroff
        assert!(c.power_cut());
        c.battery.usb_connected = true;
        assert!(c.ship_mode() && !c.power_cut());
        write(&mut c, 0x14, REG14_REG_RST);
        assert!(!c.ship_mode());
        assert_eq!(read(&mut c, 0x14), 0x02);
    }
}

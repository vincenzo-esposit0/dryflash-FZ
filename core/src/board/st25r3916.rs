// SPDX-License-Identifier: GPL-3.0-or-later
//! ST ST25R3916 NFC front end on SPI1 (CS = PE4; targets/f7/furi_hal/furi_hal_resources.h, firmware
//! 1.5.1). A stub, as BRIEF §5.2 asks: enough for init and an empty field. Framing and registers are those
//! of lib/drivers/st25r3916_reg.c and st25r3916_reg.h. It never raises its interrupt line, so the firmware's
//! waits run out; there is no field, no tag and nothing to read from the FIFO.

const FIFO_LOAD: u8 = 0x80;
const FIFO_READ: u8 = 0x9F;
const CMD_SET_DEFAULT: u8 = 0xC1;
const CMD_SPACE_B_ACCESS: u8 = 0xFB;
const CMD_TEST_ACCESS: u8 = 0xFC;
const OP_CONTROL: usize = 0x02;
const OP_CONTROL_EN: u8 = 1 << 7;
const AD_RESULT: usize = 0x25;
const AUX_DISPLAY: usize = 0x31;
const AUX_DISPLAY_OSC_OK: u8 = 1 << 4;
const IC_IDENTITY: usize = 0x3F;
/// `ic_type_st25r3916` (5 << 3), with revision 2 in the low bits (st25r3916_reg.h: "v2 silicon: 0x09").
const IDENTITY: u8 = (5 << 3) | 1;
/// furi_hal_nfc_init takes 23.4 mV per count; this is 3.3 V, the board's supply.
const VDD_COUNTS: u8 = 141;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Frame {
    Command,
    Read(usize),
    Write(usize),
    /// FIFO and passive-target memory: nothing behind it.
    Nothing,
}

#[derive(Clone, Debug)]
pub struct St25r3916 {
    space_a: [u8; 64],
    space_b: [u8; 64],
    frame: Frame,
    /// The next access is to space B.
    use_b: bool,
    /// The next access is to the test registers, which nothing here depends on.
    use_test: bool,
    /// Bytes transferred since power-on, so the machine can tell it was touched.
    pub accesses: u64,
}

impl Default for St25r3916 {
    fn default() -> Self {
        Self::new()
    }
}

impl St25r3916 {
    pub fn new() -> Self {
        St25r3916 {
            space_a: [0; 64],
            space_b: [0; 64],
            frame: Frame::Command,
            use_b: false,
            use_test: false,
            accesses: 0,
        }
    }

    pub fn reset(&mut self) {
        self.space_a = [0; 64];
        self.space_b = [0; 64];
        self.deselect();
    }

    /// CS went high.
    pub fn deselect(&mut self) {
        self.frame = Frame::Command;
        self.use_b = false;
        self.use_test = false;
    }

    fn read(&self, reg: usize) -> u8 {
        if self.use_test {
            return 0;
        }
        if self.use_b {
            return self.space_b[reg];
        }
        match reg {
            IC_IDENTITY => IDENTITY,
            AD_RESULT => VDD_COUNTS,
            AUX_DISPLAY if self.space_a[OP_CONTROL] & OP_CONTROL_EN != 0 => AUX_DISPLAY_OSC_OK,
            r => self.space_a[r],
        }
    }

    fn write(&mut self, reg: usize, v: u8) {
        if self.use_test {
        } else if self.use_b {
            self.space_b[reg] = v;
        } else {
            self.space_a[reg] = v;
        }
    }

    /// One byte on the bus.
    pub fn transfer(&mut self, mosi: u8) -> u8 {
        self.accesses += 1;
        match self.frame {
            Frame::Command => {
                match mosi {
                    CMD_SPACE_B_ACCESS => self.use_b = true,
                    CMD_TEST_ACCESS => self.use_test = true,
                    CMD_SET_DEFAULT => {
                        self.space_a = [0; 64];
                        self.space_b = [0; 64];
                    }
                    0xC0.. => {}
                    FIFO_LOAD..=FIFO_READ | 0xA0..=0xBF => self.frame = Frame::Nothing,
                    0x40..=0x7F => self.frame = Frame::Read((mosi & 0x3F) as usize),
                    _ => self.frame = Frame::Write((mosi & 0x3F) as usize),
                }
                0
            }
            Frame::Read(reg) => {
                self.frame = Frame::Read((reg + 1) % 64);
                self.read(reg)
            }
            Frame::Write(reg) => {
                self.write(reg, mosi);
                self.frame = Frame::Write((reg + 1) % 64);
                0
            }
            Frame::Nothing => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(c: &mut St25r3916, reg: u8) -> u8 {
        c.deselect();
        c.transfer(0x40 | reg);
        let v = c.transfer(0);
        c.deselect();
        v
    }

    #[test]
    fn it_says_it_is_the_chip_the_firmware_checks_for() {
        let mut c = St25r3916::new();
        // The mask from furi_hal_nfc_init.
        assert_eq!(read(&mut c, 0x3F) & (0x1F << 3), 5 << 3);
    }

    #[test]
    fn the_oscillator_is_ok_once_enabled() {
        let mut c = St25r3916::new();
        assert_eq!(read(&mut c, 0x31) & AUX_DISPLAY_OSC_OK, 0);
        c.transfer(OP_CONTROL as u8);
        c.transfer(OP_CONTROL_EN);
        assert_ne!(read(&mut c, 0x31) & AUX_DISPLAY_OSC_OK, 0);
    }

    #[test]
    fn space_b_is_its_own_and_burst_writes_go_on() {
        let mut c = St25r3916::new();
        c.transfer(CMD_SPACE_B_ACCESS);
        c.transfer(0x05);
        c.transfer(0x11);
        c.transfer(0x22);
        c.deselect();
        assert_eq!(read(&mut c, 0x05), 0, "space A");
        c.transfer(CMD_SPACE_B_ACCESS);
        c.transfer(0x40 | 0x05);
        assert_eq!(c.transfer(0), 0x11);
        assert_eq!(c.transfer(0), 0x22);
    }
}

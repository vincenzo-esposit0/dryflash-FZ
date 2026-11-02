// SPDX-License-Identifier: GPL-3.0-or-later
//! TI CC1101 Sub-GHz radio on SPI1 (CS = PD0, GD0 = PA1; targets/f7/furi_hal/furi_hal_resources.h,
//! firmware 1.5.1). A stub, as BRIEF §5.2 asks: a register file and strobes so the drivers initialise,
//! and no signal, ever. The framing and register map are those of lib/drivers/cc1101.c and cc1101_regs.h.
//! PARTNUM and VERSION are from the CC1101 datasheet. Nothing is transmitted or received: FIFOs read
//! empty, RSSI is flat, and GD0 follows IOCFG0 only for the constant levels the firmware's self-test
//! asks for.

const READ: u8 = 1 << 7;
const BURST: u8 = 1 << 6;
const ADDRESS: u8 = 0x3F;
const FIRST_STROBE: u8 = 0x30;
const PATABLE: u8 = 0x3E;
const FIFO: u8 = 0x3F;

const SRES: u8 = 0x30;
const SFSTXON: u8 = 0x31;
const SCAL: u8 = 0x33;
const SRX: u8 = 0x34;
const STX: u8 = 0x35;
const SIDLE: u8 = 0x36;
const SPWD: u8 = 0x39;
const SFRX: u8 = 0x3A;
const SFTX: u8 = 0x3B;

const IOCFG0: usize = 0x02;
/// CC1101IocfgHighImpedance, CC1101IocfgHW and CC1101_IOCFG_INV (cc1101_regs.h).
const IOCFG_HIGH_IMPEDANCE: u8 = 0x2E;
const IOCFG_HW: u8 = 0x2F;
const IOCFG_INV: u8 = 1 << 6;

/// CC1101State (cc1101_regs.h).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Idle = 0,
    Rx = 1,
    Tx = 2,
    FsTxOn = 3,
}

#[derive(Clone, Debug)]
pub struct Cc1101 {
    regs: [u8; 0x2F],
    patable: [u8; 8],
    state: State,
    /// Header of the frame in progress.
    header: Option<u8>,
    /// Bytes transferred since power-on, so the machine can tell it was touched.
    pub accesses: u64,
}

impl Default for Cc1101 {
    fn default() -> Self {
        Self::new()
    }
}

impl Cc1101 {
    pub fn new() -> Self {
        let mut c = Cc1101 {
            regs: [0; 0x2F],
            patable: [0; 8],
            state: State::Idle,
            header: None,
            accesses: 0,
        };
        c.reset();
        c
    }

    pub fn reset(&mut self) {
        self.regs = [0; 0x2F];
        self.regs[IOCFG0] = IOCFG_HIGH_IMPEDANCE;
        self.patable = [0; 8];
        self.state = State::Idle;
        self.header = None;
    }

    /// CS went high.
    pub fn deselect(&mut self) {
        self.header = None;
    }

    /// CHIP_RDYn low, the state, and no bytes in a FIFO (CC1101Status, cc1101_regs.h).
    fn status(&self) -> u8 {
        (self.state as u8) << 4
    }

    fn strobe(&mut self, s: u8) {
        self.state = match s {
            SRES => {
                self.reset();
                State::Idle
            }
            SFSTXON => State::FsTxOn,
            SRX => State::Rx,
            STX => State::Tx,
            SCAL | SIDLE | SPWD | SFRX | SFTX => State::Idle,
            _ => self.state,
        };
    }

    /// The MARCSTATE values of the states it can be in (CC1101 datasheet).
    fn marcstate(&self) -> u8 {
        match self.state {
            State::Idle => 0x01,
            State::Rx => 0x0D,
            State::FsTxOn => 0x12,
            State::Tx => 0x13,
        }
    }

    fn status_register(&self, address: u8) -> u8 {
        match address {
            0x30 => 0x00, // PARTNUM
            0x31 => 0x14, // VERSION
            0x34 => 0x80, // RSSI: nothing there
            0x35 => self.marcstate(),
            _ => 0,
        }
    }

    /// One byte on the bus.
    pub fn transfer(&mut self, mosi: u8) -> u8 {
        self.accesses += 1;
        let Some(header) = self.header else {
            let address = mosi & ADDRESS;
            if (FIRST_STROBE..PATABLE).contains(&address) && mosi & BURST == 0 {
                let status = self.status();
                self.strobe(address);
                return status;
            }
            self.header = Some(mosi);
            return self.status();
        };
        let address = header & ADDRESS;
        let burst = header & BURST != 0;
        let out = if header & READ != 0 {
            match address {
                FIFO => 0,
                PATABLE => self.patable[0],
                a if a >= FIRST_STROBE => self.status_register(a),
                a => self.regs[a as usize],
            }
        } else {
            match address {
                FIFO => {}
                PATABLE => self.patable[0] = mosi,
                a if a < FIRST_STROBE => self.regs[a as usize % self.regs.len()] = mosi,
                _ => {}
            }
            self.status()
        };
        if burst && address < FIRST_STROBE - 1 {
            self.header = Some((header & !ADDRESS) | (address + 1));
        } else if !burst {
            self.header = None;
        }
        out
    }

    /// What GD0 drives: only the constant levels; anything else a stub would have to invent.
    pub fn gd0(&self) -> Option<bool> {
        let v = self.regs[IOCFG0];
        (v & 0x3F == IOCFG_HW).then_some(v & IOCFG_INV != 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strobes_and_status_follow_the_firmwares_driver() {
        let mut c = Cc1101::new();
        assert_eq!(c.transfer(SRX), 0, "the status from before the strobe, CHIP_RDYn low");
        c.deselect();
        assert_eq!(c.transfer(0x3D) >> 4, 1, "SNOP: now in RX");
        c.deselect();
        assert_eq!(c.transfer(SIDLE) >> 4, 1);
        c.deselect();
        assert_eq!(c.transfer(0x3D) >> 4, 0);
    }

    #[test]
    fn the_power_on_self_test_sees_gd0_move() {
        let mut c = Cc1101::new();
        assert_eq!(c.gd0(), None);
        c.transfer(IOCFG0 as u8);
        c.transfer(IOCFG_HW);
        c.deselect();
        assert_eq!(c.gd0(), Some(false));
        c.transfer(IOCFG0 as u8);
        c.transfer(IOCFG_HW | IOCFG_INV);
        c.deselect();
        assert_eq!(c.gd0(), Some(true));
    }

    #[test]
    fn registers_read_back_and_the_chip_says_who_it_is() {
        let mut c = Cc1101::new();
        c.transfer(0x10);
        c.transfer(0xAB);
        c.deselect();
        c.transfer(0x10 | READ);
        assert_eq!(c.transfer(0), 0xAB);
        c.deselect();
        c.transfer(0x31 | READ | BURST);
        assert_eq!(c.transfer(0), 0x14);
    }

    #[test]
    fn no_signal() {
        let mut c = Cc1101::new();
        c.transfer(0x3B | READ | BURST); // RXBYTES
        assert_eq!(c.transfer(0), 0);
    }
}

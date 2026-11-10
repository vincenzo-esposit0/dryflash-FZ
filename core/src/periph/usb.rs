// SPDX-License-Identifier: GPL-3.0-or-later
//! USB full-speed device peripheral (RM0434 "Universal serial bus full-speed device interface (USB)").
//!
//! The firmware drives it through libusb_stm32 (`lib/libusb_stm32/src/usbd_stm32wb55_devfs.c`, the
//! submodule at 6ca2857 in firmware 1.5.1). The packet memory (PMA) is 1 KiB at 0x4000_6C00, with
//! 16-bit locations at consecutive addresses (stm32wb55xx.h `USB1_PMAADDR`; the driver's `EPT()`
//! puts the buffer table at `BTABLE + 8 × endpoint`).
//!
//! Modelled: the eight USB_EPnR with their mixed bit types (rc_w0, toggle, read/write, read-only),
//! CNTR, ISTR (its CTR, DIR and EP_ID computed from the endpoints), FNR, DADDR, BTABLE, BCDR.DPPU,
//! the buffer table and packet memory, single-buffered control/bulk/interrupt endpoints and
//! double-buffered bulk endpoints, and the interrupt line. The bus side — transactions, reset,
//! start of frame, suspend and resume — is called by the virtual host (`board::usb_host`).
//!
//! Not modelled: isochronous endpoints, LPM, battery charging detection (BCDR reads back what was
//! written, so the driver's `connect()` reports "no data contact"), the USB_HP line (every event
//! goes to USB_LP; the firmware handles both the same way, furi_hal_interrupt.c), data toggle
//! errors and bit-level bus timing.

/// What the device answers to a token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Handshake {
    Ack,
    Nak,
    Stall,
    /// No answer: the device is not addressed, not powered, or the endpoint is disabled.
    Timeout,
}

pub const PMA_BASE: u32 = 0x400;
pub const PMA_SIZE: usize = 0x400;

const CNTR: u32 = 0x40;
const ISTR: u32 = 0x44;
const FNR: u32 = 0x48;
const DADDR: u32 = 0x4C;
const BTABLE: u32 = 0x50;
const LPMCSR: u32 = 0x54;
const BCDR: u32 = 0x58;

// USB_EPnR fields (stm32wb55xx.h).
const EP_CTR_RX: u16 = 1 << 15;
const EP_DTOG_RX: u16 = 1 << 14;
const EP_STAT_RX: u16 = 3 << 12;
const EP_SETUP: u16 = 1 << 11;
const EP_TYPE: u16 = 3 << 9;
const EP_KIND: u16 = 1 << 8;
const EP_CTR_TX: u16 = 1 << 7;
const EP_DTOG_TX: u16 = 1 << 6;
const EP_STAT_TX: u16 = 3 << 4;
const EP_EA: u16 = 0xF;
const TYPE_BULK: u16 = 0;
const TYPE_CONTROL: u16 = 1 << 9;

const STAT_DISABLED: u16 = 0;
const STAT_STALL: u16 = 1;
const STAT_NAK: u16 = 2;

pub const CNTR_FRES: u32 = 1 << 0;
pub const CNTR_PDWN: u32 = 1 << 1;
pub const CNTR_FSUSP: u32 = 1 << 3;
pub const ISTR_CTR: u32 = 1 << 15;
pub const ISTR_WKUP: u32 = 1 << 12;
pub const ISTR_SUSP: u32 = 1 << 11;
pub const ISTR_RESET: u32 = 1 << 10;
pub const ISTR_SOF: u32 = 1 << 9;
const ISTR_DIR: u32 = 1 << 4;
/// ISTR flags software clears by writing 0 (rc_w0): PMAOVR, ERR, WKUP, SUSP, RESET, SOF, ESOF, L1REQ.
const ISTR_FLAGS: u32 = 0x7F80;
const DADDR_EF: u32 = 1 << 7;
pub const BCDR_DPPU: u32 = 1 << 15;

#[derive(Clone, Debug)]
pub struct Usb {
    pub epr: [u16; 8],
    pub cntr: u32,
    /// ISTR flags (bits 14–7); CTR, DIR and EP_ID come from the endpoints.
    istr: u32,
    fnr: u32,
    daddr: u32,
    btable: u32,
    lpmcsr: u32,
    bcdr: u32,
    pub pma: Vec<u8>,
    /// STAT_TX of a double-buffered IN endpoint was set to NAK by the peripheral (see `input`).
    hw_nak: [bool; 8],
    /// APB1ENR1.USBEN: registers and bus side work only with the clock on.
    pub clocked: bool,
}

impl Default for Usb {
    fn default() -> Self {
        Self::new()
    }
}

/// Size in bytes of a reception buffer from its COUNTn_RX: BL_SIZE (bit 15), NUM_BLOCK (14:10).
fn rx_capacity(count: u16) -> usize {
    let blocks = ((count >> 10) & 0x1F) as usize;
    if count & 0x8000 != 0 {
        (blocks + 1) * 32
    } else {
        blocks * 2
    }
}

/// Writes `bits` (a STAT field, already shifted) into `epr` under `mask`.
fn with_stat(epr: u16, mask: u16, stat: u16) -> u16 {
    let shift = mask.trailing_zeros();
    (epr & !mask) | (stat << shift)
}

fn stat(epr: u16, mask: u16) -> u16 {
    (epr & mask) >> mask.trailing_zeros()
}

impl Usb {
    pub fn new() -> Self {
        Usb {
            epr: [0; 8],
            cntr: CNTR_FRES | CNTR_PDWN,
            istr: 0,
            fnr: 0,
            daddr: 0,
            btable: 0,
            lpmcsr: 0,
            bcdr: 0,
            pma: vec![0; PMA_SIZE],
            hw_nak: [false; 8],
            clocked: false,
        }
    }

    /// Peripheral reset (system reset or RCC_APB1RSTR1.USBRST). The packet memory keeps its content.
    pub fn reset(&mut self) {
        let pma = std::mem::take(&mut self.pma);
        let clocked = self.clocked;
        *self = Self::new();
        self.pma = pma;
        self.clocked = clocked;
    }

    /// Whether the transceiver works: clock on, out of power-down and forced reset.
    pub fn running(&self) -> bool {
        self.clocked && self.cntr & (CNTR_FRES | CNTR_PDWN) == 0
    }

    /// The D+ pull-up, which tells the host a full-speed device is attached.
    pub fn pulled_up(&self) -> bool {
        self.clocked && self.bcdr & BCDR_DPPU != 0
    }

    pub fn suspended(&self) -> bool {
        self.cntr & CNTR_FSUSP != 0
    }

    fn istr(&self) -> u32 {
        let mut v = self.istr;
        if let Some(n) = (0..8).find(|&n| self.epr[n] & (EP_CTR_RX | EP_CTR_TX) != 0) {
            v |= ISTR_CTR | n as u32;
            if self.epr[n] & EP_CTR_RX != 0 {
                v |= ISTR_DIR;
            }
        }
        v
    }

    /// The USB_LP interrupt: an ISTR event with its CNTR mask bit (same bit positions, 15–7).
    pub fn irq(&self) -> bool {
        self.clocked && self.istr() & self.cntr & 0xFF80 != 0
    }

    pub fn read(&self, off: u32, size: crate::cpu::Size) -> u32 {
        use crate::cpu::Size;
        if (PMA_BASE..PMA_BASE + PMA_SIZE as u32).contains(&off) {
            let o = (off - PMA_BASE) as usize;
            let b = |i: usize| *self.pma.get(o + i).unwrap_or(&0) as u32;
            return match size {
                Size::Byte => b(0),
                Size::Half => b(0) | b(1) << 8,
                Size::Word => b(0) | b(1) << 8 | b(2) << 16 | b(3) << 24,
            };
        }
        let word = match off & !3 {
            o @ 0x00..=0x1C => self.epr[(o / 4) as usize] as u32,
            CNTR => self.cntr,
            ISTR => self.istr(),
            FNR => self.fnr,
            DADDR => self.daddr,
            BTABLE => self.btable,
            LPMCSR => self.lpmcsr,
            BCDR => self.bcdr,
            _ => 0,
        };
        // Registers are 16 bits wide; the upper half of each word reads 0.
        let shift = (off & 3) * 8;
        match size {
            Size::Byte => (word >> shift) & 0xFF,
            Size::Half => (word >> shift) & 0xFFFF,
            Size::Word => word,
        }
    }

    /// Whether `off` is a register or packet-memory location.
    pub fn known(off: u32) -> bool {
        (PMA_BASE..PMA_BASE + PMA_SIZE as u32).contains(&off)
            || matches!(
                off & !3,
                0x00..=0x1C | CNTR | ISTR | FNR | DADDR | BTABLE | LPMCSR | BCDR
            )
    }

    pub fn write(&mut self, off: u32, v: u32, size: crate::cpu::Size) {
        use crate::cpu::Size;
        if (PMA_BASE..PMA_BASE + PMA_SIZE as u32).contains(&off) {
            let o = (off - PMA_BASE) as usize;
            for i in 0..size as usize {
                if let Some(b) = self.pma.get_mut(o + i) {
                    *b = (v >> (8 * i)) as u8;
                }
            }
            return;
        }
        // The upper half of each word is reserved; byte writes merge into the low half.
        let v = match size {
            Size::Byte => {
                let shift = (off & 3) * 8;
                if shift >= 16 {
                    return;
                }
                let old = self.read(off & !3, Size::Half);
                (old & !(0xFF << shift)) | ((v & 0xFF) << shift)
            }
            _ if off & 2 != 0 => return,
            _ => v & 0xFFFF,
        };
        match off & !3 {
            o @ 0x00..=0x1C => self.write_epr((o / 4) as usize, v as u16),
            CNTR => self.cntr = v,
            ISTR => self.istr &= v & ISTR_FLAGS,
            DADDR => self.daddr = v & 0xFF,
            BTABLE => self.btable = v & 0xFFF8,
            LPMCSR => self.lpmcsr = v & 0x0B,
            BCDR => self.bcdr = v & 0x800F,
            _ => {}
        }
    }

    /// USB_EPnR write (RM0434 USB_EPnR): CTR_RX/CTR_TX rc_w0, DTOG and STAT toggle on 1, SETUP read-only,
    /// EP_TYPE, EP_KIND and EA read/write.
    fn write_epr(&mut self, n: usize, v: u16) {
        let old = self.epr[n];
        let ctr = old & v & (EP_CTR_RX | EP_CTR_TX);
        let rw = v & (EP_TYPE | EP_KIND | EP_EA);
        let toggled = (old ^ v) & (EP_DTOG_RX | EP_STAT_RX | EP_DTOG_TX | EP_STAT_TX);
        self.epr[n] = ctr | rw | toggled | (old & EP_SETUP);
        if v & EP_STAT_TX != 0 {
            // Software chose STAT_TX itself.
            self.hw_nak[n] = false;
        }
    }

    // ---- Bus side, called by the host -----------------------------------------------------------

    /// Bus reset by the host (RM0434 "USB reset"): the endpoint registers and DADDR reset, RESET is
    /// flagged, and a suspended device also sees a wakeup.
    pub fn bus_reset(&mut self) {
        if !self.running() {
            return;
        }
        if self.suspended() {
            self.istr |= ISTR_WKUP;
        }
        self.epr = [0; 8];
        self.hw_nak = [false; 8];
        self.daddr = 0;
        self.istr |= ISTR_RESET;
    }

    /// Start of frame: the frame number advances and SOF is flagged.
    pub fn sof(&mut self) {
        if !self.running() {
            return;
        }
        if self.suspended() {
            self.istr |= ISTR_WKUP;
        }
        self.fnr = (self.fnr & !0x7FF) | ((self.fnr + 1) & 0x7FF);
        self.istr |= ISTR_SOF;
    }

    /// The bus has been idle for 3 ms: SUSP is flagged (once, until software suspends or resumes).
    pub fn idle(&mut self) {
        if self.running() && !self.suspended() {
            self.istr |= ISTR_SUSP;
        }
    }

    fn pma16(&self, addr: u32) -> u16 {
        let a = (addr as usize) & (PMA_SIZE - 2);
        u16::from_le_bytes([self.pma[a], self.pma[a + 1]])
    }

    fn set_pma16(&mut self, addr: u32, v: u16) {
        let a = (addr as usize) & (PMA_SIZE - 2);
        self.pma[a..a + 2].copy_from_slice(&v.to_le_bytes());
    }

    /// Buffer-table entry `slot` (0 = ADDR_TX/COUNT_TX or buffer 0, 1 = ADDR_RX/COUNT_RX or buffer 1)
    /// of endpoint register `n`: (address, count location).
    fn table(&self, n: usize, slot: u32) -> (u32, u32) {
        let at = self.btable + n as u32 * 8 + slot * 4;
        (self.pma16(at) as u32, at + 2)
    }

    /// The endpoint register for address `ep` (EA), if the device answers to `addr`.
    fn endpoint(&self, addr: u8, ep: u8) -> Option<usize> {
        if !self.running() || self.daddr & DADDR_EF == 0 || (self.daddr & 0x7F) as u8 != addr {
            return None;
        }
        (0..8).find(|&n| (self.epr[n] & EP_EA) as u8 == ep)
    }

    fn double_buffered(&self, n: usize) -> bool {
        self.epr[n] & (EP_TYPE | EP_KIND) == TYPE_BULK | EP_KIND
    }

    fn store_rx(&mut self, slot: (u32, u32), data: &[u8]) {
        let (buf, count_at) = slot;
        let count = self.pma16(count_at);
        let len = data.len().min(rx_capacity(count)).min(0x3FF);
        for (i, b) in data[..len].iter().enumerate() {
            self.pma[(buf as usize + i) % PMA_SIZE] = *b;
        }
        self.set_pma16(count_at, (count & !0x3FF) | len as u16);
    }

    /// SETUP transaction: control endpoints accept it whatever STAT_RX says, unless disabled.
    pub fn setup(&mut self, addr: u8, ep: u8, data: &[u8; 8]) -> Handshake {
        let Some(n) = self.endpoint(addr, ep) else {
            return Handshake::Timeout;
        };
        let e = self.epr[n];
        if e & EP_TYPE != TYPE_CONTROL || stat(e, EP_STAT_RX) == STAT_DISABLED {
            return Handshake::Timeout;
        }
        let slot = self.table(n, 1);
        self.store_rx(slot, data);
        // After SETUP both directions expect DATA1.
        let e = with_stat(e, EP_STAT_RX, STAT_NAK) | EP_CTR_RX | EP_SETUP | EP_DTOG_RX | EP_DTOG_TX;
        self.epr[n] = with_stat(e, EP_STAT_TX, STAT_NAK);
        Handshake::Ack
    }

    /// OUT transaction carrying `data`.
    pub fn out(&mut self, addr: u8, ep: u8, data: &[u8]) -> Handshake {
        let Some(n) = self.endpoint(addr, ep) else {
            return Handshake::Timeout;
        };
        let e = self.epr[n];
        match stat(e, EP_STAT_RX) {
            STAT_DISABLED => return Handshake::Timeout,
            STAT_STALL => return Handshake::Stall,
            STAT_NAK => return Handshake::Nak,
            _ => {}
        }
        if self.double_buffered(n) {
            // DTOG_RX: buffer the peripheral fills; SW_BUF (DTOG_TX): buffer the application holds.
            // Equal means no free buffer: the token is NAKed and STAT_RX stays VALID.
            let dtog = e & EP_DTOG_RX != 0;
            if dtog == (e & EP_DTOG_TX != 0) {
                return Handshake::Nak;
            }
            let slot = self.table(n, dtog as u32);
            self.store_rx(slot, data);
            self.epr[n] = ((e ^ EP_DTOG_RX) | EP_CTR_RX) & !EP_SETUP;
        } else {
            let slot = self.table(n, 1);
            self.store_rx(slot, data);
            self.epr[n] = (with_stat(e, EP_STAT_RX, STAT_NAK) ^ EP_DTOG_RX | EP_CTR_RX) & !EP_SETUP;
        }
        Handshake::Ack
    }

    /// IN transaction: the packet the device has ready, or its handshake.
    ///
    /// Double-buffered IN: DTOG_TX is the buffer the peripheral sends, SW_BUF (DTOG_RX) the one
    /// the application fills; equal means nothing to send. RM0434 says the status is then "masked"
    /// as NAK. The firmware's driver writes a double-buffered IN endpoint only when it reads
    /// STAT_TX = NAK, and never sets it back to VALID (`ep_write`), so on the chip the register
    /// must read NAK while no buffer is ready and the endpoint must still send once the application
    /// toggles SW_BUF. That is how it is modelled: the peripheral sets STAT_TX to NAK when it has
    /// nothing to send, and a NAK it set itself does not stop a filled buffer (ADR-0021).
    pub fn input(&mut self, addr: u8, ep: u8) -> Result<Vec<u8>, Handshake> {
        let Some(n) = self.endpoint(addr, ep) else {
            return Err(Handshake::Timeout);
        };
        let e = self.epr[n];
        let st = stat(e, EP_STAT_TX);
        match st {
            STAT_DISABLED => return Err(Handshake::Timeout),
            STAT_STALL => return Err(Handshake::Stall),
            STAT_NAK if !(self.double_buffered(n) && self.hw_nak[n]) => return Err(Handshake::Nak),
            _ => {}
        }
        let slot = if self.double_buffered(n) {
            let dtog = e & EP_DTOG_TX != 0;
            if dtog == (e & EP_DTOG_RX != 0) {
                self.epr[n] = with_stat(e, EP_STAT_TX, STAT_NAK);
                self.hw_nak[n] = true;
                return Err(Handshake::Nak);
            }
            dtog as u32
        } else {
            0
        };
        let (buf, count_at) = self.table(n, slot);
        let len = (self.pma16(count_at) & 0x3FF) as usize;
        let data = (0..len).map(|i| self.pma[(buf as usize + i) % PMA_SIZE]).collect();
        let mut e = (e ^ EP_DTOG_TX) | EP_CTR_TX;
        if self.double_buffered(n) {
            if (e & EP_DTOG_TX != 0) == (e & EP_DTOG_RX != 0) {
                e = with_stat(e, EP_STAT_TX, STAT_NAK);
                self.hw_nak[n] = true;
            }
        } else {
            e = with_stat(e, EP_STAT_TX, STAT_NAK);
        }
        self.epr[n] = e;
        Ok(data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::Size;

    const STAT_VALID: u16 = 3;
    const USB_EP_RX_VALID: u16 = 0x3000;
    const USB_EP_TX_VALID: u16 = 0x0030;
    const USB_EP_TX_NAK: u16 = 0x0020;
    const EPREG_MASK: u16 = EP_CTR_RX | EP_SETUP | EP_TYPE | EP_KIND | EP_CTR_TX | EP_EA;

    fn on() -> Usb {
        let mut u = Usb::new();
        u.clocked = true;
        u.write(CNTR, 0xBF00, Size::Half);
        u.write(DADDR, DADDR_EF, Size::Half);
        u
    }

    /// usbd_stm32wb55_devfs.c `EP_TOGGLE_SET`.
    fn toggle_set(u: &mut Usb, n: usize, bits: u16, mask: u16) {
        let r = u.read(n as u32 * 4, Size::Half) as u16;
        u.write(n as u32 * 4, ((r ^ bits) & (EPREG_MASK | mask)) as u32, Size::Half);
    }

    fn table(u: &mut Usb, n: usize, entries: [u16; 4]) {
        for (i, v) in entries.iter().enumerate() {
            u.write(PMA_BASE + n as u32 * 8 + i as u32 * 2, *v as u32, Size::Half);
        }
    }

    #[test]
    fn epnr_bits_follow_their_types() {
        let mut u = on();
        u.write(0, (TYPE_CONTROL | 3) as u32, Size::Half);
        toggle_set(&mut u, 0, USB_EP_RX_VALID, EP_STAT_RX | EP_DTOG_RX);
        assert_eq!(u.epr[0], TYPE_CONTROL | 3 | USB_EP_RX_VALID);
        toggle_set(&mut u, 0, USB_EP_RX_VALID, EP_STAT_RX);
        assert_eq!(
            u.epr[0] & EP_STAT_RX,
            USB_EP_RX_VALID,
            "toggling to the same value keeps it"
        );
        u.epr[0] |= EP_CTR_RX | EP_CTR_TX;
        // `*reg &= (USB_EPREG_MASK ^ USB_EP_CTR_TX)` clears CTR_TX only.
        let r = u.read(0, Size::Half) as u16;
        u.write(0, (r & (EPREG_MASK ^ EP_CTR_TX)) as u32, Size::Half);
        assert_eq!(u.epr[0] & (EP_CTR_RX | EP_CTR_TX), EP_CTR_RX);
        assert_eq!(u.read(ISTR, Size::Half) & 0x801F, ISTR_CTR | ISTR_DIR);
        assert!(u.irq());
    }

    #[test]
    fn control_setup_then_in_then_status() {
        let mut u = on();
        table(&mut u, 0, [0x100, 0, 0x140, 4 << 10]); // 8-byte buffers
        u.write(0, TYPE_CONTROL as u32, Size::Half);
        toggle_set(&mut u, 0, USB_EP_RX_VALID, EP_STAT_RX);
        toggle_set(&mut u, 0, USB_EP_TX_NAK, EP_STAT_TX);
        assert_eq!(u.input(0, 0), Err(Handshake::Nak));
        assert_eq!(u.setup(0, 0, &[0x80, 6, 0, 1, 0, 0, 18, 0]), Handshake::Ack);
        assert_eq!(u.pma[0x140..0x148], [0x80, 6, 0, 1, 0, 0, 18, 0]);
        assert_eq!(u.pma16(6) & 0x3FF, 8);
        assert_ne!(u.epr[0] & EP_SETUP, 0);
        u.pma[0x100..0x108].copy_from_slice(b"\x12\x01\x00\x02\xEF\x02\x01\x08");
        u.set_pma16(2, 8);
        toggle_set(&mut u, 0, USB_EP_TX_VALID, EP_STAT_TX);
        assert_eq!(u.input(0, 0).unwrap(), b"\x12\x01\x00\x02\xEF\x02\x01\x08");
        assert_eq!(stat(u.epr[0], EP_STAT_TX), STAT_NAK);
        assert_eq!(u.out(0, 0, &[]), Handshake::Nak, "RX not re-armed yet");
        assert_eq!(u.setup(1, 0, &[0; 8]), Handshake::Timeout, "another address");
    }

    /// The firmware's CDC data endpoints: double-buffered bulk (furi_hal_usb_cdc.c `cdc_ep_config`).
    #[test]
    fn double_buffered_bulk_in_and_out() {
        let mut u = on();
        // EP1 OUT: buffers at 0x200/0x240; EP2 IN: buffers at 0x280/0x2C0.
        table(&mut u, 1, [0x200, 2 << 10, 0x240, 2 << 10]);
        table(&mut u, 2, [0x280, 0, 0x2C0, 0]);
        u.write(4, (TYPE_BULK | EP_KIND | 1) as u32, Size::Half);
        // EP_DRX_UNSTALL: VALID, DTOG_RX = 0, SW_BUF_RX = 1.
        toggle_set(
            &mut u,
            1,
            USB_EP_RX_VALID | EP_DTOG_TX,
            EP_STAT_RX | EP_DTOG_RX | EP_DTOG_TX,
        );
        assert_eq!(u.out(0, 1, b"ab"), Handshake::Ack);
        assert_eq!(
            u.out(0, 1, b"cd"),
            Handshake::Nak,
            "the application holds the other buffer"
        );
        assert_eq!(stat(u.epr[1], EP_STAT_RX), STAT_VALID);
        // ep_read: (DTOG_RX, SW_BUF) = (1, 1) → toggle SW_BUF, read buffer 0.
        let r = u.read(4, Size::Half) as u16;
        u.write(4, ((r & EPREG_MASK) | EP_DTOG_TX) as u32, Size::Half);
        assert_eq!(&u.pma[0x200..0x202], b"ab");
        assert_eq!(u.out(0, 1, b"cd"), Handshake::Ack);
        assert_eq!(&u.pma[0x240..0x242], b"cd");

        u.write(8, (TYPE_BULK | EP_KIND | 2) as u32, Size::Half);
        // EP_DTX_UNSTALL: VALID, DTOG_TX = 0, SW_BUF_TX = 0.
        toggle_set(&mut u, 2, USB_EP_TX_VALID, EP_STAT_TX | EP_DTOG_TX | EP_DTOG_RX);
        assert_eq!(u.input(0, 2), Err(Handshake::Nak), "nothing written yet");
        assert_eq!(u.read(8, Size::Half) as u16 & EP_STAT_TX, USB_EP_TX_NAK);
        // ep_write: buffer SW_BUF = 0, then toggle SW_BUF.
        u.pma[0x280..0x283].copy_from_slice(b"xyz");
        u.set_pma16(0x10 + 2, 3);
        let r = u.read(8, Size::Half) as u16;
        u.write(8, ((r & EPREG_MASK) | EP_DTOG_RX) as u32, Size::Half);
        assert_eq!(u.input(0, 2).unwrap(), b"xyz");
        assert_eq!(u.input(0, 2), Err(Handshake::Nak));
        assert_eq!(
            u.read(8, Size::Half) as u16 & EP_STAT_TX,
            USB_EP_TX_NAK,
            "ready for the next write"
        );
    }

    #[test]
    fn bus_reset_clears_endpoints_and_wakes_a_suspended_device() {
        let mut u = on();
        u.write(0, TYPE_CONTROL as u32, Size::Half);
        u.write(CNTR, 0xBF00 | CNTR_FSUSP, Size::Half);
        u.bus_reset();
        assert_eq!(u.epr[0], 0);
        assert_eq!(u.read(DADDR, Size::Half), 0);
        assert_eq!(
            u.read(ISTR, Size::Half) & (ISTR_RESET | ISTR_WKUP),
            ISTR_RESET | ISTR_WKUP
        );
        // `USB->ISTR &= ~USB_ISTR_RESET`
        let v = u.read(ISTR, Size::Half) & !ISTR_RESET;
        u.write(ISTR, v, Size::Half);
        assert_eq!(u.read(ISTR, Size::Half) & ISTR_RESET, 0);
    }
}

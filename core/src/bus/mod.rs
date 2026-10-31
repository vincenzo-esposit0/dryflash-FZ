// SPDX-License-Identifier: GPL-3.0-or-later
//! System bus of the STM32WB55 as seen by CPU1 (everything outside the internal PPB).
//!
//! Memories are modelled here. Peripherals are added from M2; until a block has a model,
//! its registers read as zero, ignore writes, and every access is recorded in the
//! bring-up digest (QEMU-style "unimplemented device" semantics — never a fake value).

pub mod map;

use crate::cpu::{Bus, BusError, Size};
use crate::periph::{Ctx, Periphs, ResetKind};
use crate::trace::digest::Digest;
use map::*;

/// Fidelity of a modelled component (shown in the UI's "Hardware fidelity" table).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fidelity {
    Emulated,
    Partial,
    Stubbed,
}

pub struct SystemBus {
    pub flash: Vec<u8>,
    pub sram1: Vec<u8>,
    pub sram2a: Vec<u8>,
    pub sram2b: Vec<u8>,
    /// One-time-programmable area; provisioned with board data in M2 (BRIEF §5.1).
    pub otp: Vec<u8>,
    /// Engineering bytes: package, UID64, UID, flash size.
    pub engi: Vec<u8>,
    pub digest: Digest,
    pub periphs: Periphs,
    /// Current virtual time, updated by the machine before each CPU slice.
    pub now: u64,
    access_pc: u32,
}

impl Default for SystemBus {
    fn default() -> Self {
        Self::new()
    }
}

fn rd(mem: &[u8], off: usize, size: Size) -> u32 {
    match size {
        Size::Byte => mem[off] as u32,
        Size::Half => u16::from_le_bytes([mem[off], mem[off + 1]]) as u32,
        Size::Word => u32::from_le_bytes([mem[off], mem[off + 1], mem[off + 2], mem[off + 3]]),
    }
}

fn wr(mem: &mut [u8], off: usize, v: u32, size: Size) {
    let n = size as usize;
    mem[off..off + n].copy_from_slice(&v.to_le_bytes()[..n]);
}

/// Offset of `addr` inside `[base, base + len)` if the whole access fits.
#[inline]
fn within(addr: u32, size: Size, base: u32, len: u32) -> Option<usize> {
    let off = addr.wrapping_sub(base);
    (off < len && off + size as u32 <= len).then_some(off as usize)
}

impl SystemBus {
    pub fn new() -> Self {
        let mut engi = vec![0xFF; ENGI_SIZE as usize];
        let fs = (FLASHSIZE_ADDR - ENGI_BASE) as usize;
        // 1024 KiB of flash on the STM32WB55RG.
        engi[fs..fs + 2].copy_from_slice(&1024u16.to_le_bytes());
        SystemBus {
            flash: vec![0xFF; FLASH_SIZE as usize],
            sram1: vec![0; SRAM1_SIZE as usize],
            sram2a: vec![0; SRAM2A_SIZE as usize],
            sram2b: vec![0; SRAM2B_SIZE as usize],
            otp: vec![0xFF; OTP_SIZE as usize],
            engi,
            digest: Digest::default(),
            periphs: Periphs::new(),
            now: 0,
            access_pc: 0,
        }
    }

    /// Sets the 64-bit unique device ID (UID64), used by the firmware for the BLE address
    /// and device identity. Each virtual device profile chooses its own (M5).
    pub fn set_uid64(&mut self, uid: u64) {
        let off = (UID64_ADDR - ENGI_BASE) as usize;
        self.engi[off..off + 8].copy_from_slice(&uid.to_le_bytes());
    }

    /// Resets the peripherals; memories are handled by `power_on_clear`.
    pub fn reset_peripherals(&mut self, kind: ResetKind) {
        self.periphs.reset(kind, self.now);
    }

    /// RAM contents are lost on power-on reset; a system reset keeps them (only the
    /// firmware's own startup code clears .bss).
    pub fn power_on_clear(&mut self) {
        self.sram1.fill(0);
        self.sram2a.fill(0);
        self.sram2b.fill(0);
    }

    #[inline]
    fn read_mem(&self, addr: u32, size: Size) -> Option<u32> {
        if let Some(o) = within(addr, size, SRAM1_BASE, SRAM1_SIZE) {
            return Some(rd(&self.sram1, o, size));
        }
        if let Some(o) = within(addr, size, FLASH_BASE, FLASH_SIZE) {
            return Some(rd(&self.flash, o, size));
        }
        if let Some(o) = within(addr, size, 0, BOOT_ALIAS_SIZE) {
            return Some(rd(&self.flash, o, size));
        }
        if let Some(o) = within(addr, size, SRAM2A_BASE, SRAM2A_SIZE) {
            return Some(rd(&self.sram2a, o, size));
        }
        if let Some(o) = within(addr, size, SRAM2B_BASE, SRAM2B_SIZE) {
            return Some(rd(&self.sram2b, o, size));
        }
        if let Some(o) = within(addr, size, OTP_BASE, OTP_SIZE) {
            return Some(rd(&self.otp, o, size));
        }
        if let Some(o) = within(addr, size, ENGI_BASE, ENGI_SIZE) {
            return Some(rd(&self.engi, o, size));
        }
        None
    }
}

impl Bus for SystemBus {
    #[inline]
    fn read(&mut self, addr: u32, size: Size) -> Result<u32, BusError> {
        if let Some(v) = self.read_mem(addr, size) {
            return Ok(v);
        }
        let mut ctx = Ctx {
            now: self.now,
            digest: &mut self.digest,
            pc: self.access_pc,
        };
        if let Some(v) = self.periphs.read(addr, size, &mut ctx) {
            return Ok(v);
        }
        if block_of(addr).is_some() || within(addr, size, SYSTEM_MEMORY_BASE, SYSTEM_MEMORY_SIZE).is_some() {
            // Peripheral without a model, or the ST system-memory ROM (not provided).
            self.digest.unimplemented(addr, false, None, self.access_pc);
            return Ok(0);
        }
        self.digest.unmapped(addr, false, None, self.access_pc);
        Err(BusError { addr, write: false })
    }

    #[inline]
    fn write(&mut self, addr: u32, value: u32, size: Size) -> Result<(), BusError> {
        if let Some(o) = within(addr, size, SRAM1_BASE, SRAM1_SIZE) {
            wr(&mut self.sram1, o, value, size);
            return Ok(());
        }
        if let Some(o) = within(addr, size, SRAM2A_BASE, SRAM2A_SIZE) {
            wr(&mut self.sram2a, o, value, size);
            return Ok(());
        }
        if let Some(o) = within(addr, size, SRAM2B_BASE, SRAM2B_SIZE) {
            wr(&mut self.sram2b, o, value, size);
            return Ok(());
        }
        let mut ctx = Ctx {
            now: self.now,
            digest: &mut self.digest,
            pc: self.access_pc,
        };
        if self.periphs.write(addr, value, size, &mut ctx) {
            return Ok(());
        }
        let in_flash =
            within(addr, size, FLASH_BASE, FLASH_SIZE).is_some() || within(addr, size, 0, BOOT_ALIAS_SIZE).is_some();
        if in_flash
            || block_of(addr).is_some()
            || within(addr, size, OTP_BASE, OTP_SIZE + ENGI_SIZE).is_some()
            || within(addr, size, SYSTEM_MEMORY_BASE, SYSTEM_MEMORY_SIZE).is_some()
        {
            // Flash/OTP are only programmable through the FLASH controller (M2); a direct
            // write without it is recorded and ignored, like an unimplemented register.
            self.digest.unimplemented(addr, true, Some(value), self.access_pc);
            return Ok(());
        }
        self.digest.unmapped(addr, true, Some(value), self.access_pc);
        Err(BusError { addr, write: true })
    }

    #[inline]
    fn fetch16(&mut self, addr: u32) -> Result<u16, BusError> {
        self.read_mem(addr, Size::Half)
            .map(|v| v as u16)
            .ok_or(BusError { addr, write: false })
    }

    #[inline]
    fn set_access_pc(&mut self, pc: u32) {
        self.access_pc = pc;
    }

    fn ppb_unimplemented(&mut self, addr: u32, write: bool, pc: u32) {
        self.digest.unimplemented(addr, write, None, pc);
    }

    #[inline]
    fn irq_lines(&self) -> u64 {
        self.periphs.irq_lines
    }

    #[inline]
    fn take_irq_dirty(&mut self) -> bool {
        std::mem::take(&mut self.periphs.irq_dirty)
    }

    #[inline]
    fn next_event(&self) -> Option<u64> {
        self.periphs.next_event
    }

    fn advance(&mut self, now: u64) {
        self.now = now;
        let mut ctx = Ctx {
            now,
            digest: &mut self.digest,
            pc: self.access_pc,
        };
        self.periphs.advance(&mut ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flash_is_aliased_at_zero() {
        let mut bus = SystemBus::new();
        bus.flash[..4].copy_from_slice(&0x2003_0000u32.to_le_bytes());
        assert_eq!(bus.read(0, Size::Word).unwrap(), 0x2003_0000);
        assert_eq!(bus.read(FLASH_BASE, Size::Word).unwrap(), 0x2003_0000);
    }

    #[test]
    fn unimplemented_peripheral_reads_zero_and_is_recorded() {
        let mut bus = SystemBus::new();
        bus.set_access_pc(0x0800_1234);
        // TSC+0x050 is inside the TSC block but no register exists there in the SVD.
        assert_eq!(bus.read(0x4002_4050, Size::Word).unwrap(), 0);
        bus.write(0x4002_4050, 0x100, Size::Word).unwrap();
        let st = bus.digest.unimplemented[&0x4002_4050];
        assert_eq!(
            (st.reads, st.writes, st.first_pc, st.last_write),
            (1, 1, 0x0800_1234, Some(0x100))
        );
    }

    #[test]
    fn svd_registers_have_reset_values_and_latch() {
        let mut bus = SystemBus::new();
        // RCC.CR resets to 0x61 (MSI on, 4 MHz); MSIRDY reads 1 because MSI is stable at once.
        assert_eq!(bus.read(0x5800_0000, Size::Word).unwrap(), 0x63);
        // IWDG.RLR (register file only until the watchdog is modelled) resets to the SVD value.
        let rlr = crate::periph::svd::IWDG
            .regs
            .iter()
            .find(|r| r.name == "RLR")
            .unwrap()
            .reset;
        assert_eq!(bus.read(0x4000_3008, Size::Word).unwrap(), rlr);
        assert_ne!(rlr, 0, "a register with a non-zero reset value");
        bus.write(0x4000_002C, 0x1234, Size::Word).unwrap();
        assert_eq!(bus.read(0x4000_002C, Size::Word).unwrap(), 0x1234);
        assert!(
            bus.digest.latched.contains_key(&0x4000_002C),
            "reported as register-file only"
        );
    }

    #[test]
    fn unmapped_access_is_a_bus_error() {
        let mut bus = SystemBus::new();
        assert!(bus.read(0x3000_0000, Size::Word).is_err());
        assert!(bus.digest.unmapped.contains_key(&0x3000_0000));
    }

    #[test]
    fn flash_size_engineering_byte() {
        let mut bus = SystemBus::new();
        assert_eq!(bus.read(FLASHSIZE_ADDR, Size::Half).unwrap(), 1024);
    }
}

// SPDX-License-Identifier: GPL-3.0-or-later
//! System bus of the STM32WB55 as seen by CPU1 (everything outside the internal PPB).
//!
//! Memories are modelled here. Peripherals are added from M2; until a block has a model,
//! its registers read as zero, ignore writes, and every access is recorded in the
//! bring-up digest (QEMU-style "unimplemented device" semantics — never a fake value).

pub mod cpu2;
pub mod dma;
pub mod map;

use crate::cpu::{Bus, BusError, Size};
use crate::periph::{Ctx, Periphs, ResetKind};
use crate::trace::digest::Digest;
use map::*;

/// Fidelity of a modelled component (shown in the UI's "Hardware fidelity" table).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fidelity {
    /// Behaves like the hardware for everything the firmware uses.
    Emulated,
    /// Main behaviour modelled; gaps listed in the note.
    Partial,
    /// Plausible fixed values from a virtual device (battery, sensors).
    Simulated,
    /// Answers without doing the work (register file only, or a reduced stand-in).
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
    /// Virtual-device setting: Settings → System → Sleep Method = Legacy (ADR-0012).
    pub legacy_sleep: bool,
    /// CPU2 behavioural model (FUS without wireless stack, ADR-0012).
    pub cpu2: cpu2::Cpu2,
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
        let mut bus = SystemBus {
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
            cpu2: cpu2::Cpu2::default(),
            legacy_sleep: true,
        };
        // A virtual device ships provisioned, like a production unit (BRIEF §5.1).
        bus.provision_otp(&crate::board::otp::BoardInfo::default());
        bus
    }

    /// Programs the board data into the OTP area (virtual device settings, BRIEF §5.1).
    pub fn provision_otp(&mut self, info: &crate::board::otp::BoardInfo) {
        self.otp.fill(0xFF);
        let b = info.otp_bytes();
        self.otp[..b.len()].copy_from_slice(&b);
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
        self.cpu2.reset();
        if kind == ResetKind::PowerOn && self.legacy_sleep {
            self.preset_backup_domain();
        }
    }

    /// The backup domain of a device whose user chose Sleep Method = Legacy
    /// (applications/settings/system/system_settings.c): Furi's RTC header
    /// (`FURI_HAL_RTC_HEADER_MAGIC` 0x10F1, version 0, targets/f7/furi_hal/furi_hal_rtc.c) and
    /// `SystemReg.flags` = `FuriHalRtcFlagLegacySleep` (bit 5, furi_hal_rtc.h), every other field
    /// at the default `furi_hal_rtc_reset_registers` writes. With CPU2 in FUS mode the default
    /// (STOP2) sleep cannot switch clocks and never reaches WFI (ADR-0012).
    fn preset_backup_domain(&mut self) {
        self.periphs.rtc.set_backup(0, 0x0000_10F1);
        self.periphs.rtc.set_backup(1, (1 << 5) << 8);
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
            if dma::concerns_dma(addr) {
                self.dma_service();
            }
            if addr == cpu2::PWR_CR4 || addr.wrapping_sub(crate::periph::svd::IPCC.base) < 0x400 {
                self.cpu2_service();
            }
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

    /// Button wiring (board::Button::pin) through GPIO, SYSCFG_EXTICRx and EXTI to the NVIC.
    #[test]
    fn button_press_reaches_exti_through_exticr() {
        use crate::board::Button;
        use crate::periph::{irq, svd::EXTI};
        const GPIOB: u32 = 0x4800_0400;
        const GPIOH: u32 = 0x4800_1C00;
        const SYSCFG: u32 = 0x4001_0000;
        const EXTI_BASE: u32 = 0x5800_0800;
        let mut bus = SystemBus::new();
        let w = |bus: &mut SystemBus, a: u32, v: u32| bus.write(a, v, Size::Word).unwrap();
        // Left = PB11 (input, pull-up), OK = PH3 (input, pull-down), as furi_hal_resources.c.
        w(&mut bus, GPIOB, 0);
        w(&mut bus, GPIOB + 0x0C, 1 << 22);
        w(&mut bus, GPIOH, 0);
        w(&mut bus, GPIOH + 0x0C, 2 << 6);
        w(&mut bus, SYSCFG + 0x10, 1 << 12); // EXTICR3: line 11 ← port B
        w(&mut bus, SYSCFG + 0x08, 7 << 12); // EXTICR1: line 3 ← port H
        w(&mut bus, EXTI_BASE + EXTI.offset_of("RTSR1"), (1 << 11) | (1 << 3));
        w(&mut bus, EXTI_BASE + EXTI.offset_of("FTSR1"), (1 << 11) | (1 << 3));
        w(&mut bus, EXTI_BASE + EXTI.offset_of("C1IMR1"), (1 << 11) | (1 << 3));
        let pr1 = EXTI_BASE + EXTI.offset_of("PR1");
        let idr = |bus: &mut SystemBus, port: u32| bus.read(port + 0x10, Size::Word).unwrap();
        assert_eq!(bus.read(pr1, Size::Word).unwrap(), 0, "no edge from configuration");
        assert_ne!(idr(&mut bus, GPIOB) & (1 << 11), 0, "released: pulled up");

        bus.periphs.set_button(Button::Left, true);
        assert_eq!(idr(&mut bus, GPIOB) & (1 << 11), 0, "pressed: active low");
        assert_eq!(bus.read(pr1, Size::Word).unwrap(), 1 << 11);
        assert_ne!(bus.irq_lines() & (1 << irq::EXTI15_10), 0);
        w(&mut bus, pr1, 1 << 11);
        bus.periphs.set_button(Button::Left, false);
        assert_eq!(bus.read(pr1, Size::Word).unwrap(), 1 << 11, "release edge");
        w(&mut bus, pr1, 1 << 11);

        bus.periphs.set_button(Button::Ok, true);
        assert_ne!(idr(&mut bus, GPIOH) & (1 << 3), 0, "OK is active high");
        assert_eq!(bus.read(pr1, Size::Word).unwrap(), 1 << 3);
        assert_ne!(bus.irq_lines() & (1 << irq::EXTI3), 0);
        w(&mut bus, pr1, 1 << 3);

        // Line 11 routed to port A: PB11 edges no longer reach EXTI.
        w(&mut bus, SYSCFG + 0x10, 0);
        bus.periphs.set_button(Button::Left, true);
        assert_eq!(bus.read(pr1, Size::Word).unwrap(), 0);
    }

    #[test]
    fn flash_size_engineering_byte() {
        let mut bus = SystemBus::new();
        assert_eq!(bus.read(FLASHSIZE_ADDR, Size::Half).unwrap(), 1024);
    }
}

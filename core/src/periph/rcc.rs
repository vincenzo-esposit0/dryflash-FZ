// SPDX-License-Identifier: GPL-3.0-or-later
//! Reset and clock control (RCC).
//!
//! Register layout and reset values: generated SVD tables (svd::RCC). Bit positions below
//! are the SVD field offsets. Prescaler and MSI range tables are the firmware's own
//! (`AHBPrescTable`, `APBPrescTable`, `MSIRangeTable` in targets/f7/src/stm32wb55_startup.c).
//!
//! Model: oscillators and PLLs become ready as soon as they are enabled, the system clock
//! switch and prescaler changes apply immediately, and clock gating (xxENR) is latched but
//! not enforced on the gated peripherals. Startup delays are not modelled.

use super::regfile::RegFile;
use super::svd;
use super::{Clocks, Ctx, ResetKind};
use crate::bus::Fidelity;

pub const FIDELITY: (Fidelity, &str) = (
    Fidelity::Partial,
    "oscillators ready immediately; clock tree computed; gating not enforced",
);

const CR: u32 = svd::RCC.offset_of("CR");
pub const CFGR: u32 = svd::RCC.offset_of("CFGR");
const PLLCFGR: u32 = svd::RCC.offset_of("PLLCFGR");
const CIER: u32 = svd::RCC.offset_of("CIER");
const CIFR: u32 = svd::RCC.offset_of("CIFR");
const CICR: u32 = svd::RCC.offset_of("CICR");
const BDCR: u32 = svd::RCC.offset_of("BDCR");
const CSR: u32 = svd::RCC.offset_of("CSR");
const CRRCR: u32 = svd::RCC.offset_of("CRRCR");
const EXTCFGR: u32 = svd::RCC.offset_of("EXTCFGR");
pub const CCIPR: u32 = svd::RCC.offset_of("CCIPR");
pub const APB1RSTR1: u32 = svd::RCC.offset_of("APB1RSTR1");
pub const APB1RSTR2: u32 = svd::RCC.offset_of("APB1RSTR2");
pub const APB2RSTR: u32 = svd::RCC.offset_of("APB2RSTR");
pub const APB2ENR: u32 = svd::RCC.offset_of("APB2ENR");
/// TIM16 in APB2ENR and APB2RSTR (stm32wb55xx.h: RCC_APB2ENR_TIM16EN_Pos, RCC_APB2RSTR_TIM16RST_Pos, 17).
pub const APB2_TIM16: u32 = 1 << 17;

/// (enable bit, ready bit) pairs in CR.
const CR_READY: &[(u32, u32)] = &[(0, 1), (8, 10), (9, 12), (16, 17), (24, 25), (26, 27)];
/// CSR: LSI1ON→LSI1RDY, LSI2ON→LSI2RDY.
const CSR_READY: &[(u32, u32)] = &[(0, 1), (2, 3)];
/// CSR reset flags OBLRSTF..LPWRRSTF (bits 25–31), cleared by RMVF (bit 23).
const CSR_RESET_FLAGS: u32 = 0xFE00_0000;
const CSR_SFTRSTF: u32 = 1 << 28;
const CSR_IWDGRSTF: u32 = 1 << 29;

/// HSE crystal on the Flipper Zero: 32 MHz (the STM32WB radio needs a 32 MHz HSE).
const HSE_HZ: u32 = 32_000_000;
const HSI_HZ: u32 = 16_000_000;
const MSI_RANGE_HZ: [u32; 16] = [
    100_000, 200_000, 400_000, 800_000, 1_000_000, 2_000_000, 4_000_000, 8_000_000, 16_000_000, 24_000_000, 32_000_000,
    48_000_000, 0, 0, 0, 0,
];
const AHB_DIV: [u32; 16] = [1, 3, 5, 1, 1, 6, 10, 32, 2, 4, 8, 16, 64, 128, 256, 512];
const APB_SHIFT: [u32; 8] = [0, 0, 0, 0, 1, 2, 3, 4];

#[derive(Clone, Debug)]
pub struct Rcc {
    pub regs: RegFile,
}

impl Default for Rcc {
    fn default() -> Self {
        Self::new()
    }
}

impl Rcc {
    pub fn new() -> Self {
        let mut r = Rcc {
            regs: RegFile::new(&svd::RCC),
        };
        r.update_status();
        r
    }

    pub fn reset(&mut self, kind: ResetKind) {
        // The backup domain (BDCR) survives every reset but power-on; CSR keeps its reset
        // flags until RMVF and adds the cause of this reset.
        let bdcr = self.regs.get(BDCR);
        let flags = self.regs.get(CSR) & CSR_RESET_FLAGS;
        self.regs.reset();
        match kind {
            ResetKind::PowerOn => {}
            ResetKind::Software => {
                self.regs.set(BDCR, bdcr);
                self.regs
                    .set(CSR, (self.regs.get(CSR) & !CSR_RESET_FLAGS) | flags | CSR_SFTRSTF);
            }
            ResetKind::Iwdg => {
                self.regs.set(BDCR, bdcr);
                self.regs
                    .set(CSR, (self.regs.get(CSR) & !CSR_RESET_FLAGS) | flags | CSR_IWDGRSTF);
            }
        }
        self.update_status();
    }

    /// Recomputes hardware-owned status bits from the control bits.
    fn update_status(&mut self) {
        let cr = self.regs.get(CR);
        for &(on, rdy) in CR_READY {
            self.regs.set_bits(CR, 1 << rdy, cr & (1 << on) != 0);
        }
        let csr = self.regs.get(CSR);
        for &(on, rdy) in CSR_READY {
            self.regs.set_bits(CSR, 1 << rdy, csr & (1 << on) != 0);
        }
        // LSEON → LSERDY; HSI48ON → HSI48RDY.
        let bdcr = self.regs.get(BDCR);
        self.regs.set_bits(BDCR, 1 << 1, bdcr & 1 != 0);
        let crrcr = self.regs.get(CRRCR);
        self.regs.set_bits(CRRCR, 1 << 1, crrcr & 1 != 0);
        // SWS follows SW; prescaler-applied flags are always set (changes apply at once).
        let cfgr = self.regs.get(CFGR);
        self.regs
            .set(CFGR, (cfgr & !(0b11 << 2)) | (cfgr & 0b11) << 2 | 0b111 << 16);
        self.regs.set_bits(EXTCFGR, 0b11 << 16, true);
    }

    pub fn read(&mut self, off: u32, _ctx: &mut Ctx, _addr: u32) -> Option<u32> {
        self.regs.read(off)
    }

    pub fn write(&mut self, off: u32, v: u32, ctx: &mut Ctx, _addr: u32) -> bool {
        let before = self.regs.get(CR);
        match off {
            CICR => {
                // Write-1-to-clear of the CIFR flags.
                let f = self.regs.get(CIFR);
                self.regs.set(CIFR, f & !v);
                return true;
            }
            CSR if v & (1 << 23) != 0 => {
                // RMVF clears the reset flags.
                self.regs.write(CSR, v & !(1 << 23));
                self.regs.set_bits(CSR, CSR_RESET_FLAGS, false);
            }
            BDCR if v & (1 << 16) != 0 => {
                // BDRST: backup domain software reset.
                self.regs.set(BDCR, 1 << 16);
                ctx.digest
                    .event(ctx.now, "RCC: backup domain reset (BDRST)".to_string());
            }
            _ => {
                if !self.regs.write(off, v) {
                    return false;
                }
            }
        }
        self.update_status();
        // Ready interrupts (CIER/CIFR) for the CR oscillators that just became ready.
        let after = self.regs.get(CR);
        let rose = after & !before;
        let cier = self.regs.get(CIER);
        // CIFR bit order: LSI1, LSE, MSI, HSI, HSE, PLL, PLLSAI1 — CR ready bits 1,10,17,25,27.
        for (rdy, flag) in [(1u32, 2u32), (10, 3), (17, 4), (25, 5), (27, 6)] {
            if rose & (1 << rdy) != 0 && cier & (1 << flag) != 0 {
                self.regs.set_bits(CIFR, 1 << flag, true);
            }
        }
        true
    }

    /// True while BDCR.BDRST is set: the backup domain (RTC, backup registers) is held in reset.
    pub fn backup_domain_reset_requested(&self) -> bool {
        self.regs.get(BDCR) & (1 << 16) != 0
    }

    /// Clock tree (RM0434 "Clock tree"; dividers as programmed in CFGR/PLLCFGR/CR).
    pub fn clocks(&self) -> Clocks {
        let cr = self.regs.get(CR);
        let cfgr = self.regs.get(CFGR);
        let msi = MSI_RANGE_HZ[((cr >> 4) & 0xF) as usize];
        let hse_sys = if cr & (1 << 20) != 0 { HSE_HZ / 2 } else { HSE_HZ };
        let pll = {
            let p = self.regs.get(PLLCFGR);
            let input = match p & 0b11 {
                1 => msi,
                2 => HSI_HZ,
                3 => HSE_HZ,
                _ => 0,
            };
            let m = ((p >> 4) & 0b111) + 1;
            let n = (p >> 8) & 0x7F;
            let r = ((p >> 29) & 0b111) + 1;
            (input as u64 / m as u64 * n as u64 / r as u64) as u32
        };
        let sysclk = match (cfgr >> 2) & 0b11 {
            0 => msi,
            1 => HSI_HZ,
            2 => hse_sys,
            _ => pll,
        };
        let hclk1 = sysclk / AHB_DIV[((cfgr >> 4) & 0xF) as usize];
        let pclk1 = hclk1 >> APB_SHIFT[((cfgr >> 8) & 7) as usize];
        let pclk2 = hclk1 >> APB_SHIFT[((cfgr >> 11) & 7) as usize];
        Clocks {
            sysclk,
            hclk1,
            pclk1,
            pclk2,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trace::digest::Digest;

    fn ctx(d: &mut Digest) -> Ctx<'_> {
        Ctx {
            now: 0,
            digest: d,
            pc: 0,
        }
    }

    #[test]
    fn reset_state_runs_on_msi_4mhz() {
        let r = Rcc::new();
        // CR reset value 0x61 (SVD): MSION, MSIRANGE = 6 (4 MHz, = SystemCoreClock at boot).
        // MSIRDY (bit 1) rises once MSI is stable; oscillators are ready at once here.
        assert_eq!(r.regs.get(CR), 0x63);
        assert_eq!(r.clocks().hclk1, 4_000_000);
    }

    #[test]
    fn furi_clock_setup_gives_64mhz() {
        let mut d = Digest::default();
        let mut r = Rcc::new();
        let mut c = ctx(&mut d);
        r.write(CR, 0x61 | 1 << 16 | 1 << 8, &mut c, 0);
        assert_ne!(r.regs.get(CR) & (1 << 17), 0, "HSERDY");
        // PLL from HSE: M = 2, N = 8, R = 2 → 32 MHz / 2 * 8 / 2 = 64 MHz.
        r.write(PLLCFGR, 3 | (1 << 4) | (8 << 8) | (1 << 29) | (1 << 28), &mut c, 0);
        r.write(CR, r.regs.get(CR) | 1 << 24, &mut c, 0);
        r.write(CFGR, 0b11, &mut c, 0);
        assert_eq!((r.regs.get(CFGR) >> 2) & 3, 3, "SWS follows SW");
        assert_eq!(r.clocks().hclk1, 64_000_000);
    }

    #[test]
    fn software_reset_sets_sftrstf_and_keeps_backup_domain() {
        let mut d = Digest::default();
        let mut r = Rcc::new();
        r.write(BDCR, 1, &mut ctx(&mut d), 0);
        r.reset(ResetKind::Software);
        assert_ne!(r.regs.get(CSR) & CSR_SFTRSTF, 0);
        assert_ne!(r.regs.get(BDCR) & 0b11, 0, "LSEON/LSERDY kept");
        r.write(CSR, 1 << 23, &mut ctx(&mut d), 0);
        assert_eq!(r.regs.get(CSR) & CSR_RESET_FLAGS, 0);
    }
}

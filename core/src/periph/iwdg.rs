// SPDX-License-Identifier: GPL-3.0-or-later
//! Independent watchdog (RM0434 "IWDG"; registers from the SVD). Counts down from RLR on LSI, at
//! 32 kHz over the prescaler in PR, and resets the system at zero. The time is a deadline in virtual
//! time, which the run loop compares; SR always reads 0 because register updates are instant.
//! The window (WINR) is latched but not enforced. Firmware 1.5.1 never starts it.

use super::regfile::RegFile;
use super::svd;

const KR: u32 = svd::IWDG.offset_of("KR");
const PR: u32 = svd::IWDG.offset_of("PR");
const RLR: u32 = svd::IWDG.offset_of("RLR");
const SR: u32 = svd::IWDG.offset_of("SR");
const WINR: u32 = svd::IWDG.offset_of("WINR");

const KEY_RELOAD: u32 = 0xAAAA;
const KEY_ENABLE: u32 = 0x5555;
const KEY_START: u32 = 0xCCCC;
const LSI_HZ: u128 = 32_000;

#[derive(Clone, Debug)]
pub struct Iwdg {
    pub regs: RegFile,
    /// PR, RLR and WINR are writable.
    unlocked: bool,
    /// Virtual time at which it resets the system; `None` until started.
    pub deadline_ns: Option<u128>,
}

impl Iwdg {
    pub fn new() -> Self {
        Iwdg {
            regs: RegFile::new(&svd::IWDG),
            unlocked: false,
            deadline_ns: None,
        }
    }

    pub fn reset(&mut self) {
        self.regs.reset();
        self.unlocked = false;
        self.deadline_ns = None;
    }

    fn period_ns(&self) -> u128 {
        let divider = 4u128 << self.regs.get(PR).min(6);
        (self.regs.get(RLR) as u128 + 1) * divider * 1_000_000_000 / LSI_HZ
    }

    pub fn read(&mut self, off: u32) -> Option<u32> {
        match off {
            SR => Some(0),
            _ => self.regs.read(off),
        }
    }

    pub fn write(&mut self, off: u32, v: u32, now_ns: u128) -> bool {
        match off {
            KR => {
                self.unlocked = false;
                match v & 0xFFFF {
                    KEY_ENABLE => self.unlocked = true,
                    KEY_START => self.deadline_ns = Some(now_ns + self.period_ns()),
                    KEY_RELOAD if self.deadline_ns.is_some() => self.deadline_ns = Some(now_ns + self.period_ns()),
                    _ => {}
                }
                true
            }
            PR | RLR | WINR => {
                if self.unlocked {
                    self.regs.write(off, v);
                }
                true
            }
            _ => self.regs.write(off, v),
        }
    }
}

impl Default for Iwdg {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_from_rlr_at_lsi_over_the_prescaler() {
        let mut w = Iwdg::new();
        w.write(RLR, 99, 0);
        assert_eq!(w.regs.get(RLR), 0xFFF, "locked until the key");
        assert_eq!(w.deadline_ns, None);
        w.write(KR, KEY_ENABLE, 0);
        w.write(PR, 3, 0); // /32
        w.write(RLR, 99, 0);
        w.write(KR, KEY_START, 1_000);
        // 100 counts at 1 kHz.
        assert_eq!(w.deadline_ns, Some(100_001_000));
        w.write(KR, KEY_RELOAD, 50_000_000);
        assert_eq!(w.deadline_ns, Some(150_000_000));
        w.write(PR, 0, 0);
        assert_eq!(w.regs.get(PR), 3, "any other key locks again");
    }

    #[test]
    fn reload_does_not_start_it() {
        let mut w = Iwdg::new();
        w.write(KR, KEY_RELOAD, 0);
        assert_eq!(w.deadline_ns, None);
    }
}

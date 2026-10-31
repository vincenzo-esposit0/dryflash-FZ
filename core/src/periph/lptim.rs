// SPDX-License-Identifier: GPL-3.0-or-later
//! Low-power timer LPTIM1/LPTIM2 (RM0434 "LPTIM"; registers and bits from the SVD).
//!
//! Furi's tickless idle runs on LPTIM1 clocked by LSE (targets/f7/furi_hal/
//! furi_hal_idle_timer.h): one-shot, CMP = count - 3, ARR = count, CMPM interrupt.
//! The counter is derived from virtual time; CMP/ARR matches are scheduled events.
//! Modelled: internal clock (CKSEL = 0) with prescaler, one-shot and continuous modes,
//! CMPM/ARRM/CMPOK/ARROK, interrupts, COUNTRST. External clock, triggers, encoder and
//! waveform output are not modelled.

use super::regfile::{PeriphDesc, RegFile};
use super::svd;

const ISR: u32 = svd::LPTIM1.offset_of("ISR");
const ICR: u32 = svd::LPTIM1.offset_of("ICR");
const IER: u32 = svd::LPTIM1.offset_of("IER");
const CFGR: u32 = svd::LPTIM1.offset_of("CFGR");
const CR: u32 = svd::LPTIM1.offset_of("CR");
const CMP: u32 = svd::LPTIM1.offset_of("CMP");
const ARR: u32 = svd::LPTIM1.offset_of("ARR");
const CNT: u32 = svd::LPTIM1.offset_of("CNT");

const CMPM: u32 = 1 << 0;
const ARRM: u32 = 1 << 1;
const CMPOK: u32 = 1 << 3;
const ARROK: u32 = 1 << 4;

const CR_ENABLE: u32 = 1 << 0;
const CR_SNGSTRT: u32 = 1 << 1;
const CR_CNTSTRT: u32 = 1 << 2;
const CR_COUNTRST: u32 = 1 << 3;

const NS: u128 = 1_000_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Stopped,
    OneShot,
    Continuous,
}

#[derive(Clone, Debug)]
pub struct Lptim {
    pub regs: RegFile,
    mode: Mode,
    /// Virtual time (ns) at which the counter was 0 in the current period.
    start_ns: u128,
    /// Kernel clock in Hz (from RCC CCIPR), set by the owner.
    pub clock_hz: u32,
    /// CMP already matched in the current period.
    cmp_done: bool,
}

impl Lptim {
    pub fn new(desc: &'static PeriphDesc) -> Self {
        Lptim {
            regs: RegFile::new(desc),
            mode: Mode::Stopped,
            start_ns: 0,
            clock_hz: 32_768,
            cmp_done: false,
        }
    }

    pub fn reset(&mut self) {
        self.regs.reset();
        self.mode = Mode::Stopped;
        self.cmp_done = false;
    }

    pub fn irq(&self) -> bool {
        self.regs.get(ISR) & self.regs.get(IER) & 0x7F != 0
    }

    /// Counter ticks per second after the prescaler (CFGR.PRESC = 2^n).
    fn tick_hz(&self) -> u128 {
        let presc = 1u128 << ((self.regs.get(CFGR) >> 9) & 7);
        (self.clock_hz.max(1) as u128 / presc).max(1)
    }

    /// Time (ns) at which the counter reaches `count` in the current period.
    fn time_of(&self, count: u32) -> u128 {
        self.start_ns + (count as u128 * NS).div_ceil(self.tick_hz())
    }

    fn counter(&self, now_ns: u128) -> u32 {
        if self.mode == Mode::Stopped {
            return 0;
        }
        let ticks = (now_ns.saturating_sub(self.start_ns) * self.tick_hz() / NS) as u32;
        ticks.min(self.regs.get(ARR) & 0xFFFF)
    }

    /// Earliest pending match, in ns.
    pub fn next_event_ns(&self) -> Option<u128> {
        if self.mode == Mode::Stopped {
            return None;
        }
        let arr = self.regs.get(ARR) & 0xFFFF;
        let cmp = self.regs.get(CMP) & 0xFFFF;
        let t_arr = self.time_of(arr);
        if !self.cmp_done && cmp <= arr {
            Some(self.time_of(cmp).min(t_arr))
        } else {
            Some(t_arr)
        }
    }

    /// Processes matches up to `now_ns`.
    pub fn advance(&mut self, now_ns: u128) {
        while let Some(t) = self.next_event_ns() {
            if t > now_ns {
                break;
            }
            let arr = self.regs.get(ARR) & 0xFFFF;
            let cmp = self.regs.get(CMP) & 0xFFFF;
            if !self.cmp_done && cmp <= arr && self.time_of(cmp) == t {
                self.cmp_done = true;
                self.regs.set_bits(ISR, CMPM, true);
                if cmp != arr {
                    continue;
                }
            }
            // ARR match: one-shot stops, continuous starts a new period.
            self.regs.set_bits(ISR, ARRM, true);
            let period = self.time_of(arr) - self.start_ns + NS / self.tick_hz();
            match self.mode {
                Mode::OneShot => self.mode = Mode::Stopped,
                _ => {
                    self.start_ns += period.max(1);
                    self.cmp_done = false;
                }
            }
        }
    }

    pub fn read(&mut self, off: u32, now_ns: u128) -> Option<u32> {
        if off == CNT {
            self.advance(now_ns);
            return Some(self.counter(now_ns));
        }
        self.regs.read(off)
    }

    pub fn write(&mut self, off: u32, v: u32, now_ns: u128) -> bool {
        self.advance(now_ns);
        match off {
            ICR => {
                let isr = self.regs.get(ISR);
                self.regs.set(ISR, isr & !(v & 0x7F));
                true
            }
            CR => {
                if v & CR_ENABLE == 0 {
                    self.regs.set(CR, 0);
                    self.mode = Mode::Stopped;
                    return true;
                }
                self.regs.set_bits(CR, CR_ENABLE, true);
                if v & (CR_SNGSTRT | CR_CNTSTRT | CR_COUNTRST) != 0 {
                    self.start_ns = now_ns;
                    self.cmp_done = false;
                }
                if v & CR_SNGSTRT != 0 {
                    self.mode = Mode::OneShot;
                } else if v & CR_CNTSTRT != 0 {
                    self.mode = Mode::Continuous;
                }
                true
            }
            CMP | ARR => {
                if self.regs.get(CR) & CR_ENABLE == 0 {
                    return true; // CMP/ARR can only be written while enabled.
                }
                self.regs.write(off, v);
                self.regs.set_bits(ISR, if off == CMP { CMPOK } else { ARROK }, true);
                true
            }
            _ => self.regs.write(off, v),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: u128 = 1_000_000;

    #[test]
    fn one_shot_as_furi_idle_timer() {
        let mut t = Lptim::new(&svd::LPTIM1);
        t.write(IER, 1, 0);
        t.write(CR, CR_ENABLE, 0);
        t.write(CMP, 32 - 3, 0);
        t.write(ARR, 32, 0);
        t.write(CR, CR_ENABLE | CR_SNGSTRT, 0);
        // 29 ticks of 32768 Hz ≈ 885 µs.
        let due = t.next_event_ns().unwrap();
        assert!((884_000..886_000).contains(&due), "{due}");
        t.advance(MS);
        assert!(t.irq(), "CMPM raises the interrupt");
        assert_eq!(t.regs.get(ISR) & (CMPM | ARRM), CMPM | ARRM);
        assert_eq!(t.next_event_ns(), None, "one-shot stopped");
        t.write(ICR, CMPM | ARRM, MS);
        assert!(!t.irq());
    }

    #[test]
    fn counter_follows_time() {
        let mut t = Lptim::new(&svd::LPTIM1);
        t.write(CR, CR_ENABLE, 0);
        t.write(ARR, 0xFFFF, 0);
        t.write(CR, CR_ENABLE | CR_CNTSTRT, 0);
        assert_eq!(t.read(CNT, NS / 2), Some(16384));
    }
}

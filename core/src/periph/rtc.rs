// SPDX-License-Identifier: GPL-3.0-or-later
//! Real-time clock (RM0434 "RTC"; registers and bits from the SVD).
//!
//! Lives in the backup domain: its registers, including BKP0R–BKP19R (where Furi records
//! crash data), survive system resets and are cleared only by power-on or BDRST.
//! Modelled: write protection (WPR keys 0xCA, 0x53), init mode (INIT → INITF, RSF, INITS),
//! and a BCD calendar that advances with virtual time. The calendar starts from the SVD
//! reset value (2000-01-01) unless the firmware sets it. Wakeup timer, alarms, timestamps
//! and smooth calibration are not modelled yet (register file only).

use super::regfile::RegFile;
use super::svd;

const TR: u32 = svd::RTC.offset_of("TR");
const DR: u32 = svd::RTC.offset_of("DR");
const CR: u32 = svd::RTC.offset_of("CR");
const ISR: u32 = svd::RTC.offset_of("ISR");
const PRER: u32 = svd::RTC.offset_of("PRER");
const WPR: u32 = svd::RTC.offset_of("WPR");
const SSR: u32 = svd::RTC.offset_of("SSR");
const BKP0R: u32 = svd::RTC.offset_of("BKP0R");
const BKP19R: u32 = svd::RTC.offset_of("BKP19R");

const ALRAWF: u32 = 1 << 0;
const ALRBWF: u32 = 1 << 1;
const WUTWF: u32 = 1 << 2;
const INITS: u32 = 1 << 4;
const RSF: u32 = 1 << 5;
const INITF: u32 = 1 << 6;
const INIT: u32 = 1 << 7;

#[derive(Clone, Debug)]
pub struct Rtc {
    pub regs: RegFile,
    /// WPR unlock sequence progress: 0 locked, 1 got 0xCA, 2 unlocked.
    unlock: u8,
    /// Calendar seconds since 2000-01-01 00:00:00 at `base_ns`.
    base_seconds: i64,
    base_ns: u128,
    running: bool,
}

impl Default for Rtc {
    fn default() -> Self {
        Self::new()
    }
}

fn bcd(v: u32) -> u32 {
    (v / 10) << 4 | (v % 10)
}

fn from_bcd(v: u32) -> u32 {
    (v >> 4) * 10 + (v & 0xF)
}

/// Days from civil date (proleptic Gregorian), years 2000–2099.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + if m <= 2 { 1 } else { 0 }, m, d)
}

const EPOCH_2000: i64 = 10957; // days_from_civil(2000, 1, 1)

impl Rtc {
    pub fn new() -> Self {
        let mut r = Rtc {
            regs: RegFile::new(&svd::RTC),
            unlock: 0,
            base_seconds: 0,
            base_ns: 0,
            running: true,
        };
        r.load_calendar(0);
        r
    }

    /// Backup-domain reset (power-on or RCC_BDCR.BDRST).
    pub fn reset(&mut self, now_ns: u128) {
        *self = Rtc::new();
        self.base_ns = now_ns;
    }

    fn unlocked(&self) -> bool {
        self.unlock == 2
    }

    /// Takes TR/DR as the calendar value at `now_ns`.
    fn load_calendar(&mut self, now_ns: u128) {
        let tr = self.regs.get(TR);
        let dr = self.regs.get(DR);
        let (h, m, s) = (
            from_bcd((tr >> 16) & 0x3F),
            from_bcd((tr >> 8) & 0x7F),
            from_bcd(tr & 0x7F),
        );
        let (y, mo, d) = (
            2000 + from_bcd((dr >> 16) & 0xFF),
            from_bcd((dr >> 8) & 0x1F),
            from_bcd(dr & 0x3F),
        );
        let days = days_from_civil(y as i64, mo.max(1) as i64, d.max(1) as i64) - EPOCH_2000;
        self.base_seconds = days * 86400 + (h * 3600 + m * 60 + s) as i64;
        self.base_ns = now_ns;
    }

    /// Writes the running calendar into TR/DR/SSR (the shadow registers).
    fn sync_shadow(&mut self, now_ns: u128) {
        if !self.running {
            return;
        }
        let elapsed_ns = now_ns.saturating_sub(self.base_ns);
        let total = self.base_seconds + (elapsed_ns / 1_000_000_000) as i64;
        let frac = (elapsed_ns % 1_000_000_000) as u64;
        let days = total.div_euclid(86400);
        let secs = total.rem_euclid(86400) as u32;
        let (y, m, d) = civil_from_days(days + EPOCH_2000);
        // Weekday: 2000-01-01 was a Saturday; RTC WDU 1 = Monday … 7 = Sunday.
        let wdu = ((days + 5).rem_euclid(7) + 1) as u32;
        let tr = bcd(secs / 3600) << 16 | bcd(secs / 60 % 60) << 8 | bcd(secs % 60);
        let dr = bcd((y - 2000).clamp(0, 99) as u32) << 16 | wdu << 13 | bcd(m) << 8 | bcd(d);
        self.regs.set(TR, tr);
        self.regs.set(DR, dr);
        // SSR counts down from PREDIV_S within each second.
        let prediv_s = self.regs.get(PRER) & 0x7FFF;
        let ss = prediv_s - ((frac * (prediv_s as u64 + 1)) / 1_000_000_000) as u32;
        self.regs.set(SSR, ss);
    }

    fn isr(&self) -> u32 {
        let cr = self.regs.get(CR);
        let mut v = self.regs.get(ISR) & !(ALRAWF | ALRBWF | WUTWF | INITS | INITF);
        if cr & (1 << 8) == 0 {
            v |= ALRAWF;
        }
        if cr & (1 << 9) == 0 {
            v |= ALRBWF;
        }
        if cr & (1 << 10) == 0 {
            v |= WUTWF;
        }
        if v & INIT != 0 {
            v |= INITF;
        }
        // INITS: the calendar has been initialised (year field not zero).
        if self.regs.get(DR) & 0x00FF_0000 != 0 {
            v |= INITS;
        }
        v
    }

    /// Backup register BKPnR.
    pub fn backup(&self, n: usize) -> u32 {
        self.regs.get(BKP0R + 4 * n as u32)
    }

    /// Sets backup register BKPnR (virtual-device state kept by the backup battery).
    pub fn set_backup(&mut self, n: usize, v: u32) {
        self.regs.set(BKP0R + 4 * n as u32, v);
    }

    pub fn read(&mut self, off: u32, now_ns: u128) -> Option<u32> {
        match off {
            TR | DR | SSR => {
                self.sync_shadow(now_ns);
                self.regs.read(off)
            }
            ISR => Some(self.isr()),
            _ => self.regs.read(off),
        }
    }

    pub fn write(&mut self, off: u32, v: u32, now_ns: u128) -> bool {
        if off == WPR {
            self.unlock = match (self.unlock, v & 0xFF) {
                (0, 0xCA) => 1,
                (1, 0x53) => 2,
                _ => 0,
            };
            return true;
        }
        if (BKP0R..=BKP19R).contains(&off) {
            return self.regs.write(off, v);
        }
        if !self.unlocked() {
            // Write-protected registers ignore writes until the WPR sequence.
            return self.regs.idx(off).is_some();
        }
        match off {
            ISR => {
                let old = self.regs.get(ISR);
                let entering = v & INIT != 0 && old & INIT == 0;
                let leaving = v & INIT == 0 && old & INIT != 0;
                if entering {
                    // Freeze the calendar so TR/DR can be programmed.
                    self.sync_shadow(now_ns);
                    self.running = false;
                }
                // INIT is writable; RSF and the event flags are cleared by writing 0.
                let keep_clear = 0x0003_FF20 & v;
                let flags = (old & 0x0003_FF20) & (keep_clear | !0x0003_FF20);
                self.regs.set(ISR, (old & !(INIT | 0x0003_FF20)) | (v & INIT) | flags);
                if leaving {
                    self.load_calendar(now_ns);
                    self.running = true;
                }
                // Shadow registers resynchronise at once: RSF set again.
                self.regs.set_bits(ISR, RSF, true);
                true
            }
            TR | DR => {
                if self.regs.get(ISR) & INIT == 0 {
                    return true; // Calendar registers need init mode.
                }
                self.regs.write(off, v)
            }
            _ => self.regs.write(off, v),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEC: u128 = 1_000_000_000;

    #[test]
    fn calendar_runs_and_can_be_set() {
        let mut r = Rtc::new();
        // 2000-01-01 00:00:00 at reset; 61 s later.
        assert_eq!(r.read(TR, 61 * SEC), Some(0x0000_0101));
        r.write(WPR, 0xCA, 0);
        r.write(WPR, 0x53, 0);
        r.write(ISR, INIT, 100 * SEC);
        assert_ne!(r.read(ISR, 100 * SEC).unwrap() & INITF, 0);
        r.write(TR, 0x0023_5958, 100 * SEC); // 23:59:58
        r.write(DR, 0x0024_C229, 100 * SEC); // 2024-02-29 (Tuesday in WDU)
        r.write(ISR, 0, 100 * SEC);
        // Three seconds later it is 2024-03-01 00:00:01.
        assert_eq!(r.read(TR, 103 * SEC), Some(0x0000_0001));
        assert_eq!(r.read(DR, 103 * SEC).unwrap() & 0x00FF_1F3F, 0x0024_0301);
    }

    #[test]
    fn writes_need_unlock() {
        let mut r = Rtc::new();
        r.write(ISR, INIT, 0);
        assert_eq!(r.read(ISR, 0).unwrap() & INITF, 0);
    }
}

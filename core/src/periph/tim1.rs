// SPDX-License-Identifier: GPL-3.0-or-later
//! TIM1, the infrared transmitter's timer (targets/f7/furi_hal/furi_hal_infrared.c, firmware 1.5.1).
//!
//! The firmware puts the IR carrier on channel 3N (PB9, AF1) in PWM mode 2 and lets two DMA2
//! channels, both requested by the update event (DMAMUX `LL_DMAMUX_REQ_TIM1_UP`), write CCMR2 (carrier
//! on or forced inactive) and RCR (how many carrier periods until the next update) at each update.
//! A mark or space therefore lasts RCR + 1 periods of the counter.
//!
//! Modelled (RM0434 "Advanced-control timer (TIM1)"): the upcounter's update events from PSC, ARR
//! and the repetition counter, with ARR, PSC and RCR taking effect at an update event as their
//! preload does in the chip; EGR.UG; SR.UIF (rc_w0); the update DMA request (DIER.UDE), served by
//! the bus. Not modelled: CNT as read by software (it reads its last written value), other count
//! modes, slave modes, capture/compare events and every TIM1 interrupt.

use super::regfile::{PeriphDesc, RegFile};
use super::svd;

const CR1: u32 = svd::TIM1.offset_of("CR1");
const DIER: u32 = svd::TIM1.offset_of("DIER");
const SR: u32 = svd::TIM1.offset_of("SR");
const EGR: u32 = svd::TIM1.offset_of("EGR");
pub const CCMR2: u32 = svd::TIM1.offset_of("CCMR2_Output");
const CCER: u32 = svd::TIM1.offset_of("CCER");
const PSC: u32 = svd::TIM1.offset_of("PSC");
const ARR: u32 = svd::TIM1.offset_of("ARR");
pub const RCR: u32 = svd::TIM1.offset_of("RCR");
const CCR3: u32 = svd::TIM1.offset_of("CCR3");
const BDTR: u32 = svd::TIM1.offset_of("BDTR");

const CR1_CEN: u32 = 1 << 0;
const SR_UIF: u32 = 1 << 0;
const EGR_UG: u32 = 1 << 0;
const DIER_UDE: u32 = 1 << 8;
const CCER_CC3NE: u32 = 1 << 10;
const BDTR_MOE: u32 = 1 << 15;

const NS: u128 = 1_000_000_000;

/// What channel 3N drives: nothing, the carrier (a PWM wave, as a fraction of each period), or a
/// constant active level.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Output {
    Off,
    Carrier { hz: f32, duty: f32 },
    On,
}

#[derive(Clone, Debug)]
pub struct Tim1 {
    pub regs: RegFile,
    /// Timer kernel clock (Hz); 0 while the clock is gated.
    pub clock_hz: u32,
    /// Shadow registers, loaded at update events.
    psc: u32,
    arr: u32,
    rep: u32,
    /// Counting since `start_ns`; the next update comes `next_ticks` kernel ticks after it.
    start_ns: u128,
    next_ticks: u64,
    running: bool,
    /// Update DMA requests not yet served.
    pub dma_requests: u32,
}

impl Tim1 {
    pub fn new(desc: &'static PeriphDesc) -> Self {
        Tim1 {
            regs: RegFile::new(desc),
            clock_hz: 0,
            psc: 0,
            arr: 0xFFFF,
            rep: 0,
            start_ns: 0,
            next_ticks: 0,
            running: false,
            dma_requests: 0,
        }
    }

    pub fn reset(&mut self) {
        let clock_hz = self.clock_hz;
        *self = Self::new(self.regs.desc);
        self.clock_hz = clock_hz;
    }

    pub fn read(&mut self, off: u32) -> Option<u32> {
        self.regs.read(off)
    }

    pub fn write(&mut self, off: u32, v: u32, now_ns: u128) -> bool {
        match off {
            // rc_w0: writing 1 leaves a flag as it is.
            SR => {
                let sr = self.regs.get(SR);
                self.regs.set(SR, sr & v);
                true
            }
            EGR => {
                if v & EGR_UG != 0 {
                    // The counter restarts with the update (RM0434 TIM1 "UG").
                    self.update(now_ns);
                }
                true
            }
            CR1 => {
                let was = self.regs.get(CR1) & CR1_CEN != 0;
                let h = self.regs.write(off, v);
                let on = self.regs.get(CR1) & CR1_CEN != 0;
                if on && !was {
                    self.start(now_ns);
                } else if !on {
                    self.running = false;
                }
                h
            }
            _ => self.regs.write(off, v),
        }
    }

    /// Kernel ticks per counter period with the shadow registers.
    fn period_ticks(&self) -> u64 {
        (self.psc as u64 + 1) * (self.arr as u64 + 1)
    }

    fn start(&mut self, now_ns: u128) {
        self.running = self.clock_hz > 0;
        self.start_ns = now_ns;
        self.next_ticks = self.period_ticks() * (self.rep as u64 + 1);
    }

    /// An update event: shadows reloaded, UIF set, a DMA request if enabled; the counter starts again.
    fn update(&mut self, now_ns: u128) {
        self.psc = self.regs.get(PSC) & 0xFFFF;
        self.arr = self.regs.get(ARR) & 0xFFFF;
        self.rep = self.regs.get(RCR) & 0xFFFF;
        self.regs.set_bits(SR, SR_UIF, true);
        if self.regs.get(DIER) & DIER_UDE != 0 {
            self.dma_requests += 1;
        }
        if self.running {
            self.start(now_ns);
        }
    }

    pub fn running(&self) -> bool {
        self.running
    }

    pub fn next_event_ns(&self) -> Option<u128> {
        self.running
            .then(|| self.start_ns + (self.next_ticks as u128 * NS).div_ceil(self.clock_hz.max(1) as u128))
    }

    /// Runs the next update event if it is due by `now_ns`, and returns its time. One at a time:
    /// the DMA it requests rewrites RCR, which sets the time of the following one.
    pub fn step(&mut self, now_ns: u128) -> Option<u128> {
        let t = self.next_event_ns().filter(|&t| t <= now_ns)?;
        self.update(t);
        Some(t)
    }

    /// Channel 3N as the firmware's IR driver sets it up: complementary output enabled, main
    /// output enabled, counter running, and OC3M PWM mode 2 (mark) or forced inactive (space).
    pub fn ch3n(&self) -> Output {
        let r = |off| self.regs.get(off);
        if !self.running || r(BDTR) & BDTR_MOE == 0 || r(CCER) & CCER_CC3NE == 0 {
            return Output::Off;
        }
        // OC3M is bits 6:4 plus bit 16 of CCMR2.
        let ccmr2 = r(CCMR2);
        let mode = ((ccmr2 >> 4) & 7) | ((ccmr2 >> 13) & 8);
        let period = self.arr as f64 + 1.0;
        let active = (r(CCR3) & 0xFFFF).min(self.arr + 1) as f64 / period;
        let duty = match mode {
            0b0110 => active,       // PWM mode 1
            0b0111 => 1.0 - active, // PWM mode 2
            0b0101 => return Output::On,
            _ => return Output::Off,
        };
        if duty <= 0.0 {
            Output::Off
        } else if duty >= 1.0 {
            Output::On
        } else {
            let hz = self.clock_hz as f64 / (self.psc as f64 + 1.0) / period;
            Output::Carrier {
                hz: hz as f32,
                duty: duty as f32,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLK: u32 = 64_000_000;

    /// furi_hal_infrared_configure_tim_pwm_tx(38 kHz, 0.33) and the start sequence, without DMA.
    fn carrier(t: &mut Tim1) -> u32 {
        t.clock_hz = CLK;
        let arr = CLK / 38_000 - 1;
        t.write(PSC, 0, 0);
        t.write(ARR, arr, 0);
        t.write(CCR3, ((arr + 1) as f32 * (1.0 - 0.33)) as u32, 0);
        t.write(CCER, CCER_CC3NE, 0);
        t.write(BDTR, BDTR_MOE, 0);
        t.write(DIER, DIER_UDE, 0);
        arr
    }

    #[test]
    fn updates_come_every_rcr_plus_one_periods() {
        let mut t = Tim1::new(&svd::TIM1);
        let arr = carrier(&mut t);
        t.write(RCR, 9, 0);
        t.write(EGR, EGR_UG, 0); // RCR → repetition counter
        assert_eq!(t.dma_requests, 1);
        t.write(RCR, 4, 0); // as the DMA would: takes effect at the next update
        t.write(CR1, CR1_CEN, 1000);
        let periods = |n: u128| (n * (arr as u128 + 1) * NS).div_ceil(CLK as u128);
        let first = t.next_event_ns().unwrap();
        assert_eq!(first - 1000, periods(10));
        assert_eq!(t.step(first - 1), None);
        assert_eq!(t.step(first), Some(first));
        let second = t.next_event_ns().unwrap();
        assert!((second - first).abs_diff(periods(5)) <= 1);
        assert_eq!(t.dma_requests, 2);
        t.write(SR, !SR_UIF, 0);
        assert_eq!(t.regs.get(SR) & SR_UIF, 0);
        t.write(CR1, 0, 0);
        assert_eq!(t.next_event_ns(), None);
    }

    #[test]
    fn ch3n_carries_in_pwm2_and_is_off_when_forced_inactive() {
        let mut t = Tim1::new(&svd::TIM1);
        carrier(&mut t);
        t.write(CCMR2, (1 << 3) | (0b111 << 4), 0); // INFRARED_TX_CCMR_HIGH
        assert_eq!(t.ch3n(), Output::Off, "not counting yet");
        t.write(EGR, EGR_UG, 0);
        t.write(CR1, CR1_CEN, 0);
        let Output::Carrier { hz, duty } = t.ch3n() else {
            panic!("{:?}", t.ch3n())
        };
        assert!(
            (hz - 38_000.0).abs() < 10.0 && (duty - 0.33).abs() < 0.01,
            "{hz} {duty}"
        );
        t.write(CCMR2, (1 << 3) | (0b100 << 4), 0); // INFRARED_TX_CCMR_LOW
        assert_eq!(t.ch3n(), Output::Off);
    }
}

// SPDX-License-Identifier: GPL-3.0-or-later
//! TIM16, the speaker's timer (targets/f7/furi_hal/furi_hal_speaker.c: channel 1 in PWM mode 1 on
//! PB8, prescaler 500, autoreload from the frequency, compare from the volume).
//!
//! Only what reaches the speaker is modelled: the tone that channel 1 puts out, as a frequency and a
//! duty cycle computed from the registers (RM0434 TIM16/TIM17: CR1.CEN, BDTR.MOE, CCER.CC1E,
//! CCMR1.OC1M, PSC, ARR, CCR1). The counter does not run, so CNT reads its last written value, and
//! there are no update or compare interrupts. PSC is taken as written: in the chip it is preloaded
//! until the next update event, which LL_TIM_Init generates at once (EGR.UG), so the difference is
//! at most one period.

use super::regfile::{PeriphDesc, RegFile};
use super::svd;

const CR1: u32 = svd::TIM16.offset_of("CR1");
const CCMR1: u32 = svd::TIM16.offset_of("CCMR1_Output");
const CCER: u32 = svd::TIM16.offset_of("CCER");
const PSC: u32 = svd::TIM16.offset_of("PSC");
const ARR: u32 = svd::TIM16.offset_of("ARR");
const CCR1: u32 = svd::TIM16.offset_of("CCR1");
const BDTR: u32 = svd::TIM16.offset_of("BDTR");

/// A square wave on the timer output.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tone {
    pub hz: f32,
    /// Fraction of each period the output is active, strictly between 0 and 1.
    pub duty: f32,
}

#[derive(Clone, Debug)]
pub struct Tim16 {
    pub regs: RegFile,
}

impl Tim16 {
    pub fn new(desc: &'static PeriphDesc) -> Self {
        Tim16 {
            regs: RegFile::new(desc),
        }
    }

    pub fn reset(&mut self) {
        self.regs.reset();
    }

    pub fn read(&mut self, off: u32) -> Option<u32> {
        self.regs.read(off)
    }

    pub fn write(&mut self, off: u32, v: u32) -> bool {
        self.regs.write(off, v)
    }

    /// The tone on channel 1 with the timer clocked at `timer_hz`, if the output toggles.
    pub fn tone(&self, timer_hz: u32) -> Option<Tone> {
        let r = |off| self.regs.get(off);
        let counting = r(CR1) & 1 != 0; // CEN
        let outputs = r(BDTR) & (1 << 15) != 0; // MOE
        let ch1 = r(CCER) & 1 != 0; // CC1E
        // OC1M is bits 6:4 plus bit 16: 0110 PWM mode 1, 0111 PWM mode 2.
        let ccmr1 = r(CCMR1);
        let mode = ((ccmr1 >> 4) & 7) | ((ccmr1 >> 13) & 8);
        if !(counting && outputs && ch1 && matches!(mode, 6 | 7)) || timer_hz == 0 {
            return None;
        }
        let period = r(ARR) as f64 + 1.0;
        let active = (r(CCR1) as f64).min(period) / period;
        let duty = if mode == 6 { active } else { 1.0 - active };
        if duty <= 0.0 || duty >= 1.0 {
            return None; // a constant level: no sound
        }
        let hz = timer_hz as f64 / (r(PSC) as f64 + 1.0) / period;
        Some(Tone {
            hz: hz as f32,
            duty: duty as f32,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What furi_hal_speaker_start(523.25 Hz, volume 1.0) writes at 64 MHz.
    fn speaker_c5(t: &mut Tim16) {
        let arr = (64_000_000.0f32 / 500.0 / 523.25) as u32 - 1;
        t.write(PSC, 499);
        t.write(ARR, arr);
        t.write(CCMR1, 6 << 4);
        t.write(CCER, 1);
        t.write(CCR1, arr / 2);
        t.write(BDTR, 1 << 15);
        t.write(CR1, 1);
    }

    #[test]
    fn speaker_start_gives_the_note() {
        let mut t = Tim16::new(&svd::TIM16);
        assert_eq!(t.tone(64_000_000), None);
        speaker_c5(&mut t);
        let tone = t.tone(64_000_000).unwrap();
        assert!((tone.hz - 523.25).abs() < 2.0, "{tone:?}");
        assert!((tone.duty - 0.5).abs() < 0.01, "{tone:?}");
    }

    #[test]
    fn stop_or_constant_output_is_silent() {
        let mut t = Tim16::new(&svd::TIM16);
        speaker_c5(&mut t);
        // furi_hal_speaker_stop: outputs off, counter off.
        t.write(BDTR, 0);
        assert_eq!(t.tone(64_000_000), None);
        t.write(BDTR, 1 << 15);
        t.write(CCR1, 0);
        assert_eq!(t.tone(64_000_000), None);
        t.write(CCR1, 0xFFFF);
        assert_eq!(t.tone(64_000_000), None);
    }
}

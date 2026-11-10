// SPDX-License-Identifier: GPL-3.0-or-later
//! The IR LED on PB9 (`gpio_infrared_tx`, targets/f7/furi_hal/furi_hal_resources.c), driven by
//! TIM1 channel 3N through alternate function 1 (furi_hal_infrared.c `furi_hal_infrared_async_tx_start`).
//!
//! What the LED sends is turned back into what the firmware was asked to send: the carrier and
//! alternating mark and space durations, the form of a raw signal in a Flipper `.ir` file. A
//! transmission ends when the timer stops (`furi_hal_infrared_tx_dma_terminate`); the space after
//! the last mark, which the firmware sends only to finish its buffer, is not part of it.
//!
//! The external IR module output (PA7, TIM1 channel 1N) is not watched: the board has no module on
//! PA7, so `furi_hal_infrared_detect_tx_output` keeps the internal LED.

use std::collections::VecDeque;

use crate::periph::tim1::Output;

/// Transmissions kept for a host that does not collect them: the oldest go first.
const MAX_SIGNALS: usize = 64;
/// A transmission longer than this is handed on in parts, so memory stays bounded.
const MAX_TIMINGS: usize = 4096;

#[derive(Clone, Debug, PartialEq)]
pub struct IrSignal {
    /// Virtual time of the first mark.
    pub at_ns: u128,
    /// Carrier of the first mark; 0 if the LED was driven constantly on.
    pub hz: f32,
    pub duty: f32,
    /// Mark, space, mark, … in µs; always an odd count (it ends with a mark).
    pub timings_us: Vec<u32>,
}

#[derive(Debug, Default)]
pub struct Infrared {
    /// LED lit (carrier or constant) since `since_ns`.
    lit: bool,
    since_ns: u128,
    current: Option<IrSignal>,
    done: VecDeque<IrSignal>,
}

fn us(from: u128, to: u128) -> u32 {
    (to.saturating_sub(from).div_ceil(500) / 2).min(u32::MAX as u128) as u32
}

impl Infrared {
    /// The LED output at `at_ns`; `running` is false once the timer has stopped.
    pub fn update(&mut self, at_ns: u128, out: Output, running: bool) {
        let lit = out != Output::Off;
        if lit != self.lit {
            let span = us(self.since_ns, at_ns);
            if let Some(s) = self.current.as_mut() {
                s.timings_us.push(span);
            } else if lit {
                let (hz, duty) = match out {
                    Output::Carrier { hz, duty } => (hz, duty),
                    _ => (0.0, 1.0),
                };
                self.current = Some(IrSignal {
                    at_ns,
                    hz,
                    duty,
                    timings_us: Vec::new(),
                });
            }
            self.lit = lit;
            self.since_ns = at_ns;
            if !lit && self.current.as_ref().is_some_and(|s| s.timings_us.len() >= MAX_TIMINGS) {
                self.finish();
            }
        }
        if !running && !lit {
            self.finish();
        }
    }

    fn finish(&mut self) {
        let Some(mut s) = self.current.take() else {
            return;
        };
        if s.timings_us.len() % 2 == 0 {
            s.timings_us.pop(); // the trailing space
        }
        if s.timings_us.is_empty() {
            return;
        }
        if self.done.len() == MAX_SIGNALS {
            self.done.pop_front();
        }
        self.done.push_back(s);
    }

    pub fn take(&mut self) -> Vec<IrSignal> {
        self.done.drain(..).collect()
    }

    /// Power gone: whatever was being sent is dropped.
    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const C: Output = Output::Carrier {
        hz: 38_000.0,
        duty: 0.33,
    };

    #[test]
    fn marks_and_spaces_until_the_timer_stops() {
        let mut ir = Infrared::default();
        ir.update(1_000, Output::Off, true);
        ir.update(10_000, C, true);
        ir.update(10_000 + 9_000_000, Output::Off, true);
        ir.update(10_000 + 13_500_000, C, true);
        ir.update(10_000 + 14_060_000, Output::Off, true);
        assert!(ir.take().is_empty(), "still sending");
        ir.update(10_000 + 20_000_000, Output::Off, false);
        let s = ir.take();
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].at_ns, 10_000);
        assert_eq!(s[0].hz, 38_000.0);
        assert_eq!(s[0].timings_us, vec![9000, 4500, 560]);
    }

    #[test]
    fn a_stop_without_marks_is_nothing() {
        let mut ir = Infrared::default();
        ir.update(5, Output::Off, true);
        ir.update(10, Output::Off, false);
        assert!(ir.take().is_empty());
    }
}

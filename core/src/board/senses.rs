// SPDX-License-Identifier: GPL-3.0-or-later
//! What the device makes heard and felt: the speaker on PB8 (TIM16 channel 1, alternate function
//! 14) and the vibration motor on PA8, a plain output (targets/f7/furi_hal/furi_hal_speaker.c,
//! furi_hal_vibro.c, furi_hal_resources.h).
//!
//! Changes are kept with their virtual time, so a host can play them back at the right moment
//! even though it only looks between slices of execution.

use std::collections::VecDeque;

use crate::periph::tim16::Tone;

/// Changes kept for a host that does not collect them (the headless runner): the oldest go first.
const MAX_EVENTS: usize = 1024;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Sense {
    /// The speaker plays this tone, or is silent with `None`.
    Speaker(Option<Tone>),
    Vibro(bool),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SenseEvent {
    pub at_ns: u128,
    pub sense: Sense,
}

#[derive(Debug, Default)]
pub struct Senses {
    pub tone: Option<Tone>,
    pub vibro: bool,
    events: VecDeque<SenseEvent>,
}

impl Senses {
    /// Records the outputs at `at_ns`; only changes are kept.
    pub fn update(&mut self, at_ns: u128, tone: Option<Tone>, vibro: bool) {
        if tone != self.tone {
            self.tone = tone;
            self.push(at_ns, Sense::Speaker(tone));
        }
        if vibro != self.vibro {
            self.vibro = vibro;
            self.push(at_ns, Sense::Vibro(vibro));
        }
    }

    fn push(&mut self, at_ns: u128, sense: Sense) {
        // A driver sets several registers in a row: only where it ends up at that instant matters.
        let same =
            |e: &SenseEvent| e.at_ns == at_ns && std::mem::discriminant(&e.sense) == std::mem::discriminant(&sense);
        if let Some(e) = self.events.iter_mut().rev().take(2).find(|e| same(e)) {
            e.sense = sense;
            return;
        }
        if self.events.len() == MAX_EVENTS {
            self.events.pop_front();
        }
        self.events.push_back(SenseEvent { at_ns, sense });
    }

    pub fn take(&mut self) -> Vec<SenseEvent> {
        self.events.drain(..).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: Tone = Tone { hz: 440.0, duty: 0.5 };

    #[test]
    fn keeps_changes_with_their_time() {
        let mut s = Senses::default();
        s.update(10, Some(A), false);
        s.update(20, Some(A), false);
        s.update(30, None, true);
        let e = s.take();
        assert_eq!(
            e,
            vec![
                SenseEvent {
                    at_ns: 10,
                    sense: Sense::Speaker(Some(A))
                },
                SenseEvent {
                    at_ns: 30,
                    sense: Sense::Speaker(None)
                },
                SenseEvent {
                    at_ns: 30,
                    sense: Sense::Vibro(true)
                },
            ]
        );
        assert!(s.take().is_empty());
    }

    #[test]
    fn steps_at_one_instant_collapse() {
        let mut s = Senses::default();
        s.update(10, Some(A), false);
        s.update(10, Some(Tone { hz: 880.0, duty: 0.5 }), false);
        assert_eq!(s.take().len(), 1);
    }

    #[test]
    fn bounded_when_nobody_collects() {
        let mut s = Senses::default();
        for i in 0..5000u128 {
            s.update(i, None, i % 2 == 0);
        }
        let e = s.take();
        assert_eq!(e.len(), MAX_EVENTS);
        assert_eq!(e.last().unwrap().at_ns, 4999);
    }
}

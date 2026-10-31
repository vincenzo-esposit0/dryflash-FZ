// SPDX-License-Identifier: GPL-3.0-or-later
//! TI LP5562 RGBW LED driver on I2C1 (7-bit address 0x30; `LP5562_ADDRESS` 0x60 in
//! lib/drivers/lp5562_reg.h). Red/green/blue drive the status LED; white drives the LCD
//! backlight (targets/f7/furi_hal/furi_hal_light.c).
//!
//! Registers: ENABLE 0x00, OP_MODE 0x01, B/G/R PWM 0x02–0x04, B/G/R current 0x05–0x07,
//! W PWM 0x0E, W current 0x0F, engine program memory 0x10/0x30/0x50 (16 words each),
//! LED_MAP 0x70 (source per channel: direct register or engine 1–3).
//! Engines run the program format the firmware builds (lib/drivers/lp5562.c): set PWM
//! (0x40nn), ramp/wait (prescale, step time, sign, increment), branch/loop (0xA000), end
//! (0xC000). They are evaluated lazily from virtual time when the output is read. Step
//! durations (0.49 ms / 15.6 ms per step unit) are the datasheet values as recalled, not
//! verified here: they only affect fade timing in the UI.

use crate::periph::i2c::I2cDevice;

#[derive(Clone, Copy, Debug, Default)]
struct Engine {
    pc: usize,
    pwm: u8,
    /// Ramp in progress: steps left, direction, ns per step.
    ramp_left: u32,
    ramp_up: bool,
    step_ns: u128,
    /// Virtual time at which the engine has been evaluated so far.
    t_ns: u128,
    loop_left: Option<u32>,
    running: bool,
}

#[derive(Debug)]
pub struct Lp5562 {
    regs: [u8; 0x80],
    ptr: u8,
    expect_ptr: bool,
    engines: [Engine; 3],
    /// Virtual time in ns, set by the owner before each bus access and evaluation.
    pub now_ns: u128,
}

impl Default for Lp5562 {
    fn default() -> Self {
        Self::new()
    }
}

/// Output levels (PWM 0–255) of the four channels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Lights {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
    /// LCD backlight.
    pub white: u8,
}

impl Lp5562 {
    pub fn new() -> Self {
        Lp5562 {
            regs: [0; 0x80],
            ptr: 0,
            expect_ptr: false,
            engines: [Engine::default(); 3],
            now_ns: 0,
        }
    }

    fn word(&self, eng: usize, i: usize) -> u16 {
        let base = 0x10 + 0x20 * eng + 2 * i;
        u16::from_be_bytes([self.regs[base], self.regs[base + 1]])
    }

    fn start_engine(&mut self, e: usize) {
        self.engines[e] = Engine {
            running: true,
            t_ns: self.now_ns,
            ..Engine::default()
        };
    }

    /// Runs engine `e` up to `now_ns`.
    fn run_engine(&mut self, e: usize) {
        let now = self.now_ns;
        for _ in 0..256 {
            let eng = self.engines[e];
            if !eng.running {
                return;
            }
            if eng.ramp_left > 0 {
                let due = eng.t_ns + eng.step_ns;
                if due > now {
                    return;
                }
                let en = &mut self.engines[e];
                en.pwm = if en.ramp_up {
                    en.pwm.saturating_add(1)
                } else {
                    en.pwm.saturating_sub(1)
                };
                en.ramp_left -= 1;
                en.t_ns = due;
                continue;
            }
            if eng.pc >= 16 {
                self.engines[e].running = false;
                return;
            }
            let w = self.word(e, eng.pc);
            let en = &mut self.engines[e];
            en.pc += 1;
            match w >> 13 {
                0b010 => en.pwm = w as u8, // set PWM
                0b101 => {
                    // Branch: loop count bits 12:7 (0 = forever), target step bits 3:0.
                    let count = ((w >> 7) & 0x3F) as u32;
                    let target = (w & 0xF) as usize;
                    match en.loop_left {
                        None if count == 0 => en.pc = target,
                        None => {
                            en.loop_left = Some(count);
                            en.pc = target;
                        }
                        Some(1) => en.loop_left = None,
                        Some(n) => {
                            en.loop_left = Some(n - 1);
                            en.pc = target;
                        }
                    }
                }
                0b110 => {
                    en.running = false; // end
                    return;
                }
                0b111 => {} // trigger: no other engines to synchronise with here
                _ if w & 0x8000 == 0 && w != 0 => {
                    // Ramp/wait: prescale bit 14, step time bits 13:8, sign bit 7, increment 6:0.
                    let unit_ns: u128 = if w & 0x4000 != 0 { 15_600_000 } else { 490_000 };
                    let step = ((w >> 8) & 0x3F) as u128 * unit_ns;
                    let inc = (w & 0x7F) as u32;
                    if inc == 0 {
                        en.t_ns += step; // wait
                        if en.t_ns > now {
                            return;
                        }
                    } else {
                        en.ramp_left = inc;
                        en.ramp_up = w & 0x80 == 0;
                        en.step_ns = step.max(1);
                    }
                }
                _ => en.pc = 0, // go-to-start (0x0000)
            }
        }
    }

    /// Channel levels now. LED_MAP selects the source of each channel.
    pub fn lights(&mut self) -> Lights {
        for e in 0..3 {
            self.run_engine(e);
        }
        let map = self.regs[0x70];
        let level = |src: u8, direct: u8, eng: &[Engine; 3]| -> u8 {
            match src {
                0 => direct,
                n => eng[(n - 1) as usize].pwm,
            }
        };
        let enabled = self.regs[0x00] & 0x40 != 0; // CHIP_EN
        let l = Lights {
            red: level((map >> 4) & 3, self.regs[0x04], &self.engines),
            green: level((map >> 2) & 3, self.regs[0x03], &self.engines),
            blue: level(map & 3, self.regs[0x02], &self.engines),
            white: level((map >> 6) & 3, self.regs[0x0E], &self.engines),
        };
        if enabled { l } else { Lights::default() }
    }

    fn write_reg(&mut self, r: u8, v: u8) {
        if r == 0x0D && v == 0xFF {
            *self = Lp5562 {
                now_ns: self.now_ns,
                ..Lp5562::new()
            }; // RESET register
            return;
        }
        let old_op = self.regs[0x01];
        self.regs[r as usize & 0x7F] = v;
        if r == 0x01 || r == 0x00 {
            // OP_MODE/ENABLE engine fields: 10 = run (engine 1 in bits 5:4).
            for e in 0..3 {
                let shift = (2 - e) * 2;
                let run_op = (self.regs[0x01] >> shift) & 3 == 0b10;
                let was = (old_op >> shift) & 3 == 0b10;
                if run_op && !was {
                    self.start_engine(e);
                }
                if !run_op && r == 0x01 {
                    self.engines[e].running = false;
                }
            }
        }
    }
}

impl I2cDevice for Lp5562 {
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn set_time(&mut self, now_ns: u128) {
        self.now_ns = now_ns;
    }

    fn address(&self) -> u8 {
        0x30
    }

    fn start(&mut self, read: bool) -> bool {
        self.expect_ptr = !read;
        true
    }

    fn write(&mut self, byte: u8) -> bool {
        if self.expect_ptr {
            self.ptr = byte;
            self.expect_ptr = false;
        } else {
            self.write_reg(self.ptr, byte);
            self.ptr = self.ptr.wrapping_add(1);
        }
        true
    }

    fn read(&mut self) -> u8 {
        let v = self.regs[self.ptr as usize & 0x7F];
        self.ptr = self.ptr.wrapping_add(1);
        v
    }
}

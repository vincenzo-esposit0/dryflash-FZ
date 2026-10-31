// SPDX-License-Identifier: GPL-3.0-or-later
//! wasm-bindgen bindings for `dryflash-core` (ARCHITECTURE §8): the API the Web Worker and the
//! Node runner drive. Values cross the boundary as numbers, byte arrays and JSON strings
//! (no serde dependency); 64-bit counters are returned as `f64` (exact below 2^53).

use dryflash_core::board::Button;
use dryflash_core::bus::Fidelity;
use dryflash_core::loader::dfu;
use dryflash_core::machine::{Machine, RunExit};
use wasm_bindgen::prelude::*;

pub use dryflash_core::VERSION;

#[wasm_bindgen]
pub fn version() -> String {
    VERSION.to_string()
}

/// Why `Emulator::run` returned, as a small integer for the worker.
#[wasm_bindgen]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exit {
    Budget = 0,
    IdleForever = 1,
    Stopped = 2,
    BootLoop = 3,
}

#[wasm_bindgen]
pub struct Emulator {
    m: Machine,
    image: Option<dfu::Dfu>,
    cpu2_start: Option<u32>,
    /// USART1 bytes already handed to the host.
    log_read: usize,
    /// Description of the last non-budget exit.
    last_stop: String,
}

impl Default for Emulator {
    fn default() -> Self {
        Self::new()
    }
}

fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[wasm_bindgen]
impl Emulator {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Emulator {
        Emulator {
            m: Machine::new(),
            image: None,
            cpu2_start: None,
            log_read: 0,
            last_stop: String::new(),
        }
    }

    /// Parses and installs a DfuSe image. `cpu2_start` is the first flash address reserved for
    /// CPU2 (`Radio address` of the update package), 0 if unknown. Returns an error message.
    pub fn load_dfu(&mut self, bytes: &[u8], cpu2_start: u32) -> Result<(), String> {
        let image = dfu::parse(bytes).map_err(|e| format!("not a valid DfuSe file: {e}"))?;
        let cpu2 = (cpu2_start != 0).then_some(cpu2_start);
        self.m = Machine::new();
        self.m.load_dfu(&image, cpu2).map_err(|e| e.to_string())?;
        self.image = Some(image);
        self.cpu2_start = cpu2;
        Ok(())
    }

    /// Virtual-device setting Sleep Method = Legacy (ADR-0012); applies at the next power-on.
    pub fn set_legacy_sleep(&mut self, on: bool) {
        self.m.bus.legacy_sleep = on;
    }

    pub fn legacy_sleep(&self) -> bool {
        self.m.bus.legacy_sleep
    }

    /// Power-on reset: a fresh device with the loaded firmware (RAM and backup domain cleared).
    pub fn power_on(&mut self) -> Result<(), String> {
        let image = self.image.as_ref().ok_or("no firmware loaded")?;
        let legacy = self.m.bus.legacy_sleep;
        let mut m = Machine::new();
        m.bus.legacy_sleep = legacy;
        m.load_dfu(image, self.cpu2_start).map_err(|e| e.to_string())?;
        m.power_on();
        self.m = m;
        self.log_read = 0;
        self.last_stop.clear();
        Ok(())
    }

    /// Runs up to `cycles` of virtual time (HCLK1 cycles).
    pub fn run(&mut self, cycles: f64) -> Exit {
        match self.m.run(cycles as u64) {
            RunExit::Budget => Exit::Budget,
            RunExit::IdleForever { pc } => {
                self.last_stop = format!("CPU asleep with nothing scheduled to wake it (pc 0x{pc:08X})");
                Exit::IdleForever
            }
            RunExit::Stopped(s) => {
                self.last_stop = format!("{s:?}");
                Exit::Stopped
            }
            RunExit::BootLoop { resets } => {
                self.last_stop = format!("boot loop: {resets} resets");
                Exit::BootLoop
            }
        }
    }

    pub fn last_stop(&self) -> String {
        self.last_stop.clone()
    }

    /// Button index: 0 Up, 1 Down, 2 Left, 3 Right, 4 OK, 5 Back.
    pub fn set_button(&mut self, button: u8, pressed: bool) {
        if let Some(&b) = Button::ALL.get(button as usize) {
            self.m.set_button(b, pressed);
        }
    }

    /// The 128×64 LCD, 1 bit per pixel (1 = dark), rows top to bottom, MSB = leftmost: 1 KiB.
    pub fn frame(&self) -> Vec<u8> {
        let px = self.m.display_frame();
        let mut out = vec![0u8; 1024];
        for (i, &p) in px.iter().enumerate() {
            if p != 0 {
                out[i / 8] |= 0x80 >> (i % 8);
            }
        }
        out
    }

    /// Changes whenever the LCD content may have changed.
    pub fn display_generation(&self) -> f64 {
        self.m.display_generation() as f64
    }

    /// LED levels `[red, green, blue, backlight]`, 0–255.
    pub fn lights(&mut self) -> Vec<u8> {
        let l = self.m.lights();
        vec![l.red, l.green, l.blue, l.white]
    }

    pub fn cycles(&self) -> f64 {
        self.m.cpu.cycles as f64
    }

    pub fn instructions(&self) -> f64 {
        self.m.cpu.instret as f64
    }

    /// Virtual time since power-on in milliseconds.
    pub fn virtual_ms(&self) -> f64 {
        self.m.bus.periphs.now_ns(self.m.cpu.cycles) as f64 / 1e6
    }

    /// Current HCLK1 frequency (cycles per virtual second).
    pub fn hclk_hz(&self) -> f64 {
        self.m.bus.periphs.clocks.hclk1 as f64
    }

    pub fn resets(&self) -> u32 {
        self.m.resets
    }

    /// USART1 bytes written since the previous call (the firmware's log).
    pub fn take_log(&mut self) -> Vec<u8> {
        let tx = &self.m.bus.periphs.usart[0].tx;
        let new = tx[self.log_read.min(tx.len())..].to_vec();
        self.log_read = tx.len();
        new
    }

    /// Hardware fidelity table as JSON: `[{"name", "level", "note"}]`.
    pub fn fidelity_json(&self) -> String {
        let rows: Vec<String> = self
            .m
            .bus
            .periphs
            .fidelity()
            .into_iter()
            .map(|(name, level, note)| {
                let level = match level {
                    Fidelity::Emulated => "emulated",
                    Fidelity::Partial => "partial",
                    Fidelity::Simulated => "simulated",
                    Fidelity::Stubbed => "stubbed",
                };
                format!(
                    "{{\"name\":{},\"level\":\"{level}\",\"note\":{}}}",
                    json_str(name),
                    json_str(note)
                )
            })
            .collect();
        format!("[{}]", rows.join(","))
    }

    /// Firmware crashes decoded from Furi's records, as JSON `[{"atMs", "message"}]`.
    pub fn crashes_json(&self) -> String {
        let rows: Vec<String> = self
            .m
            .crashes
            .iter()
            .map(|c| {
                let msg = c
                    .message
                    .clone()
                    .unwrap_or_else(|| format!("message at 0x{:08X}", c.message_ptr));
                format!("{{\"atMs\":{},\"message\":{}}}", c.at_ns / 1_000_000, json_str(&msg))
            })
            .collect();
        format!("[{}]", rows.join(","))
    }

    /// Bring-up digest (unimplemented/latched registers, events) as text.
    pub fn digest_text(&self) -> String {
        self.m.digest().render(&|_| None, 12)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_escaping() {
        assert_eq!(json_str("a\"b\\c\n"), "\"a\\\"b\\\\c\\u000a\"");
    }

    #[test]
    fn fidelity_is_valid_json_shape() {
        let e = Emulator::new();
        let j = e.fidelity_json();
        assert!(j.starts_with("[{\"name\":") && j.ends_with("}]"));
        assert!(j.contains("\"level\":\"stubbed\""));
    }

    #[test]
    fn power_on_without_firmware_is_an_error() {
        assert!(Emulator::new().power_on().is_err());
    }
}

// SPDX-License-Identifier: GPL-3.0-or-later
//! wasm-bindgen bindings for `dryflash-core` (ARCHITECTURE §8): the API the Web Worker and the
//! Node runner drive. Values cross the boundary as numbers, byte arrays and JSON strings
//! (no serde dependency); 64-bit counters are returned as `f64` (exact below 2^53).

use dryflash_core::board::Button;
use dryflash_core::board::fat32;
use dryflash_core::board::sdcard::{BLOCK_SIZE, BlockStore, SdCard};
use dryflash_core::bus::Fidelity;
use dryflash_core::loader::dfu;
use dryflash_core::loader::package::{Package, install_resources, stage_ota};
use dryflash_core::machine::{Machine, RunExit};
use wasm_bindgen::prelude::*;

pub use dryflash_core::VERSION;

/// Virtual card capacity: 1 GiB, sparse (ADR-0013).
const CARD_BLOCKS: u32 = 2_097_152;
/// Volume serial of a new card; fixed so that runs are reproducible.
const CARD_SERIAL: u32 = 0x4446_0001;
/// Host record of one block: LBA (little-endian) and its data.
const RECORD: usize = 4 + BLOCK_SIZE;
/// The hardware target this emulator models (`Target` in update.fuf).
const TARGET_F7: u32 = 7;

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
    /// A firmware is in flash.
    loaded: bool,
    /// USART1 bytes already handed to the host.
    log_read: usize,
    /// Description of the last non-budget exit.
    last_stop: String,
}

impl Emulator {
    /// A powered-off machine that keeps the device's settings and its card, whose protocol state
    /// restarts while the data stays.
    fn fresh_machine(&mut self) -> Machine {
        let mut m = Machine::new();
        m.bus.legacy_sleep = self.m.bus.legacy_sleep;
        // What the updater or the firmware programmed survives, like on real flash.
        m.bus.flash.copy_from_slice(&self.m.bus.flash);
        m.bus.periphs.flash.ob = self.m.bus.periphs.flash.ob;
        if let Some(old) = self.m.bus.periphs.remove_sd() {
            let mut card = SdCard::new(old.capacity_blocks(), BlockStore::default());
            card.store = old.store;
            card.unsupported = old.unsupported;
            m.bus.periphs.insert_sd(card);
        }
        m
    }
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
            loaded: false,
            log_read: 0,
            last_stop: String::new(),
        }
    }

    /// Parses and installs a DfuSe image. `cpu2_start` is the first flash address reserved for
    /// CPU2 (`Radio address` of the update package), 0 if unknown. Returns an error message.
    pub fn load_dfu(&mut self, bytes: &[u8], cpu2_start: u32) -> Result<(), String> {
        let image = dfu::parse(bytes).map_err(|e| format!("not a valid DfuSe file: {e}"))?;
        let cpu2 = (cpu2_start != 0).then_some(cpu2_start);
        // Checked on a scratch machine first: a rejected image must not touch the running one.
        Machine::new().load_dfu(&image, cpu2).map_err(|e| e.to_string())?;
        let mut m = self.fresh_machine();
        m.load_dfu(&image, cpu2).map_err(|e| e.to_string())?;
        self.m = m;
        self.loaded = true;
        Ok(())
    }

    /// Quick install of an update package: its firmware into flash, its resources on the card.
    /// Returns `{"info", "files"}` as JSON, or an error message.
    pub fn load_tgz(&mut self, bytes: &[u8]) -> Result<String, String> {
        let pkg = Package::parse(bytes).map_err(|e| e.to_string())?;
        if pkg.manifest.target != TARGET_F7 {
            return Err(format!("package is for target {}, not f7", pkg.manifest.target));
        }
        let image = dfu::parse(pkg.firmware()).map_err(|e| format!("firmware in the package: {e}"))?;
        let entries = pkg.resources().map_err(|e| e.to_string())?;
        Machine::new()
            .load_dfu(&image, pkg.manifest.radio_address)
            .map_err(|e| e.to_string())?;
        let card = self.m.bus.periphs.sd.as_mut().ok_or("no card")?;
        let files = install_resources(&entries, &mut card.store).map_err(|e| e.to_string())?;
        let mut m = self.fresh_machine();
        m.load_dfu(&image, pkg.manifest.radio_address)
            .map_err(|e| e.to_string())?;
        self.m = m;
        self.loaded = true;
        Ok(format!(
            "{{\"info\":{},\"files\":{files}}}",
            json_str(&pkg.manifest.info)
        ))
    }

    /// Real OTA: stages a package on the card without its radio stack (ADR-0014) and restarts into
    /// the running firmware's own updater. Returns `{"info", "path"}` as JSON, or an error message.
    pub fn start_ota(&mut self, bytes: &[u8]) -> Result<String, String> {
        if !self.loaded {
            return Err("an update needs a firmware that is already installed".to_string());
        }
        let pkg = Package::parse(bytes).map_err(|e| e.to_string())?;
        if pkg.manifest.target != TARGET_F7 {
            return Err(format!("package is for target {}, not f7", pkg.manifest.target));
        }
        let card = self.m.bus.periphs.sd.as_mut().ok_or("no card")?;
        let path = stage_ota(&pkg, &mut card.store).map_err(|e| e.to_string())?;
        self.m.start_ota();
        Ok(format!(
            "{{\"info\":{},\"path\":{}}}",
            json_str(&pkg.manifest.info),
            json_str(&path)
        ))
    }

    /// Inserts the card from saved block records; with none, a new empty FAT32 card.
    pub fn sd_restore(&mut self, records: &[u8]) {
        let mut store = BlockStore::default();
        for r in records.as_chunks::<RECORD>().0 {
            store.write(u32::from_le_bytes([r[0], r[1], r[2], r[3]]), &r[4..]);
        }
        store.take_dirty(); // these are already saved
        if records.len() < RECORD {
            fat32::format(&mut store, CARD_BLOCKS, CARD_SERIAL);
        }
        self.m.bus.periphs.insert_sd(SdCard::new(CARD_BLOCKS, store));
    }

    /// Replaces the card with a new empty one. The host forgets its saved blocks first.
    pub fn sd_erase(&mut self) {
        self.sd_restore(&[]);
    }

    /// Blocks written since the previous call, as records of LBA + 512 bytes.
    pub fn sd_take_dirty(&mut self) -> Vec<u8> {
        let Some(card) = &mut self.m.bus.periphs.sd else {
            return Vec::new();
        };
        let dirty = card.store.take_dirty();
        let mut out = vec![0u8; dirty.len() * RECORD];
        for (lba, r) in dirty.into_iter().zip(out.as_chunks_mut::<RECORD>().0) {
            r[..4].copy_from_slice(&lba.to_le_bytes());
            card.store.read(lba, &mut r[4..]);
        }
        out
    }

    /// Bytes in use on the card (non-zero blocks).
    pub fn sd_used_bytes(&self) -> f64 {
        self.m
            .bus
            .periphs
            .sd
            .as_ref()
            .map_or(0.0, |c| (c.store.used_blocks() * BLOCK_SIZE) as f64)
    }

    pub fn sd_capacity_bytes(&self) -> f64 {
        self.m
            .bus
            .periphs
            .sd
            .as_ref()
            .map_or(0.0, |c| c.capacity_blocks() as f64 * BLOCK_SIZE as f64)
    }

    /// Virtual-device setting Sleep Method = Legacy (ADR-0012); applies at the next power-on.
    pub fn set_legacy_sleep(&mut self, on: bool) {
        self.m.bus.legacy_sleep = on;
    }

    pub fn legacy_sleep(&self) -> bool {
        self.m.bus.legacy_sleep
    }

    /// Power-on reset: RAM and backup domain cleared; flash and the card keep their contents.
    pub fn power_on(&mut self) -> Result<(), String> {
        if !self.loaded {
            return Err("no firmware loaded".to_string());
        }
        let mut m = self.fresh_machine();
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

    #[test]
    fn card_records_round_trip_and_a_new_card_is_formatted() {
        let mut e = Emulator::new();
        e.sd_restore(&[]);
        assert!(e.sd_used_bytes() > 0.0, "formatted");
        let new = e.sd_take_dirty();
        assert_eq!(new.len() % RECORD, 0);
        assert!(!new.is_empty(), "a new card must be saved");
        assert!(e.sd_take_dirty().is_empty());

        let mut again = Emulator::new();
        again.sd_restore(&new);
        assert_eq!(again.sd_used_bytes(), e.sd_used_bytes());
        assert!(again.sd_take_dirty().is_empty(), "restored blocks are not dirty");
    }

    #[test]
    fn flash_survives_a_new_machine() {
        let mut e = Emulator::new();
        e.m.bus.flash[100] = 0x42;
        assert_eq!(e.fresh_machine().bus.flash[100], 0x42);
    }

    #[test]
    fn ota_needs_an_installed_firmware() {
        assert!(Emulator::new().start_ota(&[]).is_err());
    }

    #[test]
    fn settings_and_card_survive_a_new_machine() {
        let mut e = Emulator::new();
        e.sd_restore(&[]);
        e.set_legacy_sleep(false);
        let used = e.sd_used_bytes();
        let m = e.fresh_machine();
        assert!(!m.bus.legacy_sleep);
        assert_eq!(
            m.bus.periphs.sd.as_ref().unwrap().store.used_blocks() * BLOCK_SIZE,
            used as usize
        );
    }
}

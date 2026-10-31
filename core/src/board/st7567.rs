// SPDX-License-Identifier: GPL-3.0-or-later
//! Sitronix ST7567 128×64 monochrome LCD controller on SPI2 (CS = PC11, A0 = PB1,
//! RST = PB0; targets/f7/furi_hal/furi_hal_resources.h, firmware 1.5.1).
//!
//! Command set as used by Flipper's u8x8 driver (lib/u8g2/u8g2_glue.c): display on/off,
//! start line, page/column address, SEG and COM direction, inverse, all-pixels-on, bias,
//! power control, regulation ratio, electronic volume (contrast), booster, reset, NOP.
//! Display RAM: 132 columns × 9 pages (65 lines). Glass mapping is derived from the
//! firmware's own normal orientation: `u8x8_d_st756x_init` sends A0 (SEG normal) and C8 (COM
//! reverse), and flip mode 0 — the default — sends the sequence named `flip1` (A0, C8) with
//! x offset 0. That must look upright, so glass pixel (x, y) shows SEG x and RAM line y under
//! A0/C8; flip mode 1 (A1, C0, x offset 4) then rotates the image by 180°, as on hardware.

pub const WIDTH: usize = 128;
pub const HEIGHT: usize = 64;
const COLUMNS: usize = 132;
const PAGES: usize = 9;

#[derive(Clone, Debug)]
pub struct St7567 {
    ram: [[u8; COLUMNS]; PAGES],
    page: usize,
    column: usize,
    start_line: u8,
    seg_reverse: bool,
    com_reverse: bool,
    pub display_on: bool,
    pub inverse: bool,
    pub all_on: bool,
    /// Electronic volume (contrast), 0–63.
    pub contrast: u8,
    /// Command waiting for its argument byte (0x81 electronic volume, 0xF8 booster).
    pending: Option<u8>,
    /// Bumped on every change of the visible image (frame-on-change for the UI).
    pub generation: u64,
}

impl Default for St7567 {
    fn default() -> Self {
        Self::new()
    }
}

impl St7567 {
    pub fn new() -> Self {
        St7567 {
            ram: [[0; COLUMNS]; PAGES],
            page: 0,
            column: 0,
            start_line: 0,
            seg_reverse: false,
            com_reverse: false,
            display_on: false,
            inverse: false,
            all_on: false,
            contrast: 0x20,
            pending: None,
            generation: 0,
        }
    }

    /// Hardware reset (RST low): registers to defaults; RAM content is undefined on
    /// hardware and kept here.
    pub fn reset(&mut self) {
        let ram = self.ram;
        *self = St7567::new();
        self.ram = ram;
        self.generation += 1;
    }

    /// One byte on the bus with A0 = `data` (1 = display data, 0 = command).
    pub fn write(&mut self, byte: u8, data: bool) {
        if data {
            if self.page < PAGES && self.column < COLUMNS {
                self.ram[self.page][self.column] = byte;
                self.generation += 1;
            }
            // The column address auto-increments and stops at the last column.
            if self.column < COLUMNS {
                self.column += 1;
            }
            return;
        }
        if let Some(cmd) = self.pending.take() {
            if cmd == 0x81 {
                self.contrast = byte & 0x3F;
                self.generation += 1;
            }
            return;
        }
        match byte {
            0xAE | 0xAF => self.display_on = byte & 1 != 0,
            0x40..=0x7F => self.start_line = byte & 0x3F,
            0xB0..=0xBF => self.page = (byte & 0x0F) as usize,
            0x10..=0x1F => self.column = (self.column & 0x0F) | ((byte as usize & 0x0F) << 4),
            0x00..=0x0F => self.column = (self.column & 0xF0) | (byte as usize & 0x0F),
            0xA0 | 0xA1 => self.seg_reverse = byte & 1 != 0,
            0xA6 | 0xA7 => self.inverse = byte & 1 != 0,
            0xA4 | 0xA5 => self.all_on = byte & 1 != 0,
            0xC0..=0xCF => self.com_reverse = byte & 0x08 != 0,
            0xE2 => {
                // Software reset: start line, column, page, COM direction (datasheet "Reset").
                self.start_line = 0;
                self.column = 0;
                self.page = 0;
                self.com_reverse = false;
            }
            0x81 | 0xF8 => self.pending = Some(byte),
            // Bias, power control, regulation ratio, read-modify-write, NOP: no visible effect.
            _ => return,
        }
        self.generation += 1;
    }

    /// Visible pixel at (x, y), true = dark (pixel on).
    pub fn pixel(&self, x: usize, y: usize) -> bool {
        if !self.display_on {
            return false;
        }
        if self.all_on {
            return true;
        }
        // Glass column x is SEG x; SEG s shows RAM column s (A0) or 131 - s (A1).
        let col = if self.seg_reverse { COLUMNS - 1 - x } else { x };
        // Glass row y is reached by COM scan index y with C8 (reverse) and 63 - y with C0.
        let com = if self.com_reverse { y } else { HEIGHT - 1 - y };
        let line = (com + self.start_line as usize) % HEIGHT;
        let on = self.ram[line / 8][col] >> (line % 8) & 1 != 0;
        on != self.inverse
    }

    /// The visible 128×64 image, 1 byte per pixel (1 = on).
    pub fn frame(&self) -> Vec<u8> {
        let mut out = vec![0u8; WIDTH * HEIGHT];
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                out[y * WIDTH + x] = self.pixel(x, y) as u8;
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The init sequence of lib/u8g2/u8g2_glue.c (reset, bias, A0, C8, start line 0, …).
    fn init(d: &mut St7567) {
        for c in [0xE2u8, 0xA2, 0xA0, 0xC8, 0x40, 0x26, 0x81, 28, 0x2F, 0xA4, 0xAF] {
            d.write(c, false);
        }
    }

    #[test]
    fn normal_orientation_maps_ram_column_x_line_y_to_pixel_x_y() {
        let mut d = St7567::new();
        init(&mut d);
        for c in [0xB2u8, 0x10, 0x05] {
            d.write(c, false);
        }
        d.write(0b0000_0001, true); // page 2, column 5, bit 0 → line 16
        assert!(d.pixel(5, 16));
        assert!(!d.pixel(5, 17));
        assert_eq!(d.contrast, 28);
    }

    #[test]
    fn flip_mode_1_rotates_180() {
        let mut d = St7567::new();
        init(&mut d);
        // Flip mode 1: A1, C0 and u8x8 flipmode_x_offset 4 → pixel (5, 16) is column 9.
        for c in [0xA1u8, 0xC0, 0xB2, 0x10, 0x09] {
            d.write(c, false);
        }
        d.write(0b0000_0001, true);
        assert!(d.pixel(127 - 5, 63 - 16));
    }
}

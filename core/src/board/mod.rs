// SPDX-License-Identifier: GPL-3.0-or-later
//! Devices on the f7 board, wired as in targets/f7/furi_hal (firmware 1.5.1).

/// GPIO port index in `Periphs::gpio` (A, B, C, D, E, H).
pub const PORT_A: usize = 0;
pub const PORT_B: usize = 1;
pub const PORT_C: usize = 2;
pub const PORT_H: usize = 5;

/// Display wiring (furi_hal_resources.h): CS = PC11, DI/A0 = PB1, RST = PB0.
pub const DISPLAY_CS: (usize, u8) = (PORT_C, 11);
pub const DISPLAY_A0: (usize, u8) = (PORT_B, 1);
/// microSD chip select and card detect (furi_hal_resources.h: SD_CS PC12, SD_CD PC10).
pub const SD_CS: (usize, u8) = (PORT_C, 12);
/// Sub-GHz radio and NFC front end on SPI1 (furi_hal_resources.h: CC1101_CS PD0, CC1101_G0 PA1, NFC_CS PE4).
pub const CC1101_CS: (usize, u8) = (3, 0);
pub const CC1101_G0: (usize, u8) = (PORT_A, 1);
pub const NFC_CS: (usize, u8) = (4, 4);
pub const SD_CD: (usize, u8) = (PORT_C, 10);
pub const DISPLAY_RST: (usize, u8) = (PORT_B, 0);
/// Speaker and vibration motor (furi_hal_resources.h: SPEAKER PB8, VIBRO PA8).
pub const SPEAKER: (usize, u8) = (PORT_B, 8);
pub const VIBRO: (usize, u8) = (PORT_A, 8);

/// The six buttons (furi_hal_resources.c `input_pins`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Button {
    Up,
    Down,
    Left,
    Right,
    Ok,
    Back,
}

impl Button {
    pub const ALL: [Button; 6] = [
        Button::Up,
        Button::Down,
        Button::Left,
        Button::Right,
        Button::Ok,
        Button::Back,
    ];

    /// (port, pin, active-high). All buttons pull to ground when pressed (`inverted`, pull-up)
    /// except OK, which is active high with a pull-down.
    pub fn pin(self) -> (usize, u8, bool) {
        match self {
            Button::Up => (PORT_B, 10, false),
            Button::Down => (PORT_C, 6, false),
            Button::Right => (PORT_B, 12, false),
            Button::Left => (PORT_B, 11, false),
            Button::Ok => (PORT_H, 3, true),
            Button::Back => (PORT_C, 13, false),
        }
    }
}

pub mod bq25896;
pub mod bq27220;
pub mod cc1101;
pub mod fat32;
pub mod lp5562;
pub mod otp;
pub mod sdcard;
pub mod senses;
pub mod st25r3916;
pub mod st7567;

/// Battery state shown by the fuel gauge and charger (UI controls in M7).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Battery {
    pub state_of_charge: u8,
    pub voltage_mv: u16,
    pub current_ma: i16,
    pub temperature_c: f32,
    pub charging: bool,
    pub usb_connected: bool,
}

impl Default for Battery {
    fn default() -> Self {
        Battery {
            state_of_charge: 80,
            voltage_mv: 3950,
            current_ma: -50,
            temperature_c: 25.0,
            charging: false,
            usb_connected: false,
        }
    }
}

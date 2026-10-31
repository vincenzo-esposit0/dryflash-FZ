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
pub const DISPLAY_RST: (usize, u8) = (PORT_B, 0);

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
pub mod lp5562;
pub mod otp;
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

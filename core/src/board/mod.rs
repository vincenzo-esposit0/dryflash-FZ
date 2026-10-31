// SPDX-License-Identifier: GPL-3.0-or-later
//! Devices on the f7 board, wired as in targets/f7/furi_hal (firmware 1.5.1).

pub mod bq25896;
pub mod bq27220;
pub mod otp;

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

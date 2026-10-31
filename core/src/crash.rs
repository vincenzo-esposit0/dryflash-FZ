// SPDX-License-Identifier: GPL-3.0-or-later
//! Firmware-level crash records (BRIEF §7). M2: Furi's own record; full reports in M6.
//!
//! In release builds, `__furi_crash_implementation` (furi/core/check.c, firmware 1.5.1)
//! stores a pointer to the crash message in RTC backup register `FuriHalRtcRegisterFaultData`
//! (index 4 of `FuriHalRtcRegister`, targets/f7/furi_hal/furi_hal_rtc.h) and then resets.
//! The message is a NUL-terminated string in flash.

use crate::bus::SystemBus;
use crate::bus::map::{FLASH_BASE, FLASH_SIZE};

/// RTC backup register holding Furi's fault message pointer.
pub const FURI_FAULT_DATA_BKP: usize = 4;

/// Reads the message a pointer in flash points to (at most 128 bytes, printable ASCII).
pub fn flash_string(bus: &SystemBus, ptr: u32) -> Option<String> {
    let off = ptr.checked_sub(FLASH_BASE)? as usize;
    if off >= FLASH_SIZE as usize {
        return None;
    }
    let bytes: Vec<u8> = bus.flash[off..]
        .iter()
        .take(128)
        .take_while(|&&b| b != 0)
        .copied()
        .collect();
    if bytes.is_empty() || !bytes.iter().all(|b| b.is_ascii_graphic() || *b == b' ') {
        return None;
    }
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// The fault pointer Furi left in the RTC backup register, and its message if it decodes.
pub fn furi_fault(bus: &SystemBus) -> Option<(u32, Option<String>)> {
    let ptr = bus.periphs.rtc.backup(FURI_FAULT_DATA_BKP);
    (ptr != 0).then(|| (ptr, flash_string(bus, ptr)))
}

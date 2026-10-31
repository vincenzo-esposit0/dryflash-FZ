// SPDX-License-Identifier: GPL-3.0-or-later
//! Board data in the OTP area (0x1FFF7000), in the `FuriHalVersionOTPv2` layout of
//! targets/f7/furi_hal/furi_hal_version.c (firmware 1.5.1): magic 0xBABE, version 2.
//!
//! These are the "virtual device settings" of BRIEF §5.1. Fields that change firmware
//! behaviour: `target` must be 7 (furi_hal_version_device.c), `version` > 10 selects the
//! production I2C configuration (furi_hal_i2c_config.c:100), `region` selects the allowed
//! Sub-GHz bands (furi_hal_region.c). The others are informational (About screen).

/// `FURI_HAL_VERSION_NAME_LENGTH`.
pub const NAME_LENGTH: usize = 8;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoardInfo {
    pub timestamp: u32,
    pub version: u8,
    pub target: u8,
    pub body: u8,
    pub connect: u8,
    /// `FuriHalVersionDisplay`: 1 = ERC, 2 = MGG.
    pub display: u8,
    /// `FuriHalVersionColor`: 1 = black, 2 = white, 3 = transparent.
    pub color: u8,
    /// `FuriHalVersionRegion`: 1 = EU/RU, 2 = US/CA/AU, 3 = JP, 4 = world.
    pub region: u8,
    /// Device name, up to 8 ASCII characters (shown as "xFlipper <name>").
    pub name: String,
}

impl Default for BoardInfo {
    /// dryflash defaults; editable per virtual device (M5). `body`, `connect` and
    /// `display` are not known for a specific production run and only appear in About.
    fn default() -> Self {
        BoardInfo {
            timestamp: 0,
            version: 12,
            target: 7,
            body: 9,
            connect: 6,
            display: 2,
            color: 1,
            region: 4,
            name: "dryflash".into(),
        }
    }
}

impl BoardInfo {
    /// The 32-byte OTP v2 image (four 64-bit double-words, little-endian fields).
    pub fn otp_bytes(&self) -> [u8; 32] {
        let mut b = [0u8; 32];
        b[0..2].copy_from_slice(&0xBABEu16.to_le_bytes());
        b[2] = 2; // header_version
        b[3] = 0; // header_reserved
        b[4..8].copy_from_slice(&self.timestamp.to_le_bytes());
        b[8] = self.version;
        b[9] = self.target;
        b[10] = self.body;
        b[11] = self.connect;
        b[12] = self.display;
        // b[13..16]: reserved, 0
        b[16] = self.color;
        b[17] = self.region;
        // b[18..24]: reserved, 0
        for (i, c) in self.name.bytes().take(NAME_LENGTH).enumerate() {
            b[24 + i] = c;
        }
        b
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_matches_furi_hal_version_otp_v2() {
        let b = BoardInfo::default().otp_bytes();
        assert_eq!(&b[0..4], &[0xBE, 0xBA, 2, 0]);
        assert_eq!(b[9], 7, "board_target");
        assert_eq!(&b[24..32], b"dryflash");
    }
}

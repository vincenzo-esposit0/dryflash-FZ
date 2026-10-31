// SPDX-License-Identifier: GPL-3.0-or-later
//! DfuSe container (ST UM0391 "DfuSe File Format Specification", DFU 1.1 suffix).
//!
//! Layout: prefix (11 bytes) · targets (274-byte prefix + elements) · 16-byte suffix.
//! The suffix CRC is the CRC-32 register over every byte except the CRC itself, stored
//! without the final complement (verified on the official 1.5.1 `firmware.dfu`).

use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Element {
    pub address: u32,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub alt_setting: u8,
    pub name: Option<String>,
    pub elements: Vec<Element>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dfu {
    pub targets: Vec<Target>,
    pub vendor_id: u16,
    pub product_id: u16,
    pub device_id: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DfuError {
    TooShort,
    BadSignature,
    BadVersion(u8),
    BadSuffix,
    Crc { stored: u32, computed: u32 },
    Truncated { at: usize },
    SizeMismatch { declared: u32, actual: usize },
}

impl fmt::Display for DfuError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DfuError::TooShort => write!(f, "file too short for a DfuSe image"),
            DfuError::BadSignature => write!(f, "missing \"DfuSe\" signature"),
            DfuError::BadVersion(v) => write!(f, "unsupported DfuSe version {v}"),
            DfuError::BadSuffix => write!(f, "invalid DFU suffix"),
            DfuError::Crc { stored, computed } => {
                write!(
                    f,
                    "DFU suffix CRC mismatch: stored 0x{stored:08X}, computed 0x{computed:08X}"
                )
            }
            DfuError::Truncated { at } => write!(f, "image truncated at byte {at}"),
            DfuError::SizeMismatch { declared, actual } => {
                write!(f, "prefix declares {declared} bytes, image has {actual}")
            }
        }
    }
}

impl std::error::Error for DfuError {}

/// CRC-32 (IEEE 802.3, reflected, init 0xFFFFFFFF) without the final XOR.
fn crc32_register(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xEDB8_8320 & (crc & 1).wrapping_neg());
        }
    }
    crc
}

fn u32_at(b: &[u8], at: usize) -> Result<u32, DfuError> {
    b.get(at..at + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or(DfuError::Truncated { at })
}

pub fn parse(file: &[u8]) -> Result<Dfu, DfuError> {
    if file.len() < 11 + 16 {
        return Err(DfuError::TooShort);
    }
    let (body, suffix) = file.split_at(file.len() - 16);
    if &suffix[8..11] != b"UFD" || suffix[11] != 16 {
        return Err(DfuError::BadSuffix);
    }
    let stored = u32_at(suffix, 12)?;
    let computed = crc32_register(&file[..file.len() - 4]);
    if stored != computed {
        return Err(DfuError::Crc { stored, computed });
    }
    if &body[..5] != b"DfuSe" {
        return Err(DfuError::BadSignature);
    }
    if body[5] != 1 {
        return Err(DfuError::BadVersion(body[5]));
    }
    let declared = u32_at(body, 6)?;
    if declared as usize != body.len() {
        return Err(DfuError::SizeMismatch {
            declared,
            actual: body.len(),
        });
    }
    let n_targets = body[10];
    let mut at = 11;
    let mut targets = Vec::new();
    for _ in 0..n_targets {
        let pfx = body.get(at..at + 274).ok_or(DfuError::Truncated { at })?;
        if &pfx[..6] != b"Target" {
            return Err(DfuError::BadSignature);
        }
        let named = u32_at(pfx, 7)? != 0;
        let name = named.then(|| {
            let raw = &pfx[11..11 + 255];
            let end = raw.iter().position(|&c| c == 0).unwrap_or(raw.len());
            String::from_utf8_lossy(&raw[..end]).into_owned()
        });
        let n_elements = u32_at(pfx, 270)?;
        at += 274;
        let mut elements = Vec::new();
        for _ in 0..n_elements {
            let address = u32_at(body, at)?;
            let size = u32_at(body, at + 4)? as usize;
            let data = body
                .get(at + 8..at + 8 + size)
                .ok_or(DfuError::Truncated { at })?
                .to_vec();
            at += 8 + size;
            elements.push(Element { address, data });
        }
        targets.push(Target {
            alt_setting: pfx[6],
            name,
            elements,
        });
    }
    Ok(Dfu {
        targets,
        device_id: u16::from_le_bytes([suffix[0], suffix[1]]),
        product_id: u16::from_le_bytes([suffix[2], suffix[3]]),
        vendor_id: u16::from_le_bytes([suffix[4], suffix[5]]),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a minimal valid DfuSe image with one element.
    pub(crate) fn build(address: u32, data: &[u8]) -> Vec<u8> {
        let mut t = Vec::new();
        t.extend_from_slice(b"Target");
        t.push(0);
        t.extend_from_slice(&1u32.to_le_bytes());
        let mut name = [0u8; 255];
        name[..4].copy_from_slice(b"test");
        t.extend_from_slice(&name);
        t.extend_from_slice(&((8 + data.len()) as u32).to_le_bytes());
        t.extend_from_slice(&1u32.to_le_bytes());
        t.extend_from_slice(&address.to_le_bytes());
        t.extend_from_slice(&(data.len() as u32).to_le_bytes());
        t.extend_from_slice(data);
        let mut f = Vec::new();
        f.extend_from_slice(b"DfuSe");
        f.push(1);
        f.extend_from_slice(&((11 + t.len()) as u32).to_le_bytes());
        f.push(1);
        f.extend_from_slice(&t);
        f.extend_from_slice(&[0xFF, 0xFF, 0x11, 0xDF, 0x83, 0x04, 0x1A, 0x01]);
        f.extend_from_slice(b"UFD");
        f.push(16);
        let crc = crc32_register(&f);
        f.extend_from_slice(&crc.to_le_bytes());
        f
    }

    #[test]
    fn round_trip() {
        let img = build(0x0800_0000, &[1, 2, 3, 4]);
        let dfu = parse(&img).unwrap();
        assert_eq!(dfu.vendor_id, 0x0483);
        assert_eq!(dfu.product_id, 0xDF11);
        assert_eq!(dfu.targets[0].name.as_deref(), Some("test"));
        assert_eq!(
            dfu.targets[0].elements,
            vec![Element {
                address: 0x0800_0000,
                data: vec![1, 2, 3, 4]
            }]
        );
    }

    #[test]
    fn corrupt_byte_fails_crc() {
        let mut img = build(0x0800_0000, &[1, 2, 3, 4]);
        img[300] ^= 1;
        assert!(matches!(parse(&img), Err(DfuError::Crc { .. })));
    }
}

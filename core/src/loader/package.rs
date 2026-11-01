// SPDX-License-Identifier: GPL-3.0-or-later
//! Update packages (`.tgz`): gzip + tar with an `update.fuf` manifest (firmware's
//! lib/update_util/update_manifest.c) naming the firmware, the radio stack and the resources.
//! Resources are a tar compressed as a heatshrink data stream (`.ths`: scripts/flipper/assets/
//! tarball.py; lib/toolbox/tar/tar_archive.c) or a plain or gzipped tar.

use std::collections::BTreeMap;
use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PackageError {
    Gzip(String),
    Tar(String),
    Heatshrink(String),
    /// A required entry is missing or malformed.
    Manifest(String),
}

impl fmt::Display for PackageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PackageError::Gzip(s) => write!(f, "gzip: {s}"),
            PackageError::Tar(s) => write!(f, "tar: {s}"),
            PackageError::Heatshrink(s) => write!(f, "heatshrink: {s}"),
            PackageError::Manifest(s) => write!(f, "update.fuf: {s}"),
        }
    }
}

impl std::error::Error for PackageError {}

/// Decompresses a gzip member (RFC 1952), checking its CRC32 and size.
pub fn gunzip(data: &[u8]) -> Result<Vec<u8>, PackageError> {
    let bad = |s: &str| PackageError::Gzip(s.to_string());
    if data.len() < 18 || data[0] != 0x1F || data[1] != 0x8B || data[2] != 8 {
        return Err(bad("not a gzip stream"));
    }
    let flags = data[3];
    let mut p = 10;
    if flags & 4 != 0 {
        let n =
            *data.get(p).ok_or(bad("truncated"))? as usize | (*data.get(p + 1).ok_or(bad("truncated"))? as usize) << 8;
        p += 2 + n;
    }
    for bit in [8u8, 16] {
        if flags & bit != 0 {
            p += data
                .get(p..)
                .ok_or(bad("truncated"))?
                .iter()
                .position(|&b| b == 0)
                .ok_or(bad("truncated"))?
                + 1;
        }
    }
    if flags & 2 != 0 {
        p += 2;
    }
    let body = data.get(p..data.len() - 8).ok_or(bad("truncated"))?;
    let out = miniz_oxide::inflate::decompress_to_vec(body).map_err(|e| PackageError::Gzip(format!("{e:?}")))?;
    let tail = &data[data.len() - 8..];
    if crc32(&out) != u32::from_le_bytes(tail[0..4].try_into().unwrap()) {
        return Err(bad("CRC mismatch"));
    }
    if out.len() as u32 != u32::from_le_bytes(tail[4..8].try_into().unwrap()) {
        return Err(bad("size mismatch"));
    }
    Ok(out)
}

fn crc32(data: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for &b in data {
        c ^= b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
        }
    }
    !c
}

/// One entry of a tar archive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TarEntry {
    /// Path without a leading `./` or `/`.
    pub path: String,
    pub is_dir: bool,
    pub data: Vec<u8>,
}

/// Reads a ustar archive, with GNU long names and pax headers skipped or honoured as needed.
pub fn untar(data: &[u8]) -> Result<Vec<TarEntry>, PackageError> {
    let mut out = Vec::new();
    let mut p = 0;
    let mut long_name: Option<String> = None;
    while p + 512 <= data.len() {
        let h = &data[p..p + 512];
        if h.iter().all(|&b| b == 0) {
            break;
        }
        let stored: u32 = h
            .iter()
            .enumerate()
            .map(|(i, &b)| if (148..156).contains(&i) { 32 } else { b as u32 })
            .sum();
        if octal(&h[148..156]) != stored as u64 {
            return Err(PackageError::Tar(format!("bad header checksum at offset {p}")));
        }
        let size = octal(&h[124..136]) as usize;
        let kind = h[156];
        let body = data
            .get(p + 512..p + 512 + size)
            .ok_or_else(|| PackageError::Tar(format!("truncated entry at offset {p}")))?;
        p += 512 + size.div_ceil(512) * 512;
        match kind {
            b'L' => long_name = Some(text(body)),
            b'x' | b'g' => {}
            b'0' | 0 | b'5' => {
                let path = long_name.take().unwrap_or_else(|| {
                    let name = text(&h[0..100]);
                    let prefix = if &h[257..262] == b"ustar" {
                        text(&h[345..500])
                    } else {
                        String::new()
                    };
                    if prefix.is_empty() {
                        name
                    } else {
                        format!("{prefix}/{name}")
                    }
                });
                let is_dir = kind == b'5' || path.ends_with('/');
                let path = path.trim_start_matches("./").trim_matches('/').to_string();
                if !path.is_empty() {
                    if path.split('/').any(|c| c == "..") {
                        return Err(PackageError::Tar(format!("unsafe path {path:?}")));
                    }
                    out.push(TarEntry {
                        path,
                        is_dir,
                        data: if is_dir { Vec::new() } else { body.to_vec() },
                    });
                }
            }
            // Links and special files have no meaning on the card.
            _ => long_name = None,
        }
    }
    Ok(out)
}

fn text(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).into_owned()
}

fn octal(b: &[u8]) -> u64 {
    b.iter()
        .filter(|c| c.is_ascii_digit())
        .fold(0, |v, &c| v * 8 + (c - b'0') as u64)
}

/// Decodes a heatshrink stream with a `HSDS` header (magic, version, window, lookahead).
pub fn unheatshrink(data: &[u8]) -> Result<Vec<u8>, PackageError> {
    if data.len() < 7 || &data[..4] != b"HSDS" {
        return Err(PackageError::Heatshrink("missing HSDS header".into()));
    }
    let (window, lookahead) = (data[5] as usize, data[6] as usize);
    if !(4..=15).contains(&window) || lookahead < 3 || lookahead >= window {
        return Err(PackageError::Heatshrink(format!("bad parameters {window}/{lookahead}")));
    }
    let bits_total = (data.len() - 7) * 8;
    let body = &data[7..];
    let mut pos = 0usize;
    let mut take = |n: usize| -> Option<usize> {
        if pos + n > bits_total {
            return None;
        }
        let mut v = 0usize;
        for _ in 0..n {
            v = (v << 1) | ((body[pos / 8] >> (7 - pos % 8)) & 1) as usize;
            pos += 1;
        }
        Some(v)
    };
    let mut out: Vec<u8> = Vec::with_capacity(data.len() * 3);
    // The stream ends with padding bits: a token that does not fit is the end.
    while let Some(tag) = take(1) {
        if tag == 1 {
            let Some(b) = take(8) else { break };
            out.push(b as u8);
        } else {
            let Some(index) = take(window) else { break };
            let Some(count) = take(lookahead) else { break };
            let (back, count) = (index + 1, count + 1);
            for _ in 0..count {
                // The decoder's window starts zeroed.
                let b = out.len().checked_sub(back).map_or(0, |i| out[i]);
                out.push(b);
            }
        }
    }
    Ok(out)
}

/// The fields of `update.fuf` the emulator uses.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Manifest {
    pub info: String,
    pub target: u32,
    pub firmware: String,
    pub resources: Option<String>,
    /// First flash address reserved for CPU2 (`Radio address`, little-endian hex).
    pub radio_address: Option<u32>,
    pub radio_file: Option<String>,
}

pub fn parse_manifest(text: &str) -> Result<Manifest, PackageError> {
    let mut m = Manifest::default();
    let mut filetype = false;
    for line in text.lines() {
        let Some((k, v)) = line.split_once(':') else { continue };
        let v = v.trim();
        match k.trim() {
            "Filetype" => filetype = v == "Flipper firmware upgrade configuration",
            "Info" => m.info = v.to_string(),
            "Target" => m.target = v.parse().unwrap_or(0),
            "Firmware" => m.firmware = v.to_string(),
            "Resources" if !v.is_empty() => m.resources = Some(v.to_string()),
            "Radio" if !v.is_empty() => m.radio_file = Some(v.to_string()),
            "Radio address" => {
                let bytes: Vec<u8> = v
                    .split_whitespace()
                    .filter_map(|h| u8::from_str_radix(h, 16).ok())
                    .collect();
                if let Ok(b) = <[u8; 4]>::try_from(bytes.as_slice()) {
                    m.radio_address = Some(u32::from_le_bytes(b)).filter(|&a| a != 0);
                }
            }
            _ => {}
        }
    }
    if !filetype {
        return Err(PackageError::Manifest("not an update manifest".into()));
    }
    if m.firmware.is_empty() {
        return Err(PackageError::Manifest("no Firmware entry".into()));
    }
    Ok(m)
}

/// An update package, unpacked.
#[derive(Clone, Debug)]
pub struct Package {
    pub manifest: Manifest,
    /// Top folder in the archive, e.g. `f7-update-1.5.1`.
    pub folder: String,
    /// File name (without directories) → content.
    pub files: BTreeMap<String, Vec<u8>>,
}

impl Package {
    pub fn parse(tgz: &[u8]) -> Result<Package, PackageError> {
        let tar = gunzip(tgz)?;
        let mut files = BTreeMap::new();
        let mut folder = String::new();
        for e in untar(&tar)? {
            if !e.is_dir {
                folder = e.path.rsplit_once('/').map_or("", |(d, _)| d).to_string();
                let name = e.path.rsplit('/').next().unwrap_or(&e.path).to_string();
                files.insert(name, e.data);
            }
        }
        let fuf = files
            .get("update.fuf")
            .ok_or_else(|| PackageError::Manifest("not found in the package".into()))?;
        let manifest = parse_manifest(&String::from_utf8_lossy(fuf))?;
        if !files.contains_key(&manifest.firmware) {
            return Err(PackageError::Manifest(format!(
                "{} is not in the package",
                manifest.firmware
            )));
        }
        Ok(Package {
            manifest,
            folder,
            files,
        })
    }

    pub fn firmware(&self) -> &[u8] {
        &self.files[&self.manifest.firmware]
    }

    /// The resource tree for `/ext`, decoded by file extension. Empty without resources.
    pub fn resources(&self) -> Result<Vec<TarEntry>, PackageError> {
        let Some(name) = &self.manifest.resources else {
            return Ok(Vec::new());
        };
        let raw = self
            .files
            .get(name)
            .ok_or_else(|| PackageError::Manifest(format!("{name} is not in the package")))?;
        let tar = if raw.starts_with(b"HSDS") {
            unheatshrink(raw)?
        } else if raw.starts_with(&[0x1F, 0x8B]) {
            gunzip(raw)?
        } else {
            raw.clone()
        };
        untar(&tar)
    }
}

/// A manifest line for a package with no radio stack, as scripts/update.py writes one: the keys stay,
/// with empty or zero values, because the firmware reads them in order and a missing key would hide
/// every key after it (`update_manifest_init`).
fn without_radio(line: &str) -> String {
    match line.split_once(':').map(|(k, _)| k.trim()) {
        Some("Radio") => "Radio: ".to_string(),
        Some("Radio address") | Some("Radio CRC") => format!("{}: 00 00 00 00", line.split(':').next().unwrap()),
        Some("Radio version") => "Radio version: 00 00 00 00 00 00".to_string(),
        _ => line.to_string(),
    }
}

/// Stages a package for the firmware's own updater: `/ext/update/<package>/` and the `/ext/.fupdate`
/// pointer, as `update_operation_prepare` (lib/update_util/update_operation.c) and qFlipper leave
/// them. The radio stack is left out of the staged manifest: CPU2 is a FUS with nowhere to install
/// one (ADR-0012, ADR-0014), and the format supports packages without it.
pub fn stage_ota(
    pkg: &Package,
    store: &mut crate::board::sdcard::BlockStore,
) -> Result<String, crate::board::fat32::FatError> {
    let mut v = crate::board::fat32::Volume::open(store)?;
    let dir = format!("update/{}", pkg.folder.rsplit('/').next().unwrap_or("package"));
    for (name, data) in &pkg.files {
        if name == "update.fuf" || pkg.manifest.radio_file.as_deref() == Some(name) {
            continue;
        }
        v.write_file(&format!("{dir}/{name}"), data)?;
    }
    let manifest = String::from_utf8_lossy(&pkg.files["update.fuf"]);
    let stripped: String = manifest.lines().map(|l| format!("{}\n", without_radio(l))).collect();
    v.write_file(&format!("{dir}/update.fuf"), stripped.as_bytes())?;
    let path = format!("/ext/{dir}/update.fuf");
    v.write_file(".fupdate", path.as_bytes())?;
    v.sync();
    Ok(path)
}

/// Quick install of the resources: writes the tree to the card's root, which the firmware serves
/// as `/ext` (STORAGE_EXT_PATH_PREFIX). Returns the number of files written.
pub fn install_resources(
    entries: &[TarEntry],
    store: &mut crate::board::sdcard::BlockStore,
) -> Result<usize, crate::board::fat32::FatError> {
    let mut v = crate::board::fat32::Volume::open(store)?;
    let mut files = 0;
    for e in entries {
        if e.is_dir {
            v.mkdir_all(&e.path)?;
        } else {
            v.write_file(&e.path, &e.data)?;
            files += 1;
        }
    }
    v.sync();
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tar_header(name: &str, kind: u8, size: usize) -> [u8; 512] {
        let mut h = [0u8; 512];
        h[..name.len()].copy_from_slice(name.as_bytes());
        h[124..135].copy_from_slice(format!("{size:011o}").as_bytes());
        h[156] = kind;
        h[257..262].copy_from_slice(b"ustar");
        h[148..156].fill(b' ');
        let sum: u32 = h.iter().map(|&b| b as u32).sum();
        h[148..155].copy_from_slice(format!("{sum:06o}\0").as_bytes());
        h
    }

    #[test]
    fn untar_reads_files_and_dirs_and_refuses_escapes() {
        let mut t = Vec::new();
        t.extend(tar_header("./apps/", b'5', 0));
        t.extend(tar_header("./apps/a.txt", b'0', 3));
        let mut body = [0u8; 512];
        body[..3].copy_from_slice(b"abc");
        t.extend(body);
        t.extend([0u8; 1024]);
        let e = untar(&t).unwrap();
        assert_eq!(
            e[0],
            TarEntry {
                path: "apps".into(),
                is_dir: true,
                data: vec![]
            }
        );
        assert_eq!(
            e[1],
            TarEntry {
                path: "apps/a.txt".into(),
                is_dir: false,
                data: b"abc".to_vec()
            }
        );

        let mut bad = tar_header("../evil", b'0', 0).to_vec();
        bad.extend([0u8; 1024]);
        assert!(matches!(untar(&bad), Err(PackageError::Tar(_))));

        let mut corrupt = tar_header("x", b'0', 0).to_vec();
        corrupt[0] = b'y';
        assert!(matches!(untar(&corrupt), Err(PackageError::Tar(_))));
    }

    #[test]
    fn heatshrink_literals_and_backrefs() {
        // Bits: literal 'a', literal 'b', backref index-1 = 1 (window 4), count-1 = 3 (lookahead 3).
        let bits = "1_01100001_1_01100010_0_0001_011";
        let bits: Vec<u8> = bits.bytes().filter(|&b| b != b'_').map(|b| b - b'0').collect();
        let mut body = vec![0u8; bits.len().div_ceil(8)];
        for (i, b) in bits.iter().enumerate() {
            body[i / 8] |= b << (7 - i % 8);
        }
        let mut s = b"HSDS\x01\x04\x03".to_vec();
        s.extend(body);
        assert_eq!(unheatshrink(&s).unwrap(), b"ababab");
        assert!(unheatshrink(b"nope").is_err());
    }

    #[test]
    fn gzip_checks_crc() {
        let body = miniz_oxide::deflate::compress_to_vec(b"hello", 6);
        let mut z = vec![0x1F, 0x8B, 8, 0, 0, 0, 0, 0, 0, 3];
        z.extend(&body);
        z.extend(crc32(b"hello").to_le_bytes());
        z.extend(5u32.to_le_bytes());
        assert_eq!(gunzip(&z).unwrap(), b"hello");
        let n = z.len();
        z[n - 8] ^= 1;
        assert!(matches!(gunzip(&z), Err(PackageError::Gzip(_))));
    }

    #[test]
    fn a_staged_manifest_keeps_the_radio_keys_with_no_values() {
        assert_eq!(without_radio("Radio: radio.bin"), "Radio: ");
        assert_eq!(
            without_radio("Radio address: 00 70 0D 08"),
            "Radio address: 00 00 00 00"
        );
        assert_eq!(
            without_radio("Radio version: 01 14 00 00 02 03"),
            "Radio version: 00 00 00 00 00 00"
        );
        assert_eq!(without_radio("Radio CRC: C1 C3 5B 8C"), "Radio CRC: 00 00 00 00");
        assert_eq!(without_radio("Resources: resources.ths"), "Resources: resources.ths");
    }

    #[test]
    fn manifest_fields() {
        let m = parse_manifest(
            "Filetype: Flipper firmware upgrade configuration\nVersion: 2\nInfo: 1.5.1\nTarget: 7\n\
             Firmware: firmware.dfu\nRadio address: 00 70 0D 08\nResources: resources.ths\n",
        )
        .unwrap();
        assert_eq!(m.radio_address, Some(0x080D_7000));
        assert_eq!(m.resources.as_deref(), Some("resources.ths"));
        assert!(parse_manifest("Filetype: other\nFirmware: x\n").is_err());
    }
}

// SPDX-License-Identifier: GPL-3.0-or-later
//! Minimal PNG writer (8-bit greyscale or RGB, uncompressed deflate blocks) for screenshots
//! of the emulated display in tests and tools. Browsers use the canvas instead.

fn crc32(chunks: &[&[u8]]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for data in chunks {
        for &b in *data {
            c ^= b as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            }
        }
    }
    !c
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    out.extend_from_slice(&crc32(&[kind, data]).to_be_bytes());
}

/// Encodes `pixels` (`width * height * channels` bytes, row-major; 1 = grey, 3 = RGB).
pub fn encode(width: u32, height: u32, channels: u8, pixels: &[u8]) -> Vec<u8> {
    assert!(channels == 1 || channels == 3);
    let row = width as usize * channels as usize;
    assert_eq!(pixels.len(), row * height as usize);
    // Scanlines with filter type 0.
    let mut raw = Vec::with_capacity((row + 1) * height as usize);
    for line in pixels.chunks(row) {
        raw.push(0);
        raw.extend_from_slice(line);
    }
    // zlib stream of stored deflate blocks.
    let mut z = vec![0x78, 0x01];
    let mut blocks = raw.chunks(0xFFFF).peekable();
    if raw.is_empty() {
        z.extend_from_slice(&[1, 0, 0, 0xFF, 0xFF]);
    }
    while let Some(b) = blocks.next() {
        z.push(blocks.peek().is_none() as u8);
        z.extend_from_slice(&(b.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(b.len() as u16)).to_le_bytes());
        z.extend_from_slice(b);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &x in &raw {
        a = (a + x as u32) % 65521;
        b = (b + a) % 65521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());

    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, if channels == 1 { 0 } else { 2 }, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

/// The 128×64 LCD frame (1 = dark pixel) as an RGB image scaled by `scale`, in the
/// orange-backlit look of the real glass (backlight off: grey).
pub fn lcd(frame: &[u8], scale: u32, backlight: bool) -> Vec<u8> {
    let (w, h) = (128u32, 64u32);
    let (on, off): ([u8; 3], [u8; 3]) = if backlight {
        ([0x00, 0x00, 0x00], [0xFF, 0x8C, 0x29])
    } else {
        ([0x10, 0x10, 0x10], [0x9A, 0x8F, 0x80])
    };
    let mut px = Vec::with_capacity((w * h * scale * scale * 3) as usize);
    for y in 0..h * scale {
        for x in 0..w * scale {
            let dark = frame[((y / scale) * w + x / scale) as usize] != 0;
            px.extend_from_slice(if dark { &on } else { &off });
        }
    }
    encode(w * scale, h * scale, 3, &px)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_matches_the_png_iend_constant() {
        // Every PNG ends with IEND and CRC AE 42 60 82.
        let png = encode(1, 1, 1, &[0]);
        assert_eq!(&png[png.len() - 4..], &[0xAE, 0x42, 0x60, 0x82]);
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    }

    #[test]
    fn large_images_use_several_stored_blocks() {
        let png = encode(400, 400, 1, &vec![7; 160_000]);
        assert!(png.len() > 160_000);
    }
}

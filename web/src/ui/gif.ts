// SPDX-License-Identifier: GPL-3.0-or-later
// A small GIF89a encoder for the 1-bit LCD (GIF spec, LZW as variant in Appendix F).

/** Encodes pixels (palette indices) with GIF's variable-width LZW. */
export function lzw(pixels: Uint8Array, minCodeSize: number): Uint8Array {
  const clear = 1 << minCodeSize;
  const end = clear + 1;
  const out: number[] = [];
  let acc = 0;
  let bits = 0;
  let width = minCodeSize + 1;
  const emit = (code: number) => {
    acc |= code << bits;
    bits += width;
    while (bits >= 8) {
      out.push(acc & 0xff);
      acc >>>= 8;
      bits -= 8;
    }
  };
  let table = new Map<number, number>();
  let next = end + 1;
  emit(clear);
  let prefix = pixels[0];
  for (let i = 1; i < pixels.length; i++) {
    const p = pixels[i];
    const key = (prefix << 8) | p;
    const found = table.get(key);
    if (found !== undefined) {
      prefix = found;
      continue;
    }
    emit(prefix);
    table.set(key, next++);
    // The decoder adds its entry one code later, hence ">" rather than ">=".
    if (next > 1 << width) {
      if (width < 12) width++;
    }
    if (next >= 4095) {
      emit(clear);
      table = new Map();
      next = end + 1;
      width = minCodeSize + 1;
    }
    prefix = p;
  }
  emit(prefix);
  emit(end);
  if (bits > 0) out.push(acc & 0xff);
  return Uint8Array.from(out);
}

export interface GifFrame {
  /** 1 KiB frame from the core: rows top to bottom, MSB leftmost, 1 = dark. */
  bits: Uint8Array;
  /** Time until the next frame. */
  delayMs: number;
}

/** A looping GIF of LCD frames at `scale`, in two colours: `[ink, glass]` as RGB. */
export function encodeGif(
  frames: GifFrame[],
  scale: number,
  ink: [number, number, number],
  glass: [number, number, number],
  width = 128,
  height = 64,
): Uint8Array {
  const [w, h] = [width * scale, height * scale];
  const parts: number[] = [];
  const u16 = (v: number) => parts.push(v & 0xff, (v >> 8) & 0xff);
  const text = (s: string) => parts.push(...[...s].map((c) => c.charCodeAt(0)));
  text("GIF89a");
  u16(w);
  u16(h);
  parts.push(0x80, 0, 0); // global palette of 2 colours
  parts.push(...glass, ...ink); // index 0 = glass, 1 = ink
  // Netscape loop extension: repeat forever.
  parts.push(0x21, 0xff, 11);
  text("NETSCAPE2.0");
  parts.push(3, 1, 0, 0, 0);
  for (const f of frames) {
    parts.push(0x21, 0xf9, 4, 0);
    u16(Math.max(2, Math.round(f.delayMs / 10))); // hundredths; below 2 many viewers misbehave
    parts.push(0, 0);
    parts.push(0x2c);
    u16(0);
    u16(0);
    u16(w);
    u16(h);
    parts.push(0);
    const px = new Uint8Array(w * h);
    for (let y = 0; y < h; y++) {
      const sy = Math.floor(y / scale);
      for (let x = 0; x < w; x++) {
        const i = sy * width + Math.floor(x / scale);
        px[y * w + x] = (f.bits[i >> 3] >> (7 - (i & 7))) & 1;
      }
    }
    const min = 2; // GIF's minimum, also for two colours
    const data = lzw(px, min);
    parts.push(min);
    for (let o = 0; o < data.length; o += 255) {
      const n = Math.min(255, data.length - o);
      parts.push(n);
      for (let k = 0; k < n; k++) parts.push(data[o + k]);
    }
    parts.push(0);
  }
  parts.push(0x3b);
  return Uint8Array.from(parts);
}

/** The standard decoder, for tests and nothing else. */
export function lzwDecode(data: Uint8Array, minCodeSize: number, count: number): Uint8Array {
  const clear = 1 << minCodeSize;
  const end = clear + 1;
  const out: number[] = [];
  let table: number[][] = [];
  let width = minCodeSize + 1;
  const reset = () => {
    table = [];
    for (let i = 0; i < clear; i++) table[i] = [i];
    table[clear] = [];
    table[end] = [];
    width = minCodeSize + 1;
  };
  reset();
  let acc = 0;
  let bits = 0;
  let prev: number[] | null = null;
  for (const byte of data) {
    acc |= byte << bits;
    bits += 8;
    while (bits >= width) {
      const code = acc & ((1 << width) - 1);
      acc >>>= width;
      bits -= width;
      if (code === clear) {
        reset();
        prev = null;
        continue;
      }
      if (code === end) return Uint8Array.from(out.slice(0, count));
      let entry: number[];
      if (code < table.length) entry = table[code];
      else if (prev) entry = [...prev, prev[0]];
      else throw new Error(`bad LZW code ${code}`);
      out.push(...entry);
      if (prev) {
        table.push([...prev, entry[0]]);
        if (table.length === 1 << width && width < 12) width++;
      }
      prev = entry;
    }
  }
  return Uint8Array.from(out.slice(0, count));
}

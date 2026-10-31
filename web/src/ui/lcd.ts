// SPDX-License-Identifier: GPL-3.0-or-later
// Paints the 1-bit 128×64 frame from the core into RGBA, tinted by the backlight level.

export const LCD_W = 128;
export const LCD_H = 64;

/** Glass colour with the backlight off (grey) and at full brightness (orange). */
const OFF = [0x9a, 0x8f, 0x80];
const ON = [0xff, 0x8c, 0x29];
const INK = [0x10, 0x10, 0x10];

export function glass(backlight: number): [number, number, number] {
  const k = Math.min(1, Math.max(0, backlight / 255));
  return [0, 1, 2].map((i) => Math.round(OFF[i] + (ON[i] - OFF[i]) * k)) as [number, number, number];
}

/** `bits`: 1 KiB, rows top to bottom, MSB = leftmost pixel, 1 = dark. Writes into `rgba`. */
export function paint(bits: Uint8Array, backlight: number, rgba: Uint8ClampedArray): void {
  const bg = glass(backlight);
  for (let i = 0; i < LCD_W * LCD_H; i++) {
    const dark = (bits[i >> 3] >> (7 - (i & 7))) & 1;
    const c = dark ? INK : bg;
    const o = i * 4;
    rgba[o] = c[0];
    rgba[o + 1] = c[1];
    rgba[o + 2] = c[2];
    rgba[o + 3] = 255;
  }
}

// SPDX-License-Identifier: GPL-3.0-or-later
// The screen as text, for where a picture cannot go: a pull request comment made through the API cannot
// attach images. Each Braille character holds 2×4 pixels (Unicode block U+2800), so 128×64 is 64×16.

/** Dot bit for (x, y) inside a cell, in Unicode's numbering. */
const DOT = [
  [0x01, 0x08],
  [0x02, 0x10],
  [0x04, 0x20],
  [0x40, 0x80],
];

/** @param {Uint8Array} frame 128×64, one bit per pixel, 1 = dark */
export function toBraille(frame) {
  const dark = (x, y) => (frame[(y * 128 + x) >> 3] >> (7 - ((y * 128 + x) & 7))) & 1;
  const lines = [];
  for (let row = 0; row < 16; row++) {
    let line = "";
    for (let col = 0; col < 64; col++) {
      let cell = 0;
      for (let y = 0; y < 4; y++) {
        for (let x = 0; x < 2; x++) {
          if (dark(col * 2 + x, row * 4 + y)) cell |= DOT[y][x];
        }
      }
      line += String.fromCodePoint(0x2800 + cell);
    }
    lines.push(line);
  }
  return lines;
}

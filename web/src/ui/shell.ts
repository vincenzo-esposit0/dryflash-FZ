// SPDX-License-Identifier: GPL-3.0-or-later
// The device, drawn as pixel art by code on the LCD's own pixel grid (ADR-0015): one art pixel is one
// LCD pixel, whatever the scale, so the screen sits in the drawing without a seam. No image files.

export const COLOURS = [
  [0, 0, 0, 0], // 0 transparent
  [0x1b, 0x1c, 0x20, 255], // 1 outline
  [0xe9, 0xe9, 0xe6, 255], // 2 body
  [0xfb, 0xfb, 0xf8, 255], // 3 highlight
  [0xc9, 0xc9, 0xc4, 255], // 4 body shadow
  [0xb5, 0xb5, 0xb0, 255], // 5 side
  [0x8f, 0x8f, 0x8b, 255], // 6 side shadow
  [0x3a, 0x3b, 0x3f, 255], // 7 screen surround
  [0xff, 0x8a, 0x1f, 255], // 8 orange
  [0xc9, 0x63, 0x0f, 255], // 9 orange shadow
  [0xff, 0xab, 0x5c, 255], // 10 orange highlight
  [0x2a, 0x2b, 0x2e, 255], // 11 slots
] as const;

const [OUTLINE, BODY, LIGHT, SHADOW, SIDE, SIDE_DARK, SURROUND, ORANGE, ORANGE_DARK, ORANGE_LIGHT, SLOT] = [
  1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11,
];

/** 5×7 letters for the name on the body. */
const GLYPHS: Record<string, string[]> = {
  d: ["00001", "00001", "01111", "10001", "10001", "10001", "01111"],
  r: ["00000", "00000", "10110", "11001", "10000", "10000", "10000"],
  y: ["00000", "10001", "10001", "10001", "01111", "00001", "01110"],
  f: ["00110", "01001", "01000", "11100", "01000", "01000", "01000"],
  l: ["01100", "00100", "00100", "00100", "00100", "00100", "01110"],
  a: ["00000", "00000", "01110", "00001", "01111", "10001", "01111"],
  s: ["00000", "00000", "01111", "10000", "01110", "00001", "11110"],
  h: ["10000", "10000", "10110", "11001", "10001", "10001", "10001"],
};

export const LCD_ART_W = 128;
export const LCD_ART_H = 64;

export interface Circle {
  cx: number;
  cy: number;
  r: number;
}

export interface Geometry {
  width: number;
  height: number;
  /** Top-left of the LCD. */
  lcd: { x: number; y: number };
  disc: Circle;
  back: Circle;
  led: { x: number; y: number };
  name: { x: number; y: number };
  /** Rows from here down are the body's side, the "thickness" of the device. */
  sideY: number;
}

const DISC_R = 32;
const BACK_R = 9;
const SIDE_ROWS = 10;

/** Screen on the left, controls on the right, as the device is held. */
export function landscape(): Geometry {
  const margin = 12;
  const lcd = { x: margin + 4, y: margin + 3 };
  const discCx = lcd.x + LCD_ART_W + 4 + 10 + DISC_R + 2;
  const faceBottom = lcd.y + LCD_ART_H + 4 + 13;
  return {
    width: discCx + DISC_R + 8 + BACK_R,
    height: faceBottom + SIDE_ROWS,
    lcd,
    disc: { cx: discCx, cy: lcd.y + DISC_R - 1, r: DISC_R },
    back: { cx: discCx + DISC_R + 2, cy: lcd.y + 2 * DISC_R + 2, r: BACK_R },
    led: { x: lcd.x + LCD_ART_W + 7, y: lcd.y + LCD_ART_H - 2 },
    name: { x: lcd.x, y: lcd.y + LCD_ART_H + 7 },
    sideY: faceBottom,
  };
}

/** For narrow screens: controls below the screen. */
export function portrait(): Geometry {
  const margin = 10;
  const lcd = { x: margin + 3, y: margin + 3 };
  const width = lcd.x * 2 + LCD_ART_W;
  const discCy = lcd.y + LCD_ART_H + 4 + 12 + 8 + DISC_R;
  const faceBottom = discCy + DISC_R + 8;
  return {
    width,
    height: faceBottom + SIDE_ROWS,
    lcd,
    disc: { cx: Math.floor(width / 2) - 6, cy: discCy, r: DISC_R },
    back: { cx: width - margin - 8, cy: discCy + DISC_R - 2, r: BACK_R },
    led: { x: width - margin - 6, y: lcd.y + LCD_ART_H + 7 },
    name: { x: lcd.x, y: lcd.y + LCD_ART_H + 7 },
    sideY: faceBottom,
  };
}

/** Chooses layout and scale for the available width and height. Scales are whole numbers. */
export function fit(available: number, height = Infinity, maxScale = 4): { geometry: Geometry; scale: number } {
  const wide = landscape();
  const scaleFor = (g: Geometry) =>
    Math.min(maxScale, Math.floor(available / g.width), Math.floor(height / g.height));
  const s = scaleFor(wide);
  if (s >= 2) return { geometry: wide, scale: s };
  const tall = portrait();
  return { geometry: tall, scale: Math.max(1, scaleFor(tall)) };
}

/** Is (x, y) on the body? The silhouette: long cuts on the left corners, rounded right ones. */
export function inBody(g: Geometry, x: number, y: number): boolean {
  if (x < 0 || y < 0 || x >= g.width || y >= g.height) return false;
  const [w, h] = [g.width, g.height];
  const cutTL = 14;
  const cutBL = 18;
  if (x + y < cutTL) return false;
  if (x + (h - 1 - y) < cutBL) return false;
  const round = (cx: number, cy: number, r: number) => (x - cx) ** 2 + (y - cy) ** 2 <= r * r;
  const rTR = 12;
  const rBR = 16;
  if (x >= w - rTR && y < rTR && !round(w - rTR - 0.5, rTR - 0.5, rTR)) return false;
  if (x >= w - rBR && y >= h - rBR && !round(w - rBR - 0.5, h - rBR - 0.5, rBR)) return false;
  return true;
}

/** Draws the device: width × height palette indices (COLOURS), row by row. */
export function drawShell(g: Geometry): Uint8Array {
  const { width: w, height: h } = g;
  const px = new Uint8Array(w * h);
  const set = (x: number, y: number, c: number) => {
    if (x >= 0 && y >= 0 && x < w && y < h) px[y * w + x] = c;
  };
  for (let y = 0; y < h; y++) {
    for (let x = 0; x < w; x++) {
      if (!inBody(g, x, y)) continue;
      const edge =
        !inBody(g, x - 1, y) || !inBody(g, x + 1, y) || !inBody(g, x, y - 1) || !inBody(g, x, y + 1);
      let c = BODY;
      if (edge) c = OUTLINE;
      else if (y >= g.sideY) c = y >= g.sideY + SIDE_ROWS - 4 ? SIDE_DARK : SIDE;
      else if (y === g.sideY - 1) c = SHADOW;
      else if (!inBody(g, x, y - 2) || !inBody(g, x - 2, y)) c = LIGHT;
      else if (!inBody(g, x, y + 2) || !inBody(g, x + 2, y)) c = SHADOW;
      set(x, y, c);
    }
  }
  // Where the face meets the side.
  for (let x = 0; x < w; x++) if (inBody(g, x, g.sideY) && px[g.sideY * w + x] !== OUTLINE) set(x, g.sideY, SIDE_DARK);

  // Screen surround, with its corners cut.
  const pad = 3;
  const [sx0, sy0, sx1, sy1] = [g.lcd.x - pad, g.lcd.y - pad, g.lcd.x + LCD_ART_W + pad, g.lcd.y + LCD_ART_H + pad];
  for (let y = sy0; y < sy1; y++) {
    for (let x = sx0; x < sx1; x++) {
      const corner = (x === sx0 || x === sx1 - 1) && (y === sy0 || y === sy1 - 1);
      if (!corner) set(x, y, SURROUND);
    }
  }

  const disc = (c: Circle, fill: number, shade: number, light: number) => {
    for (let y = c.cy - c.r - 1; y <= c.cy + c.r + 1; y++) {
      for (let x = c.cx - c.r - 1; x <= c.cx + c.r + 1; x++) {
        const d = Math.hypot(x - c.cx, y - c.cy);
        if (d > c.r + 0.5) continue;
        if (d > c.r - 0.7) set(x, y, OUTLINE);
        else if (x - c.cx + (y - c.cy) > c.r * 0.9) set(x, y, shade);
        else if (x - c.cx + (y - c.cy) < -c.r * 0.9) set(x, y, light);
        else set(x, y, fill);
      }
    }
  };
  disc(g.disc, ORANGE, ORANGE_DARK, ORANGE_LIGHT);
  disc({ cx: g.disc.cx, cy: g.disc.cy, r: 10 }, ORANGE_LIGHT, ORANGE, ORANGE_LIGHT);

  // Arrows on the disc, pointing away from the middle.
  const reach = 22;
  for (let i = 0; i < 6; i++) {
    for (let j = -i; j <= i; j++) {
      set(g.disc.cx + j, g.disc.cy - reach - 3 + i, ORANGE_DARK);
      set(g.disc.cx + j, g.disc.cy + reach + 3 - i, ORANGE_DARK);
      set(g.disc.cx - reach - 3 + i, g.disc.cy + j, ORANGE_DARK);
      set(g.disc.cx + reach + 3 - i, g.disc.cy + j, ORANGE_DARK);
    }
  }

  disc(g.back, ORANGE, ORANGE_DARK, ORANGE_LIGHT);
  // The return arrow on the back button.
  const b = g.back;
  for (let x = -3; x <= 3; x++) set(b.cx + x, b.cy - 2, ORANGE_DARK);
  for (let y = -2; y <= 2; y++) set(b.cx + 3, b.cy + y, ORANGE_DARK);
  for (let x = -1; x <= 3; x++) set(b.cx + x, b.cy + 2, ORANGE_DARK);
  for (let i = 1; i <= 2; i++) {
    set(b.cx - 3 + i, b.cy - 2 - i, ORANGE_DARK);
    set(b.cx - 3 + i, b.cy - 2 + i, ORANGE_DARK);
  }

  // The name.
  let nx = g.name.x;
  for (const ch of "dryflash") {
    GLYPHS[ch].forEach((row, y) => {
      [...row].forEach((bit, x) => {
        if (bit === "1") set(nx + x, g.name.y + y, ORANGE);
      });
    });
    nx += 6;
  }

  // Ports on the side.
  const slot = (x: number, len: number) => {
    for (let i = 0; i < len; i++) {
      set(x + i, g.sideY + 3, SLOT);
      set(x + i, g.sideY + 4, SLOT);
    }
  };
  slot(Math.floor(w * 0.45), 18);
  slot(Math.floor(w * 0.45) + 24, 6);
  return px;
}

/** RGBA pixels for a canvas. */
export function toRgba(px: Uint8Array, out: Uint8ClampedArray): void {
  for (let i = 0; i < px.length; i++) {
    const c = COLOURS[px[i]];
    out[i * 4] = c[0];
    out[i * 4 + 1] = c[1];
    out[i * 4 + 2] = c[2];
    out[i * 4 + 3] = c[3];
  }
}

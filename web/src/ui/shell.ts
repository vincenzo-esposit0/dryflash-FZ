// SPDX-License-Identifier: GPL-3.0-or-later
// The device as pixel art (ADR-0015), rasterized by code and seen from the front: a model of the body's
// face, in "face units" where one unit is one LCD pixel, drawn in chunkier art pixels with its thickness
// showing below. The real LCD canvas and buttons sit on the same face.

/** The word on the body. The maintainer's choice (ADR-0015): change it here and nowhere else. */
export const BODY_NAME = "FL1PP3R";

export const COLOURS = [
  [0, 0, 0, 0], // 0 transparent
  [0x14, 0x15, 0x24, 255], // 1 outline
  [0xe6, 0xe6, 0xe6, 255], // 2 body
  [0xff, 0xff, 0xff, 255], // 3 highlight
  [0xcf, 0xcf, 0xd2, 255], // 4 body shadow
  [0xbd, 0xbd, 0xc2, 255], // 5 side, lit
  [0x9c, 0x9c, 0xa3, 255], // 6 side
  [0x5d, 0x5e, 0x66, 255], // 7 screen surround
  [0xf5, 0x7c, 0x1f, 255], // 8 orange
  [0xc4, 0x58, 0x10, 255], // 9 orange shadow
  [0xff, 0xa2, 0x4d, 255], // 10 orange highlight
  [0x2c, 0x2d, 0x38, 255], // 11 slots
  [0xff, 0x96, 0x2a, 255], // 12 LCD glass under the canvas
] as const;

const [OUTLINE, BODY, LIGHT, SHADOW, SIDE, SIDE_DARK, SURROUND, ORANGE, ORANGE_DARK, ORANGE_LIGHT, SLOT, GLASS] = [
  1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12,
];

/** 5×7 capitals for the word on the body. */
const GLYPHS: Record<string, string[]> = {
  F: ["11111", "10000", "10000", "11110", "10000", "10000", "10000"],
  L: ["10000", "10000", "10000", "10000", "10000", "10000", "11111"],
  P: ["11110", "10001", "10001", "11110", "10000", "10000", "10000"],
  R: ["11110", "10001", "10001", "11110", "10100", "10010", "10001"],
  "1": ["00100", "01100", "00100", "00100", "00100", "00100", "01110"],
  "3": ["11110", "00001", "00001", "01110", "00001", "00001", "11110"],
};

export const LCD_WIDTH = 128;
export const LCD_HEIGHT = 64;

export interface Circle {
  cx: number;
  cy: number;
  r: number;
}

/** The face of the body, in face units. */
export interface Model {
  width: number;
  height: number;
  lcd: { x: number; y: number };
  disc: Circle;
  back: Circle;
  led: { x: number; y: number };
  name: { x: number; y: number; scale: number };
  /** From here down the face slopes away: the bevel the name sits on. */
  facetY: number;
  /** Size of the cuts on the left end. */
  chamfer: number;
}

/** Screen on the left, controls on the right, as the device is held. */
export function landscape(): Model {
  return {
    width: 270,
    height: 124,
    lcd: { x: 28, y: 14 },
    disc: { cx: 208, cy: 50, r: 38 },
    back: { cx: 248, cy: 96, r: 14 },
    led: { x: 162, y: 74 },
    name: { x: 44, y: 94, scale: 3 },
    facetY: 86,
    chamfer: 34,
  };
}

/** For narrow screens: controls below the screen. */
export function portrait(): Model {
  return {
    width: 152,
    height: 186,
    lcd: { x: 12, y: 12 },
    disc: { cx: 66, cy: 138, r: 36 },
    back: { cx: 128, cy: 164, r: 11 },
    led: { x: 140, y: 84 },
    name: { x: 12, y: 84, scale: 2 },
    facetY: 80,
    chamfer: 14,
  };
}

/** The silhouette: long cuts on the left end, rounded corners on the right. */
export function inBody(m: Model, u: number, v: number): boolean {
  if (u < 0 || v < 0 || u >= m.width || v >= m.height) return false;
  const cut = m.chamfer;
  if (u + v < cut) return false;
  if (u + (m.height - v) < cut * 0.9) return false;
  const round = (cx: number, cy: number, r: number) => (u - cx) ** 2 + (v - cy) ** 2 <= r * r;
  const r = 20;
  if (u > m.width - r && v < r && !round(m.width - r, r, r)) return false;
  if (u > m.width - r && v > m.height - r && !round(m.width - r, m.height - r, r)) return false;
  return true;
}

/** Colour of a point of the face (buttons, screen and name included). */
export function faceColour(m: Model, u: number, v: number): number {
  const { lcd, disc, back, led, name } = m;
  // The screen and its surround.
  const pad = 4;
  const [l, t, r, b] = [lcd.x, lcd.y, lcd.x + LCD_WIDTH, lcd.y + LCD_HEIGHT];
  if (u >= l && u < r && v >= t && v < b) return GLASS;
  if (u >= l - pad && u < r + pad && v >= t - pad && v < b + pad) return SURROUND;
  // The controls.
  const round = (c: Circle) => {
    const [x, y] = [u - c.cx, v - c.cy];
    const d = Math.hypot(x, y);
    if (d > c.r) return 0;
    if (d > c.r - 2) return ORANGE_DARK;
    if (x + y > c.r * 0.95) return ORANGE_DARK;
    if (x + y < -c.r * 0.95) return ORANGE_LIGHT;
    return ORANGE;
  };
  const inDisc = round(disc);
  if (inDisc) {
    const [x, y] = [u - disc.cx, v - disc.cy];
    const d = Math.hypot(x, y);
    if (d < 12) return d > 10 ? ORANGE_DARK : x + y < -6 ? ORANGE_LIGHT : ORANGE;
    const arrow = (along: number, across: number) => {
      const from = disc.r - 18;
      return along > from && along < from + 9 && Math.abs(across) < from + 9 - along;
    };
    if (arrow(-y, x) || arrow(y, x) || arrow(-x, y) || arrow(x, y)) return ORANGE_DARK;
    return inDisc;
  }
  const inBack = round(back);
  if (inBack) {
    const [x, y] = [Math.round(u - back.cx), Math.round(v - back.cy)];
    const glyph =
      (y === -3 && x >= -4 && x <= 3) ||
      (x === 4 && y >= -3 && y <= 3) ||
      (y === 4 && x >= -1 && x <= 3) ||
      (x === -4 + Math.abs(y + 3) && Math.abs(y + 3) <= 2);
    return glyph ? ORANGE_DARK : inBack;
  }
  if (u >= led.x && u < led.x + 4 && v >= led.y && v < led.y + 4) return SLOT;
  // The word.
  const n = name.scale;
  const lit = (a: number, b: number) => {
    const nu = Math.floor((a - name.x) / n);
    const nv = Math.floor((b - name.y) / n);
    if (nv < 0 || nv >= 7 || nu < 0) return false;
    const ch = BODY_NAME[Math.floor(nu / 7)];
    return !!ch && nu % 7 < 5 && GLYPHS[ch][nv][nu % 7] === "1";
  };
  // Bold, so that the strokes survive the coarse art pixels.
  if (lit(u, v) || lit(u - 1, v) || lit(u, v - 1)) return ORANGE;
  // The body: bright bevels at the top and left, shade at the bottom and right, a slope for the name.
  const bevel = 5;
  if (v >= m.facetY && v < m.facetY + 2) return LIGHT;
  if (!inBody(m, u, v - bevel) || !inBody(m, u - bevel, v)) return LIGHT;
  if (!inBody(m, u, v + bevel) || !inBody(m, u + bevel, v)) return SHADOW;
  if (v > m.facetY) return SHADOW;
  return BODY;
}

// ---- Fitting and drawing ----

/** Art pixels are chunkier than the LCD's: this many face units each. */
export const ART_STEP = 2;
/** Rows of body side below the face, in face units. */
export const SIDE_UNITS = 12;

/** The device at a whole-number scale (CSS px per face unit). */
export interface View {
  model: Model;
  scale: number;
}

const totalHeight = (m: Model) => m.height + SIDE_UNITS + ART_STEP;

/** Screen beside the controls where that fits at 2× or more, stacked on phones. */
export function fitView(available: number, height = Infinity, maxScale = 4): View {
  const scaleFor = (m: Model) => Math.min(maxScale, Math.floor(available / m.width), Math.floor(height / totalHeight(m)));
  const wide = landscape();
  if (scaleFor(wide) >= 2) return { model: wide, scale: scaleFor(wide) };
  const tall = portrait();
  return { model: tall, scale: Math.max(1, scaleFor(tall)) };
}

/** Size of the drawing in art pixels. */
export function artSize(m: Model): { width: number; height: number } {
  return { width: Math.ceil(m.width / ART_STEP), height: Math.ceil(totalHeight(m) / ART_STEP) };
}

/** Draws the device: width × height palette indices (COLOURS), row by row. */
export function drawView(m: Model): Uint8Array {
  const { width: w, height: h } = artSize(m);
  const px = new Uint8Array(w * h);
  for (let y = 0; y < h; y++) {
    for (let x = 0; x < w; x++) {
      const [u, v] = [(x + 0.5) * ART_STEP, (y + 0.5) * ART_STEP];
      if (inBody(m, u, v)) {
        px[y * w + x] = faceColour(m, u, v);
        continue;
      }
      // The thickness: the outline of the face, repeated downwards.
      for (let t = ART_STEP; t <= SIDE_UNITS; t += ART_STEP) {
        if (!inBody(m, u, v - t)) continue;
        let c = t <= SIDE_UNITS * 0.4 ? SIDE : SIDE_DARK;
        const slot = t > SIDE_UNITS * 0.4 && t <= SIDE_UNITS * 0.8;
        if (slot && ((u > m.width * 0.5 && u < m.width * 0.68) || (u > m.width * 0.72 && u < m.width * 0.76))) c = SLOT;
        px[y * w + x] = c;
        break;
      }
    }
  }
  const out = px.slice();
  const at = (x: number, y: number) => (x < 0 || y < 0 || x >= w || y >= h ? 0 : px[y * w + x]);
  for (let y = 0; y < h; y++) {
    for (let x = 0; x < w; x++) {
      if (px[y * w + x] && (!at(x - 1, y) || !at(x + 1, y) || !at(x, y - 1) || !at(x, y + 1))) out[y * w + x] = OUTLINE;
    }
  }
  return out;
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

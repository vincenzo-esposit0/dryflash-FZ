// SPDX-License-Identifier: GPL-3.0-or-later
// The device as pixel art (ADR-0015), rasterized by code: a model of the body's face, in "face units"
// where one unit is one LCD pixel, seen through a projective map and extruded for its thickness. Every
// art pixel is aligned to the screen, whatever the angle: staircase edges rather than rotated squares.
// The real LCD canvas and buttons sit on the same face through the same map (`faceMatrix`).

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

// ---- Projection ----

export type Matrix = [number, number, number, number, number, number, number, number, number];
export type Point = [number, number];

/** The projective map taking the rectangle (0,0)–(w,h) to a quad, corners in order TL, TR, BR, BL. */
export function quadMatrix(w: number, h: number, q: [Point, Point, Point, Point]): Matrix {
  const [[x0, y0], [x1, y1], [x2, y2], [x3, y3]] = q;
  // Unit square to quad (Heckbert).
  const [dx1, dx2, sx] = [x1 - x2, x3 - x2, x0 - x1 + x2 - x3];
  const [dy1, dy2, sy] = [y1 - y2, y3 - y2, y0 - y1 + y2 - y3];
  const det = dx1 * dy2 - dx2 * dy1;
  const g = (sx * dy2 - dx2 * sy) / det;
  const k = (dx1 * sy - sx * dy1) / det;
  return [
    (x1 - x0 + g * x1) / w,
    (x3 - x0 + k * x3) / h,
    x0,
    (y1 - y0 + g * y1) / w,
    (y3 - y0 + k * y3) / h,
    y0,
    g / w,
    k / h,
    1,
  ];
}

export function apply(m: Matrix, u: number, v: number): Point {
  const d = m[6] * u + m[7] * v + m[8];
  return [(m[0] * u + m[1] * v + m[2]) / d, (m[3] * u + m[4] * v + m[5]) / d];
}

export function invert(m: Matrix): Matrix {
  const [a, b, c, d, e, f, g, h, i] = m;
  return [e * i - f * h, c * h - b * i, b * f - c * e, f * g - d * i, a * i - c * g, c * d - a * f, d * h - e * g, b * g - a * h, a * e - b * d];
}

/** CSS `matrix3d` placing the face (1 unit = 1 CSS px) on the drawing, at `cell` CSS px per art pixel. */
export function faceMatrix(m: Matrix, cell: number): string {
  const n = (x: number) => +x.toFixed(9);
  return `matrix3d(${n(m[0] * cell)},${n(m[3] * cell)},0,${n(m[6])},${n(m[1] * cell)},${n(m[4] * cell)},0,${n(m[7])},0,0,1,0,${n(m[2] * cell)},${n(m[5] * cell)},0,${n(m[8])})`;
}

/** A way of showing the device: the model, its map to art pixels, and the size of an art pixel. */
export interface View {
  model: Model;
  matrix: Matrix;
  /** Art pixels. */
  width: number;
  height: number;
  /** CSS px per art pixel. */
  cell: number;
  /** Where the thickness goes, in art pixels. */
  depth: Point;
  tilted: boolean;
}

const TILT_WIDTH = 200;
const TILT_HEIGHT = 132;

function tiltedView(cell: number): View {
  const model = landscape();
  return {
    model,
    matrix: quadMatrix(model.width, model.height, [
      [16, 8],
      [192, 36],
      [186, 106],
      [6, 74],
    ]),
    width: TILT_WIDTH,
    height: TILT_HEIGHT,
    cell,
    depth: [3, 18],
    tilted: true,
  };
}

function frontView(model: Model, cell: number): View {
  const depth: Point = [0, 8];
  return {
    model,
    matrix: [1, 0, 0, 0, 1, 0, 0, 0, 1],
    width: model.width,
    height: model.height + depth[1] + 1,
    cell,
    depth,
    tilted: false,
  };
}

/**
 * Chooses a view for the available space. The angled view needs chunky pixels and room for a readable
 * screen; otherwise the device faces the viewer, stacked on phones. Scales are whole numbers.
 */
export function fitView(available: number, height = Infinity, front = false, maxCell = 6): View {
  const cell = Math.min(maxCell, Math.floor(available / TILT_WIDTH), Math.floor(height / TILT_HEIGHT));
  if (!front && cell >= 3) return tiltedView(cell);
  const wide = landscape();
  const s = Math.min(4, Math.floor(available / wide.width), Math.floor(height / (wide.height + 9)));
  if (s >= 2) return frontView(wide, s);
  const tall = portrait();
  return frontView(tall, Math.max(1, Math.min(4, Math.floor(available / tall.width), Math.floor(height / (tall.height + 9)))));
}

/** Draws the view: width × height palette indices (COLOURS), row by row. */
export function drawView(view: View): Uint8Array {
  const { model: m, width: w, height: h, depth } = view;
  const inv = invert(view.matrix);
  const px = new Uint8Array(w * h);
  const steps = Math.max(1, Math.ceil(Math.max(Math.abs(depth[0]), Math.abs(depth[1]))));
  for (let y = 0; y < h; y++) {
    for (let x = 0; x < w; x++) {
      // The face first, then the side layers behind it; the first hit is what shows.
      const [fu, fv] = apply(inv, x + 0.5, y + 0.5);
      if (inBody(m, fu, fv)) {
        px[y * w + x] = faceColour(m, fu, fv);
        continue;
      }
      for (let t = 1; t <= steps; t++) {
        const k = t / steps;
        const [u, v] = apply(inv, x + 0.5 - depth[0] * k, y + 0.5 - depth[1] * k);
        if (!inBody(m, u, v)) continue;
        let c = k < 0.35 ? SIDE : SIDE_DARK;
        // Ports along the long side, as a row of slots.
        const slot = v > m.height - 14 && k > 0.45 && k < 0.75;
        if (slot && ((u > m.width * 0.5 && u < m.width * 0.68) || (u > m.width * 0.72 && u < m.width * 0.76))) c = SLOT;
        px[y * w + x] = c;
        break;
      }
    }
  }
  // The outline: every painted pixel next to an empty one, and the line where the face meets the side.
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

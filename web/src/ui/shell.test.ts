// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from "vitest";
import {
  BODY_NAME,
  COLOURS,
  LCD_HEIGHT,
  LCD_WIDTH,
  apply,
  drawView,
  faceColour,
  faceMatrix,
  fitView,
  inBody,
  invert,
  landscape,
  portrait,
  quadMatrix,
  toRgba,
  type Circle,
  type Model,
} from "./shell";

const circleInside = (m: Model, c: Circle) =>
  [
    [c.cx - c.r, c.cy],
    [c.cx + c.r, c.cy],
    [c.cx, c.cy - c.r],
    [c.cx, c.cy + c.r],
  ].every(([u, v]) => inBody(m, u, v));

describe.each([
  ["landscape", landscape()],
  ["portrait", portrait()],
])("%s model", (_n, m) => {
  it("holds the screen and the controls on the face", () => {
    for (const [u, v] of [
      [m.lcd.x - 4, m.lcd.y - 4],
      [m.lcd.x + LCD_WIDTH + 3, m.lcd.y - 4],
      [m.lcd.x - 4, m.lcd.y + LCD_HEIGHT + 3],
      [m.lcd.x + LCD_WIDTH + 3, m.lcd.y + LCD_HEIGHT + 3],
    ]) {
      expect(inBody(m, u, v)).toBe(true);
    }
    expect(circleInside(m, m.disc)).toBe(true);
    expect(circleInside(m, m.back)).toBe(true);
  });

  it("keeps the controls off the screen and each other", () => {
    const off = (c: Circle) =>
      c.cx + c.r < m.lcd.x - 4 ||
      c.cx - c.r > m.lcd.x + LCD_WIDTH + 4 ||
      c.cy + c.r < m.lcd.y - 4 ||
      c.cy - c.r > m.lcd.y + LCD_HEIGHT + 4;
    expect(off(m.disc)).toBe(true);
    expect(off(m.back)).toBe(true);
    expect(Math.hypot(m.disc.cx - m.back.cx, m.disc.cy - m.back.cy)).toBeGreaterThan(m.disc.r + m.back.r);
  });

  it("fits the word on the body, clear of the screen", () => {
    const { x, y, scale } = m.name;
    const right = x + (BODY_NAME.length * 7 - 2) * scale;
    expect(right).toBeLessThan(m.disc.cx + m.disc.r + m.width);
    expect(y).toBeGreaterThanOrEqual(m.lcd.y + LCD_HEIGHT + 4);
    expect(inBody(m, x, y)).toBe(true);
    expect(inBody(m, right, y + 7 * scale - 1)).toBe(true);
  });

  it("paints the screen under the canvas in one colour", () => {
    const c = faceColour(m, m.lcd.x + 1, m.lcd.y + 1);
    expect(faceColour(m, m.lcd.x + LCD_WIDTH - 1, m.lcd.y + LCD_HEIGHT - 1)).toBe(c);
  });
});

describe("projection", () => {
  const q: [[number, number], [number, number], [number, number], [number, number]] = [
    [16, 8],
    [192, 36],
    [186, 106],
    [6, 74],
  ];
  const h = quadMatrix(270, 124, q);

  it("takes the corners to the quad", () => {
    [
      [0, 0],
      [270, 0],
      [270, 124],
      [0, 124],
    ].forEach(([u, v], i) => {
      const [x, y] = apply(h, u, v);
      expect(x).toBeCloseTo(q[i][0], 6);
      expect(y).toBeCloseTo(q[i][1], 6);
    });
  });

  it("inverts", () => {
    const [x, y] = apply(h, 100, 50);
    const [u, v] = apply(invert(h), x, y);
    expect(u).toBeCloseTo(100, 6);
    expect(v).toBeCloseTo(50, 6);
  });

  it("writes a CSS matrix that agrees with the map", () => {
    const n = faceMatrix(h, 3).slice(9, -1).split(",").map(Number);
    const [u, v] = [100, 50];
    const w = n[3] * u + n[7] * v + n[15];
    const [x, y] = apply(h, u, v);
    expect((n[0] * u + n[4] * v + n[12]) / w).toBeCloseTo(x * 3, 3);
    expect((n[1] * u + n[5] * v + n[13]) / w).toBeCloseTo(y * 3, 3);
  });
});

describe("fitView", () => {
  it("never overflows and uses whole scales", () => {
    for (const w of [320, 343, 375, 480, 600, 700, 900, 1148, 2000]) {
      const v = fitView(w);
      expect(Number.isInteger(v.cell)).toBe(true);
      expect(v.width * v.cell).toBeLessThanOrEqual(w);
    }
  });

  it("is angled where there is room, like the reference", () => {
    const v = fitView(1148);
    expect(v.tilted).toBe(true);
    expect(v.cell).toBeGreaterThanOrEqual(3);
  });

  it("faces the viewer on a phone, stacked, with a readable screen", () => {
    const v = fitView(343);
    expect(v.tilted).toBe(false);
    expect(v.model.width).toBe(portrait().width);
    expect(v.cell).toBe(2);
  });

  it("can be asked to face the viewer", () => {
    const v = fitView(1148, Infinity, true);
    expect(v.tilted).toBe(false);
    expect(v.model.width).toBe(landscape().width);
    expect(v.cell).toBe(4);
  });

  it("respects a height limit", () => {
    const v = fitView(4000, 500, false, 12);
    expect(v.height * v.cell).toBeLessThanOrEqual(500);
  });
});

describe("drawView", () => {
  it.each([
    ["angled", fitView(1148)],
    ["front", fitView(1148, Infinity, true)],
    ["stacked", fitView(343)],
  ])("%s: known colours, outlined, with a side", (_n, v) => {
    const px = drawView(v);
    expect(px.length).toBe(v.width * v.height);
    expect(Math.max(...px)).toBeLessThan(COLOURS.length);
    expect(px[0]).toBe(0);
    expect(px.includes(1)).toBe(true);
    expect(px.includes(6)).toBe(true);
    const rgba = new Uint8ClampedArray(px.length * 4);
    toRgba(px, rgba);
    expect(rgba[3]).toBe(0);
  });

  it("paints the word somewhere on the angled body", () => {
    const v = fitView(1148);
    const m = v.model;
    let orangeBelowScreen = 0;
    for (let u = m.name.x; u < m.name.x + 100; u++) {
      if (faceColour(m, u, m.name.y + 1) === 8) orangeBelowScreen++;
    }
    expect(orangeBelowScreen).toBeGreaterThan(5);
  });
});

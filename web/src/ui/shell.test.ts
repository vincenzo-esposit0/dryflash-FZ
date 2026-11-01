// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from "vitest";
import { COLOURS, LCD_ART_H, LCD_ART_W, drawShell, fit, inBody, landscape, portrait, toRgba, type Circle, type Geometry } from "./shell";

const circleInside = (g: Geometry, c: Circle) =>
  [
    [c.cx - c.r, c.cy],
    [c.cx + c.r, c.cy],
    [c.cx, c.cy - c.r],
    [c.cx, c.cy + c.r],
  ].every(([x, y]) => inBody(g, x, y) && y < g.sideY);

describe.each([
  ["landscape", landscape()],
  ["portrait", portrait()],
])("%s geometry", (_n, g) => {
  it("holds the LCD and the controls on the face", () => {
    for (const [x, y] of [
      [g.lcd.x - 3, g.lcd.y - 3],
      [g.lcd.x + LCD_ART_W + 2, g.lcd.y - 3],
      [g.lcd.x - 3, g.lcd.y + LCD_ART_H + 2],
      [g.lcd.x + LCD_ART_W + 2, g.lcd.y + LCD_ART_H + 2],
    ]) {
      expect(inBody(g, x, y)).toBe(true);
    }
    expect(g.lcd.y + LCD_ART_H + 3).toBeLessThan(g.sideY);
    expect(circleInside(g, g.disc)).toBe(true);
    expect(circleInside(g, g.back)).toBe(true);
  });

  it("keeps the controls off the screen and each other", () => {
    const off = (c: Circle) =>
      c.cx + c.r < g.lcd.x - 3 ||
      c.cx - c.r > g.lcd.x + LCD_ART_W + 3 ||
      c.cy + c.r < g.lcd.y - 3 ||
      c.cy - c.r > g.lcd.y + LCD_ART_H + 3;
    expect(off(g.disc)).toBe(true);
    expect(off(g.back)).toBe(true);
    expect(Math.hypot(g.disc.cx - g.back.cx, g.disc.cy - g.back.cy)).toBeGreaterThan(g.disc.r + g.back.r);
  });

  it("draws only known colours, with an outlined body", () => {
    const px = drawShell(g);
    expect(px.length).toBe(g.width * g.height);
    expect(Math.max(...px)).toBeLessThan(COLOURS.length);
    expect(px[0]).toBe(0);
    const rgba = new Uint8ClampedArray(px.length * 4);
    toRgba(px, rgba);
    expect(rgba[3]).toBe(0);
    const mid = Math.floor(g.height / 2);
    expect(px[mid * g.width + 0]).toBe(1);
  });
});

describe("fit", () => {
  it("uses whole scales and never overflows", () => {
    for (const w of [320, 343, 375, 480, 700, 900, 1148, 2000]) {
      const { geometry, scale } = fit(w);
      expect(Number.isInteger(scale)).toBe(true);
      if (w >= 320) expect(geometry.width * scale).toBeLessThanOrEqual(w);
    }
  });

  it("keeps the screen readable on a phone by stacking", () => {
    const { geometry, scale } = fit(343);
    expect(geometry.width).toBe(portrait().width);
    expect(scale).toBe(2);
  });

  it("goes landscape when there is room", () => {
    const { geometry, scale } = fit(1148);
    expect(geometry.width).toBe(landscape().width);
    expect(scale).toBe(4);
  });

  it("respects a height limit", () => {
    const { scale } = fit(2000, 500, 12);
    expect(scale * landscape().height).toBeLessThanOrEqual(500);
  });
});

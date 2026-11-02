// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from "vitest";
import {
  ART_STEP,
  BODY_NAME,
  COLOURS,
  LCD_HEIGHT,
  LCD_WIDTH,
  artSize,
  drawView,
  faceColour,
  skinForColour,
  fitView,
  inBody,
  landscape,
  portrait,
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

describe("skinForColour", () => {
  it("follows the firmware's colour codes", () => {
    expect([1, 2, 3, 0].map(skinForColour)).toEqual(["black", "white", "transparent", "white"]);
  });
});

describe("fitView", () => {
  it("never overflows and uses whole scales", () => {
    for (const w of [320, 343, 375, 480, 600, 700, 900, 1148, 2000]) {
      const v = fitView(w);
      expect(Number.isInteger(v.scale)).toBe(true);
      expect(v.model.width * v.scale).toBeLessThanOrEqual(w);
    }
  });

  it("puts the screen beside the controls where there is room", () => {
    const v = fitView(1148);
    expect(v.model.width).toBe(landscape().width);
    expect(v.scale).toBe(4);
  });

  it("stacks on a phone, with a readable screen", () => {
    const v = fitView(343);
    expect(v.model.width).toBe(portrait().width);
    expect(v.scale).toBe(2);
  });

  it("respects a height limit", () => {
    const v = fitView(4000, 500, 12);
    expect(artSize(v.model).height * ART_STEP * v.scale).toBeLessThanOrEqual(500);
  });
});

describe.each([
  ["landscape", landscape()],
  ["portrait", portrait()],
])("drawing the %s model", (_n, m) => {
  it("uses known colours, outlined, with a side", () => {
    const px = drawView(m);
    const { width, height } = artSize(m);
    expect(px.length).toBe(width * height);
    expect(Math.max(...px)).toBeLessThan(COLOURS.length);
    expect(px[0]).toBe(0);
    expect(px.includes(1)).toBe(true);
    expect(px.includes(6)).toBe(true);
    const rgba = new Uint8ClampedArray(px.length * 4);
    toRgba(px, rgba);
    expect(rgba[3]).toBe(0);
  });

  it.each(["white", "black", "transparent"] as const)("draws the %s skin with an opaque screen and controls", (skin) => {
    const px = drawView(m, skin);
    const rgba = new Uint8ClampedArray(px.length * 4);
    toRgba(px, rgba, skin);
    expect(Math.max(...px)).toBeLessThan(COLOURS.length);
    for (let i = 0; i < px.length; i++) {
      if (px[i] === 8 || px[i] === 12) expect(rgba[i * 4 + 3]).toBe(255);
      if (px[i] !== 0) expect(rgba[i * 4 + 3]).toBeGreaterThan(0);
    }
  });

  it("shows the board through the transparent skin only", () => {
    const has = (skin: "white" | "black" | "transparent") => drawView(m, skin).some((c) => c === 13 || c === 14);
    expect(has("transparent")).toBe(true);
    expect(has("white")).toBe(false);
    expect(has("black")).toBe(false);
  });

  it("gives each skin its own colours", () => {
    const body = (skin: "white" | "black" | "transparent") => {
      const out = new Uint8ClampedArray(4);
      toRgba(Uint8Array.of(2), out, skin);
      return [...out].join();
    };
    expect(new Set([body("white"), body("black"), body("transparent")]).size).toBe(3);
  });

  it("paints the word on the body", () => {
    let orange = 0;
    for (let u = m.name.x; u < m.name.x + 100; u++) if (faceColour(m, u, m.name.y + 1) === 8) orange++;
    expect(orange).toBeGreaterThan(5);
  });
});

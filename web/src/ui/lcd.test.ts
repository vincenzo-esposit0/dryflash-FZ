// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from "vitest";
import { LCD_H, LCD_W, glass, paint } from "./lcd";

describe("paint", () => {
  it("maps bit order and backlight", () => {
    const bits = new Uint8Array(1024);
    bits[0] = 0x80; // pixel (0, 0)
    bits[1023] = 0x01; // pixel (127, 63)
    const rgba = new Uint8ClampedArray(LCD_W * LCD_H * 4);
    paint(bits, 255, rgba);
    expect([...rgba.slice(0, 3)]).toEqual([0x10, 0x10, 0x10]);
    expect([...rgba.slice(4, 7)]).toEqual(glass(255));
    expect([...rgba.slice(rgba.length - 4, rgba.length - 1)]).toEqual([0x10, 0x10, 0x10]);
  });

  it("dims to grey without backlight", () => {
    expect(glass(0)).toEqual([0x9a, 0x8f, 0x80]);
    expect(glass(255)).toEqual([0xff, 0x8c, 0x29]);
  });
});

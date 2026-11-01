// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from "vitest";
import { encodeGif, lzw, lzwDecode } from "./gif";

function randomBits(n: number, seed: number) {
  let s = seed;
  return Uint8Array.from({ length: n }, () => {
    s = (s * 1103515245 + 12345) & 0x7fffffff;
    return (s >> 16) & 1;
  });
}

describe("lzw", () => {
  it.each([
    ["all zeros", new Uint8Array(100_000)],
    ["noise, past the 4096 code table", randomBits(200_000, 7)],
    ["alternating", Uint8Array.from({ length: 50_000 }, (_, i) => i & 1)],
    ["one pixel", Uint8Array.from([1])],
  ])("round-trips %s through a standard decoder", (_name, px) => {
    expect([...lzwDecode(lzw(px, 2), 2, px.length)]).toEqual([...px]);
  });
});

describe("encodeGif", () => {
  const bits = new Uint8Array(1024);
  bits[0] = 0x80;
  const gif = encodeGif([{ bits, delayMs: 100 }, { bits, delayMs: 40 }], 2, [0, 0, 0], [255, 140, 41]);

  it("has a valid header, size and trailer", () => {
    expect(new TextDecoder().decode(gif.slice(0, 6))).toBe("GIF89a");
    expect(gif[6] | (gif[7] << 8)).toBe(256);
    expect(gif[8] | (gif[9] << 8)).toBe(128);
    expect(gif[gif.length - 1]).toBe(0x3b);
  });

  it("puts glass first and ink second in the palette", () => {
    expect([...gif.slice(13, 19)]).toEqual([255, 140, 41, 0, 0, 0]);
  });

  it("scales frames and writes delays in hundredths", () => {
    const small = encodeGif([{ bits, delayMs: 100 }], 1, [0, 0, 0], [255, 255, 255]);
    const gce = small.indexOf(0xf9, 19);
    expect(small[gce + 3] | (small[gce + 4] << 8)).toBe(10);
    const start = small.indexOf(0x2c, gce) + 10;
    const min = small[start];
    const data: number[] = [];
    let o = start + 1;
    while (small[o] !== 0) {
      data.push(...small.slice(o + 1, o + 1 + small[o]));
      o += small[o] + 1;
    }
    const px = lzwDecode(Uint8Array.from(data), min, 128 * 64);
    expect(px[0]).toBe(1);
    expect(px.slice(1).every((v) => v === 0)).toBe(true);
  });
});

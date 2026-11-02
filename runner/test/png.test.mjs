// SPDX-License-Identifier: GPL-3.0-or-later
import { deflateSync } from "node:zlib";
import { describe, expect, it } from "vitest";
import { pngToFrame } from "../src/png.mjs";

function chunk(type, body) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(body.length);
  // The reader does not check CRCs, so these stay zero.
  return Buffer.concat([len, Buffer.from(type, "latin1"), body, Buffer.alloc(4)]);
}

function png(width, height, color, rows) {
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(width, 0);
  ihdr.writeUInt32BE(height, 4);
  ihdr[8] = 8;
  ihdr[9] = color;
  return Buffer.concat([
    Buffer.from("\x89PNG\r\n\x1a\n", "latin1"),
    chunk("IHDR", ihdr),
    chunk("IDAT", deflateSync(Buffer.concat(rows))),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

const grey = (at) =>
  Array.from({ length: 64 }, (_, y) => Buffer.from([0, ...Array.from({ length: 128 }, (_, x) => (at(x, y) ? 0 : 255))]));

describe("goldens", () => {
  it("reads dark pixels, top left first", () => {
    const frame = pngToFrame(png(128, 64, 0, grey((x, y) => x === 0 && y === 0)));
    expect(frame[0]).toBe(0x80);
    expect(frame.slice(1).every((b) => b === 0)).toBe(true);
  });

  it("undoes the filters an image editor would use", () => {
    // Sub (1) and Up (2) rows that say the same as the plain image: all dark but the first pixel of each row.
    const rows = Array.from({ length: 64 }, (_, y) => {
      if (y === 0) return Buffer.from([1, 255, 1, ...new Array(126).fill(0)]);
      return Buffer.from([2, ...new Array(128).fill(0)]);
    });
    const frame = pngToFrame(png(128, 64, 0, rows));
    expect(frame[0]).toBe(0x7f);
    expect(frame[16]).toBe(0x7f);
  });

  it("takes any whole scale and any backlight colour", () => {
    const row = (dark) => [0, ...Array.from({ length: 256 }, (_, x) => (dark && x < 2 ? [20, 10, 0] : [255, 140, 30])).flat()];
    const rows = Array.from({ length: 128 }, (_, y) => Buffer.from(row(y < 2)));
    const frame = pngToFrame(png(256, 128, 2, rows));
    expect(frame[0]).toBe(0x80);
  });

  it("calls one shade all over a blank screen", () => {
    expect(pngToFrame(png(128, 64, 0, grey(() => false))).every((b) => b === 0)).toBe(true);
  });

  it("refuses what is not a screen", () => {
    expect(() => pngToFrame(Buffer.from("nope"))).toThrow(/not a PNG/);
    expect(() => pngToFrame(png(100, 50, 0, Array.from({ length: 50 }, () => Buffer.alloc(101))))).toThrow(/128×64/);
  });
});

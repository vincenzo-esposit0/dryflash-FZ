// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from "vitest";
import { decodeSenses } from "../emu/senses";
import { LATENCY_MS, Timeline, loudness } from "./sound";

describe("decodeSenses", () => {
  it("reads the core's four numbers per change", () => {
    expect(decodeSenses([10, 1, 524.5, 0.43, 110, 0, 0, 0, 200, 3, 0, 0, 300, 2, 0, 0])).toEqual([
      { atMs: 10, tone: { hz: 524.5, duty: 0.43 } },
      { atMs: 110, tone: null },
      { atMs: 200, vibro: true },
      { atMs: 300, vibro: false },
    ]);
  });
});

describe("Timeline", () => {
  it("keeps the spacing of changes in virtual time", () => {
    const t = new Timeline();
    expect(t.at(5000, 1000)).toBe(1000 + LATENCY_MS);
    expect(t.at(5100, 1010)).toBe(1100 + LATENCY_MS);
  });

  it("lines up again after a pause or a burst", () => {
    const t = new Timeline();
    t.at(5000, 1000);
    // Resumed 10 s later: the old offset would put this in the past.
    expect(t.at(5200, 11000)).toBe(11000 + LATENCY_MS);
    // Virtual time ran far ahead (turbo): not queued seconds into the future.
    expect(t.at(15200, 11010)).toBe(11010 + LATENCY_MS);
  });
});

describe("loudness", () => {
  it("is loudest at 50% duty and silent at a constant level", () => {
    expect(loudness(0.5)).toBeCloseTo(1);
    expect(loudness(0)).toBe(0);
    expect(loudness(1)).toBeCloseTo(0);
    expect(loudness(0.43)).toBeLessThan(1);
    expect(loudness(0.05)).toBeLessThan(loudness(0.2));
  });
});

// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from "vitest";
import { MAX_LAG_MS, MIN_PRESS_MS, Pacer, PressTimer } from "./scheduler";

describe("Pacer", () => {
  it("tracks wall time at 1×", () => {
    const p = new Pacer();
    p.anchor(1000, 0);
    expect(p.behind(1000, 0)).toBe(0);
    expect(p.behind(1016, 0)).toBe(16);
    expect(p.behind(1016, 16)).toBe(0);
    expect(p.behind(1016, 20)).toBe(0);
  });

  it("drops a backlog instead of fast-forwarding", () => {
    const p = new Pacer();
    p.anchor(0, 0);
    expect(p.behind(5000, 100)).toBe(MAX_LAG_MS);
    expect(p.behind(5010, 100 + MAX_LAG_MS)).toBe(10);
  });
});

describe("PressTimer", () => {
  it("defers a quick release until the minimum press time", () => {
    const p = new PressTimer();
    p.press(4, 100);
    expect(p.release(4, 101)).toBe(false);
    expect(p.next(101)).toBe(MIN_PRESS_MS - 1);
    expect(p.due(120)).toEqual([]);
    expect(p.due(100 + MIN_PRESS_MS)).toEqual([4]);
  });

  it("releases a long press at once", () => {
    const p = new PressTimer();
    p.press(1, 0);
    expect(p.release(1, 500)).toBe(true);
  });
});

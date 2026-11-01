// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from "vitest";
import { RECORD } from "./cardstore";
import { decodeExport, encodeExport, newDevice, newUid, normalize } from "./devices";

describe("device export", () => {
  const sample = () => ({
    meta: newDevice("Lab", "abc", "0011223344556677"),
    flash: Uint8Array.from([1, 2, 3]),
    ob: new Uint8Array(128).fill(9),
    card: new Uint8Array(RECORD * 2).fill(5),
  });

  it("round-trips", () => {
    const back = decodeExport(encodeExport(sample()));
    expect(back.meta).toEqual(sample().meta);
    expect([...back.flash]).toEqual([1, 2, 3]);
    expect(back.ob.length).toBe(128);
    expect(back.card.length).toBe(RECORD * 2);
  });

  it("refuses other files and truncated ones", () => {
    expect(() => decodeExport(new TextEncoder().encode("hello, this is not it"))).toThrow(/not a dryflash/);
    const whole = encodeExport(sample());
    expect(() => decodeExport(whole.slice(0, whole.length - 10))).toThrow(/truncated/);
  });

  it("refuses card data that is not whole records", () => {
    const bad = { ...sample(), card: new Uint8Array(RECORD + 1) };
    expect(() => decodeExport(encodeExport(bad))).toThrow(/bad card/);
  });
});

describe("device settings", () => {
  it("fills what an older device did not save", () => {
    const d = normalize({ id: "default" });
    expect(d.legacySleep).toBe(true);
    expect(d.board.name).toBe("dryflash");
    expect(d.battery.percent).toBe(80);
    expect(d.uid).toMatch(/^[0-9a-f]{16}$/);
  });

  it("keeps what it saved", () => {
    const d = normalize({ id: "x", legacySleep: false, board: { name: "Bob", region: 1, color: 2 } });
    expect(d.legacySleep).toBe(false);
    expect(d.board.name).toBe("Bob");
  });

  it("makes 64-bit ids in hex", () => {
    expect(newUid((a) => void a.fill(0xab))).toBe("abababababababab");
  });
});

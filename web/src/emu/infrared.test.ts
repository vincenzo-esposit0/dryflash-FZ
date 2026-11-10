// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from "vitest";
import { decodeIr, durationMs, irFile } from "./infrared";

describe("IR transmissions", () => {
  it("decode one after the other, each with its own timings", () => {
    const s = decodeIr([10, 38000, 0.33, 3, 9000, 4500, 560, 20, 40000, 0.5, 1, 1500]);
    expect(s).toEqual([
      { atMs: 10, hz: 38000, duty: 0.33, timingsUs: [9000, 4500, 560] },
      { atMs: 20, hz: 40000, duty: 0.5, timingsUs: [1500] },
    ]);
    expect(durationMs(s[0])).toBe(14.06);
  });

  it("become a raw entry of an .ir file, only with a carrier", () => {
    const [s] = decodeIr([0, 38004.75, 0.33, 3, 9000, 4500, 560]);
    expect(irFile(s, "Sent")).toBe(
      "Filetype: IR signals file\nVersion: 1\n#\nname: Sent\ntype: raw\nfrequency: 38005\nduty_cycle: 0.330000\ndata: 9000 4500 560\n",
    );
    expect(irFile({ ...s, hz: 0 }, "Sent")).toBeNull();
  });
});

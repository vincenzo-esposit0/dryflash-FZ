// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from "vitest";
import { parseScript, ScriptError } from "../src/script.mjs";

describe("input scripts", () => {
  it("reads every step the brief names", () => {
    const steps = parseScript(`
- wait_for_desktop
- press: [down, ok]
- long_press: back
- wait: 2s
- wait: 500ms
- expect_screen: golden.png
- expect_screen: { file: golden.png, tolerance: 1% }
- expect_no_crash
`);
    expect(steps.map((s) => s.type)).toEqual([
      "wait_for_desktop",
      "press",
      "long_press",
      "wait",
      "wait",
      "expect_screen",
      "expect_screen",
      "expect_no_crash",
    ]);
    expect(steps[1].buttons).toEqual(["down", "ok"]);
    expect(steps[2].buttons).toEqual(["back"]);
    expect(steps[3].ms).toBe(2000);
    expect(steps[4].ms).toBe(500);
    expect(steps[5].tolerance).toBe(0);
    expect(steps[6].tolerance).toBe(82);
  });

  it("takes the steps under a key too", () => {
    expect(parseScript("steps:\n  - wait: 1s\n")).toHaveLength(1);
  });

  it.each([
    ["- bogus", /not a step/],
    ["- press: [sideways]", /not a button/],
    ["- wait: soon", /not a time/],
    ["- screenshot: ../up", /plain file name/],
    ["- { press: ok, wait: 1s }", /one name/],
    ["- expect_screen: { tolerance: 1 }", /needs a file/],
    ["- expect_screen: { file: a.png, tolerance: lots }", /tolerance/],
    ["just: text", /list of steps/],
    ["- [unclosed", /not valid YAML/],
    ["", /list of steps/],
  ])("refuses %s", (source, message) => {
    expect(() => parseScript(source)).toThrow(ScriptError);
    expect(() => parseScript(source)).toThrow(message);
  });
});

// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from "vitest";
import { changed, judge, pickAsset } from "../../scripts/compat-matrix.mjs";

const step = (name, status = "passed", message) => ({ name, status, message });
const smoke = (over = {}) => ({
  result: "pass",
  log: "Startup complete",
  report: null,
  steps: [step("wait_for_desktop"), step("menu entry 1"), step("menu entry 2"), step("walk_menu")],
  ...over,
});
const ota = { ok: true, problem: null };

describe("the matrix", () => {
  it("records a full pass, with the entries it opened", () => {
    expect(judge(smoke(), ota)).toEqual({ boots: true, desktop: true, apps: true, entries: 2, ota: true, problem: null, verdict: null });
  });

  it("records a crash in an app as that, not as a failed boot", () => {
    const r = judge(
      smoke({
        result: "fail",
        report: { summary: "The firmware crashed: furi_check failed", verdict: "unclear" },
        steps: [step("wait_for_desktop"), step("menu entry 1"), step("walk_menu", "failed", "crashed")],
      }),
      ota,
    );
    expect(r).toMatchObject({ boots: true, desktop: true, apps: false, entries: 1, problem: "The firmware crashed: furi_check failed", verdict: "unclear" });
  });

  it("records a firmware that never reaches the Desktop", () => {
    const r = judge(smoke({ result: "fail", log: "", steps: [step("wait_for_desktop", "failed", "not within 60 s")] }), { ok: false, problem: "real OTA: x" });
    expect(r).toMatchObject({ boots: false, desktop: false, apps: false, entries: 0, ota: false, problem: "not within 60 s" });
  });

  it("does not call a menu with no entries a pass", () => {
    expect(judge(smoke({ steps: [step("wait_for_desktop")] }), ota).apps).toBe(false);
  });

  it("finds the f7 package among a release's assets, not a variant", () => {
    const names = ["flipper-z-any-scripts-x.tgz", "flipper-z-f7-update-xc.tgz", "flipper-z-f7-update-x.tgz"];
    expect(pickAsset(names, "x")).toBe("flipper-z-f7-update-x.tgz");
    expect(pickAsset(["RM1.tgz", "other.zip"], "RM1")).toBe("RM1.tgz");
    expect(pickAsset(["other.zip"], "x")).toBe(null);
  });

  it("does not count a new date as a change", () => {
    const a = { checked: "2026-01-01", commit: "a", results: { x: { ota: true } }, latest: {} };
    expect(changed(a, { ...a, checked: "2026-01-02", commit: "b" })).toBe(false);
    expect(changed(a, { ...a, results: { x: { ota: false } } })).toBe(true);
    expect(changed(null, a)).toBe(true);
  });
});

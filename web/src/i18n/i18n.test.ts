// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from "vitest";
import en from "./en.json";
import itTable from "./it.json";
import { detectLocale, t } from ".";

describe("i18n", () => {
  it("has the same keys in every locale", () => {
    expect(Object.keys(itTable).sort()).toEqual(Object.keys(en).sort());
  });

  it("detects Italian and falls back to English", () => {
    expect(detectLocale(["it-IT", "en"])).toBe("it");
    expect(detectLocale(["de-DE", "fr"])).toBe("en");
  });

  it("looks up strings", () => {
    expect(t("en", "disclaimer")).toContain("Not affiliated");
  });
});

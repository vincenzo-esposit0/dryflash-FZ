// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from "vitest";
import { formatSize, joinPath, parentPath } from "./download";

describe("card paths", () => {
  it("joins and goes up", () => {
    expect(joinPath("/", "apps")).toBe("/apps");
    expect(joinPath("/apps", "Games")).toBe("/apps/Games");
    expect(parentPath("/apps/Games")).toBe("/apps");
    expect(parentPath("/apps")).toBe("/");
    expect(parentPath("/")).toBe("/");
  });
});

describe("formatSize", () => {
  it("picks a unit", () => {
    expect(formatSize(300)).toBe("300 B");
    expect(formatSize(12_400)).toBe("12 KB");
    expect(formatSize(2_000_000)).toBe("2.0 MB");
  });
});

// SPDX-License-Identifier: GPL-3.0-or-later
// JUnit XML: one test case per step, the shape CI systems read.

/** XML 1.0 has no way to write most control characters, and the firmware's log is full of ANSI escapes. */
function escape(s) {
  return String(s)
    .replace(/\x1b\[[0-9;]*m/g, "")
    .replace(/[\x00-\x08\x0b\x0c\x0e-\x1f]/g, "")
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

export function toJunit(result) {
  const failures = result.steps.filter((s) => s.status === "failed").length;
  const cases = result.steps.map((s) => {
    const open = `    <testcase classname="dryflash" name="${escape(s.name)}" time="${(s.ms / 1000).toFixed(3)}"`;
    if (s.status === "failed") {
      return `${open}>\n      <failure message="${escape(s.message ?? "failed")}">${escape(s.detail ?? "")}</failure>\n    </testcase>`;
    }
    if (s.status === "skipped") return `${open}>\n      <skipped/>\n    </testcase>`;
    return `${open}/>`;
  });
  return [
    '<?xml version="1.0" encoding="UTF-8"?>',
    `<testsuites>`,
    `  <testsuite name="${escape(result.firmware)}" tests="${result.steps.length}" failures="${failures}" errors="0" time="${(result.virtualMs / 1000).toFixed(3)}">`,
    ...cases,
    `    <system-out>${escape(result.logTail)}</system-out>`,
    "  </testsuite>",
    "</testsuites>",
    "",
  ].join("\n");
}

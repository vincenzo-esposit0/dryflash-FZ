// SPDX-License-Identifier: GPL-3.0-or-later
/** Removes ANSI colour sequences from the firmware log. */
export function stripAnsi(s: string): string {
  // eslint-disable-next-line no-control-regex
  return s.replace(/\x1b\[[0-9;]*m/g, "");
}

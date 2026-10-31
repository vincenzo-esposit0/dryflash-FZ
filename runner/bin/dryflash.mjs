#!/usr/bin/env node
// SPDX-License-Identifier: GPL-3.0-or-later
// dryflash headless runner. The `run` command is implemented in milestone M6
// (docs/ROADMAP.md); until then it refuses to run instead of pretending to pass.

const USAGE = "usage: dryflash run <firmware.tgz|.dfu> [--elf f.elf] [--script smoke.yaml] [--timeout 60] [--junit out.xml] [--screenshots dir/]";

const [cmd] = process.argv.slice(2);
if (cmd !== "run") {
  console.error(USAGE);
  process.exit(3);
}
console.error("dryflash run: not implemented yet (milestone M6). No firmware was executed.");
process.exit(3);

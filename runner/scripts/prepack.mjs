// SPDX-License-Identifier: GPL-3.0-or-later
// A package without the core would install and then fail for everyone, and the licence must travel with it.
import { copyFileSync, existsSync } from "node:fs";

if (!existsSync("wasm/dryflash_bg.wasm")) {
  console.error("runner/wasm is missing: run scripts/build-wasm.sh first");
  process.exit(1);
}
copyFileSync("../LICENSE", "LICENSE");

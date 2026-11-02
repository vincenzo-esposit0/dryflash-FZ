#!/usr/bin/env node
// SPDX-License-Identifier: GPL-3.0-or-later
// dryflash headless runner (BRIEF §10). Exit codes: 0 pass, 1 firmware failure, 2 emulator limitation
// hit, 3 usage error.
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { basename, dirname, resolve } from "node:path";
import { parseArgs } from "node:util";
import { toJunit } from "../src/junit.mjs";
import { EXIT, run } from "../src/run.mjs";
import { parseScript, ScriptError, SMOKE } from "../src/script.mjs";

const USAGE =
  "usage: dryflash run <firmware.tgz|.dfu> [--elf f.elf] [--script smoke.yaml] [--timeout 300] [--junit out.xml] [--screenshots dir/] [--report out.json] [--update-goldens]";

function usage(message) {
  if (message) console.error(`dryflash: ${message}`);
  console.error(USAGE);
  process.exit(EXIT.usage);
}

function read(path, what) {
  try {
    return readFileSync(path);
  } catch (err) {
    usage(`cannot read the ${what} ${path}: ${err.code ?? err.message}`);
  }
}

function write(path, data) {
  mkdirSync(dirname(resolve(path)), { recursive: true });
  writeFileSync(path, data);
}

let parsed;
try {
  parsed = parseArgs({
    allowPositionals: true,
    options: {
      elf: { type: "string" },
      script: { type: "string" },
      timeout: { type: "string", default: "300" },
      junit: { type: "string" },
      screenshots: { type: "string" },
      report: { type: "string" },
      "update-goldens": { type: "boolean", default: false },
    },
  });
} catch (err) {
  usage(err.message);
}
const { values, positionals } = parsed;
const [command, firmwarePath, ...extra] = positionals;
if (command !== "run" || !firmwarePath || extra.length) usage();
const timeoutS = Number(values.timeout);
if (!(timeoutS > 0)) usage(`--timeout ${values.timeout} is not a number of seconds`);

const firmware = read(firmwarePath, "firmware");
const elf = values.elf ? read(values.elf, "ELF") : undefined;
let steps = SMOKE;
if (values.script) {
  try {
    steps = parseScript(read(values.script, "script").toString("utf8"));
  } catch (err) {
    if (err instanceof ScriptError) usage(`${values.script}: ${err.message}`);
    throw err;
  }
}

console.log("dryflash is a smoke test, not a hardware certification: passing here does not guarantee it works on a device.");
let result;
try {
  result = run({
    firmware,
    elf,
    steps,
    name: basename(firmwarePath),
    timeoutS,
    screenshots: values.screenshots,
    scriptDir: values.script ? dirname(resolve(values.script)) : process.cwd(),
    updateGoldens: values["update-goldens"],
    log: (line) => console.log(line),
  });
} catch (err) {
  // wasm-bindgen errors from the loaders are plain strings: the file is not a firmware it can use.
  if (typeof err === "string" || err instanceof ScriptError) usage(typeof err === "string" ? `${firmwarePath}: ${err}` : err.message);
  throw err;
}

if (values.junit) write(values.junit, toJunit(result));
if (values.report) write(values.report, JSON.stringify(result, null, 2));
console.log(`${result.result} after ${(result.virtualMs / 1000).toFixed(1)} s of virtual time`);
process.exit(result.exitCode);

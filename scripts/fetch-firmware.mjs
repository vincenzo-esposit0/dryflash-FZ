#!/usr/bin/env node
// SPDX-License-Identifier: GPL-3.0-or-later
// Downloads a pinned firmware package from firmware/compat.json into firmware/cache/<id>/,
// verifies its SHA-256 and extracts it. Binaries stay out of git (see .gitignore, ADR-0005).
//
// usage: node scripts/fetch-firmware.mjs <id> [--elf]
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const compat = JSON.parse(readFileSync(join(root, "firmware/compat.json"), "utf8"));
const [id, ...flags] = process.argv.slice(2);
const fw = compat.firmwares.find((f) => f.id === id);
if (!fw) {
  console.error(`usage: fetch-firmware.mjs <${compat.firmwares.map((f) => f.id).join("|")}> [--elf]`);
  process.exit(3);
}
const dir = join(root, "firmware/cache", id);
mkdirSync(dir, { recursive: true });

async function fetchVerified(asset) {
  const file = join(dir, asset.url.split("/").pop());
  if (existsSync(file) && sha256(readFileSync(file)) === asset.sha256) return file;
  const res = await fetch(asset.url);
  if (!res.ok) throw new Error(`${asset.url}: HTTP ${res.status}`);
  const buf = Buffer.from(await res.arrayBuffer());
  const got = sha256(buf);
  if (got !== asset.sha256) throw new Error(`${asset.url}: sha256 ${got} != pinned ${asset.sha256}`);
  writeFileSync(file, buf);
  return file;
}

function sha256(buf) {
  return createHash("sha256").update(buf).digest("hex");
}

const tgz = await fetchVerified(fw.update_tgz);
execFileSync("tar", ["xzf", tgz, "-C", dir]);
console.log(`${id} ${fw.version}: ${tgz} (sha256 OK), extracted into ${dir}`);
if (flags.includes("--elf")) {
  if (!fw.elf) throw new Error(`${id}: no ELF pinned`);
  console.log(`elf: ${await fetchVerified(fw.elf)} (sha256 OK)`);
}

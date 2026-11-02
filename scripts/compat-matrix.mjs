#!/usr/bin/env node
// SPDX-License-Identifier: GPL-3.0-or-later
// The compatibility matrix behind /compat (BRIEF §10): runs the pinned firmwares and each one's latest release in the
// emulator and writes what happened to firmware/compat-results.json. Nothing here is typed in by hand.
//
// usage: node scripts/compat-matrix.mjs [--pinned-only]
// Exit code 1 when a pinned firmware does not pass everything, after writing the results all the same.
import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { atDesktop, run } from "../runner/src/run.mjs";
import { SMOKE } from "../runner/src/script.mjs";
import { frameDistance, Session, Stopped, TimedOut } from "../runner/src/session.mjs";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const RESULTS = join(root, "firmware/compat-results.json");
const COMMENT =
  "SPDX-License-Identifier: GPL-3.0-or-later. Written by scripts/compat-matrix.mjs in the nightly CI, never by hand. true = seen working, false = seen failing, null = could not run. `checked` is the last run that wrote this file.";
const SMOKE_TIMEOUT_S = 300;
const OTA_TIMEOUT_MS = 300_000;
const SETTLE_MS = 600;
/** Into the updater, out of it, and out of post-update (core/tests/ota_official.rs). */
const UPDATE_RESETS = 3;
/** Icons and the status bar may have moved a little; another screen is thousands of pixels. */
const SAME_SCREEN = 400;

/** The firmware's update package among a GitHub release's assets: f7 only, not the variants with a suffix. */
export function pickAsset(names, tag) {
  return [`flipper-z-f7-update-${tag}.tgz`, `${tag}.tgz`].find((n) => names.includes(n)) ?? null;
}

/** What a smoke test and an update say, for the table. */
export function judge(smoke, ota) {
  const entries = smoke.steps.filter((s) => s.name.startsWith("menu entry"));
  const reached = smoke.steps.find((s) => s.name === "wait_for_desktop");
  return {
    boots: smoke.log.includes("Startup complete"),
    desktop: reached?.status === "passed",
    apps: entries.length > 0 && entries.every((s) => s.status === "passed") && smoke.result === "pass",
    entries: entries.filter((s) => s.status === "passed").length,
    ota: ota.ok,
    problem: smoke.report?.summary ?? smoke.steps.find((s) => s.status === "failed")?.message ?? ota.problem ?? null,
    verdict: smoke.report?.verdict ?? null,
  };
}

/** Whether anything but the date differs. A quiet night should not make a commit and a deploy. */
export function changed(before, after) {
  const strip = ({ checked, commit, ...rest }) => JSON.stringify(rest);
  return !before || strip(before) !== strip(after);
}

/**
 * The updater of `firmware` installing it over a different firmware `from`, as when a user changes firmware (ADR-0014). Success
 * is what can be measured: flash not holding the package before and holding it byte for byte after, the three resets of an update,
 * and the package's own Desktop screen back. Not the log: after they write flash, the updaters of Momentum, Unleashed and
 * RogueMaster and the firmware they restart print nothing.
 */
export function realOta(from, firmware, name) {
  const expected = new Session({ firmware, name, timeoutMs: OTA_TIMEOUT_MS });
  const s = new Session({ firmware: from, name, timeoutMs: OTA_TIMEOUT_MS });
  try {
    expected.waitFor(() => atDesktop(expected.log), 60_000, "the Desktop");
    expected.run(SETTLE_MS);
    const desktop = expected.frame();
    s.waitFor(() => atDesktop(s.log), 60_000, "the Desktop of the firmware to update from");
    s.run(SETTLE_MS);
    if (s.emu.package_installed(firmware)) throw new Error("the two firmwares are the same");
    const resets = s.emu.resets();
    s.emu.start_ota(firmware);
    // The first screens can still be drawing, so that resets alone would end this too early.
    s.waitFor(
      () => s.emu.resets() >= resets + UPDATE_RESETS && frameDistance(s.frame(), desktop) < SAME_SCREEN,
      OTA_TIMEOUT_MS - 60_000,
      "the updater to finish and the Desktop to come back",
    );
    if (!s.emu.package_installed(firmware)) return { ok: false, problem: "real OTA: flash does not hold the package" };
    return { ok: true, problem: null };
  } catch (err) {
    if (err instanceof Stopped || err instanceof TimedOut) return { ok: false, problem: `real OTA: ${err.message}` };
    throw err;
  } finally {
    s.free();
    expected.free();
  }
}

function check(from, firmware, name) {
  const logs = [];
  const smoke = run({ firmware, name, steps: SMOKE, timeoutS: SMOKE_TIMEOUT_S, scriptDir: root, log: (l) => logs.push(l) });
  console.log(`${name}: ${smoke.result}`);
  const row = judge({ ...smoke, log: smoke.fullLog }, realOta(from, firmware, name));
  console.log(`${name}: ${JSON.stringify(row)}`);
  return row;
}

/** Networks fail now and then, and that says nothing about a firmware. */
async function fetchRetrying(url, init) {
  for (let attempt = 0; ; attempt++) {
    try {
      return await fetch(url, init);
    } catch (err) {
      if (attempt === 2) throw err;
      await new Promise((r) => setTimeout(r, 2000));
    }
  }
}

async function latestRelease(fw) {
  if (fw.channel) {
    const res = await fetchRetrying(fw.channel);
    if (!res.ok) throw new Error(`${fw.channel}: HTTP ${res.status}`);
    const release = (await res.json()).channels.find((c) => c.id === "release").versions[0];
    const file = release.files.find((f) => f.type === "update_tgz" && f.target === "f7");
    return { version: release.version, url: file?.url };
  }
  const repo = /github\.com\/([^/]+\/[^/]+)/.exec(fw.source)[1];
  const headers = process.env.GITHUB_TOKEN ? { authorization: `Bearer ${process.env.GITHUB_TOKEN}` } : {};
  const res = await fetchRetrying(`https://api.github.com/repos/${repo}/releases/latest`, { headers });
  if (!res.ok) throw new Error(`${repo}: HTTP ${res.status}`);
  const release = await res.json();
  const name = pickAsset(release.assets.map((a) => a.name), release.tag_name);
  return { version: release.tag_name, url: release.assets.find((a) => a.name === name)?.browser_download_url };
}

async function main() {
  const compat = JSON.parse(readFileSync(join(root, "firmware/compat.json"), "utf8"));
  const pinnedOnly = process.argv.includes("--pinned-only");
  const out = { $comment: COMMENT, checked: new Date().toISOString().slice(0, 10), commit: process.env.GITHUB_SHA?.slice(0, 7) ?? null, results: {}, latest: {} };

  const pinned = (fw) => {
    execFileSync(process.execPath, [join(root, "scripts/fetch-firmware.mjs"), fw.id], { stdio: "ignore" });
    return readFileSync(join(root, "firmware/cache", fw.id, fw.update_tgz.url.split("/").pop()));
  };
  for (const fw of compat.firmwares) {
    const file = pinned(fw);
    // Each is updated from another one in the list, so that the update always changes the firmware.
    const others = compat.firmwares.filter((f) => f.id !== fw.id);
    const from = pinned(others[0]);
    out.results[fw.id] = check(from, file, `${fw.name} ${fw.version}`);

    if (pinnedOnly) continue;
    try {
      const latest = await latestRelease(fw);
      if (latest.version === fw.version) continue; // the row above already says it
      if (!latest.url) throw new Error(`no update package in release ${latest.version}`);
      const res = await fetchRetrying(latest.url);
      if (!res.ok) throw new Error(`${latest.url}: HTTP ${res.status}`);
      const bytes = new Uint8Array(await res.arrayBuffer());
      out.latest[fw.id] = { version: latest.version, url: latest.url, ...check(file, bytes, `${fw.name} ${latest.version}`) };
    } catch (err) {
      // A release we cannot get says nothing about the emulator or the firmware. The row just says so.
      console.log(`${fw.name} latest: ${err.message ?? err}`);
      out.latest[fw.id] = { version: null, url: null, boots: null, desktop: null, apps: null, entries: 0, ota: null, problem: String(err.message ?? err), verdict: null };
    }
  }

  let before = null;
  try {
    before = JSON.parse(readFileSync(RESULTS, "utf8"));
  } catch {}
  // Without a change, keep the file: the date moves at least weekly, through the workflow's own check.
  const stale = !before || Date.now() - Date.parse(before.checked) >= 7 * 86_400_000;
  if (changed(before, out) || stale) writeFileSync(RESULTS, `${JSON.stringify(out, null, 2)}\n`);
  const bad = Object.entries(out.results).filter(([, r]) => !(r.boots && r.desktop && r.apps && r.ota));
  for (const [id, r] of bad) console.log(`::error::${id}: ${r.problem ?? "did not pass"}`);
  process.exit(bad.length ? 1 : 0);
}

if (process.argv[1] === fileURLToPath(import.meta.url)) await main();

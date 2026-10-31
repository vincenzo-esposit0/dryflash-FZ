#!/usr/bin/env node
// SPDX-License-Identifier: GPL-3.0-or-later
// Generates the GitHub wiki from docs/ (the single source of truth) and, with --push,
// publishes it to <repo>.wiki.git. Wiki pages are never edited by hand: edit docs/ instead.
//
// usage: node scripts/sync-wiki.mjs <out-dir> [--push]
//   <out-dir> is a clone of the wiki repository (created and pulled when --push is given).
import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const REPO = "vincenzo-esposit0/dryflash-FZ";
const BLOB = `https://github.com/${REPO}/blob/main`;
const [outDir, ...flags] = process.argv.slice(2);
if (!outDir) {
  console.error("usage: sync-wiki.mjs <out-dir> [--push]");
  process.exit(3);
}
const push = flags.includes("--push");

const read = (p) => readFileSync(join(root, p), "utf8");
const stripSpdx = (md) => md.replace(/^<!-- SPDX-License-Identifier:[^>]*-->\n/, "");

// docs/adr/0001-cpu-engine.md → "ADR-0001-cpu-engine"
const adrFiles = readdirSync(join(root, "docs/adr")).filter((f) => /^\d{4}-.*\.md$/.test(f)).sort();
const adrPage = (f) => `ADR-${f.replace(/\.md$/, "")}`;

const pages = new Map(); // wiki page name → { source, title }
pages.set("Architecture", { source: "docs/ARCHITECTURE.md" });
pages.set("Roadmap", { source: "docs/ROADMAP.md" });
pages.set("Bring-up-Log", { source: "docs/BRINGUP_LOG.md" });
for (const f of adrFiles) pages.set(adrPage(f), { source: `docs/adr/${f}` });

/** Rewrites repository-relative links so they work from the wiki. */
function rewriteLinks(md, sourcePath) {
  const base = dirname(sourcePath);
  return md.replace(/\]\(([^)#\s]+)(#[^)]*)?\)/g, (all, target, anchor = "") => {
    if (/^[a-z]+:/i.test(target)) return all; // absolute URL
    const resolved = join(base, target).replace(/\\/g, "/");
    for (const [name, p] of pages) if (p.source === resolved) return `](${name}${anchor})`;
    return `](${BLOB}/${resolved}${anchor})`;
  });
}

function generated(source) {
  return `> Generated from [\`${source}\`](${BLOB}/${source}) — edit that file, not this page.\n\n`;
}

function title(md) {
  const m = md.match(/^#\s+(.+)$/m);
  return m ? m[1].trim() : "";
}

mkdirSync(outDir, { recursive: true });
if (push) {
  if (existsSync(join(outDir, ".git"))) {
    execFileSync("git", ["-C", outDir, "pull", "-q", "--rebase"], { stdio: "inherit" });
  } else {
    rmSync(outDir, { recursive: true, force: true });
    execFileSync("git", ["clone", "-q", `https://github.com/${REPO}.wiki.git`, outDir], { stdio: "inherit" });
  }
}
for (const f of readdirSync(outDir)) {
  if (f.endsWith(".md")) rmSync(join(outDir, f));
}
const write = (name, body) => writeFileSync(join(outDir, `${name}.md`), body);

// Pages generated from docs/.
for (const [name, { source }] of pages) {
  const md = stripSpdx(read(source));
  pages.get(name).title = title(md);
  write(name, generated(source) + rewriteLinks(md, source));
}

// ADR index.
const adrRows = adrFiles.map((f) => {
  const md = read(`docs/adr/${f}`);
  const status = (md.match(/^- Status: \*\*([^*]+)\*\*/m) || [])[1] || "";
  return `| [${title(stripSpdx(md)).replace(/^ADR-\d{4} — /, "")}](${adrPage(f)}) | ${f.slice(0, 4)} | ${status} |`;
});
write(
  "Decisions",
  `${generated("docs/adr/")}# Architecture decision records\n\nNon-obvious decisions are recorded as ADRs (working rule 5).\n\n| Decision | ADR | Status |\n|---|---|---|\n${adrRows.join("\n")}\n`,
);

// Firmware compatibility from firmware/compat.json.
const compat = JSON.parse(read("firmware/compat.json"));
const fwRows = compat.firmwares.map(
  (f) =>
    `| ${f.name} | \`${f.version}\` | [source](${f.source}) | ${f.licence} | ${f.cors ? "direct" : "via proxy"} | \`${f.layout.resources}\` | \`${f.update_tgz.sha256.slice(0, 12)}…\` |`,
);
write(
  "Firmware-Compatibility",
  `${generated("firmware/compat.json")}# Firmware compatibility\n\nPinned firmware packages used by the tests and, from M6, by the nightly compatibility matrix.\nBinaries are always fetched from the original host and checked against the pinned SHA-256;\nthey are never stored in the repository (ADR-0005).\n\n| Firmware | Version | Source | Licence | Browser download | Resources | SHA-256 |\n|---|---|---|---|---|---|---|\n${fwRows.join("\n")}\n\nThe boot status of each firmware will be published here by CI once the headless runner exists (M6).\n`,
);

// Home, getting started, sidebar, footer.
const readme = rewriteLinks(stripSpdx(read("README.md")), "README.md");
const status = (readme.match(/> \*\*Status:[\s\S]*?(?=\n\n)/) || [""])[0];
const positioning = (readme.match(/## What it is — and what it is not\n\n([\s\S]*?)\n\n## /) || [, ""])[1];
write(
  "Home",
  `# dryflash\n\n**An emulator for Flipper Zero firmware that runs entirely in your browser.**\nTry any Flipper Zero firmware before flashing it to your real device: dryflash executes the real,\nunmodified release binaries on an instruction-level model of the STM32WB55, with no backend and no\nhardware.\n\n${status}\n\n## What it is — and what it is not\n\n${positioning}\n\n## Where to start\n\n- [[Getting Started|Getting-Started]] — build and run the emulator core and the web app\n- [[Architecture]] — how the emulator is built\n- [[Roadmap]] — milestones and current status\n- [[Bring-up Log|Bring-up-Log]] — what boots, measurements, known gaps, per milestone\n- [[Decisions]] — architecture decision records\n- [[Firmware Compatibility|Firmware-Compatibility]] — pinned firmware versions\n\nThe repository's \`docs/\` folder is the source of truth; this wiki is generated from it.\n`,
);
const dev = (readme.match(/## Development\n\n([\s\S]*?)\n## Licence/) || [, ""])[1];
write(
  "Getting-Started",
  `${generated("README.md")}# Getting started\n\n${dev}\nSee also [CLAUDE.md](${BLOB}/CLAUDE.md) for the commands used during development.\n`,
);
const adrLinks = adrFiles.map((f) => `  - [[${f.slice(0, 4)}|${adrPage(f)}]]`).join("\n");
write(
  "_Sidebar",
  `**dryflash**\n\n- [[Home]]\n- [[Getting Started|Getting-Started]]\n- [[Architecture]]\n- [[Roadmap]]\n- [[Bring-up Log|Bring-up-Log]]\n- [[Firmware Compatibility|Firmware-Compatibility]]\n- [[Decisions]]\n${adrLinks}\n`,
);
write(
  "_Footer",
  "GPL-3.0-or-later · Not affiliated with or endorsed by Flipper Devices Inc. Flipper Zero is a trademark of its respective owner. For educational and authorised security research only.\n",
);
console.log(`wiki written to ${outDir} (${readdirSync(outDir).filter((f) => f.endsWith(".md")).length} pages)`);

if (push) {
  execFileSync("git", ["-C", outDir, "add", "-A"], { stdio: "inherit" });
  const changed = execFileSync("git", ["-C", outDir, "status", "--porcelain"]).toString().trim();
  if (!changed) {
    console.log("wiki already up to date");
  } else {
    const sha = execFileSync("git", ["-C", root, "rev-parse", "--short", "HEAD"]).toString().trim();
    execFileSync("git", ["-C", outDir, "commit", "-q", "-m", `Sync from docs at ${sha}`], { stdio: "inherit" });
    execFileSync("git", ["-C", outDir, "push", "-q"], { stdio: "inherit" });
    console.log("wiki pushed");
  }
}

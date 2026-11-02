// SPDX-License-Identifier: GPL-3.0-or-later
// The command line as a user meets it: real process, real exit codes, small hand-assembled firmwares.
// The smoke test on a real firmware needs it: node scripts/fetch-firmware.mjs official
import { spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { toJunit } from "../src/junit.mjs";

const BIN = fileURLToPath(new URL("../bin/dryflash.mjs", import.meta.url));
const OFFICIAL = fileURLToPath(new URL("../../firmware/cache/official/flipper-z-f7-update-1.5.1.tgz", import.meta.url));
const dir = mkdtempSync(join(tmpdir(), "dryflash-runner-"));

/** A DfuSe file (ST UM0391) with `code` at 0x100, `pool` at 0x108 and a spinning HardFault handler. */
function firmware(code, pool = []) {
  const image = Buffer.alloc(0x200);
  image.writeUInt32LE(0x20030000, 0);
  image.writeUInt32LE(0x08000101, 4);
  image.writeUInt32LE(0x08000181, 12);
  code.forEach((h, i) => image.writeUInt16LE(h, 0x100 + 2 * i));
  pool.forEach((w, i) => image.writeUInt32LE(w, 0x108 + 4 * i));
  image.writeUInt16LE(0xe7fe, 0x180);
  const u32 = (v) => {
    const b = Buffer.alloc(4);
    b.writeUInt32LE(v);
    return b;
  };
  const name = Buffer.alloc(255);
  name.write("test");
  const target = Buffer.concat([Buffer.from("Target\0"), u32(1), name, u32(8 + image.length), u32(1), u32(0x08000000), u32(image.length), image]);
  const body = Buffer.concat([
    Buffer.from("DfuSe\x01"),
    u32(11 + target.length),
    Buffer.from([1]),
    target,
    Buffer.from([0xff, 0xff, 0x11, 0xdf, 0x83, 0x04, 0x1a, 0x01]),
    Buffer.from("UFD\x10"),
  ]);
  let crc = 0xffffffff;
  for (const byte of body) {
    crc ^= byte;
    for (let i = 0; i < 8; i++) crc = (crc >>> 1) ^ (crc & 1 ? 0xedb88320 : 0);
  }
  const file = join(dir, `fw-${Math.random().toString(36).slice(2)}.dfu`);
  writeFileSync(file, Buffer.concat([body, u32(crc >>> 0)]));
  return file;
}

const script = (source) => {
  const file = join(dir, `s-${Math.random().toString(36).slice(2)}.yaml`);
  writeFileSync(file, source);
  return file;
};
const dryflash = (...args) => spawnSync(process.execPath, [BIN, ...args], { encoding: "utf8" });

const SPIN = [0xe7fe];
/** ldr r0, =PKA (a block with no behavioural model) ; ldr r1, [r0] ; b back to the load. */
const POLL_UNMODELLED = [0x4801, 0x6801, 0xe7fd];
const PKA = 0x58002000;

describe("exit codes", () => {
  it("0: the script held", () => {
    const r = dryflash("run", firmware(SPIN), "--script", script("- wait: 1s\n- expect_no_crash\n"));
    expect(r.status).toBe(0);
    expect(r.stdout).toContain("not a hardware certification");
    expect(r.stdout).toContain("pass after");
  });

  it("1: the firmware hung, with nothing to blame on the emulator", () => {
    const r = dryflash("run", firmware(SPIN), "--script", script("- wait: 10s\n"));
    expect(r.status).toBe(1);
    expect(r.stdout).toContain("Hang: 5 s");
    expect(r.stdout).toContain("firmware bug");
  });

  it("1: a step's check did not hold, or time ran out", () => {
    expect(dryflash("run", firmware(SPIN), "--script", script("- expect_log: never said\n")).status).toBe(1);
    const r = dryflash("run", firmware(SPIN), "--timeout", "1", "--script", script("- wait: 3s\n"));
    expect(r.status).toBe(1);
    expect(r.stdout).toContain("ran out");
  });

  it("2: stuck on a register the emulator has no behaviour for", () => {
    const r = dryflash("run", firmware(POLL_UNMODELLED, [PKA]), "--script", script("- wait: 10s\n"));
    expect(r.status).toBe(2);
    expect(r.stdout).toContain("emulator limitation");
    expect(r.stdout).toContain("PKA");
  });

  it.each([
    ["no command", []],
    ["another command", ["jump", "x"]],
    ["no firmware", ["run"]],
    ["a file that is not there", ["run", "/nonexistent.tgz"]],
    ["an unknown option", ["run", "x", "--nope"]],
    ["a bad timeout", ["run", "x", "--timeout", "soon"]],
  ])("3: %s", (_, args) => {
    expect(dryflash(...args).status).toBe(3);
  });

  it("3: a file that is not a firmware, or a bad script, and nothing ran", () => {
    const junk = join(dir, "junk.dfu");
    writeFileSync(junk, "not a firmware");
    expect(dryflash("run", junk).status).toBe(3);
    expect(dryflash("run", firmware(SPIN), "--script", script("- bogus\n")).status).toBe(3);
    expect(dryflash("run", firmware(SPIN), "--script", "/nonexistent.yaml").status).toBe(3);
    expect(dryflash("run", firmware(SPIN), "--script", script("- expect_screen: missing.png\n")).status).toBe(3);
  });
});

describe("what it leaves behind", () => {
  it("a crash report and screenshots, and JUnit naming the failure", () => {
    const out = join(dir, "hang");
    const junit = join(out, "j.xml");
    const report = join(out, "r.json");
    const r = dryflash("run", firmware(SPIN), "--script", script("- screenshot: first\n- wait: 10s\n- expect_no_crash\n"), "--screenshots", out, "--junit", junit, "--report", report);
    expect(r.status).toBe(1);
    for (const f of ["first.png", "crash.png", "crash.json", "crash.md", "digest.txt"]) expect(existsSync(join(out, f)), f).toBe(true);
    const xml = readFileSync(junit, "utf8");
    expect(xml).toContain('tests="3" failures="1"');
    expect(xml).toContain("Hang: 5 s");
    expect(xml).toContain("<skipped/>");
    const json = JSON.parse(readFileSync(report, "utf8"));
    expect(json.exitCode).toBe(1);
    expect(json.report.kind).toBe("hang");
    expect(json.steps.map((s) => s.status)).toEqual(["passed", "failed", "skipped"]);
  });

  it("JUnit stays well formed whatever the firmware logged", () => {
    const xml = toJunit({
      firmware: 'a&b<"c">',
      virtualMs: 1,
      logTail: "\x1b[0;31m[E]\x1b[0m <tag> & \x01",
      steps: [{ name: "x", ms: 1, status: "failed", message: "<&>", detail: "\x02" }],
    });
    expect(xml).toContain('name="a&amp;b&lt;&quot;c&quot;&gt;"');
    expect(xml).toContain("[E] &lt;tag&gt; &amp; ");
    // biome-ignore lint: looking for what must not be there
    expect(/[\x00-\x08\x0b\x0c\x0e-\x1f]/.test(xml)).toBe(false);
  });

  it("goldens: written on request, then compared, with a tolerance", () => {
    const fw = firmware(SPIN);
    const golden = join(dir, "goldens", "blank.png");
    const write = script(`- expect_screen: goldens/blank.png\n`);
    expect(dryflash("run", fw, "--script", write).status).toBe(3);
    expect(dryflash("run", fw, "--script", write, "--update-goldens").status).toBe(0);
    expect(existsSync(golden)).toBe(true);
    expect(dryflash("run", fw, "--script", write).status).toBe(0);
  });
});

describe.skipIf(!existsSync(OFFICIAL))("the default smoke test on the official firmware", () => {
  it("passes, the same way twice", () => {
    const run = (name) => {
      const out = join(dir, name);
      const r = dryflash("run", OFFICIAL, "--screenshots", out, "--report", join(out, "r.json"));
      return { r, out, json: JSON.parse(readFileSync(join(out, "r.json"), "utf8")) };
    };
    const a = run("smoke-a");
    expect(a.r.status, a.r.stdout).toBe(0);
    const entries = a.json.steps.filter((s) => s.name.startsWith("menu entry"));
    expect(entries.length).toBeGreaterThanOrEqual(10);
    expect(entries.every((s) => s.status === "passed")).toBe(true);
    const shots = entries.map((_, i) => readFileSync(join(a.out, `menu-${String(i + 1).padStart(2, "0")}.png`)).toString("latin1"));
    expect(new Set(shots).size, "each entry opened a screen of its own").toBe(entries.length);

    const b = run("smoke-b");
    expect(b.json.virtualMs).toBe(a.json.virtualMs);
    expect(readFileSync(join(b.out, "final.png")).equals(readFileSync(join(a.out, "final.png")))).toBe(true);
  }, 120_000);
});

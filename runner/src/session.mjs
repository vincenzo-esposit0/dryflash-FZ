// SPDX-License-Identifier: GPL-3.0-or-later
// One emulated device under a script: runs in virtual time, as fast as the machine can (BRIEF §6).
import { readFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { fileURLToPath } from "node:url";
import { Emulator, Exit, initSync } from "../wasm/dryflash.js";

const BUTTONS = ["up", "down", "left", "right", "ok", "back"];
/** Past debounce, and short of a long press: 2 × INPUT_PRESS_TICKS, 300 ms (applications/services/input/input.c). */
const PRESS_MS = 100;
/** Well past that long press. */
const LONG_PRESS_MS = 800;
/** The screen and the app under it get this long to respond to a press. */
const SETTLE_MS = 400;

let ready = false;
function initWasm() {
  if (ready) return;
  initSync({ module: readFileSync(fileURLToPath(new URL("../wasm/dryflash_bg.wasm", import.meta.url))) });
  ready = true;
}

/** The emulator stopped by itself: a crash, a hang or something it cannot do. */
export class Stopped extends Error {
  constructor(reason, report, markdown, png, digest) {
    super(reason);
    this.report = report;
    this.markdown = markdown;
    this.png = png;
    this.digest = digest;
  }
}

/** The script's time ran out, or a step did. */
export class TimedOut extends Error {}

export class Session {
  /** @param {{firmware: Uint8Array, name: string, elf?: Uint8Array, timeoutMs: number}} o */
  constructor({ firmware, name, elf, timeoutMs }) {
    initWasm();
    this.emu = new Emulator();
    this.timeoutMs = timeoutMs;
    this.log = "";
    this.decoder = new TextDecoder();
    this.emu.sd_restore(new Uint8Array(0)); // a blank card
    if (elf) this.emu.load_symbols(elf);
    const isPackage = firmware[0] === 0x1f && firmware[1] === 0x8b;
    const info = isPackage ? JSON.parse(this.emu.load_tgz(firmware)) : null;
    if (!isPackage) this.emu.load_dfu(firmware, 0);
    const sha256 = createHash("sha256").update(firmware).digest("hex");
    this.emu.set_firmware_info(name, info?.info ?? undefined, sha256);
    this.emu.power_on();
  }

  get virtualMs() {
    return this.emu.virtual_ms();
  }

  drainLog() {
    const bytes = this.emu.take_log();
    if (bytes.length) this.log += this.decoder.decode(bytes, { stream: true });
  }

  /** Runs `ms` of virtual time, or until the emulator stops or the script's time is up. */
  run(ms) {
    const end = this.virtualMs + ms;
    while (this.virtualMs < end) {
      if (this.virtualMs >= this.timeoutMs) throw new TimedOut(`the script's ${this.timeoutMs / 1000} s ran out`);
      const left = Math.min(end - this.virtualMs, 100, this.timeoutMs - this.virtualMs);
      const exit = this.emu.run(Math.max(1, Math.round((this.emu.hclk_hz() * left) / 1000)));
      this.drainLog();
      if (exit !== Exit.Budget) this.stopped();
    }
  }

  stopped() {
    const e = this.emu;
    const png = e.report_png();
    throw new Stopped(
      e.last_stop(),
      JSON.parse(e.report_json() ?? "null"),
      e.report_markdown() ?? null,
      png ? Buffer.from(png) : null,
      e.digest_text(),
    );
  }

  /** Runs until `test()` holds. */
  waitFor(test, timeoutMs, what) {
    const end = this.virtualMs + timeoutMs;
    while (!test()) {
      if (this.virtualMs >= end) throw new TimedOut(`${what}: not within ${timeoutMs / 1000} s`);
      this.run(50);
    }
  }

  press(name, holdMs = PRESS_MS, settleMs = SETTLE_MS) {
    const button = BUTTONS.indexOf(name);
    if (button < 0) throw new Error(`no button "${name}"`);
    this.emu.set_button(button, true);
    this.run(holdMs);
    this.emu.set_button(button, false);
    this.run(settleMs);
  }

  longPress(name) {
    this.press(name, LONG_PRESS_MS);
  }

  /** 128×64, one bit per pixel, as `Emulator.frame`. */
  frame() {
    return this.emu.frame().slice();
  }

  png(scale = 4) {
    return Buffer.from(this.emu.frame_png(scale));
  }

  digest() {
    return this.emu.digest_text();
  }

  free() {
    this.emu.free();
  }
}

/** How many pixels two frames differ by. */
export function frameDistance(a, b) {
  let n = 0;
  for (let i = 0; i < a.length; i++) {
    let x = a[i] ^ b[i];
    while (x) {
      n += x & 1;
      x >>= 1;
    }
  }
  return n;
}

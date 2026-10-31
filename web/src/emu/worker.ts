// SPDX-License-Identifier: GPL-3.0-or-later
// Emulator host: owns the wasm core, runs it in slices and posts frames, LEDs, log and status.
import init, { Emulator, Exit, version } from "../wasm/dryflash.js";
import { BUTTONS, PROTOCOL_VERSION, type FromWorker, type ToWorker } from "./protocol";
import { Pacer, PressTimer, SLICE_BUDGET_MS, STEP_MS } from "./scheduler";

const ctx = self as unknown as DedicatedWorkerGlobalScope;
const post = (m: FromWorker, transfer: Transferable[] = []) => ctx.postMessage(m, transfer);

let emu: Emulator | null = null;
let loaded = false;
let running = false;
let turbo = false;
let timer: ReturnType<typeof setTimeout> | undefined;
const pacer = new Pacer();
const presses = new PressTimer();
let lastGeneration = -1;
let lastLights = "";
let report = { wall: 0, virtualMs: 0, instructions: 0 };

function schedule(delay: number) {
  clearTimeout(timer);
  if (running) timer = setTimeout(slice, delay);
}

function restartPacing() {
  pacer.anchor(performance.now(), emu!.virtual_ms());
}

function stepMs(ms: number): boolean {
  const e = emu!;
  // Stop at the next deferred button release so it lands at the right virtual time.
  const v = e.virtual_ms();
  ms = Math.min(ms, Math.max(presses.next(v), 0.001));
  const exit = e.run(Math.max(1, Math.round((e.hclk_hz() * ms) / 1000)));
  for (const b of presses.due(e.virtual_ms())) e.set_button(b, false);
  if (exit === Exit.Budget) return true;
  running = false;
  post({
    type: "stopped",
    reason: e.last_stop(),
    crashes: JSON.parse(e.crashes_json()),
    digest: e.digest_text(),
  });
  return false;
}

function slice() {
  if (!emu || !running) return;
  const start = performance.now();
  let ok = true;
  if (turbo) {
    while (ok && performance.now() - start < SLICE_BUDGET_MS) ok = stepMs(STEP_MS);
  } else {
    let lag = pacer.behind(start, emu.virtual_ms());
    while (ok && lag > 0 && performance.now() - start < SLICE_BUDGET_MS) {
      ok = stepMs(Math.min(lag, STEP_MS));
      lag = pacer.behind(performance.now(), emu.virtual_ms());
    }
  }
  publish(false);
  schedule(turbo ? 0 : 4);
}

/** Sends what changed since the last call; `force` also sends an up-to-date status. */
function publish(force: boolean) {
  const e = emu!;
  const generation = e.display_generation();
  if (generation !== lastGeneration) {
    lastGeneration = generation;
    const bits = e.frame().slice().buffer;
    post({ type: "frame", bits }, [bits]);
  }
  const l = e.lights();
  const key = l.join(",");
  if (key !== lastLights) {
    lastLights = key;
    post({ type: "lights", rgbw: [l[0], l[1], l[2], l[3]] });
  }
  const log = e.take_log();
  if (log.length) {
    const bytes = log.slice().buffer;
    post({ type: "log", bytes }, [bytes]);
  }
  const now = performance.now();
  if (force || now - report.wall >= 250) {
    const v = e.virtual_ms();
    const n = e.instructions();
    const dt = now - report.wall;
    const speed = report.wall && dt > 0 ? (v - report.virtualMs) / dt : 0;
    const mips = report.wall && dt > 0 ? (n - report.instructions) / dt / 1000 : 0;
    report = { wall: now, virtualMs: v, instructions: n };
    post({ type: "status", status: { running, turbo, virtualMs: v, speed, mips, resets: e.resets() } });
  }
}

function powerOn() {
  const e = emu!;
  presses.clear();
  e.power_on();
  lastGeneration = -1;
  lastLights = "";
  report = { wall: 0, virtualMs: 0, instructions: 0 };
  running = true;
  restartPacing();
  publish(true);
  schedule(0);
}

ctx.onmessage = (ev: MessageEvent<ToWorker>) => {
  const m = ev.data;
  if (!emu) return;
  try {
    switch (m.type) {
      case "load":
        running = false;
        emu.load_dfu(new Uint8Array(m.bytes), m.cpu2Start);
        loaded = true;
        post({ type: "loaded", name: m.name });
        powerOn();
        break;
      case "button": {
        const b = BUTTONS.indexOf(m.button);
        const v = emu.virtual_ms();
        if (m.down) {
          presses.press(b, v);
          emu.set_button(b, true);
        } else if (presses.release(b, v)) {
          emu.set_button(b, false);
        }
        break;
      }
      case "control":
        if (!loaded) break;
        if (m.action === "reset") powerOn();
        else {
          running = m.action === "resume";
          if (running) restartPacing();
          publish(true);
          schedule(0);
        }
        break;
      case "turbo":
        turbo = m.on;
        if (loaded) {
          restartPacing();
          publish(true);
        }
        break;
      case "settings":
        emu.set_legacy_sleep(m.legacySleep);
        break;
    }
  } catch (err) {
    running = false;
    post({ type: "error", message: String(err) });
  }
};

await init();
emu = new Emulator();
post({ type: "ready", version: version(), protocol: PROTOCOL_VERSION, fidelity: JSON.parse(emu.fidelity_json()) });

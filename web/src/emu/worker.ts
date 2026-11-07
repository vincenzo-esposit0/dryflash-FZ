// SPDX-License-Identifier: GPL-3.0-or-later
// Emulator host: owns the wasm core, runs it in slices and posts frames, LEDs, log and status.
import init, { Emulator, Exit, version } from "../wasm/dryflash.js";
import { MemoryOnlyStore, type CardStore } from "./cardstore";
import {
  decodeExport,
  encodeExport,
  gunzip,
  gzip,
  MemoryBackend,
  newDevice,
  openDevices,
  type DeviceBackend,
  type DeviceMeta,
} from "./devices";
import { BUTTONS, PROTOCOL_VERSION, type FromWorker, type ToWorker } from "./protocol";
import { Pacer, PressTimer, SLICE_BUDGET_MS, STEP_MS } from "./scheduler";
import { decodeSenses } from "./senses";

const ctx = self as unknown as DedicatedWorkerGlobalScope;
const post = (m: FromWorker, transfer: Transferable[] = []) => ctx.postMessage(m, transfer);

/** How often written card blocks are saved (ms of wall time). */
const FLUSH_MS = 1000;
/** Attempts 250 ms apart: about 3 seconds. */
const CARD_OPEN_RETRIES = 12;

// Without these an exception here reaches neither the page nor the page's error handlers: a failed start
// would leave "Loading" on the screen for good, and the E2E suite would not see it.
function reportUncaught(reason: unknown) {
  const err = reason instanceof Error ? reason : null;
  post({ type: "uncaught", message: err?.message ?? String(reason), stack: err?.stack });
}
ctx.addEventListener("unhandledrejection", (e) => {
  e.preventDefault();
  reportUncaught(e.reason);
});
ctx.addEventListener("error", (e) => {
  e.preventDefault();
  reportUncaught(e.error ?? e.message);
});

let emu: Emulator | null = null;
let card: CardStore | null = null;
let devices: DeviceBackend | null = null;
let device: DeviceMeta | null = null;
/** Flash contents last saved, by the core's write counter. */
let savedFlash = 0;
let lastFlush = 0;
/** Saves run one after another, in order, whatever the store's speed. */
let saving: Promise<void> = Promise.resolve();
let loaded = false;
let running = false;
let turbo = false;
let timer: ReturnType<typeof setTimeout> | undefined;
const pacer = new Pacer();
const presses = new PressTimer();
let lastGeneration = -1;
let lastLights = "";
let lastCrashes = 0;
/** Recent log text, enough to find a marker split across two reads. */
let logTail = "";
let milestones = { booted: false, desktopReached: false };
const logDecoder = new TextDecoder();
let report = { wall: 0, virtualMs: 0, instructions: 0 };

// With work pending the next slice comes through a message port: timers wait at least 4 ms when
// nested and are throttled heavily in background pages. Messages from the UI still get a turn.
const wake = new MessageChannel();
let wakePending = false;
wake.port1.onmessage = () => {
  wakePending = false;
  slice();
};

function schedule(delay: number) {
  clearTimeout(timer);
  if (!running) return;
  if (delay > 0) timer = setTimeout(slice, delay);
  else if (!wakePending) {
    wakePending = true;
    wake.port2.postMessage(0);
  }
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
  const png = e.report_png();
  post({
    type: "stopped",
    reason: e.last_stop(),
    crashes: JSON.parse(e.crashes_json()),
    digest: e.digest_text(),
    report: JSON.parse(e.report_json() ?? "null"),
    markdown: e.report_markdown() ?? null,
    png: png ? png.slice().buffer : null,
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
  publish(!ok);
  schedule(turbo || pacer.behind(performance.now(), emu.virtual_ms()) > 0 ? 0 : 4);
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
  const senses = e.take_senses();
  if (senses.length) post({ type: "senses", changes: decodeSenses(senses) });
  const log = e.take_log();
  if (log.length) {
    logTail = (logTail + logDecoder.decode(log, { stream: true })).slice(-4096);
    if (!milestones.booted && logTail.includes("Startup complete")) {
      milestones.booted = true;
      post({ type: "milestone", name: "booted" });
    }
    if (!milestones.desktopReached && logTail.includes("[AnimationManager]") && /Select '/.test(logTail)) {
      milestones.desktopReached = true;
      post({ type: "milestone", name: "desktopReached" });
    }
    const bytes = log.slice().buffer;
    post({ type: "log", bytes }, [bytes]);
  }
  if (e.crash_count() !== lastCrashes) {
    lastCrashes = e.crash_count();
    post({ type: "crashes", crashes: JSON.parse(e.crashes_json()) });
  }
  const now = performance.now();
  if (now - lastFlush >= FLUSH_MS) void flushCard();
  if (force || now - report.wall >= 250) {
    const v = e.virtual_ms();
    const n = e.instructions();
    const dt = now - report.wall;
    const speed = report.wall && dt > 0 ? (v - report.virtualMs) / dt : 0;
    const mips = report.wall && dt > 0 ? (n - report.instructions) / dt / 1000 : 0;
    report = { wall: now, virtualMs: v, instructions: n };
    post({
      type: "status",
      status: {
        running,
        turbo,
        virtualMs: v,
        speed,
        mips,
        resets: e.resets(),
        cardUsedBytes: e.sd_used_bytes(),
        off: e.powered_off(),
        charging: e.charging(),
      },
    });
  }
}

function flushCard(): Promise<void> {
  if (!emu || !card) return saving;
  lastFlush = performance.now();
  const records = emu.sd_take_dirty();
  if (records.length) {
    const store = card;
    saving = saving
      .then(() => store.put(records))
      .catch((err) => post({ type: "error", message: `saving the card: ${err}` }));
  }
  if (card.kind !== "none" && emu.flash_generation() !== savedFlash) saveFlash();
  return saving;
}

/** Saves what is in flash and the option bytes: an update changes them while running. */
function saveFlash() {
  const [e, store, id] = [emu!, devices!, device!.id];
  savedFlash = e.flash_generation();
  const [flash, ob] = [e.flash_image(), e.option_bytes()];
  saving = saving
    .then(() => store.write(id, "flash", flash))
    .then(() => store.write(id, "ob", ob))
    .catch((err) => post({ type: "error", message: `saving the firmware: ${err}` }));
}

/** Runs device operations in order with the saves, reporting failures. */
function queue(what: string, op: () => Promise<void>) {
  saving = saving.then(op).catch((err) => post({ type: "error", message: `${what}: ${err}` }));
}

function applySettings(e: Emulator, d: DeviceMeta) {
  e.set_legacy_sleep(d.legacySleep);
  e.set_board(d.board.name, d.board.region, d.board.color);
  e.set_uid64(parseInt(d.uid.slice(0, 8), 16), parseInt(d.uid.slice(8), 16));
  e.set_battery(d.battery.percent, d.battery.usb);
}

async function reportDevices() {
  post({
    type: "devices",
    devices: await devices!.list(),
    active: device!,
    persistence: card!.kind,
    hasFirmware: loaded,
    cardCapacityBytes: emu!.sd_capacity_bytes(),
    cardUsedBytes: emu!.sd_used_bytes(),
  });
}

/**
 * Opens a device's card. Right after a reload the old page's worker can still hold the file locks
 * for a moment, so a refusal is retried. If another tab really holds them, the device runs from
 * memory and the page says so.
 */
async function openCardRetrying(id: string): Promise<CardStore> {
  for (let attempt = 0; ; attempt++) {
    try {
      return await devices!.openCard(id);
    } catch (err) {
      if (attempt >= CARD_OPEN_RETRIES) {
        console.warn("card not available", err);
        return new MemoryOnlyStore();
      }
      await new Promise((r) => setTimeout(r, 250));
    }
  }
}

/** Stops the current device and brings up `d`: its card, its flash and, if it has one, its firmware. */
async function activate(d: DeviceMeta) {
  running = false;
  clearTimeout(timer);
  const e = emu;
  emu = null;
  e?.free();
  device = d;
  card = await openCardRetrying(d.id);
  const next = new Emulator();
  applySettings(next, d);
  next.set_firmware_info(d.firmware ?? "", d.firmwareVersion ?? undefined, d.firmwareSha256 ?? undefined);
  next.sd_restore(await card.load());
  const flash = card.kind === "none" ? null : await devices!.read(d.id, "flash");
  const ob = await devices!.read(d.id, "ob");
  if (flash?.length) next.flash_restore(flash, ob ?? new Uint8Array(0));
  emu = next;
  loaded = !!flash?.length;
  savedFlash = next.flash_generation();
  await devices!.save(d);
  await devices!.setActive(d.id);
  void flushCard(); // a new card has just been prepared
  await reportDevices();
  if (loaded) powerOn();
  else {
    const bits = new Uint8Array(1024).buffer;
    post({ type: "frame", bits }, [bits]);
    post({ type: "lights", rgbw: [0, 0, 0, 0] });
  }
  // After the current operation, so that what it queued behind it keeps its order.
  queueMicrotask(drainWaiting);
}

/**
 * Changes files on the card from outside the firmware, which must not be running: its FatFs keeps
 * state in RAM. It stops, the change is made, and it starts again to mount the changed card.
 */
function changeCard(what: string, dir: string, change: () => void) {
  void flushCard();
  running = false;
  queue(what, async () => {
    try {
      change();
    } finally {
      void flushCard();
      listCard(dir);
      await reportDevices();
      if (loaded) powerOn();
    }
  });
}

function listCard(path: string) {
  post({ type: "sdListing", path, entries: JSON.parse(emu!.sd_list(path)) });
}

/** Saves the active device's description, and tells the UI. */
async function saveDevice(changes: Partial<DeviceMeta>) {
  device = { ...device!, ...changes };
  await devices!.save(device);
  await reportDevices();
}

/** What was installed: on the device, and for the emulator's crash reports. */
async function recordFirmware(name: string, version: string | null, file: Uint8Array<ArrayBuffer>) {
  const digest = await crypto.subtle.digest("SHA-256", file);
  const sha256 = [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, "0")).join("");
  emu?.set_firmware_info(name, version ?? undefined, sha256);
  await saveDevice({ firmware: name, firmwareVersion: version, firmwareSha256: sha256 });
}

function powerOn() {
  const e = emu!;
  presses.clear();
  e.power_on();
  lastGeneration = -1;
  lastLights = "";
  lastCrashes = 0;
  logTail = "";
  milestones = { booted: false, desktopReached: false };
  post({ type: "crashes", crashes: [] });
  report = { wall: 0, virtualMs: 0, instructions: 0 };
  running = true;
  restartPacing();
  publish(true);
  schedule(0);
}

// Messages that arrive while a device is being brought up wait for it.
const waiting: ToWorker[] = [];

ctx.onmessage = (ev: MessageEvent<ToWorker>) => {
  if (ev.data.type === "init") void start(ev.data.ephemeral);
  else if (emu) handle(ev.data);
  else waiting.push(ev.data);
};

function drainWaiting() {
  while (emu && waiting.length) handle(waiting.shift()!);
}

function handle(m: ToWorker) {
  if (!emu) return;
  try {
    switch (m.type) {
      case "symbols":
        emu.load_symbols(new Uint8Array(m.bytes));
        break;
      case "load": {
        if (m.deviceLabel !== undefined) {
          const { deviceLabel, ...load } = m;
          void flushCard();
          queue("switching device", async () => {
            if (device!.label !== deviceLabel) {
              const known = (await devices!.list()).find((d) => d.label === deviceLabel);
              await activate(known ?? newDevice(deviceLabel, crypto.randomUUID().slice(0, 8)));
            }
            handle(load);
          });
          break;
        }
        running = false;
        const bytes = new Uint8Array(m.bytes);
        // By content, not by file name: gzip is an update package, anything else must be DfuSe.
        // A bare .dfu does not say where the CPU2 region starts, so no guard applies (0).
        const isPackage = bytes[0] === 0x1f && bytes[1] === 0x8b;
        if (m.ota && isPackage) {
          const { info } = JSON.parse(emu.start_ota(bytes));
          post({ type: "otaStarted", name: m.name, info });
          queue("saving the device", () => recordFirmware(m.name, info, bytes));
          running = true;
          restartPacing();
          void flushCard();
          publish(true);
          schedule(0);
          break;
        }
        const packageInfo = isPackage ? JSON.parse(emu.load_tgz(bytes)) : undefined;
        if (!isPackage) emu.load_dfu(bytes, 0);
        loaded = true;
        post({ type: "loaded", name: m.name, packageInfo });
        powerOn();
        void flushCard();
        saveFlash();
        queue("saving the device", () => recordFirmware(m.name, packageInfo?.info ?? null, bytes));
        break;
      }
      case "flush":
        void flushCard();
        break;
      case "eraseCard": {
        const e = emu;
        running = false;
        const store = card;
        saving = saving
          .then(() => store?.clear())
          .then(() => {
            e.sd_take_dirty(); // blocks of the old card must not reach the cleared store
            e.sd_erase();
            if (loaded) powerOn();
            void flushCard();
          })
          .catch((err) => post({ type: "error", message: `erasing the card: ${err}` }));
        break;
      }
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
      case "settings": {
        const d = device!;
        const next: DeviceMeta = {
          ...d,
          legacySleep: m.legacySleep ?? d.legacySleep,
          board: m.board ?? d.board,
          battery: m.battery ?? d.battery,
        };
        applySettings(emu, next);
        queue("saving the settings", () => saveDevice(next));
        break;
      }
      case "selectDevice":
        void flushCard();
        queue("switching device", async () => {
          const d = (await devices!.list()).find((x) => x.id === m.id);
          if (d) await activate(d);
        });
        break;
      case "createDevice":
        void flushCard();
        queue("creating the device", () => activate(newDevice(m.label, crypto.randomUUID().slice(0, 8))));
        break;
      case "renameDevice":
        queue("renaming the device", () => saveDevice({ label: m.label }));
        break;
      case "deleteDevice":
        queue("deleting the device", async () => {
          const gone = device!.id;
          running = false;
          card = null;
          await devices!.remove(gone);
          const rest = (await devices!.list()).filter((d) => d.id !== gone);
          await activate(rest[0] ?? newDevice("Device 1", crypto.randomUUID().slice(0, 8)));
        });
        break;
      case "factoryReset":
        queue("resetting the device", async () => {
          running = false;
          const d = device!;
          await card!.clear();
          emu!.sd_take_dirty(); // blocks of the old card must not reach the cleared store
          await devices!.write(d.id, "flash", new Uint8Array(0));
          await activate({ ...newDevice(d.label, d.id, d.uid) });
        });
        break;
      case "sdList":
        listCard(m.path);
        break;
      case "sdRead": {
        const bytes = emu.sd_read(m.path).slice().buffer;
        post({ type: "sdFile", name: m.path.split("/").pop()!, bytes }, [bytes]);
        break;
      }
      case "sdWrite": {
        const e = emu;
        changeCard("writing to the card", m.dir, () => {
          for (const f of m.files) e.sd_write(`${m.dir}/${f.name}`, new Uint8Array(f.bytes));
        });
        break;
      }
      case "sdMkdir": {
        const e = emu;
        const dir = m.path.slice(0, m.path.lastIndexOf("/")) || "/";
        changeCard("creating the folder", dir, () => e.sd_mkdir(m.path));
        break;
      }
      case "sdRemove": {
        const e = emu;
        changeCard("deleting from the card", m.dir, () => e.sd_remove(m.path));
        break;
      }
      case "exportDevice":
        void flushCard();
        queue("exporting the device", async () => {
          const whole = encodeExport({
            meta: device!,
            flash: emu!.flash_image(),
            ob: emu!.option_bytes(),
            card: await card!.load(),
          });
          const bytes = (await gzip(whole)).buffer as ArrayBuffer;
          post({ type: "exported", name: device!.label, bytes }, [bytes]);
        });
        break;
      case "importDevice":
        void flushCard();
        queue("importing the device", async () => {
          const x = decodeExport(await gunzip(new Uint8Array(m.bytes)));
          // A new device: importing a file twice must not overwrite the first copy.
          const d: DeviceMeta = { ...x.meta, id: crypto.randomUUID().slice(0, 8), label: `${x.meta.label} (import)` };
          await devices!.save(d);
          if (x.flash.length) await devices!.write(d.id, "flash", x.flash);
          await devices!.write(d.id, "ob", x.ob);
          const store = await devices!.openCard(d.id);
          await store.load();
          await store.put(x.card);
          await activate(d);
        });
        break;
    }
  } catch (err) {
    running = false;
    post({ type: "error", message: String(err) });
  }
}

async function start(ephemeral: boolean) {
  await init();
  devices = ephemeral ? new MemoryBackend() : await openDevices();
  const known = await devices.list();
  const activeId = await devices.getActive();
  await activate(known.find((d) => d.id === activeId) ?? known[0] ?? newDevice("Device 1", "default"));
  post({
    type: "ready",
    version: version(),
    protocol: PROTOCOL_VERSION,
    fidelity: JSON.parse(emu!.fidelity_json()),
  });
}

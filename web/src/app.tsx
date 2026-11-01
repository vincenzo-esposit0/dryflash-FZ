// SPDX-License-Identifier: GPL-3.0-or-later
import { useEffect, useRef, useState } from "preact/hooks";
import type { Persistence } from "./emu/cardstore";
import type { DeviceMeta } from "./emu/devices";
import type {
  ButtonName,
  CardEntry,
  CrashRecord,
  FidelityRow,
  FromWorker,
  Status,
  ToWorker,
} from "./emu/protocol";
import { detectLocale, t, type Key } from "./i18n";
import { LINK_DEVICE, fetchFirmware, linkMustAsk, parseLink, type LinkParams } from "./emu/remote";
import { stripAnsi } from "./ui/ansi";
import { download, formatSize, joinPath, parentPath } from "./ui/download";
import { encodeGif, type GifFrame } from "./ui/gif";
import { buttonForKey } from "./ui/keys";
import { Fidelity } from "./pages";
import { SiteFooter, SiteHeader } from "./ui/chrome";
import { ART_STEP, artSize, drawView, fitView, toRgba, type View } from "./ui/shell";
import { LCD_H, LCD_W, ghostStep, glass, inkColour, paint, paintInk } from "./ui/lcd";

const locale = detectLocale(navigator.languages ?? [navigator.language]);
const tr = (k: Key) => t(locale, k);
const LOG_LIMIT = 64 * 1024;
const EVENT_LIMIT = 200;
/** Screenshots and videos: 4× the panel (BRIEF §9). */
const CAPTURE_SCALE = 4;
const GIF_SCALE = 2;
const GIF_MAX_MS = 30_000;
const GIF_FRAME_MS = 40;

type Send = (m: ToWorker, transfer?: Transferable[]) => void;

/** A question in the page: the browser's own dialogs are blocked in sandboxed frames and some shells. */
interface Question {
  message: string;
  /** Present: asks for text, starting from this value. */
  input?: string;
  /** The "yes" button says what is about to happen. */
  action: string;
  danger?: boolean;
  onAnswer: (text: string) => void;
}
type Ask = (q: Question) => void;

function AskDialog({ question, close }: { question: Question | null; close: () => void }) {
  const box = useRef<HTMLDialogElement>(null);
  const field = useRef<HTMLInputElement>(null);
  useEffect(() => {
    const d = box.current;
    if (!d) return;
    if (question && !d.open) d.showModal();
    if (!question && d.open) d.close();
    if (question && field.current) field.current.value = question.input ?? "";
  }, [question]);
  const answer = (e: Event) => {
    e.preventDefault();
    const text = field.current?.value.trim() ?? "";
    if (question?.input !== undefined && !text) return;
    question?.onAnswer(text);
    close();
  };
  return (
    <dialog ref={box} class="ask" onClose={close}>
      <form onSubmit={answer}>
        <p>{question?.message}</p>
        {question?.input !== undefined && <input ref={field} type="text" aria-label={question.message} />}
        <div class="controls">
          <button type="submit" class={question?.danger ? "danger" : ""}>
            {question?.action}
          </button>
          <button type="button" onClick={close}>
            {tr("cancel")}
          </button>
        </div>
      </form>
    </dialog>
  );
}

interface Stop {
  reason: string;
  crashes: CrashRecord[];
  digest: string;
}

/** The pixel-art device (ADR-0015), with the real canvas and buttons on its face. */
function DeviceView({
  view,
  canvas,
  leds,
  pressed,
  press,
  enabled,
  box,
}: {
  view: View;
  canvas: preact.RefObject<HTMLCanvasElement | null>;
  leds: [number, number, number, number];
  pressed: Set<ButtonName>;
  press: (b: ButtonName, d: boolean) => void;
  enabled: boolean;
  box: preact.RefObject<HTMLDivElement | null>;
}) {
  const m = view.model;
  const size = artSize(m);
  const art = useRef<HTMLCanvasElement>(null);
  useEffect(() => {
    const c = art.current?.getContext("2d");
    if (!c) return;
    const img = c.createImageData(size.width, size.height);
    toRgba(drawView(m), img.data);
    c.putImageData(img, 0, 0);
  }, [m.width]);
  const at = (x: number, y: number, w: number, h: number) => ({
    left: `${x}px`,
    top: `${y}px`,
    width: `${w}px`,
    height: `${h}px`,
  });
  const cell = Math.floor((m.disc.r * 2) / 3);
  const key = (b: ButtonName, label: Key, x: number, y: number, w: number, h: number, round = false) => (
    <button
      type="button"
      class={`key ${b}${round ? " round" : ""}${pressed.has(b) ? " held" : ""}`}
      data-button={b}
      aria-label={tr(label)}
      disabled={!enabled}
      style={at(x, y, w, h)}
      onPointerDown={(e) => {
        (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
        press(b, true);
      }}
      onPointerUp={() => press(b, false)}
      onPointerCancel={() => press(b, false)}
    />
  );
  const { cx, cy } = m.disc;
  const half = Math.floor(cell / 2);
  return (
    <div
      class="device"
      ref={box}
      style={{
        width: `${size.width * ART_STEP * view.scale}px`,
        height: `${size.height * ART_STEP * view.scale}px`,
      }}
    >
      <canvas class="art" ref={art} width={size.width} height={size.height} aria-hidden="true" />
      <div class="face" style={{ width: `${m.width}px`, height: `${m.height}px`, transform: `scale(${view.scale})` }}>
        <canvas class="lcd" ref={canvas} width={LCD_W} height={LCD_H} aria-label="LCD 128×64" style={at(m.lcd.x, m.lcd.y, LCD_W, LCD_H)} />
        <span
          class="led"
          aria-hidden="true"
          style={{ ...at(m.led.x, m.led.y, 4, 4), background: `rgb(${leds[0]},${leds[1]},${leds[2]})` }}
        />
        {key("up", "btnUp", cx - half, cy - half - cell, cell, cell)}
        {key("down", "btnDown", cx - half, cy + half, cell, cell)}
        {key("left", "btnLeft", cx - half - cell, cy - half, cell, cell)}
        {key("right", "btnRight", cx + half, cy - half, cell, cell)}
        {key("ok", "btnOk", cx - half, cy - half, cell, cell, true)}
        {key("back", "btnBack", m.back.cx - m.back.r, m.back.cy - m.back.r, m.back.r * 2, m.back.r * 2, true)}
      </div>
    </div>
  );
}

function DeviceSettings({ device, send }: { device: DeviceMeta; send: Send }) {
  const board = device.board;
  const battery = device.battery;
  const options = (prefix: "region" | "color", values: number[]) =>
    values.map((v) => <option value={v}>{tr(`${prefix}${v}` as Key)}</option>);
  const value = (e: Event) => (e.currentTarget as HTMLInputElement).value;
  const checked = (e: Event) => (e.currentTarget as HTMLInputElement).checked;
  return (
    <details>
      <summary>{tr("settings")}</summary>
      <div class="settings">
        <label>
          {tr("deviceName")}{" "}
          <input
            type="text"
            maxLength={8}
            pattern="[A-Za-z0-9]{1,8}"
            value={board.name}
            onChange={(e) => {
              const input = e.currentTarget as HTMLInputElement;
              if (input.checkValidity() && input.value) send({ type: "settings", board: { ...board, name: input.value } });
              else input.value = board.name;
            }}
          />
        </label>
        <label>
          {tr("region")}{" "}
          <select value={board.region} onChange={(e) => send({ type: "settings", board: { ...board, region: +value(e) } })}>
            {options("region", [1, 2, 3, 4])}
          </select>
        </label>
        <label>
          {tr("color")}{" "}
          <select value={board.color} onChange={(e) => send({ type: "settings", board: { ...board, color: +value(e) } })}>
            {options("color", [1, 2, 3])}
          </select>
        </label>
        <label>
          <input type="checkbox" checked={device.legacySleep} onChange={(e) => send({ type: "settings", legacySleep: checked(e) })} />{" "}
          {tr("legacySleep")}
        </label>
        <p class="hint">{tr("legacySleepNote")}</p>
        <label>
          {tr("batteryCharge")} {battery.percent}%{" "}
          <input
            type="range"
            min={0}
            max={100}
            value={battery.percent}
            onChange={(e) => send({ type: "settings", battery: { ...battery, percent: +value(e) } })}
          />
        </label>
        <label>
          <input type="checkbox" checked={battery.charging} onChange={(e) => send({ type: "settings", battery: { ...battery, charging: checked(e) } })} />{" "}
          {tr("charging")}
        </label>
        <label>
          <input type="checkbox" checked={battery.usb} onChange={(e) => send({ type: "settings", battery: { ...battery, usb: checked(e) } })} />{" "}
          {tr("usb")}
        </label>
        <p class="hint">{tr("applyNext")}</p>
      </div>
    </details>
  );
}

function Devices({
  devices,
  active,
  firmware,
  send,
  ask,
}: {
  devices: DeviceMeta[];
  active: DeviceMeta | null;
  firmware: string | null;
  send: Send;
  ask: Ask;
}) {
  return (
    <section class="tab-section">
      <h2>{tr("devices")}</h2>
      <div class="controls">
        <label>
          {tr("device")}{" "}
          <select
            value={active?.id}
            disabled={!active}
            onChange={(e) => send({ type: "selectDevice", id: (e.currentTarget as HTMLSelectElement).value })}
          >
            {devices.map((d) => (
              <option value={d.id}>{d.label}</option>
            ))}
          </select>
        </label>
        <button
          type="button"
          disabled={!active}
          onClick={() =>
            ask({
              message: tr("newDeviceName"),
              input: "",
              action: tr("newDevice"),
              onAnswer: (label) => send({ type: "createDevice", label }),
            })
          }
        >
          {tr("newDevice")}
        </button>
        <button
          type="button"
          disabled={!active}
          onClick={() =>
            ask({
              message: tr("renamePrompt"),
              input: active?.label ?? "",
              action: tr("renameDevice"),
              onAnswer: (label) => send({ type: "renameDevice", label }),
            })
          }
        >
          {tr("renameDevice")}
        </button>
        <button
          type="button"
          disabled={!active}
          onClick={() =>
            ask({
              message: tr("factoryConfirm"),
              action: tr("factoryReset"),
              danger: true,
              onAnswer: () => send({ type: "factoryReset" }),
            })
          }
        >
          {tr("factoryReset")}
        </button>
        <button
          type="button"
          disabled={!active}
          onClick={() =>
            ask({
              message: tr("deleteConfirm"),
              action: tr("deleteDevice"),
              danger: true,
              onAnswer: () => send({ type: "deleteDevice" }),
            })
          }
        >
          {tr("deleteDevice")}
        </button>
      </div>
      <p class="hint">{firmware ? tr("firmwareOn").replace("{name}", firmware) : tr("noFirmwareOn")}</p>
      {active && <DeviceSettings device={active} send={send} />}
    </section>
  );
}

type Tab = "device" | "update" | "files" | "backup" | "diagnostics";
const TABS: [Tab, Key][] = [
  ["device", "tabDevice"],
  ["update", "tabUpdate"],
  ["files", "tabFiles"],
  ["backup", "tabBackup"],
  ["diagnostics", "tabDiagnostics"],
];

function Tabs({ tab, setTab, badge }: { tab: Tab; setTab: (t: Tab) => void; badge: boolean }) {
  const move = (e: KeyboardEvent) => {
    const i = TABS.findIndex(([t]) => t === tab);
    const next = e.key === "ArrowRight" ? i + 1 : e.key === "ArrowLeft" ? i - 1 : -1;
    if (next === -1 || !TABS[next]) return;
    e.preventDefault();
    e.stopPropagation();
    setTab(TABS[next][0]);
    document.getElementById(`tab-${TABS[next][0]}`)?.focus();
  };
  return (
    <div class="tabs" role="tablist" aria-label="dryflash" onKeyDown={move}>
      {TABS.map(([t, label]) => (
        <button
          type="button"
          role="tab"
          id={`tab-${t}`}
          aria-selected={tab === t}
          aria-controls="tabpanel"
          tabIndex={tab === t ? 0 : -1}
          class={tab === t ? "tab on" : "tab"}
          onClick={() => setTab(t)}
        >
          {tr(label)}
          {t === "diagnostics" && badge && <span class="dot" aria-label={tr("crashPanel")} />}
        </button>
      ))}
    </div>
  );
}

function CardFiles({
  usage,
  path,
  entries,
  ready,
  send,
  ask,
}: {
  usage: string;
  path: string;
  entries: CardEntry[] | null;
  ready: boolean;
  send: Send;
  ask: Ask;
}) {
  const open = (p: string) => send({ type: "sdList", path: p });
  useEffect(() => {
    if (ready && entries === null) open("/");
  }, [ready]);
  return (
    <section class="tab-section">
      <h2>{tr("files")}</h2>
      <p class="hint">{usage}</p>
      <div class="controls">
        <button type="button" disabled={!ready || path === "/"} onClick={() => open(parentPath(path))}>
          {tr("up")}
        </button>
        <button type="button" disabled={!ready} onClick={() => open(path)}>
          {tr("refresh")}
        </button>
        <label class="file-button">
          {tr("upload")}
          <input
            type="file"
            multiple
            disabled={!ready}
            onChange={async (e) => {
              const input = e.currentTarget as HTMLInputElement;
              const list = [...(input.files ?? [])];
              input.value = "";
              const files = await Promise.all(list.map(async (f) => ({ name: f.name, bytes: await f.arrayBuffer() })));
              if (files.length) send({ type: "sdWrite", dir: path, files }, files.map((f) => f.bytes));
            }}
          />
        </label>
        <button
          type="button"
          disabled={!ready}
          onClick={() =>
            ask({
              message: tr("folderName"),
              input: "",
              action: tr("newFolder"),
              onAnswer: (name) => send({ type: "sdMkdir", path: joinPath(path, name) }),
            })
          }
        >
          {tr("newFolder")}
        </button>
        <button
          type="button"
          disabled={!ready}
          onClick={() =>
            ask({
              message: tr("eraseConfirm"),
              action: tr("eraseCard"),
              danger: true,
              onAnswer: () => send({ type: "eraseCard" }),
            })
          }
        >
          {tr("eraseCard")}
        </button>
      </div>
      <p class="hint">{tr("cardChanges")}</p>
      <p class="path">/ext{path === "/" ? "" : path}</p>
      {entries === null ? null : entries.length === 0 ? (
        <p class="hint">{tr("empty")}</p>
      ) : (
        <ul class="files">
          {entries.map((f) => {
            const full = joinPath(path, f.name);
            return (
              <li>
                {f.dir ? (
                  <button type="button" class="link" onClick={() => open(full)}>
                    📁 {f.name}
                  </button>
                ) : (
                  <span class="name">{f.name}</span>
                )}
                {!f.dir && <span class="size">{formatSize(f.size)}</span>}
                <span class="actions">
                  {!f.dir && (
                    <button type="button" class="link" onClick={() => send({ type: "sdRead", path: full })}>
                      {tr("download")}
                    </button>
                  )}
                  <button
                    type="button"
                    class="link danger"
                    onClick={() =>
                      ask({
                        message: tr("deleteFile").replace("{name}", f.name),
                        action: tr("delete"),
                        danger: true,
                        onAnswer: () => send({ type: "sdRemove", path: full, dir: path }),
                      })
                    }
                  >
                    {tr("delete")}
                  </button>
                </span>
              </li>
            );
          })}
        </ul>
      )}
    </section>
  );
}

/** The main site, or (`embed`) just the device, for an iframe with the API of docs/EMBED.md. */
export function App({ embed = false }: { embed?: boolean }) {
  const worker = useRef<Worker | null>(null);
  const canvas = useRef<HTMLCanvasElement>(null);
  const stage = useRef<HTMLDivElement>(null);
  const deviceBox = useRef<HTMLDivElement>(null);
  const bits = useRef(new Uint8Array(1024));
  const backlight = useRef(0);
  const ink = useRef(new Float32Array(LCD_W * LCD_H));
  const ghostOn = useRef(false);
  const ghostFrame = useRef(0);
  const captureCanvas = useRef<HTMLCanvasElement | null>(null);
  const recorder = useRef<MediaRecorder | null>(null);
  const gif = useRef<{ frames: GifFrame[]; last: number; start: number } | null>(null);
  const otaRef = useRef(false);
  const deviceLabel = useRef("dryflash");
  const cardPath = useRef("/");

  const [ready, setReady] = useState(false);
  const [fidelity, setFidelity] = useState<FidelityRow[]>([]);
  const [firmware, setFirmware] = useState<string | null>(null);
  const [devices, setDevices] = useState<DeviceMeta[]>([]);
  const [active, setActive] = useState<DeviceMeta | null>(null);
  const [persistence, setPersistence] = useState<Persistence>("none");
  const [cardCapacity, setCardCapacity] = useState(0);
  const [cardUsed, setCardUsed] = useState(0);
  const [listing, setListing] = useState<{ path: string; entries: CardEntry[] | null }>({ path: "/", entries: null });
  const [installed, setInstalled] = useState<string | null>(null);
  const [ota, setOta] = useState(false);
  const [status, setStatus] = useState<Status | null>(null);
  const [leds, setLeds] = useState<[number, number, number, number]>([0, 0, 0, 0]);
  const [log, setLog] = useState("");
  const [events, setEvents] = useState<string[]>([]);
  const [crashes, setCrashes] = useState<CrashRecord[]>([]);
  const [stop, setStop] = useState<Stop | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [turbo, setTurbo] = useState(false);
  const [ghosting, setGhosting] = useState(false);
  const [recording, setRecording] = useState<"webm" | "gif" | null>(null);
  const [question, setQuestion] = useState<Question | null>(null);
  const [fetching, setFetching] = useState<string | null>(null);
  const [provenance, setProvenance] = useState<string | null>(null);
  const [dragging, setDragging] = useState(false);
  const [tab, setTab] = useState<Tab>("device");
  const [view, setView] = useState<View>(() => fitView(window.innerWidth - 48));
  const [pressed, setPressed] = useState<Set<ButtonName>>(new Set());

  const send: Send = (m, transfer = []) => worker.current?.postMessage(m, transfer);
  const toParent = (m: Record<string, unknown>, transfer: Transferable[] = []) => {
    if (embed) window.parent.postMessage({ source: "dryflash", ...m }, "*", transfer);
  };
  const framesOut = useRef<{ on: boolean; last: number }>({ on: false, last: 0 });
  const started = useRef(false);

  /** Downloads a firmware and hands it to the emulator. */
  const loadFromUrl = async (link: Pick<LinkParams, "fw" | "elf" | "ota">, deviceLabel?: string) => {
    try {
      const f = await fetchFirmware(link.fw, location.href, (done, total) => {
        const percent = total ? `${Math.round((done * 100) / total)}%` : formatSize(done);
        setFetching(tr("downloading").replace("{name}", link.fw.split("/").pop() ?? "").replace("{percent}", percent));
      });
      setFetching(null);
      setProvenance(tr(f.viaProxy ? "viaProxy" : "direct").replace("{name}", f.name).replace("{url}", link.fw));
      send({ type: "load", name: f.name, bytes: f.bytes, ota: link.ota, deviceLabel }, [f.bytes]);
      if (link.elf) {
        const e = await fetchFirmware(link.elf, location.href);
        send({ type: "symbols", bytes: e.bytes }, [e.bytes]);
      }
    } catch (err) {
      setFetching(null);
      setError(String(err instanceof Error ? err.message : err));
      toParent({ type: "error", message: String(err) });
    }
  };

  const drawCapture = () => {
    const c = captureCanvas.current;
    const from = canvas.current;
    if (!c || !from) return;
    const g = c.getContext("2d");
    if (!g) return;
    g.imageSmoothingEnabled = false;
    g.drawImage(from, 0, 0, c.width, c.height);
  };

  const redraw = () => {
    const c = canvas.current?.getContext("2d");
    if (!c) return;
    const img = c.createImageData(LCD_W, LCD_H);
    if (ghostOn.current) paintInk(ink.current, backlight.current, img.data);
    else paint(bits.current, backlight.current, img.data);
    c.putImageData(img, 0, 0);
    drawCapture();
  };

  const ghostTick = () => {
    ghostFrame.current = 0;
    if (!ghostOn.current) return;
    const moving = ghostStep(bits.current, ink.current, 0.35);
    redraw();
    if (moving) ghostFrame.current = requestAnimationFrame(ghostTick);
  };

  const onFrame = () => {
    if (ghostOn.current) {
      if (!ghostFrame.current) ghostFrame.current = requestAnimationFrame(ghostTick);
    } else redraw();
    const g = gif.current;
    if (g) {
      const now = performance.now();
      if (now - g.last >= GIF_FRAME_MS) {
        const n = g.frames.length;
        if (n) g.frames[n - 1].delayMs = now - g.last;
        g.frames.push({ bits: bits.current.slice(), delayMs: GIF_FRAME_MS });
        g.last = now;
      }
      if (now - g.start > GIF_MAX_MS) stopGif();
    }
  };

  const stopGif = () => {
    const g = gif.current;
    gif.current = null;
    setRecording(null);
    if (g?.frames.length) {
      const bytes = encodeGif(g.frames, GIF_SCALE, inkColour, glass(backlight.current));
      download(`${deviceLabel.current}.gif`, bytes.slice().buffer, "image/gif");
    }
  };

  useEffect(() => {
    const w = new Worker(new URL("./emu/worker.ts", import.meta.url), { type: "module" });
    worker.current = w;
    const decoder = new TextDecoder();
    w.onmessage = (ev: MessageEvent<FromWorker>) => {
      const m = ev.data;
      switch (m.type) {
        case "ready":
          setFidelity(m.fidelity);
          setReady(true);
          toParent({ type: "ready", version: m.version });
          break;
        case "milestone":
          toParent({ type: m.name });
          break;
        case "devices":
          setDevices(m.devices);
          setActive(m.active);
          deviceLabel.current = m.active.label.replace(/[^\w.-]+/g, "_") || "dryflash";
          setPersistence(m.persistence);
          setCardCapacity(m.cardCapacityBytes);
          setCardUsed(m.cardUsedBytes);
          setFirmware(m.hasFirmware ? (m.active.firmware ?? "?") : null);
          setStatus(null);
          break;
        case "exported":
          download(`${m.name.replace(/[^\w.-]+/g, "_")}.dryflash`, m.bytes, "application/gzip");
          break;
        case "sdListing":
          cardPath.current = m.path;
          setListing({ path: m.path, entries: m.entries });
          break;
        case "sdFile":
          download(m.name, m.bytes);
          break;
        case "crashes":
          setCrashes(m.crashes);
          if (m.crashes.length) toParent({ type: "crashed", summary: m.crashes[m.crashes.length - 1].message });
          break;
        case "loaded":
          setFirmware(m.name);
          setInstalled(
            m.packageInfo
              ? tr("installed").replace("{files}", String(m.packageInfo.files)).replace("{info}", m.packageInfo.info)
              : null,
          );
          setLog("");
          setEvents([]);
          setStop(null);
          setError(null);
          break;
        case "otaStarted":
          setFirmware(m.name);
          setStop(null);
          setError(null);
          setInstalled(tr("otaStarted").replace("{name}", m.name).replace("{info}", m.info));
          break;
        case "frame":
          bits.current = new Uint8Array(m.bits);
          onFrame();
          if (framesOut.current.on && performance.now() - framesOut.current.last >= 200) {
            framesOut.current.last = performance.now();
            const copy = bits.current.slice().buffer;
            toParent({ type: "frame", bits: copy }, [copy]);
          }
          break;
        case "lights":
          backlight.current = m.rgbw[3];
          setLeds(m.rgbw);
          setEvents((list) =>
            [
              `${new Date().toLocaleTimeString()} ${tr("ledEvent")
                .replace("{r}", String(m.rgbw[0]))
                .replace("{g}", String(m.rgbw[1]))
                .replace("{b}", String(m.rgbw[2]))
                .replace("{w}", String(m.rgbw[3]))}`,
              ...list,
            ].slice(0, EVENT_LIMIT),
          );
          redraw();
          break;
        case "log": {
          const text = stripAnsi(decoder.decode(new Uint8Array(m.bytes), { stream: true }));
          setLog((l) => (l + text).slice(-LOG_LIMIT));
          break;
        }
        case "status":
          setStatus(m.status);
          break;
        case "stopped":
          setStop({ reason: m.reason, crashes: m.crashes, digest: m.digest });
          toParent({ type: "crashed", summary: m.crashes[m.crashes.length - 1]?.message ?? m.reason });
          break;
        case "error":
          setError(m.message);
          toParent({ type: "error", message: m.message });
          break;
      }
    };
    redraw();
    w.postMessage({ type: "init", ephemeral: embed } satisfies ToWorker);
    // Development only: `?fw=/@fs/<absolute path>` loads a local firmware file through Vite.
    const fw = import.meta.env.DEV ? new URLSearchParams(location.search).get("fw") : null;
    if (fw) {
      const onReady = (ev: MessageEvent<FromWorker>) => {
        if (ev.data.type !== "ready") return;
        w.removeEventListener("message", onReady);
        void fetch(fw)
          .then((r) => r.arrayBuffer())
          .then((bytes) => w.postMessage({ type: "load", name: fw.split("/").pop()!, bytes }, [bytes]));
        // `&ota=<path>`: once it has booted, hand a package to its updater.
        const pkg = new URLSearchParams(location.search).get("ota");
        if (pkg) {
          setTimeout(() => {
            void fetch(pkg)
              .then((r) => r.arrayBuffer())
              .then((bytes) => w.postMessage({ type: "load", name: pkg.split("/").pop()!, bytes, ota: true }, [bytes]));
          }, 8000);
        }
      };
      w.addEventListener("message", onReady);
    }
    // Written blocks are saved about once a second; save the rest when the page goes away.
    const flush = () => w.postMessage({ type: "flush" } satisfies ToWorker);
    const onHide = () => {
      if (document.visibilityState === "hidden") flush();
    };
    window.addEventListener("pagehide", flush);
    document.addEventListener("visibilitychange", onHide);
    return () => {
      window.removeEventListener("pagehide", flush);
      document.removeEventListener("visibilitychange", onHide);
      w.terminate();
    };
  }, []);

  // A link's firmware: once, when the device list is known.
  useEffect(() => {
    if (!ready || !active || started.current) return;
    started.current = true;
    const dev = import.meta.env.DEV && new URLSearchParams(location.search).get("fw")?.startsWith("/@fs/");
    const link = dev ? null : parseLink(location.search);
    if (!link) return;
    framesOut.current.on = link.frames;
    if (embed) {
      void loadFromUrl(link);
      return;
    }
    const label = link.device ?? LINK_DEVICE;
    const target = devices.find((d) => d.label === label);
    const go = () => void loadFromUrl(link, label);
    if (!linkMustAsk(link, target ? { hasFirmware: !!target.firmware } : null)) go();
    else {
      setQuestion({
        message: `${tr("linkAsk")} ${link.fw} — ${tr("linkDevice").replace("{device}", label)}`,
        action: tr("load"),
        onAnswer: go,
      });
    }
  }, [ready, active]);

  // Commands from the page around an embed frame, and only from it (docs/EMBED.md).
  useEffect(() => {
    if (!embed) return;
    const onCommand = (ev: MessageEvent) => {
      if (ev.source !== window.parent) return;
      const m = ev.data as { type?: string; url?: string; bytes?: ArrayBuffer; name?: string; button?: ButtonName; down?: boolean };
      switch (m?.type) {
        case "load":
          if (m.url) void loadFromUrl({ fw: m.url, elf: null, ota: false });
          else if (m.bytes) send({ type: "load", name: m.name ?? "firmware", bytes: m.bytes }, [m.bytes]);
          break;
        case "press":
          if (m.button) {
            if (m.down === undefined) {
              send({ type: "button", button: m.button, down: true });
              send({ type: "button", button: m.button, down: false });
            } else send({ type: "button", button: m.button, down: m.down });
          }
          break;
        case "reset":
          send({ type: "control", action: "reset" });
          break;
        case "screenshot":
          ensureCapture().toBlob((blob) => {
            void blob?.arrayBuffer().then((png) => toParent({ type: "screenshot", png }, [png]));
          }, "image/png");
          break;
      }
    };
    window.addEventListener("message", onCommand);
    return () => window.removeEventListener("message", onCommand);
  }, []);

  // The drawing and the panel keep to whole multiples of their pixels.
  useEffect(() => {
    const el = stage.current;
    if (!el) return;
    const update = () => {
      const full = document.fullscreenElement === deviceBox.current;
      const next = full ? fitView(window.innerWidth, window.innerHeight, 12) : fitView(el.clientWidth - 16);
      // Only a real change: resizing the device resizes this element, which would observe itself.
      setView((v) => (v.scale === next.scale && v.model.width === next.model.width ? v : next));
    };
    const ro = new ResizeObserver(() => requestAnimationFrame(update));
    ro.observe(el);
    document.addEventListener("fullscreenchange", update);
    return () => {
      ro.disconnect();
      document.removeEventListener("fullscreenchange", update);
    };
  }, []);

  const press = (b: ButtonName, down: boolean) => {
    setPressed((p) => {
      const n = new Set(p);
      if (down) n.add(b);
      else n.delete(b);
      return n;
    });
    send({ type: "button", button: b, down });
  };

  useEffect(() => {
    // An on-screen key that has the focus works with Enter and Space as itself; other controls keep
    // their keys, and anywhere else the keyboard drives the device.
    const target = (e: KeyboardEvent): ButtonName | null => {
      const el = e.target;
      if (el instanceof HTMLElement && el.classList.contains("key")) {
        if (e.key === "Enter" || e.key === " ") return el.dataset.button as ButtonName;
        return buttonForKey(e.key);
      }
      if (document.querySelector("dialog[open]")) return null;
      if (
        el instanceof HTMLInputElement ||
        el instanceof HTMLSelectElement ||
        el instanceof HTMLButtonElement ||
        el instanceof HTMLTextAreaElement ||
        (el instanceof HTMLElement && el.tagName === "SUMMARY")
      ) {
        return null;
      }
      return buttonForKey(e.key);
    };
    const down = (e: KeyboardEvent) => {
      const b = target(e);
      if (!b || !firmware) return;
      e.preventDefault();
      if (!e.repeat) press(b, true);
    };
    const up = (e: KeyboardEvent) => {
      const b = target(e);
      if (!b || !firmware) return;
      e.preventDefault();
      press(b, false);
    };
    window.addEventListener("keydown", down);
    window.addEventListener("keyup", up);
    return () => {
      window.removeEventListener("keydown", down);
      window.removeEventListener("keyup", up);
    };
  }, [firmware]);

  const load = async (file: File) => {
    setError(null);
    const bytes = await file.arrayBuffer();
    send({ type: "load", name: file.name, bytes, ota: otaRef.current }, [bytes]);
  };

  const onDrop = (e: DragEvent) => {
    e.preventDefault();
    setDragging(false);
    const f = e.dataTransfer?.files[0];
    if (f) void load(f);
  };

  const ensureCapture = () => {
    const c = captureCanvas.current ?? document.createElement("canvas");
    c.width = LCD_W * CAPTURE_SCALE;
    c.height = LCD_H * CAPTURE_SCALE;
    captureCanvas.current = c;
    drawCapture();
    return c;
  };

  const screenshot = () => {
    ensureCapture().toBlob((blob) => {
      if (blob) download(`${deviceLabel.current}.png`, blob, "image/png");
    }, "image/png");
  };

  const toggleWebm = () => {
    if (recorder.current) {
      recorder.current.stop();
      return;
    }
    if (typeof MediaRecorder === "undefined") {
      setError(tr("noRecorder"));
      return;
    }
    const type = ["video/webm;codecs=vp9", "video/webm", "video/mp4"].find((t) => MediaRecorder.isTypeSupported(t));
    if (!type) {
      setError(tr("noRecorder"));
      return;
    }
    const rec = new MediaRecorder(ensureCapture().captureStream(30), { mimeType: type });
    const chunks: Blob[] = [];
    rec.ondataavailable = (e) => chunks.push(e.data);
    rec.onstop = () => {
      recorder.current = null;
      setRecording(null);
      download(`${deviceLabel.current}.${type.includes("mp4") ? "mp4" : "webm"}`, new Blob(chunks, { type }), type);
    };
    recorder.current = rec;
    rec.start();
    setRecording("webm");
  };

  const toggleGif = () => {
    if (gif.current) {
      stopGif();
      return;
    }
    const now = performance.now();
    gif.current = { frames: [{ bits: bits.current.slice(), delayMs: GIF_FRAME_MS }], last: now, start: now };
    setRecording("gif");
  };

  const toggleGhosting = (on: boolean) => {
    setGhosting(on);
    ghostOn.current = on;
    if (on) {
      for (let i = 0; i < ink.current.length; i++) ink.current[i] = (bits.current[i >> 3] >> (7 - (i & 7))) & 1;
    }
    redraw();
  };

  const running = status?.running ?? false;
  const cardUsage = `${tr("cardUsage")
    .replace("{used}", ((status?.cardUsedBytes ?? cardUsed) / 1e6).toFixed(1))
    .replace("{total}", (cardCapacity / 1e6).toFixed(0))} ${tr(`persist_${persistence}` as Key)}`;
  const hasFirmware = !!firmware;

  return (
    <main
      onDragOver={(e) => {
        e.preventDefault();
        setDragging(true);
      }}
      onDragLeave={() => setDragging(false)}
      onDrop={onDrop}
      class={dragging ? "dragging" : ""}
    >
      {!embed && <SiteHeader />}

      <div class={embed ? "layout embed" : "layout"}>
        <section class="stage" ref={stage} aria-label={tr("device")}>
          <DeviceView
            view={view}
            canvas={canvas}
            leds={leds}
            pressed={pressed}
            press={press}
            enabled={hasFirmware}
            box={deviceBox}
          />
          {fetching && (
            <p class="meter" role="status">
              {fetching}
            </p>
          )}
          {embed && error && (
            <p class="error" role="alert">
              {error}
            </p>
          )}
        </section>

        {!embed && (
          <>
            <div class="toolbar">
              <button type="button" disabled={!hasFirmware} onClick={() => send({ type: "control", action: "reset" })}>
                {tr("reset")}
              </button>
              <button
                type="button"
                disabled={!hasFirmware}
                onClick={() => send({ type: "control", action: running ? "pause" : "resume" })}
              >
                {running ? tr("pause") : tr("resume")}
              </button>
              <label>
                <input
                  type="checkbox"
                  checked={turbo}
                  onChange={(e) => {
                    const on = (e.currentTarget as HTMLInputElement).checked;
                    setTurbo(on);
                    send({ type: "turbo", on });
                  }}
                />{" "}
                {tr("turbo")}
              </label>
              <button type="button" disabled={!hasFirmware} onClick={screenshot}>
                {tr("screenshot")}
              </button>
              <button type="button" disabled={!hasFirmware || recording === "gif"} onClick={toggleWebm}>
                {recording === "webm" ? tr("stopRecording") : tr("recordWebm")}
              </button>
              <button type="button" disabled={!hasFirmware || recording === "webm"} onClick={toggleGif} title={tr("gifLimit")}>
                {recording === "gif" ? tr("stopRecording") : tr("recordGif")}
              </button>
              <button type="button" onClick={() => void deviceBox.current?.requestFullscreen()}>
                {tr("fullscreen")}
              </button>
              <label>
                <input
                  type="checkbox"
                  checked={ghosting}
                  onChange={(e) => toggleGhosting((e.currentTarget as HTMLInputElement).checked)}
                />{" "}
                {tr("ghosting")}
              </label>
            </div>
            {recording && (
              <p class="hint rec" role="status">
                {tr("recording")}
              </p>
            )}
            {status && (
              <p class="meter">
                {running ? tr("running") : tr("paused")} · {tr("virtualTime")} {(status.virtualMs / 1000).toFixed(1)} s ·{" "}
                {tr("speed")} {status.speed.toFixed(2)}× · {status.mips.toFixed(0)} MIPS
              </p>
            )}
            <p class="hint">{tr("keyboard")}</p>
            {error && (
              <p class="error" role="alert">
                {tr("error")}: {error}
              </p>
            )}
            {stop && (
              <div class="error" role="alert">
                <strong>{tr("stopped")}:</strong> {stop.reason}
              </div>
            )}

            <Tabs tab={tab} setTab={setTab} badge={crashes.length > 0 || !!stop} />
            <div class="tabpanel" id="tabpanel" role="tabpanel" aria-labelledby={`tab-${tab}`}>
              {tab === "device" && (
                <>
                  <section class="tab-section">
                    <h2>{tr("deviceInfo")}</h2>
                    <dl class="info">
                      <dt>{tr("firmware")}</dt>
                      <dd>{firmware ?? tr("noFirmwareOn")}</dd>
                      <dt>{tr("deviceName")}</dt>
                      <dd>{active?.board.name}</dd>
                      <dt>{tr("region")}</dt>
                      <dd>{active && tr(`region${active.board.region}` as Key)}</dd>
                      <dt>{tr("batteryCharge")}</dt>
                      <dd>{active?.battery.percent}%</dd>
                      <dt>{tr("card")}</dt>
                      <dd>{cardUsage}</dd>
                    </dl>
                  </section>
                  <Devices devices={devices} active={active} firmware={firmware} send={send} ask={setQuestion} />
                </>
              )}

              {tab === "update" && (
                <section class="tab-section">
                  <h2>{tr("firmwarePanel")}</h2>
                  <fieldset class="modes" disabled={!hasFirmware}>
                    <legend>{tr("installMode")}</legend>
                    <label>
                      <input
                        type="radio"
                        name="mode"
                        checked={!ota}
                        onChange={() => {
                          setOta(false);
                          otaRef.current = false;
                        }}
                      />{" "}
                      {tr("quickMode")}
                    </label>
                    <label>
                      <input
                        type="radio"
                        name="mode"
                        checked={ota}
                        onChange={() => {
                          setOta(true);
                          otaRef.current = true;
                        }}
                      />{" "}
                      {tr("otaMode")}
                    </label>
                  </fieldset>
                  <p class="hint">{tr("quickNote")}</p>
                  <p class="hint">{tr("otaNote")}</p>
                  {provenance && <p class="hint">{provenance}</p>}
                  {installed && <p class="hint">{installed}</p>}
                </section>
              )}

              {tab === "files" && (
                <CardFiles
                  usage={cardUsage}
                  path={listing.path}
                  entries={listing.entries}
                  ready={ready}
                  send={send}
                  ask={setQuestion}
                />
              )}

              {tab === "backup" && (
                <section class="tab-section">
                  <h2>{tr("tabBackup")}</h2>
                  <p class="hint">{tr("backupNote")}</p>
                  <div class="controls">
                    <button type="button" disabled={!active} onClick={() => send({ type: "exportDevice" })}>
          {tr("exportDevice")}
        </button>
        <label class="file-button">
          {tr("importDevice")}
          <input
            type="file"
            accept=".dryflash"
            disabled={!active}
            onChange={async (e) => {
              const input = e.currentTarget as HTMLInputElement;
              const f = input.files?.[0];
              input.value = "";
              if (f) {
                const bytes = await f.arrayBuffer();
                send({ type: "importDevice", bytes }, [bytes]);
              }
            }}
          />
        </label>
                  </div>
                </section>
              )}

              {tab === "diagnostics" && (
                <>
                  <section class="tab-section">
                    <h2>{tr("crashPanel")}</h2>
                    {crashes.length === 0 && !stop?.crashes.length ? (
                      <p class="hint">{tr("noCrashes")}</p>
                    ) : (
                      <ul class="crashes">
                        {(crashes.length ? crashes : (stop?.crashes ?? [])).map((c) => (
                          <li>
                            {(c.atMs / 1000).toFixed(3)} s — {c.message}
                          </li>
                        ))}
                      </ul>
                    )}
                    {stop && (
                      <details>
                        <summary>{tr("digest")}</summary>
                        <pre>{stop.digest}</pre>
                      </details>
                    )}
                  </section>
                  <section class="tab-section">
                    <h2>{tr("log")}</h2>
                    <pre class="log">{log}</pre>
                  </section>
                  <section class="tab-section">
                    <h2>{tr("eventsPanel")}</h2>
                    <p class="hint">{tr("moreEventsLater")}</p>
                    {events.length === 0 ? (
                      <p class="hint">{tr("noEvents")}</p>
                    ) : (
                      <ul class="events">
                        {events.map((e) => (
                          <li>{e}</li>
                        ))}
                      </ul>
                    )}
                  </section>
                  <section class="tab-section">
                    <h2>{tr("fidelity")}</h2>
                    <Fidelity rows={fidelity} />
                  </section>
                </>
              )}
            </div>

            <label class="drop">
              <span>{ready ? tr("drop") : tr("starting")}</span>
              <input
                type="file"
                accept=".dfu,.tgz"
                disabled={!ready}
                onChange={(e) => {
                  const input = e.currentTarget as HTMLInputElement;
                  const f = input.files?.[0];
                  input.value = "";
                  if (f) void load(f);
                }}
              />
              <span class="choose">{tr("choose")}</span>
              <span class="hint">{tr("privacy")}</span>
            </label>
          </>
        )}
      </div>

      <AskDialog question={question} close={() => setQuestion(null)} />

      {!embed && <SiteFooter />}
    </main>
  );
}

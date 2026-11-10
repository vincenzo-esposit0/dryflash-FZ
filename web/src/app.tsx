// SPDX-License-Identifier: GPL-3.0-or-later
import { useEffect, useRef, useState } from "preact/hooks";
import type { Persistence } from "./emu/cardstore";
import type { BatterySettings, DeviceMeta } from "./emu/devices";
import type {
  ButtonName,
  CardEntry,
  CrashRecord,
  FidelityRow,
  CrashReport,
  FromWorker,
  Status,
  ToWorker,
} from "./emu/protocol";
import { tr, type Key } from "./i18n";
import { LINK_DEVICE, fetchFirmware, linkMustAsk, parseLink, type LinkParams } from "./emu/remote";
import { durationMs, irFile, type IrSignal } from "./emu/infrared";
import { stripAnsi } from "./ui/ansi";
import { download, formatSize, joinPath, parentPath } from "./ui/download";
import { encodeGif, type GifFrame } from "./ui/gif";
import { buttonForKey } from "./ui/keys";
import { Fidelity } from "./pages";
import { Prompt, SiteFooter, SiteHeader } from "./ui/chrome";
import { Picker } from "./ui/picker";
import { ART_STEP, artSize, drawView, fitView, fitWorkspace, type Skin, skinForColour, toRgba, type View } from "./ui/shell";
import { LCD_H, LCD_W, ghostStep, glass, inkColour, paint, paintInk } from "./ui/lcd";
import { Speaker } from "./ui/sound";
import { CliPanel, noteSerial, writeSerial, type UsbConnection } from "./ui/terminal";

const LOG_LIMIT = 64 * 1024;
const EVENT_LIMIT = 200;
/** A buzz stays visible at least this long: the firmware's are often 100 ms or less. */
const BUZZ_MIN_MS = 200;
/** Longest phone vibration asked for at once; the motor going off cancels it. */
const BUZZ_MAX_MS = 10_000;
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

/** A message kept in the page: it follows the language if that changes while it is up. */
interface Notice {
  key: Key;
  vars: Record<string, string>;
}
const noticeText = (n: Notice) => Object.entries(n.vars).reduce((text, [k, v]) => text.replace(`{${k}}`, v), tr(n.key));

interface Stop {
  reason: string;
  crashes: CrashRecord[];
  digest: string;
  report: CrashReport | null;
  markdown: string | null;
  png: ArrayBuffer | null;
}

/** The report on why the emulator stopped (BRIEF §7). Its own text stays in English, as in the files. */
function CrashPanel({ stop }: { stop: Stop }) {
  const r = stop.report!;
  const file = `dryflash-crash-${Math.round(r.atMs)}ms`;
  return (
    <div class="report">
      <p class={`verdict ${r.verdict}`}>
        <strong>{tr(`verdict_${r.verdict}` as Key)}</strong>
      </p>
      <p>{r.summary}</p>
      <ul class="basis">
        {r.basis.map((b) => (
          <li>{b}</li>
        ))}
      </ul>
      <p class="hint">{tr("verdictNote")}</p>
      {tr("coreEnglish") && <p class="hint">{tr("coreEnglish")}</p>}
      <div class="toolbar">
        <button onClick={() => download(`${file}.json`, JSON.stringify(r, null, 2), "application/json")}>
          {tr("downloadJson")}
        </button>
        {stop.markdown && (
          <button onClick={() => download(`${file}.md`, stop.markdown!, "text/markdown")}>{tr("downloadMarkdown")}</button>
        )}
        {stop.png && <button onClick={() => download(`${file}.png`, stop.png!, "image/png")}>{tr("downloadPng")}</button>}
      </div>
      {r.furi && (
        <p>
          {tr("furiRecord")}: {r.furi.task ?? "?"} — {r.furi.message}
        </p>
      )}
      <details>
        <summary>{tr("technicalDetails")}</summary>
      <h3>{tr("backtrace")}</h3>
      <p class="hint">{tr("backtraceNote")}</p>
      <ul class="crashes frames">
        {r.backtrace.map((f) => (
          <li>
            {f.address} {f.symbol ?? "?"} ({f.source})
          </li>
        ))}
      </ul>
      <h3>{tr("registers")}</h3>
      <p class="hint">{r.stateFrom}</p>
      <pre>
        {Object.entries(r.registers)
          .map(([n, v]) => `${n.padEnd(8)}${v}`)
          .join("\n")}
      </pre>
      {r.faultCauses.length > 0 && (
        <ul class="basis">
          {r.faultCauses.map((c) => (
            <li>{c}</li>
          ))}
        </ul>
      )}
      <details>
        <summary>{tr("recentPcs")}</summary>
        <p class="hint">{r.recentPcsNote}</p>
        <pre>{r.recentPcs.map((p) => `${p.pc} ${p.symbol ?? ""}`).join("\n")}</pre>
      </details>
      <details>
        <summary>{tr("gaps")}</summary>
        {r.gaps.length === 0 ? (
          <p class="hint">{tr("noGaps")}</p>
        ) : (
          <ul class="basis">
            {r.gaps.map((g) => (
              <li>
                {g.block}: {g.registers.join(", ")}
              </li>
            ))}
          </ul>
        )}
      </details>
      </details>
    </div>
  );
}

/** The pixel-art device (ADR-0015), with the real canvas and buttons on its face. */
function DeviceView({
  view,
  skin,
  canvas,
  leds,
  pressed,
  press,
  enabled,
  buzzing,
  box,
}: {
  view: View;
  skin: Skin;
  canvas: preact.RefObject<HTMLCanvasElement | null>;
  leds: [number, number, number, number];
  pressed: Set<ButtonName>;
  press: (b: ButtonName, d: boolean) => void;
  enabled: boolean;
  /** The vibration motor is on. */
  buzzing: boolean;
  box: preact.RefObject<HTMLDivElement | null>;
}) {
  const m = view.model;
  const size = artSize(m);
  const art = useRef<HTMLCanvasElement>(null);
  useEffect(() => {
    const c = art.current?.getContext("2d");
    if (!c) return;
    const img = c.createImageData(size.width, size.height);
    toRgba(drawView(m, skin), img.data, skin);
    c.clearRect(0, 0, size.width, size.height);
    c.putImageData(img, 0, 0);
  }, [m.width, skin]);
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
      class={buzzing ? "device buzz" : "device"}
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

/** USB cable and charge, in effect at once and kept with the device; and what the charger does. */
/** An entry of the Events panel; an IR transmission carries itself as an `.ir` file to save. */
type EventItem = { text: string; ir?: { name: string; text: string } };

function irEvent(s: IrSignal): EventItem {
  const text = tr("irEvent")
    .replace("{khz}", (s.hz / 1000).toFixed(1))
    .replace("{duty}", (s.duty * 100).toFixed(0))
    .replace("{n}", String(s.timingsUs.length))
    .replace("{ms}", durationMs(s).toFixed(1));
  const file = irFile(s, "Sent");
  const name = `ir-${s.atMs.toFixed(0)}ms.ir`; // virtual time: unique within a run
  return {
    text: s.hz > 0 ? text : tr("irEventConstant").replace("{n}", String(s.timingsUs.length)),
    ir: file ? { name, text: file } : undefined,
  };
}

function BatteryControls({ battery: stored, status, send }: { battery: BatterySettings; status: Status | null; send: Send }) {
  // What the person just set, until the worker's device list catches up: a status update that
  // re-renders in between would otherwise put the stored value back into the controls.
  const [battery, setLocal] = useState(stored);
  useEffect(() => setLocal(stored), [stored.usb, stored.percent]);
  const set = (b: Partial<BatterySettings>) => {
    const next = { ...battery, ...b };
    setLocal(next);
    send({ type: "settings", battery: next });
  };
  const state = status?.off
    ? tr("poweredOff")
    : status?.charging
      ? battery.percent >= 100
        ? tr("charged")
        : tr("charging")
      : tr("onBattery");
  return (
    <div class="group" role="group" aria-label={tr("battery")}>
      <span aria-hidden="true">{tr("battery")}</span>
      <label title={tr("usbNote")}>
        <input type="checkbox" checked={battery.usb} onChange={(e) => set({ usb: (e.currentTarget as HTMLInputElement).checked })} />{" "}
        {tr("usb")}
      </label>
      <label>
        {tr("batteryCharge")}{" "}
        <input
          type="range"
          min={0}
          max={100}
          value={battery.percent}
          onChange={(e) => set({ percent: +(e.currentTarget as HTMLInputElement).value })}
        />{" "}
        <span>{battery.percent}%</span>
      </label>
      <span class="state" role="status">
        {state}
      </span>
      {status?.off && (
        <button type="button" title={tr("powerOnNote")} onClick={() => send({ type: "control", action: "reset" })}>
          {tr("powerOn")}
        </button>
      )}
    </div>
  );
}

function DeviceSettings({ device, send }: { device: DeviceMeta; send: Send }) {
  const board = device.board;
  const options = (prefix: "region", values: number[]) =>
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
          <input type="checkbox" checked={device.legacySleep} onChange={(e) => send({ type: "settings", legacySleep: checked(e) })} />{" "}
          {tr("legacySleep")}
        </label>
        <p class="hint">{tr("legacySleepNote")}</p>
        <p class="hint">{tr("applyNext")}</p>
      </div>
    </details>
  );
}

/** The colour the firmware reads from OTP, which is also the colour of the drawn body (ADR-0015). */
function SkinChoice({ device, send }: { device: DeviceMeta | null; send: Send }) {
  return (
    <fieldset class="skins" disabled={!device}>
      <legend>{tr("color")}</legend>
      {[1, 2, 3].map((c) => (
        <label class="skin">
          <input
            type="radio"
            name="skin"
            checked={device?.board.color === c}
            onChange={() => device && send({ type: "settings", board: { ...device.board, color: c } })}
          />
          <span class={`swatch c${c}`} aria-hidden="true" />
          {tr(`color${c}` as Key)}
        </label>
      ))}
    </fieldset>
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

type Tab = "device" | "cli" | "update" | "files" | "backup" | "diagnostics";
const TABS: [Tab, Key][] = [
  ["device", "tabDevice"],
  ["cli", "tabCli"],
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
  const [installed, setInstalled] = useState<Notice | null>(null);
  const [ota, setOta] = useState(false);
  const [status, setStatus] = useState<Status | null>(null);
  const [leds, setLeds] = useState<[number, number, number, number]>([0, 0, 0, 0]);
  const [log, setLog] = useState("");
  const [events, setEvents] = useState<EventItem[]>([]);
  /** What the computer on the USB cable last saw, and whether the terminal has the port open. */
  const [usbConn, setUsbConn] = useState<UsbConnection>({ state: "waiting" });
  const [serialOpen, setSerialOpen] = useState(false);
  /** Events in the order they happened; the list shows the newest first. */
  const addEvents = (...items: (string | EventItem)[]) => {
    const time = new Date().toLocaleTimeString();
    const stamped = items.map((e) => (typeof e === "string" ? { text: `${time} ${e}` } : { ...e, text: `${time} ${e.text}` }));
    setEvents((list) => [...stamped.reverse(), ...list].slice(0, EVENT_LIMIT));
  };
  const [crashes, setCrashes] = useState<CrashRecord[]>([]);
  const [stop, setStop] = useState<Stop | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [turbo, setTurbo] = useState(false);
  const [ghosting, setGhosting] = useState(false);
  const [speaker] = useState(() => new Speaker());
  const [sound, setSound] = useState(false);
  /** Sound plays only at 1×: paused or in Turbo, virtual time and wall time part. */
  const audible = useRef(false);
  /** Whether the last status said the device had no power, to log the change once. */
  const wasOff = useRef(false);
  const [buzzing, setBuzzing] = useState(false);
  const buzzUntil = useRef(0);
  const buzzTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const [recording, setRecording] = useState<"webm" | "gif" | null>(null);
  const [question, setQuestion] = useState<Question | null>(null);
  const [fetching, setFetching] = useState<{ text: string; percent: number | null } | null>(null);
  // Between handing a firmware over and its first pixels. The first pixels are the only signal every firmware gives.
  const [phase, setPhaseState] = useState<"installing" | "booting" | null>(null);
  const phaseRef = useRef<"installing" | "booting" | null>(null);
  const setPhase = (p: "installing" | "booting" | null) => {
    phaseRef.current = p;
    setPhaseState(p);
  };
  const [provenance, setProvenance] = useState<Notice | null>(null);
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
        setFetching({
          text: tr("downloading").replace("{name}", link.fw.split("/").pop() ?? "").replace("{percent}", percent),
          percent: total ? Math.round((done * 100) / total) : null,
        });
      });
      setFetching(null);
      setProvenance({ key: f.viaProxy ? "viaProxy" : "direct", vars: { name: f.name, url: link.fw } });
      setPhase("installing");
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
          quiet();
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
          // Sent at power-on, so what is left of the last stop goes too. A crash stops, and tells the parent then.
          if (!m.crashes.length) setStop(null);
          break;
        case "loaded":
          setFirmware(m.name);
          setInstalled(m.packageInfo ? { key: "installed", vars: { files: String(m.packageInfo.files), info: m.packageInfo.info } } : null);
          setLog("");
          setEvents([]);
          quiet();
          setStop(null);
          setError(null);
          setPhase("booting");
          break;
        case "otaStarted":
          setFirmware(m.name);
          setStop(null);
          setError(null);
          setPhase(null);
          setInstalled({ key: "otaStarted", vars: { name: m.name, info: m.info } });
          break;
        case "frame":
          bits.current = new Uint8Array(m.bits);
          if (phaseRef.current === "booting" && bits.current.some((b) => b !== 0)) setPhase(null);
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
          addEvents(
            tr("ledEvent")
              .replace("{r}", String(m.rgbw[0]))
              .replace("{g}", String(m.rgbw[1]))
              .replace("{b}", String(m.rgbw[2]))
              .replace("{w}", String(m.rgbw[3])),
          );
          redraw();
          break;
        case "senses":
          if (audible.current) speaker.play(m.changes);
          for (const c of m.changes) {
            if ("vibro" in c) buzz(c.vibro);
          }
          addEvents(
            ...m.changes.map((c) =>
              "vibro" in c
                ? tr(c.vibro ? "vibroOn" : "vibroOff")
                : c.tone
                  ? tr("toneEvent").replace("{hz}", c.tone.hz.toFixed(0))
                  : tr("toneOff"),
            ),
          );
          break;
        case "ir":
          addEvents(...m.signals.map(irEvent));
          break;
        case "serial":
          writeSerial(new Uint8Array(m.bytes));
          break;
        case "usb":
          for (const u of m.events) {
            if (u.event === "configured") {
              setUsbConn({ state: "connected", product: u.product, serial: u.serial });
              addEvents(tr("usbConnectedEvent").replace("{product}", u.product).replace("{serial}", u.serial));
            } else if (u.event === "disconnected") {
              setUsbConn({ state: "waiting" });
              addEvents(tr("usbDisconnectedEvent"));
            } else {
              setUsbConn({ state: "failed", detail: u.detail });
              addEvents(tr("cliFailed").replace("{detail}", u.detail));
            }
          }
          break;
        case "log": {
          const text = stripAnsi(decoder.decode(new Uint8Array(m.bytes), { stream: true }));
          setLog((l) => (l + text).slice(-LOG_LIMIT));
          break;
        }
        case "status":
          if (m.status.off !== wasOff.current) {
            const off = m.status.off;
            wasOff.current = off;
            addEvents(tr(off ? "powerOffEvent" : "powerOnEvent"));
          }
          setStatus(m.status);
          audible.current = m.status.running && !m.status.turbo;
          if (!audible.current) speaker.silence();
          break;
        case "stopped":
          setPhase(null);
          quiet();
          setStop({
            reason: m.reason,
            crashes: m.crashes,
            digest: m.digest,
            report: m.report,
            markdown: m.markdown,
            png: m.png,
          });
          toParent({ type: "crashed", summary: m.crashes[m.crashes.length - 1]?.message ?? m.reason });
          break;
        case "error":
          setPhase(null);
          setError(m.message);
          toParent({ type: "error", message: m.message });
          break;
        case "uncaught":
          console.error("dryflash worker:", m.stack ?? m.message);
          setPhase(null);
          setError(m.message);
          toParent({ type: "error", message: m.message });
          break;
      }
    };
    // Only what the worker could not report itself: its script did not load.
    w.onerror = (ev) => {
      const message = ev.message || "the emulator's worker did not start";
      console.error("dryflash worker:", message);
      setError(message);
      toParent({ type: "error", message });
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
      const next = full ? fitView(window.innerWidth, window.innerHeight, 12) : fitWorkspace(el.clientWidth - 16, window.innerWidth, window.innerHeight);
      // Only a real change: resizing the device resizes this element, which would observe itself.
      setView((v) => (v.scale === next.scale && v.model.width === next.model.width ? v : next));
    };
    const ro = new ResizeObserver(() => requestAnimationFrame(update));
    ro.observe(el);
    window.addEventListener("resize", update);
    document.addEventListener("fullscreenchange", update);
    return () => {
      ro.disconnect();
      window.removeEventListener("resize", update);
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
      // A scrolling region keeps its arrow keys, as other controls keep theirs.
      if (el instanceof HTMLElement && el.classList.contains("scroll")) return null;
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
    setPhase("installing");
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

  /** The motor's state on the page, and on a phone's own motor. */
  const buzz = (on: boolean) => {
    clearTimeout(buzzTimer.current);
    const phone = "vibrate" in navigator && matchMedia("(pointer: coarse)").matches;
    if (on) {
      buzzUntil.current = performance.now() + BUZZ_MIN_MS;
      setBuzzing(true);
      if (phone) navigator.vibrate(BUZZ_MAX_MS);
    } else {
      if (phone) navigator.vibrate(0);
      buzzTimer.current = setTimeout(() => setBuzzing(false), Math.max(0, buzzUntil.current - performance.now()));
    }
  };

  /** Nothing plays or buzzes: the firmware stopped, or another one or another device took over. */
  const quiet = () => {
    audible.current = false;
    speaker.silence();
    buzz(false);
  };

  const toggleSound = async (on: boolean) => {
    setSound(on);
    if (!on) return void (await speaker.disable());
    if (!(await speaker.enable())) {
      setSound(false);
      setError(tr("noAudio"));
    }
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

  const pick = (url: string) => {
    setError(null);
    void loadFromUrl({ fw: url, elf: null, ota: otaRef.current });
  };
  const busy = !!fetching || !!phase;
  const deviceStage = (
    <section class="stage" ref={stage} aria-label={tr("device")}>
      <DeviceView
        view={view}
        skin={skinForColour(active?.board.color ?? 2)}
        canvas={canvas}
        leds={leds}
        pressed={pressed}
        press={press}
        enabled={hasFirmware}
        buzzing={buzzing}
        box={deviceBox}
      />
      {!embed && <SkinChoice device={active} send={send} />}
      {fetching && (
        <div class="progress" role="status">
          {fetching.text}
          {fetching.percent !== null && <progress max={100} value={fetching.percent} aria-hidden="true" />}
        </div>
      )}
      {!fetching && phase && (
        <div class="progress" role="status">
          {tr(phase)}
          <progress aria-hidden="true" />
        </div>
      )}
      {embed && error && (
        <p class="error" role="alert">
          {error}
        </p>
      )}
    </section>
  );

  if (embed) {
    return (
      <main class="embed" onDragOver={(e) => e.preventDefault()} onDrop={onDrop}>
        {deviceStage}
        <AskDialog question={question} close={() => setQuestion(null)} />
      </main>
    );
  }

  return (
    <div
      class="site"
      onDragOver={(e) => {
        e.preventDefault();
        setDragging(true);
      }}
      onDragLeave={() => setDragging(false)}
      onDrop={onDrop}
    >
      <SiteHeader />
      <main id="main">
        <Prompt command={`./run ${firmware ?? "--firmware=?"}`} />
        <div class="workspace">
          <div class="left">
            {deviceStage}
          </div>

          <div class="right">
            {error && (
              <p class="error" role="alert">
                {tr("error")}: {error}
              </p>
            )}
            {stop && !running && (
              <div class="error" role="alert">
                <strong>{tr("stopped")}:</strong> {stop.reason}
                {stop.report && (
                  <div>
                    <button type="button" onClick={() => setTab("diagnostics")}>
                      {tr("openReport")}
                    </button>
                  </div>
                )}
              </div>
            )}
            {hasFirmware ? (
              <div class="console">
            {status && (
              <p class={running && !status.off ? "meter" : "meter halted"}>
                <strong>{status.off ? tr("poweredOff") : running ? tr("running") : tr("paused")}</strong> · {tr("virtualTime")} {(status.virtualMs / 1000).toFixed(1)} s ·{" "}
                {tr("speed")} {status.speed.toFixed(2)}× · {status.mips.toFixed(0)} MIPS
              </p>
            )}
            {hasFirmware && (
              <div class="toolbar">
                <div class="group" role="group" aria-label={tr("groupPower")}>
                  <span aria-hidden="true">{tr("groupPower")}</span>
                  <button type="button" onClick={() => send({ type: "control", action: "reset" })}>
                    {tr("reset")}
                  </button>
                  <button
                    type="button"
                    onClick={() => {
                      if (running) quiet();
                      send({ type: "control", action: running ? "pause" : "resume" });
                    }}
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
                        if (on) quiet();
                        send({ type: "turbo", on });
                      }}
                    />{" "}
                    {tr("turbo")}
                  </label>
                </div>
                {active && <BatteryControls battery={active.battery} status={status} send={send} />}
                <div class="group" role="group" aria-label={tr("groupCapture")}>
                  <span aria-hidden="true">{tr("groupCapture")}</span>
                  <button type="button" onClick={screenshot}>
                    {tr("screenshot")}
                  </button>
                  <button type="button" disabled={recording === "gif"} onClick={toggleWebm}>
                    {recording === "webm" ? tr("stopRecording") : tr("recordWebm")}
                  </button>
                  <button type="button" disabled={recording === "webm"} onClick={toggleGif} title={tr("gifLimit")}>
                    {recording === "gif" ? tr("stopRecording") : tr("recordGif")}
                  </button>
                </div>
                <div class="group" role="group" aria-label={tr("groupView")}>
                  <span aria-hidden="true">{tr("groupView")}</span>
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
                  <label title={tr("soundNote")}>
                    <input type="checkbox" checked={sound} onChange={(e) => void toggleSound((e.currentTarget as HTMLInputElement).checked)} />{" "}
                    {tr("sound")}
                  </label>
                </div>
              </div>
            )}
            {recording && (
              <p class="hint rec" role="status">
                {tr("recording")}
              </p>
            )}
            {hasFirmware && <p class="hint">{tr("keyboard")}</p>}
            {hasFirmware && <p class="hint">{tr("keysNote")}</p>}
            {provenance && <p class="hint">{noticeText(provenance)}</p>}
            {installed && <p class="hint">{noticeText(installed)}</p>}
              </div>
            ) : (
              <div class="start">
                <Picker ready={ready} busy={busy} heading="pickTitle" onUrl={pick} onFile={(f) => void load(f)} />
              </div>
            )}
          </div>

          <div class="side">
            <Tabs tab={tab} setTab={setTab} badge={crashes.length > 0 || !!stop} />
            <div class={tab === "device" ? "tabpanel two" : "tabpanel"} id="tabpanel" role="tabpanel" aria-labelledby={`tab-${tab}`}>
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

              {tab === "cli" && (
                <CliPanel
                  usb={active?.battery.usb ? usbConn : { state: "unplugged" }}
                  open={serialOpen}
                  setOpen={(open) => {
                    setSerialOpen(open);
                    send({ type: "serialOpen", open });
                    noteSerial(tr(open ? "cliPortOpened" : "cliPortClosed"));
                  }}
                  send={(bytes) => send({ type: "serialInput", bytes }, [bytes])}
                />
              )}

              {tab === "update" && (
                <section class="tab-section">
                  {hasFirmware && (
                    <Picker ready={ready} busy={busy} heading="pickOther" onUrl={pick} onFile={(f) => void load(f)} />
                  )}
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
                    {stop?.report && <CrashPanel stop={stop} />}
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
                    <pre class="log scroll" tabIndex={0}>{log}</pre>
                  </section>
                  <section class="tab-section">
                    <h2>{tr("eventsPanel")}</h2>
                    <p class="hint">{tr("moreEventsLater")}</p>
                    {events.length === 0 ? (
                      <p class="hint">{tr("noEvents")}</p>
                    ) : (
                      <ul class="events scroll" tabIndex={0}>
                        {events.map((e) => (
                          <li>
                            {e.text}
                            {e.ir && (
                              <>
                                {" "}
                                <button type="button" class="link" title={tr("saveIrNote")} onClick={() => download(e.ir!.name, e.ir!.text, "text/plain")}>
                                  {tr("saveIr")}
                                </button>
                              </>
                            )}
                          </li>
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
          </div>
        </div>
      </main>
      {dragging && <div class="veil">{tr("dropAnywhere")}</div>}
      <AskDialog question={question} close={() => setQuestion(null)} />
      <SiteFooter />
    </div>
  );
}

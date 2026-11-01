// SPDX-License-Identifier: GPL-3.0-or-later
import { useEffect, useRef, useState } from "preact/hooks";
import type { ButtonName, CrashRecord, FidelityRow, FromWorker, Status, ToWorker } from "./emu/protocol";
import type { Persistence } from "./emu/cardstore";
import { detectLocale, t, type Key } from "./i18n";
import { stripAnsi } from "./ui/ansi";
import { buttonForKey } from "./ui/keys";
import { LCD_H, LCD_W, paint } from "./ui/lcd";

const locale = detectLocale(navigator.languages ?? [navigator.language]);
const tr = (k: Key) => t(locale, k);
const LOG_LIMIT = 64 * 1024;

interface Stop {
  reason: string;
  crashes: CrashRecord[];
  digest: string;
}

export function App() {
  const worker = useRef<Worker | null>(null);
  const canvas = useRef<HTMLCanvasElement>(null);
  const bits = useRef(new Uint8Array(1024));
  const backlight = useRef(0);
  const [ready, setReady] = useState(false);
  const [fidelity, setFidelity] = useState<FidelityRow[]>([]);
  const [firmware, setFirmware] = useState<string | null>(null);
  const [persistence, setPersistence] = useState<Persistence>("none");
  const [cardCapacity, setCardCapacity] = useState(0);
  const [installed, setInstalled] = useState<string | null>(null);
  const [status, setStatus] = useState<Status | null>(null);
  const [leds, setLeds] = useState<[number, number, number, number]>([0, 0, 0, 0]);
  const [log, setLog] = useState("");
  const [stop, setStop] = useState<Stop | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [turbo, setTurbo] = useState(false);
  const [legacySleep, setLegacySleep] = useState(true);
  const [dragging, setDragging] = useState(false);
  const [pressed, setPressed] = useState<Set<ButtonName>>(new Set());

  const send = (m: ToWorker, transfer: Transferable[] = []) => worker.current?.postMessage(m, transfer);

  const redraw = () => {
    const c = canvas.current?.getContext("2d");
    if (!c) return;
    const img = c.createImageData(LCD_W, LCD_H);
    paint(bits.current, backlight.current, img.data);
    c.putImageData(img, 0, 0);
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
          setPersistence(m.persistence);
          setCardCapacity(m.cardCapacityBytes);
          setReady(true);
          break;
        case "loaded":
          setFirmware(m.name);
          setInstalled(
            m.packageInfo
              ? tr("installed").replace("{files}", String(m.packageInfo.files)).replace("{info}", m.packageInfo.info)
              : null,
          );
          setLog("");
          setStop(null);
          setError(null);
          break;
        case "frame":
          bits.current = new Uint8Array(m.bits);
          redraw();
          break;
        case "lights":
          backlight.current = m.rgbw[3];
          setLeds(m.rgbw);
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
          break;
        case "error":
          setError(m.message);
          break;
      }
    };
    redraw();
    // Development only: `?fw=/@fs/<absolute path>` loads a local firmware file through Vite.
    const fw = import.meta.env.DEV ? new URLSearchParams(location.search).get("fw") : null;
    if (fw) {
      const onReady = (ev: MessageEvent<FromWorker>) => {
        if (ev.data.type !== "ready") return;
        w.removeEventListener("message", onReady);
        void fetch(fw)
          .then((r) => r.arrayBuffer())
          .then((bytes) => w.postMessage({ type: "load", name: fw.split("/").pop()!, bytes }, [bytes]));
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
    const target = (e: KeyboardEvent) =>
      e.target instanceof HTMLInputElement || e.target instanceof HTMLButtonElement ? null : buttonForKey(e.key);
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
    send({ type: "load", name: file.name, bytes }, [bytes]);
  };

  const onDrop = (e: DragEvent) => {
    e.preventDefault();
    setDragging(false);
    const f = e.dataTransfer?.files[0];
    if (f) void load(f);
  };

  const button = (b: ButtonName, label: Key, cls: string, text: string) => (
    <button
      type="button"
      class={`key ${cls}${pressed.has(b) ? " held" : ""}`}
      aria-label={tr(label)}
      disabled={!firmware}
      onPointerDown={(e) => {
        (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
        press(b, true);
      }}
      onPointerUp={() => press(b, false)}
      onPointerCancel={() => press(b, false)}
    >
      {text}
    </button>
  );

  const running = status?.running ?? false;

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
      <header>
        <h1>dryflash</h1>
        <p class="tagline">{tr("tagline")}</p>
        <p class="status" role="status">
          {tr("status")}
        </p>
      </header>

      <section class="device" aria-label={tr("device")}>
        <div class="screen">
          <canvas ref={canvas} width={LCD_W} height={LCD_H} aria-label="LCD 128×64" />
          <div class="leds" aria-hidden="true">
            <span style={{ background: `rgb(${leds[0]},${leds[1]},${leds[2]})` }} />
          </div>
        </div>
        <div class="pad">
          {button("up", "btnUp", "up", "▲")}
          {button("left", "btnLeft", "left", "◀")}
          {button("ok", "btnOk", "ok", "OK")}
          {button("right", "btnRight", "right", "▶")}
          {button("down", "btnDown", "down", "▼")}
          {button("back", "btnBack", "back", "↩")}
        </div>
        <p class="hint">{tr("keyboard")}</p>
      </section>

      <section class="firmware">
        <label class="drop">
          <span>{ready ? (firmware ?? tr("drop")) : tr("starting")}</span>
          <input
            type="file"
            accept=".dfu,.tgz"
            disabled={!ready}
            onChange={(e) => {
              const f = (e.currentTarget as HTMLInputElement).files?.[0];
              if (f) void load(f);
            }}
          />
          <span class="choose">{tr("choose")}</span>
        </label>
        <p class="hint">{tr("privacy")}</p>
        {installed && <p class="hint">{installed}</p>}
        <div class="controls">
          <button
            type="button"
            disabled={!firmware}
            onClick={() => send({ type: "control", action: running ? "pause" : "resume" })}
          >
            {running ? tr("pause") : tr("resume")}
          </button>
          <button type="button" disabled={!firmware} onClick={() => send({ type: "control", action: "reset" })}>
            {tr("reset")}
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
        </div>
        {status && (
          <p class="meter">
            {running ? tr("running") : tr("paused")} · {tr("virtualTime")} {(status.virtualMs / 1000).toFixed(1)} s ·{" "}
            {tr("speed")} {status.speed.toFixed(2)}× · {status.mips.toFixed(0)} MIPS
          </p>
        )}
        {!firmware && ready && <p class="hint">{tr("noFirmware")}</p>}
        {error && (
          <p class="error" role="alert">
            {tr("error")}: {error}
          </p>
        )}
        {stop && (
          <div class="error" role="alert">
            <strong>{tr("stopped")}:</strong> {stop.reason}
            {stop.crashes.length > 0 && (
              <ul>
                {stop.crashes.map((c) => (
                  <li>
                    {(c.atMs / 1000).toFixed(3)} s — {c.message}
                  </li>
                ))}
              </ul>
            )}
            <details>
              <summary>{tr("digest")}</summary>
              <pre>{stop.digest}</pre>
            </details>
          </div>
        )}
      </section>

      <section>
        <h2>{tr("card")}</h2>
        <p class="hint">
          {tr("cardUsage")
            .replace("{used}", ((status?.cardUsedBytes ?? 0) / 1e6).toFixed(1))
            .replace("{total}", (cardCapacity / 1e6).toFixed(0))}{" "}
          {tr(`persist_${persistence}` as Key)}
        </p>
        <div class="controls">
          <button
            type="button"
            disabled={!ready}
            onClick={() => {
              if (confirm(tr("eraseConfirm"))) send({ type: "eraseCard" });
            }}
          >
            {tr("eraseCard")}
          </button>
        </div>
      </section>

      <section>
        <h2>{tr("device")}</h2>
        <label>
          <input
            type="checkbox"
            checked={legacySleep}
            onChange={(e) => {
              const on = (e.currentTarget as HTMLInputElement).checked;
              setLegacySleep(on);
              send({ type: "settings", legacySleep: on });
            }}
          />{" "}
          {tr("legacySleep")}
        </label>
        <p class="hint">{tr("legacySleepNote")}</p>
      </section>

      <section>
        <details>
          <summary>{tr("log")}</summary>
          <pre class="log">{log}</pre>
        </details>
      </section>

      <section>
        <h2>{tr("fidelity")}</h2>
        <div class="table">
          <table>
            <thead>
              <tr>
                <th>{tr("component")}</th>
                <th>{tr("level")}</th>
                <th>{tr("note")}</th>
              </tr>
            </thead>
            <tbody>
              {fidelity.map((r) => (
                <tr>
                  <td>{r.name}</td>
                  <td class={`lvl ${r.level}`}>{tr(`lvl_${r.level}` as Key)}</td>
                  <td>{r.note}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </section>

      <footer>
        <p>{tr("positioning")}</p>
        <p>{tr("disclaimer")}</p>
        <a href="https://github.com/vincenzo-esposit0/dryflash-FZ">{tr("source")}</a>
      </footer>
    </main>
  );
}

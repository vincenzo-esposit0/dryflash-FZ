// SPDX-License-Identifier: GPL-3.0-or-later
// The computer's terminal on the device's USB serial port (BRIEF §5: CLI in xterm.js). One terminal
// for the page: it keeps its scrollback while its tab is hidden, as a terminal window would.

import { useEffect, useRef } from "preact/hooks";
import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import { tr } from "../i18n";

let term: Terminal | null = null;
let fit: FitAddon | null = null;
let host: HTMLDivElement | null = null;
let sink: ((bytes: ArrayBuffer) => void) | null = null;
const encoder = new TextEncoder();

function terminal(): Terminal {
  if (term) return term;
  term = new Terminal({
    convertEol: false,
    cursorBlink: true,
    fontFamily: '"JetBrains Mono", ui-monospace, monospace',
    fontSize: 13,
    scrollback: 5000,
    // The firmware draws for a dark terminal (its MOTD is orange on black).
    theme: { background: "#141524", foreground: "#e6e6e6", cursor: "#f57c1f", selectionBackground: "#5d5e66" },
  });
  fit = new FitAddon();
  term.loadAddon(fit);
  term.onData((data) => sink?.(encoder.encode(data).buffer as ArrayBuffer));
  host = document.createElement("div");
  host.className = "terminal-host";
  return term;
}

/** Bytes the device sent: shown even while the terminal's tab is hidden. */
export function writeSerial(bytes: Uint8Array) {
  terminal().write(bytes);
}

/** A line from the page itself (not from the device), dimmed. */
export function noteSerial(text: string) {
  terminal().write(`\r\n\x1b[2m[${text}]\x1b[0m\r\n`);
}

export interface UsbConnection {
  state: "unplugged" | "waiting" | "connected" | "failed";
  product?: string;
  serial?: string;
  detail?: string;
}

export function CliPanel({
  usb,
  open,
  setOpen,
  send,
}: {
  usb: UsbConnection;
  open: boolean;
  setOpen: (open: boolean) => void;
  send: (bytes: ArrayBuffer) => void;
}) {
  const box = useRef<HTMLDivElement>(null);
  sink = open && usb.state === "connected" ? send : null;

  useEffect(() => {
    const t = terminal();
    if (!host || !box.current) return;
    box.current.appendChild(host);
    if (!t.element) t.open(host);
    const resize = new ResizeObserver(() => fit?.fit());
    resize.observe(box.current);
    fit?.fit();
    return () => {
      resize.disconnect();
      host?.remove();
    };
  }, []);

  const status =
    usb.state === "connected"
      ? tr("cliConnected").replace("{product}", usb.product ?? "").replace("{serial}", usb.serial ?? "")
      : usb.state === "failed"
        ? tr("cliFailed").replace("{detail}", usb.detail ?? "")
        : tr(usb.state === "waiting" ? "cliWaiting" : "cliUnplugged");

  return (
    <section class="tab-section cli">
      <h2>{tr("cliTitle")}</h2>
      <p class="hint">{tr("cliIntro")}</p>
      <p class="cli-status" role="status">
        {status}{" "}
        <button type="button" onClick={() => setOpen(!open)} aria-pressed={open}>
          {tr(open ? "cliClose" : "cliOpen")}
        </button>
      </p>
      <p class="hint">{tr("cliPortNote")}</p>
      <div class="terminal-box" ref={box} />
    </section>
  );
}

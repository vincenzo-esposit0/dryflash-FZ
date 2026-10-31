// SPDX-License-Identifier: GPL-3.0-or-later
// Messages between the UI thread and the emulator worker (ARCHITECTURE §8). Structured clone +
// transferable ArrayBuffers only; no SharedArrayBuffer.

export const PROTOCOL_VERSION = 1;

/** Button indices understood by the core (core-wasm `set_button`). */
export const BUTTONS = ["up", "down", "left", "right", "ok", "back"] as const;
export type ButtonName = (typeof BUTTONS)[number];

export interface FidelityRow {
  name: string;
  level: "emulated" | "partial" | "simulated" | "stubbed";
  note: string;
}

export interface CrashRecord {
  atMs: number;
  message: string;
}

export type ToWorker =
  | { type: "load"; name: string; bytes: ArrayBuffer; cpu2Start: number }
  | { type: "button"; button: ButtonName; down: boolean }
  | { type: "control"; action: "pause" | "resume" | "reset" }
  | { type: "turbo"; on: boolean }
  | { type: "settings"; legacySleep: boolean };

export interface Status {
  running: boolean;
  turbo: boolean;
  virtualMs: number;
  /** Virtual seconds per wall second over the last report period. */
  speed: number;
  mips: number;
  resets: number;
}

export type FromWorker =
  | { type: "ready"; version: string; protocol: number; fidelity: FidelityRow[] }
  | { type: "loaded"; name: string }
  | { type: "error"; message: string }
  | { type: "frame"; bits: ArrayBuffer }
  | { type: "lights"; rgbw: [number, number, number, number] }
  | { type: "log"; bytes: ArrayBuffer }
  | { type: "status"; status: Status }
  | { type: "stopped"; reason: string; crashes: CrashRecord[]; digest: string };

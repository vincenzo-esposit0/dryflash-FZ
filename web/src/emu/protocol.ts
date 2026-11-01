// SPDX-License-Identifier: GPL-3.0-or-later
// Messages between the UI thread and the emulator worker (ARCHITECTURE §8). Structured clone +
// transferable ArrayBuffers only; no SharedArrayBuffer.

import type { Persistence } from "./cardstore";

export const PROTOCOL_VERSION = 2;

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
  | { type: "load"; name: string; bytes: ArrayBuffer }
  | { type: "eraseCard" }
  | { type: "flush" }
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
  cardUsedBytes: number;
}

export type FromWorker =
  | {
      type: "ready";
      version: string;
      protocol: number;
      fidelity: FidelityRow[];
      persistence: Persistence;
      cardCapacityBytes: number;
    }
  /** `packageInfo` for an update package: its version and the files put on the card. */
  | { type: "loaded"; name: string; packageInfo?: { info: string; files: number } }
  | { type: "error"; message: string }
  | { type: "frame"; bits: ArrayBuffer }
  | { type: "lights"; rgbw: [number, number, number, number] }
  | { type: "log"; bytes: ArrayBuffer }
  | { type: "status"; status: Status }
  | { type: "stopped"; reason: string; crashes: CrashRecord[]; digest: string };

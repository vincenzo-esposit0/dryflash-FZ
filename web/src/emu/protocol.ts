// SPDX-License-Identifier: GPL-3.0-or-later
// Messages between the UI thread and the emulator worker (ARCHITECTURE §8). Structured clone +
// transferable ArrayBuffers only; no SharedArrayBuffer.

import type { Persistence } from "./cardstore";
import type { BatterySettings, BoardSettings, DeviceMeta } from "./devices";

export const PROTOCOL_VERSION = 3;

/** Button indices understood by the core (core-wasm `set_button`). */
export const BUTTONS = ["up", "down", "left", "right", "ok", "back"] as const;
export type ButtonName = (typeof BUTTONS)[number];

export interface FidelityRow {
  name: string;
  level: "emulated" | "partial" | "simulated" | "stubbed";
  note: string;
}

export interface CardEntry {
  name: string;
  dir: boolean;
  size: number;
}

export interface CrashRecord {
  atMs: number;
  message: string;
}

export type ToWorker =
  /** `ota`: hand an update package to the running firmware's updater instead of a quick install. */
  | { type: "load"; name: string; bytes: ArrayBuffer; ota?: boolean }
  | { type: "eraseCard" }
  | { type: "flush" }
  | { type: "button"; button: ButtonName; down: boolean }
  | { type: "control"; action: "pause" | "resume" | "reset" }
  | { type: "turbo"; on: boolean }
  /** Changes to the active device; board and sleep settings apply at the next power cycle. */
  | { type: "settings"; legacySleep?: boolean; board?: BoardSettings; battery?: BatterySettings }
  | { type: "selectDevice"; id: string }
  | { type: "createDevice"; label: string }
  | { type: "renameDevice"; label: string }
  | { type: "deleteDevice" }
  | { type: "factoryReset" }
  | { type: "exportDevice" }
  /** Card files. Reading works while running; changes restart the device so that it remounts. */
  | { type: "sdList"; path: string }
  | { type: "sdRead"; path: string }
  | { type: "sdWrite"; dir: string; files: { name: string; bytes: ArrayBuffer }[] }
  | { type: "sdMkdir"; path: string }
  | { type: "sdRemove"; path: string; dir: string }
  | { type: "importDevice"; bytes: ArrayBuffer };

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
    }
  /** The devices and the active one, after any change. `running`: a firmware is installed on it. */
  | {
      type: "devices";
      devices: DeviceMeta[];
      active: DeviceMeta;
      persistence: Persistence;
      hasFirmware: boolean;
      cardCapacityBytes: number;
      cardUsedBytes: number;
    }
  | { type: "exported"; name: string; bytes: ArrayBuffer }
  | { type: "sdListing"; path: string; entries: CardEntry[] }
  | { type: "sdFile"; name: string; bytes: ArrayBuffer }
  /** Firmware crashes recorded since power-on (decoded from Furi's own record). */
  | { type: "crashes"; crashes: CrashRecord[] }
  /** `packageInfo` for an update package: its version and the files put on the card. */
  | { type: "loaded"; name: string; packageInfo?: { info: string; files: number } }
  | { type: "otaStarted"; name: string; info: string }
  | { type: "error"; message: string }
  | { type: "frame"; bits: ArrayBuffer }
  | { type: "lights"; rgbw: [number, number, number, number] }
  | { type: "log"; bytes: ArrayBuffer }
  | { type: "status"; status: Status }
  | { type: "stopped"; reason: string; crashes: CrashRecord[]; digest: string };

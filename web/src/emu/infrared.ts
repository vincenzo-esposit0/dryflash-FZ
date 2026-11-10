// SPDX-License-Identifier: GPL-3.0-or-later
// IR transmissions from the core (core-wasm `take_ir`, core/src/board/infrared.rs).

/** What the IR LED sent: carrier (0 Hz for a constant level) and mark, space, …, mark in µs. */
export type IrSignal = { atMs: number; hz: number; duty: number; timingsUs: number[] };

/** Per transmission: virtual ms, Hz, duty, count, then the timings. */
export function decodeIr(flat: ArrayLike<number>): IrSignal[] {
  const out: IrSignal[] = [];
  let i = 0;
  while (i + 3 < flat.length) {
    const n = flat[i + 3];
    const timingsUs = Array.from({ length: n }, (_, k) => flat[i + 4 + k]);
    out.push({ atMs: flat[i], hz: flat[i + 1], duty: flat[i + 2], timingsUs });
    i += 4 + n;
  }
  return out;
}

/** Total length in ms. */
export function durationMs(s: IrSignal): number {
  return s.timingsUs.reduce((a, b) => a + b, 0) / 1000;
}

/**
 * The signal as a raw entry of a Flipper `.ir` file (the format the firmware reads in
 * applications/main/infrared, "Filetype: IR signals file"), or `null` without a carrier.
 */
export function irFile(s: IrSignal, name: string): string | null {
  if (s.hz <= 0) return null;
  return [
    "Filetype: IR signals file",
    "Version: 1",
    "#",
    `name: ${name}`,
    "type: raw",
    `frequency: ${Math.round(s.hz)}`,
    `duty_cycle: ${s.duty.toFixed(6)}`,
    `data: ${s.timingsUs.join(" ")}`,
    "",
  ].join("\n");
}

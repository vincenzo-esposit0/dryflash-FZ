// SPDX-License-Identifier: GPL-3.0-or-later
// Speaker and vibration motor changes from the core (core-wasm `take_senses`, core/src/board/senses.rs).

/** A change at a virtual time: the speaker's tone (`null` = silent), or the motor on or off. */
export type SenseChange =
  | { atMs: number; tone: { hz: number; duty: number } | null }
  | { atMs: number; vibro: boolean };

/** Four numbers per change: virtual ms, kind (0 silent, 1 tone, 2 motor off, 3 motor on), Hz, duty. */
export function decodeSenses(flat: ArrayLike<number>): SenseChange[] {
  const out: SenseChange[] = [];
  for (let i = 0; i + 3 < flat.length; i += 4) {
    const [atMs, kind, hz, duty] = [flat[i], flat[i + 1], flat[i + 2], flat[i + 3]];
    if (kind === 0) out.push({ atMs, tone: null });
    else if (kind === 1) out.push({ atMs, tone: { hz, duty } });
    else out.push({ atMs, vibro: kind === 3 });
  }
  return out;
}

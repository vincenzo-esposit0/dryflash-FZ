// SPDX-License-Identifier: GPL-3.0-or-later
// Pacing of emulation slices: 1× keeps virtual time locked to wall time; Turbo runs as fast as
// the host allows. Pure logic, unit-tested without a worker.

/** Longest wall time one slice may take before yielding to messages (ms). */
export const SLICE_BUDGET_MS = 12;
/** Largest single run() step in virtual ms (keeps input latency low). */
export const STEP_MS = 5;
/** If 1× falls further behind than this, the clock is re-anchored instead of catching up. */
export const MAX_LAG_MS = 250;

export class Pacer {
  private anchorWall = 0;
  private anchorVirtual = 0;

  /** Restart pacing from the current point (resume, reset, leaving Turbo). */
  anchor(wallMs: number, virtualMs: number): void {
    this.anchorWall = wallMs;
    this.anchorVirtual = virtualMs;
  }

  /** Virtual ms still to run at 1× to catch up with the wall clock. */
  behind(wallMs: number, virtualMs: number): number {
    let lag = this.anchorVirtual + (wallMs - this.anchorWall) - virtualMs;
    if (lag > MAX_LAG_MS) {
      // The host cannot keep up (or the tab was throttled): drop the backlog, never fast-forward.
      this.anchor(wallMs - MAX_LAG_MS, virtualMs);
      lag = MAX_LAG_MS;
    }
    return Math.max(0, lag);
  }
}

/**
 * Shortest press the firmware is shown, in virtual ms. The input service debounces over
 * INPUT_DEBOUNCE_TICKS = 4 ticks of 1 ms (targets/f7/furi_hal/furi_hal_resources.h), so a tap
 * whose press and release arrive between two slices would otherwise never be seen.
 */
export const MIN_PRESS_MS = 40;

/** Holds releases back until each button has been down for MIN_PRESS_MS of virtual time. */
export class PressTimer {
  private downAt = new Map<number, number>();
  private pendingRelease = new Map<number, number>();

  press(button: number, virtualMs: number): void {
    this.downAt.set(button, virtualMs);
    this.pendingRelease.delete(button);
  }

  /** True if the release can be applied now; otherwise it is queued. */
  release(button: number, virtualMs: number): boolean {
    const t = this.downAt.get(button);
    if (t !== undefined && virtualMs - t < MIN_PRESS_MS) {
      this.pendingRelease.set(button, t + MIN_PRESS_MS);
      return false;
    }
    this.downAt.delete(button);
    return true;
  }

  /** Releases that are due at `virtualMs`. */
  due(virtualMs: number): number[] {
    const out: number[] = [];
    for (const [b, at] of this.pendingRelease) {
      if (virtualMs >= at) {
        out.push(b);
        this.pendingRelease.delete(b);
        this.downAt.delete(b);
      }
    }
    return out;
  }

  /** Virtual ms until the next queued release, or Infinity. */
  next(virtualMs: number): number {
    let n = Infinity;
    for (const at of this.pendingRelease.values()) n = Math.min(n, at - virtualMs);
    return Math.max(0, n);
  }

  clear(): void {
    this.downAt.clear();
    this.pendingRelease.clear();
  }
}

// SPDX-License-Identifier: GPL-3.0-or-later
// The speaker through WebAudio (BRIEF §5.4: muted by default). The core gives the tone TIM16 puts
// out, as a frequency and a duty cycle at a virtual time; it is played as a square wave whose loudness
// follows the duty cycle, not as the exact PWM signal.
import type { SenseChange } from "../emu/senses";

/** How far ahead of the audio clock changes are placed: the worker reports them a slice late. */
export const LATENCY_MS = 120;
/** Beyond this the two clocks are lined up again (after a pause, a stall or a burst). */
const DRIFT_MS = 4 * LATENCY_MS;
/** Loudest output: a square wave is harsh, and the real speaker is small. */
const MASTER = 0.15;

/** Lines virtual time up with the audio clock. */
export class Timeline {
  private offset: number | null = null;

  /** Audio time (ms) for a change at virtual `atMs`, with the audio clock at `nowMs`. */
  at(atMs: number, nowMs: number): number {
    const t = this.offset === null ? NaN : atMs + this.offset;
    if (t >= nowMs && t <= nowMs + DRIFT_MS) return t;
    this.offset = nowMs + LATENCY_MS - atMs;
    return nowMs + LATENCY_MS;
  }

  reset() {
    this.offset = null;
  }
}

/**
 * Loudness of a pulse wave with this duty cycle, relative to a square wave: the amplitude of its
 * fundamental is proportional to sin(π·duty).
 */
export function loudness(duty: number): number {
  return Math.max(0, Math.sin(Math.PI * Math.min(Math.max(duty, 0), 1)));
}

export class Speaker {
  private ctx: AudioContext | null = null;
  private osc: OscillatorNode | null = null;
  private gain: GainNode | null = null;
  private readonly timeline = new Timeline();

  /** Turns sound on. Call it from a user gesture: browsers only start audio after one. */
  async enable(): Promise<boolean> {
    const Ctor = window.AudioContext ?? (window as unknown as { webkitAudioContext?: typeof AudioContext }).webkitAudioContext;
    if (!Ctor) return false;
    if (!this.ctx) {
      const ctx = new Ctor();
      const gain = ctx.createGain();
      gain.gain.value = 0;
      const osc = ctx.createOscillator();
      osc.type = "square";
      osc.connect(gain).connect(ctx.destination);
      osc.start();
      [this.ctx, this.osc, this.gain] = [ctx, osc, gain];
    }
    await this.ctx.resume();
    return this.ctx.state === "running";
  }

  /** Turns sound off; the audio device is released until it is turned on again. */
  async disable() {
    this.silence();
    await this.ctx?.suspend();
  }

  /** Plays the speaker's changes. Does nothing while sound is off. */
  play(changes: SenseChange[]) {
    const { ctx, osc, gain } = this;
    if (!ctx || !osc || !gain || ctx.state !== "running") return;
    const now = ctx.currentTime * 1000;
    for (const c of changes) {
      if (!("tone" in c)) continue;
      const t = this.timeline.at(c.atMs, now) / 1000;
      if (c.tone) {
        osc.frequency.setValueAtTime(c.tone.hz, t);
        gain.gain.setValueAtTime(MASTER * loudness(c.tone.duty), t);
      } else gain.gain.setValueAtTime(0, t);
    }
  }

  /** Stops what is playing and anything scheduled (pause, turbo, a new firmware). */
  silence() {
    this.timeline.reset();
    const { ctx, gain } = this;
    if (!ctx || !gain) return;
    gain.gain.cancelScheduledValues(0);
    gain.gain.setValueAtTime(0, ctx.currentTime);
  }
}

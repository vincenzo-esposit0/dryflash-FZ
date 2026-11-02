// SPDX-License-Identifier: GPL-3.0-or-later
// Runs a script against a firmware and says how it went (BRIEF §10).
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { basename, dirname, join, resolve } from "node:path";
import { pngToFrame } from "./png.mjs";
import { ScriptError } from "./script.mjs";
import { frameDistance, Session, Stopped, TimedOut } from "./session.mjs";
import { WalkFailure, walkMenu } from "./walk.mjs";

export const EXIT = { pass: 0, firmware: 1, emulator: 2, usage: 3 };

/** The firmware's own signs that it has reached the Desktop (AnimationManager picks the idle animation). */
const atDesktop = (log) => log.includes("[AnimationManager]") && /Select '/.test(log);
/** With no factory keys (ADR-0012) Desktop opens with this popup, and Back closes it. */
const POPUP_LOG = "Secure Enclave verification failed";
const DESKTOP_TIMEOUT_MS = 60_000;
const LOG_TIMEOUT_MS = 60_000;
const SETTLE_MS = 600;

/** A step's check did not hold. */
class StepFailed extends Error {}

/**
 * @param {{firmware: Uint8Array, name: string, elf?: Uint8Array, steps: object[], timeoutS: number,
 *   screenshots?: string, scriptDir: string, updateGoldens?: boolean, log?: (line: string) => void}} o
 */
export function run(o) {
  const say = o.log ?? (() => {});
  const session = new Session({ firmware: o.firmware, name: o.name, elf: o.elf, timeoutMs: o.timeoutS * 1000 });
  const steps = [];
  const out = { firmware: o.name, exitCode: EXIT.pass, result: "pass", steps, report: null, virtualMs: 0, logTail: "" };
  if (o.screenshots) mkdirSync(o.screenshots, { recursive: true });
  const shot = (name) => {
    if (o.screenshots) writeFileSync(join(o.screenshots, `${basename(name, ".png")}.png`), session.png());
  };
  session.screenshot = shot;

  const record = (name, ms, status, message, detail) => {
    steps.push({ name, ms, status, message, detail });
    const mark = { passed: "ok  ", failed: "FAIL", skipped: "skip" }[status];
    say(`${mark} ${name}${message ? `: ${message}` : ""}`);
  };

  let index = 0;
  let start = 0;
  let entryFailed = false;
  try {
    for (; index < o.steps.length; index++) {
      const step = o.steps[index];
      start = session.virtualMs;
      execute(session, step, o, shot, (name, ok, message) => {
        record(name, session.virtualMs - start, ok ? "passed" : "failed", message);
        entryFailed ||= !ok;
        start = session.virtualMs;
      });
      record(step.name, session.virtualMs - start, "passed");
      session.lastFrame = session.frame();
    }
    if (entryFailed) {
      out.exitCode = EXIT.firmware;
      out.result = "fail";
    }
  } catch (err) {
    const step = o.steps[index];
    const ms = session.virtualMs - start;
    if (err instanceof Stopped) {
      out.report = err.report;
      out.exitCode = err.report?.verdict === "emulator" ? EXIT.emulator : EXIT.firmware;
      out.result = out.exitCode === EXIT.emulator ? "emulator-limitation" : "fail";
      record(step.name, ms, "failed", err.message, err.report?.basis?.join(" "));
      say(`     ${err.report?.verdictLabel ?? "No report"}: ${err.report?.basis?.join(" ") ?? ""}`);
      if (o.screenshots) {
        if (err.png) writeFileSync(join(o.screenshots, "crash.png"), err.png);
        if (err.report) writeFileSync(join(o.screenshots, "crash.json"), JSON.stringify(err.report, null, 2));
        if (err.markdown) writeFileSync(join(o.screenshots, "crash.md"), err.markdown);
      }
    } else if (err instanceof TimedOut || err instanceof StepFailed || err instanceof WalkFailure) {
      out.exitCode = EXIT.firmware;
      out.result = "fail";
      record(step.name, ms, "failed", err.message);
      shot("failure");
    } else {
      session.free();
      throw err;
    }
    for (const rest of o.steps.slice(index + 1)) record(rest.name, 0, "skipped");
  }
  session.drainLog();
  out.virtualMs = session.virtualMs;
  out.logTail = session.log.slice(-4096);
  out.digest = session.digest();
  if (o.screenshots) {
    writeFileSync(join(o.screenshots, "digest.txt"), out.digest);
    if (!out.report) shot("final");
  }
  session.free();
  return out;
}

function execute(s, step, o, shot, onEntry) {
  switch (step.type) {
    case "wait_for_desktop":
      s.waitFor(() => atDesktop(s.log), DESKTOP_TIMEOUT_MS, "the Desktop");
      s.run(SETTLE_MS);
      if (s.log.includes(POPUP_LOG)) {
        s.press("back");
        s.run(SETTLE_MS);
      }
      return;
    case "press":
      for (const b of step.buttons) s.press(b);
      return;
    case "long_press":
      for (const b of step.buttons) s.longPress(b);
      return;
    case "wait":
      s.run(step.ms);
      return;
    case "screenshot":
      shot(step.file);
      return;
    case "expect_no_crash":
      // Anything else would have stopped the run already.
      return;
    case "expect_log":
      if (!s.log.includes(step.text)) throw new StepFailed(`the log does not say "${step.text}"`);
      return;
    case "wait_for_log":
      s.waitFor(() => s.log.includes(step.text), LOG_TIMEOUT_MS, `the log saying "${step.text}"`);
      return;
    case "expect_screen_change":
      // Since the step before it.
      if (s.lastFrame && frameDistance(s.lastFrame, s.frame()) === 0) throw new StepFailed("the screen did not change");
      return;
    case "expect_screen":
      compareScreen(s, step, o);
      return;
    case "walk_menu": {
      const n = walkMenu(s, onEntry);
      if (n < 2) throw new StepFailed(`the menu had only ${n} entry`);
      return;
    }
    default:
      throw new ScriptError(`"${step.type}" is not a step`);
  }
}

function compareScreen(s, step, o) {
  const file = resolve(o.scriptDir, step.file);
  if (o.updateGoldens) {
    mkdirSync(dirname(file), { recursive: true });
    writeFileSync(file, s.png());
    return;
  }
  if (!existsSync(file)) throw new ScriptError(`expect_screen: no golden at ${file}. --update-goldens writes it.`);
  let golden;
  try {
    golden = pngToFrame(readFileSync(file));
  } catch (err) {
    throw new ScriptError(`expect_screen: ${file}: ${err.message}`);
  }
  const distance = frameDistance(golden, s.frame());
  if (distance > step.tolerance) throw new StepFailed(`${distance} pixels differ from ${step.file}; ${step.tolerance} allowed`);
}

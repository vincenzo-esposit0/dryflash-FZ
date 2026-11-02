// SPDX-License-Identifier: GPL-3.0-or-later
// The input script (BRIEF §10): what a YAML file may say, checked before any firmware runs.
import { parse } from "yaml";

const BUTTONS = ["up", "down", "left", "right", "ok", "back"];
/** Steps that take no argument. */
const BARE = ["wait_for_desktop", "expect_no_crash", "expect_screen_change", "walk_menu"];

/** The script is wrong, not the firmware. */
export class ScriptError extends Error {}

function duration(v, step) {
  const m = /^(\d+(?:\.\d+)?)\s*(ms|s)$/.exec(String(v).trim());
  if (!m) throw new ScriptError(`${step}: "${v}" is not a time such as 2s or 500ms`);
  return Number(m[1]) * (m[2] === "s" ? 1000 : 1);
}

function buttons(v, step) {
  const list = Array.isArray(v) ? v : [v];
  for (const b of list) {
    if (!BUTTONS.includes(b)) throw new ScriptError(`${step}: "${b}" is not a button (${BUTTONS.join(", ")})`);
  }
  return list;
}

function text(v, step) {
  if (typeof v !== "string" || !v) throw new ScriptError(`${step}: needs some text`);
  return v;
}

/** A tolerance in pixels, or a share of the screen: `8` or `1%`. */
function tolerance(v) {
  if (v === undefined) return 0;
  const m = /^(\d+(?:\.\d+)?)(%?)$/.exec(String(v).trim());
  if (!m) throw new ScriptError(`expect_screen: tolerance "${v}" is not a number of pixels or a percentage`);
  return m[2] ? Math.round((Number(m[1]) / 100) * 128 * 64) : Number(m[1]);
}

function step(raw, index) {
  if (typeof raw === "string") {
    if (!BARE.includes(raw)) throw new ScriptError(`step ${index + 1}: "${raw}" is not a step`);
    return { type: raw, name: raw };
  }
  const keys = raw && typeof raw === "object" ? Object.keys(raw) : [];
  if (keys.length !== 1) throw new ScriptError(`step ${index + 1}: each step is one name, or one name and its value`);
  const [type] = keys;
  const v = raw[type];
  const name = `${type}: ${typeof v === "object" ? JSON.stringify(v) : v}`;
  switch (type) {
    case "press":
      return { type, name, buttons: buttons(v, type) };
    case "long_press":
      return { type, name, buttons: buttons(v, type) };
    case "wait":
      return { type, name, ms: duration(v, type) };
    case "screenshot":
      if (!/^[\w.-]+$/.test(String(v))) throw new ScriptError(`screenshot: "${v}" is not a plain file name`);
      return { type, name, file: String(v) };
    case "expect_log":
    case "wait_for_log":
      return { type, name, text: text(v, type) };
    case "expect_screen":
      if (typeof v === "string") return { type, name, file: v, tolerance: 0 };
      if (v && typeof v.file === "string") return { type, name, file: v.file, tolerance: tolerance(v.tolerance) };
      throw new ScriptError("expect_screen: needs a file, or a file and a tolerance");
    default:
      throw new ScriptError(`step ${index + 1}: "${type}" is not a step`);
  }
}

export function parseScript(source) {
  let doc;
  try {
    doc = parse(source);
  } catch (err) {
    throw new ScriptError(`not valid YAML: ${err.message}`);
  }
  const steps = Array.isArray(doc) ? doc : doc?.steps;
  if (!Array.isArray(steps) || !steps.length) throw new ScriptError('a script is a list of steps, or has them under "steps"');
  return steps.map(step);
}

/** The default smoke test (BRIEF §10): boot, Desktop, every entry of the main menu and back, no crash or hang. */
export const SMOKE = parseScript(`
- wait_for_desktop
- screenshot: desktop
- walk_menu
- expect_no_crash
`);

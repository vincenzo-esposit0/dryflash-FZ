// SPDX-License-Identifier: GPL-3.0-or-later
// Opens every entry of the main menu and backs out. Firmwares draw that menu very differently (a list,
// a grid, a carousel) and some go back to the Desktop from an app, so this looks at the screen instead of
// assuming a shape: it moves until it sees a screen it has seen before.
import { frameDistance } from "./session.mjs";

/** The menu's own animated icons move about 75 pixels at rest; a different screen is thousands. */
const SAME_SCREEN = 300;
const MAX_ENTRIES = 24;
const OPEN_MS = 4000;
const MAX_BACKS = 8;

const same = (a, b) => frameDistance(a, b) < SAME_SCREEN;

export class WalkFailure extends Error {}

/**
 * @param {import("./session.mjs").Session} s standing on the Desktop
 * @param {(name: string, ok: boolean, message?: string) => void} onEntry
 * @returns {number} entries opened
 */
export function walkMenu(s, onEntry) {
  const desktop = s.frame();
  s.press("ok");
  s.run(300);
  if (same(s.frame(), desktop)) throw new WalkFailure("OK on the Desktop did not open the menu");
  const seen = [];
  const path = [];

  for (let entry = 1; entry <= MAX_ENTRIES; entry++) {
    const here = s.frame();
    seen.push(here);
    const name = `menu entry ${entry}`;

    s.press("ok");
    const opened = waitForChange(s, here);
    s.screenshot?.(`menu-${String(entry).padStart(2, "0")}`);
    if (!opened) {
      onEntry(name, false, `OK changed nothing within ${OPEN_MS / 1000} s`);
    } else if (!backOut(s, here, desktop)) {
      onEntry(name, false, `Back did not return to the menu in ${MAX_BACKS} presses, or a long press`);
      return entry;
    } else {
      onEntry(name, true);
    }
    if (same(s.frame(), desktop) && !reenter(s, here, path)) {
      onEntry(name, false, "could not find this entry again after the app returned to the Desktop");
      return entry;
    }

    const next = nextEntry(s, here, seen, path);
    if (!next) return entry;
  }
  return MAX_ENTRIES;
}

function waitForChange(s, from) {
  for (let t = 0; t < OPEN_MS; t += 100) {
    if (!same(s.frame(), from)) {
      // Whatever it opened gets a moment to finish drawing.
      s.run(600);
      return true;
    }
    s.run(100);
  }
  return false;
}

function backOut(s, menu, desktop) {
  const home = () => same(s.frame(), menu) || same(s.frame(), desktop);
  for (let i = 0; i < MAX_BACKS; i++) {
    if (home()) return true;
    s.press("back");
    s.run(300);
  }
  if (home()) return true;
  s.longPress("back");
  s.run(600);
  return home();
}

/** Back on the Desktop: opens the menu again, and walks to where it was unless the firmware remembers. */
function reenter(s, target, path) {
  s.press("ok");
  s.run(300);
  if (same(s.frame(), target)) return true;
  for (const key of path) {
    s.press(key);
    s.run(100);
    if (same(s.frame(), target)) return true;
  }
  return same(s.frame(), target);
}

/** Moves to an entry not seen yet: down first, then right, for menus that run sideways. */
function nextEntry(s, from, seen, path) {
  for (const key of ["down", "right"]) {
    s.press(key);
    s.run(100);
    if (!seen.some((f) => same(f, s.frame()))) {
      path.push(key);
      return true;
    }
    // Nothing new: stand where it was before trying the other direction.
    if (!same(s.frame(), from)) {
      s.press(key === "down" ? "up" : "left");
      s.run(100);
      if (!same(s.frame(), from)) return false;
    }
  }
  return false;
}

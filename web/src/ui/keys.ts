// SPDX-License-Identifier: GPL-3.0-or-later
import type { ButtonName } from "../emu/protocol";

/** Keyboard layout: arrows, Enter/Space = OK, Backspace/Escape = Back. */
export function buttonForKey(key: string): ButtonName | null {
  switch (key) {
    case "ArrowUp":
      return "up";
    case "ArrowDown":
      return "down";
    case "ArrowLeft":
      return "left";
    case "ArrowRight":
      return "right";
    case "Enter":
    case " ":
      return "ok";
    case "Backspace":
    case "Escape":
      return "back";
    default:
      return null;
  }
}

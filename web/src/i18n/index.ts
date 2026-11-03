// SPDX-License-Identifier: GPL-3.0-or-later
import en from "./en.json";
import it from "./it.json";

export type Key = keyof typeof en;
export type Locale = "en" | "it";

const tables: Record<Locale, Record<Key, string>> = { en, it };

/** Picks the first supported language from the browser preferences; English otherwise. */
export function detectLocale(languages: readonly string[]): Locale {
  for (const l of languages) {
    const base = l.toLowerCase().split("-")[0];
    if (base === "it" || base === "en") return base;
  }
  return "en";
}

export function t(locale: Locale, key: Key): string {
  return tables[locale][key] ?? tables.en[key];
}

const STORAGE_KEY = "dryflash:lang";

/** What the visitor chose wins; otherwise the browser's preferences. */
export function pickLocale(saved: string | null, languages: readonly string[]): Locale {
  return saved === "it" || saved === "en" ? saved : detectLocale(languages);
}

function saved(): string | null {
  try {
    return localStorage.getItem(STORAGE_KEY);
  } catch {
    return null;
  }
}

let current: Locale | null = null;
let onChange = () => {};

export function getLocale(): Locale {
  current ??= pickLocale(saved(), navigator.languages ?? [navigator.language]);
  return current;
}

/** The text in the language in use. */
export const tr = (key: Key): string => t(getLocale(), key);

/** Called once, by the page's entry point, to draw it again with the new language. */
export function whenLocaleChanges(draw: () => void) {
  onChange = draw;
}

export function setLocale(locale: Locale) {
  current = locale;
  document.documentElement.lang = locale;
  try {
    localStorage.setItem(STORAGE_KEY, locale);
  } catch {
    // Not remembered: it still holds until the page goes away.
  }
  onChange();
}

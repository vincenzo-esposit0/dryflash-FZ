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

// SPDX-License-Identifier: GPL-3.0-or-later
import { detectLocale, t, type Key } from "../i18n";

const locale = detectLocale(navigator.languages ?? [navigator.language]);
const tr = (k: Key) => t(locale, k);

export function SiteHeader() {
  return (
    <header>
      <nav class="nav" aria-label="dryflash">
        <a href="/">{tr("navEmulator")}</a>
        <a href="/compat">{tr("navCompat")}</a>
        <a href="/about">{tr("navAbout")}</a>
      </nav>
      <h1>dryflash</h1>
      <p class="tagline">{tr("tagline")}</p>
      <p class="status" role="status">
        {tr("status")}
      </p>
    </header>
  );
}

export function SiteFooter() {
  return (
    <footer>
      <p>{tr("positioning")}</p>
      <p>{tr("disclaimer")}</p>
      <a href="https://github.com/vincenzo-esposit0/dryflash-FZ">{tr("source")}</a>
    </footer>
  );
}

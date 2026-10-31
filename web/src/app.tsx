// SPDX-License-Identifier: GPL-3.0-or-later
import { detectLocale, t } from "./i18n";

const locale = detectLocale(navigator.languages ?? [navigator.language]);

export function App() {
  return (
    <main>
      <h1>dryflash</h1>
      <p class="tagline">{t(locale, "tagline")}</p>
      <p class="status" role="status">{t(locale, "status")}</p>
      <p>{t(locale, "positioning")}</p>
      <footer>
        <p>{t(locale, "disclaimer")}</p>
        <a href="https://github.com/vincenzo-esposit0/dryflash-FZ">{t(locale, "source")}</a>
      </footer>
    </main>
  );
}

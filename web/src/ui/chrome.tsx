// SPDX-License-Identifier: GPL-3.0-or-later
import { version } from "../../package.json";
import { getLocale, setLocale, tr, type Key } from "../i18n";

const HUB = "https://vincenzo-esposit0.github.io/flipper-zero-firmware-hub/";
const SOURCE = "https://github.com/vincenzo-esposit0/dryflash-FZ";

export function SiteHeader() {
  const here = location.pathname.replace(/\/+$/, "") || "/";
  const link = (href: string, label: Key) => (
    <a href={href} class="chip" aria-current={here === href ? "page" : undefined}>
      {tr(label)}
    </a>
  );
  return (
    <header class="site-header">
      <a class="skip" href="#main">
        {tr("skip")}
      </a>
      <div class="bar">
        <h1 class="brand">
          <a href="/">
            <span class="mark" aria-hidden="true">
              DF
            </span>
            <span>dry</span>
            <span class="neon">flash</span>
          </a>
        </h1>
        <nav class="nav" aria-label="dryflash">
          {link("/", "navEmulator")}
          {link("/compat", "navCompat")}
          {link("/about", "navAbout")}
          <a href={HUB} class="chip" rel="noopener">
            {tr("navHub")} <span aria-hidden="true">↗</span>
          </a>
        </nav>
        <span class="stage-chip" title={tr("versionTitle")}>v{version}</span>
        <button type="button" class="lang" aria-label={tr("switchLanguage")} onClick={() => setLocale(getLocale() === "it" ? "en" : "it")}>
          <span class={getLocale() === "it" ? "on" : ""}>IT</span>
          <span>/</span>
          <span class={getLocale() === "en" ? "on" : ""}>EN</span>
        </button>
      </div>
    </header>
  );
}

/** A terminal line, as on the Firmware Hub. `command` states something true of the page: it is not a console. */
export function Prompt({ command }: { command: string }) {
  return (
    <p class="prompt" aria-hidden="true">
      <span class="neon">visitor@dryflash</span>:<span class="cyan">~</span>$ {command}
      <span class="cursor" />
    </p>
  );
}

export function SiteFooter() {
  return (
    <footer class="site-footer">
      <div class="footer-body">
        <p>{tr("positioning")}</p>
        <p>{tr("status")}</p>
        <p>{tr("disclaimer")}</p>
        <a href={SOURCE}>{tr("source")}</a>
      </div>
    </footer>
  );
}

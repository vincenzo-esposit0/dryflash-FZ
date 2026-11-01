// SPDX-License-Identifier: GPL-3.0-or-later
import { useEffect, useState } from "preact/hooks";
import compat from "../../firmware/compat.json";
import results from "../../firmware/compat-results.json";
import type { FidelityRow } from "./emu/protocol";
import { detectLocale, t, type Key } from "./i18n";
import { SiteFooter, SiteHeader } from "./ui/chrome";

const locale = detectLocale(navigator.languages ?? [navigator.language]);
const tr = (k: Key) => t(locale, k);

export function Fidelity({ rows }: { rows: FidelityRow[] }) {
  return (
    <div class="table">
      <table>
        <thead>
          <tr>
            <th>{tr("component")}</th>
            <th>{tr("level")}</th>
            <th>{tr("note")}</th>
          </tr>
        </thead>
        <tbody>
          {rows.map((r) => (
            <tr>
              <td>{r.name}</td>
              <td class={`lvl ${r.level}`}>{tr(`lvl_${r.level}` as Key)}</td>
              <td>{r.note}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function Mark({ value }: { value: boolean | null | undefined }) {
  return value ? <td class="lvl emulated">{tr("yes")}</td> : <td class="muted">{tr("notTested")}</td>;
}

export function CompatPage() {
  const table = results.results as Record<string, { boots: boolean; desktop: boolean; apps: boolean | null; ota: boolean | null }>;
  return (
    <main class="page">
      <SiteHeader />
      <h2>{tr("compatTitle")}</h2>
      <p>{tr("compatIntro").replace("{date}", results.checked)}</p>
      <div class="table">
        <table>
          <thead>
            <tr>
              <th>{tr("firmware")}</th>
              <th>{tr("version")}</th>
              <th>{tr("boots")}</th>
              <th>{tr("reachesDesktop")}</th>
              <th>{tr("appsLaunch")}</th>
              <th>{tr("otaInstalls")}</th>
              <th>{tr("licence")}</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {compat.firmwares.map((f) => {
              const r = table[f.id];
              return (
                <tr>
                  <td>
                    <a href={f.source}>{f.name}</a>
                  </td>
                  <td>{f.version}</td>
                  <Mark value={r?.boots} />
                  <Mark value={r?.desktop} />
                  <Mark value={r?.apps} />
                  <Mark value={r?.ota} />
                  <td>{f.licence}</td>
                  <td>
                    <a href={`/?fw=${encodeURIComponent(f.update_tgz.url)}`}>{tr("tryIt")}</a>
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
      <SiteFooter />
    </main>
  );
}

export function AboutPage() {
  const [rows, setRows] = useState<FidelityRow[]>([]);
  useEffect(() => {
    // The table comes from the core, not from a copy here.
    void import("./wasm/dryflash.js").then(async (w) => {
      await w.default();
      setRows(JSON.parse(w.fidelity_table()));
    });
  }, []);
  return (
    <main class="page">
      <SiteHeader />
      <h2>{tr("aboutTitle")}</h2>
      <h3>{tr("aboutHow")}</h3>
      <p>{tr("aboutHowText")}</p>
      <h3>{tr("aboutLimits")}</h3>
      <p>{tr("aboutLimitsText")}</p>
      <h3>{tr("fidelity")}</h3>
      <Fidelity rows={rows} />
      <h3>{tr("aboutCredits")}</h3>
      <p>
        {tr("aboutCreditsText")}{" "}
        <a href="https://github.com/vincenzo-esposit0/dryflash-FZ/blob/main/CREDITS.md">CREDITS.md</a>
      </p>
      <SiteFooter />
    </main>
  );
}

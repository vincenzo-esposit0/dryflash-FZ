// SPDX-License-Identifier: GPL-3.0-or-later
import { useEffect, useState } from "preact/hooks";
import compat from "../../firmware/compat.json";
import results from "../../firmware/compat-results.json";
import type { FidelityRow } from "./emu/protocol";
import { tr, type Key } from "./i18n";
import { Prompt, SiteFooter, SiteHeader } from "./ui/chrome";


export function Fidelity({ rows }: { rows: FidelityRow[] }) {
  return (
    <>
    {tr("coreEnglish") && <p class="hint">{tr("coreEnglish")}</p>}
    <div class="table scroll" tabIndex={0}>
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
    </>
  );
}

function Mark({ value, note }: { value: boolean | null | undefined; note?: string }) {
  if (value) return <td class="lvl emulated">{note ? `${tr("yes")} (${note})` : tr("yes")}</td>;
  return value === false ? <td class="lvl stubbed">{tr("no")}</td> : <td class="muted">{tr("notTested")}</td>;
}

interface CompatResult {
  boots: boolean | null;
  desktop: boolean | null;
  apps: boolean | null;
  entries: number;
  ota: boolean | null;
  problem: string | null;
}

/** One firmware's row, from what the nightly run saw. */
function CompatRow({ name, source, version, licence, url, r }: { name: string; source: string; version: string; licence: string; url: string | null; r?: CompatResult }) {
  return (
    <tr>
      <td>
        <a href={source}>{name}</a>
      </td>
      <td>{version}</td>
      <Mark value={r?.boots} />
      <Mark value={r?.desktop} />
      <Mark value={r?.apps} note={r?.entries ? String(r.entries) : undefined} />
      <Mark value={r?.ota} />
      <td>{licence}</td>
      <td>{url && <a href={`/?fw=${encodeURIComponent(url)}`}>{tr("tryIt")}</a>}</td>
    </tr>
  );
}

export function CompatPage() {
  const pinned = results.results as Record<string, CompatResult>;
  const latest = results.latest as Record<string, CompatResult & { version: string | null; url: string | null }>;
  const problems = compat.firmwares.flatMap((f) => {
    const rows = [
      [`${f.name} ${f.version}`, pinned[f.id]?.problem],
      [`${f.name} ${latest[f.id]?.version ?? tr("latestRelease")}`, latest[f.id]?.problem],
    ] as const;
    return rows.filter(([, problem]) => problem);
  });
  return (
    <div class="site">
      <SiteHeader />
      <main class="page" id="main">
      <Prompt command="cat COMPATIBILITY.md" />
      <h2>{tr("compatTitle")}</h2>
      <p>{tr("compatIntro").replace("{date}", results.checked)}</p>
      <div class="table scroll" tabIndex={0}>
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
            {compat.firmwares.flatMap((f) => {
              const newer = latest[f.id];
              const row = { name: f.name, source: f.source, licence: f.licence };
              const rows = [<CompatRow {...row} version={f.version} url={f.update_tgz.url} r={pinned[f.id]} />];
              if (newer) rows.push(<CompatRow {...row} version={newer.version ?? tr("latestRelease")} url={newer.url} r={newer} />);
              return rows;
            })}
          </tbody>
        </table>
      </div>
      {problems.length > 0 && (
        <ul class="basis">
          {problems.map(([who, problem]) => (
            <li>
              {who}: {problem}
            </li>
          ))}
        </ul>
      )}
      <p class="hint">{tr("compatLimits")}</p>
      </main>
      <SiteFooter />
    </div>
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
    <div class="site">
      <SiteHeader />
      <main class="page" id="main">
      <Prompt command="cat ABOUT.md" />
      <h2>{tr("aboutTitle")}</h2>
      <h3>{tr("aboutStart")}</h3>
      <ol class="steps">
        {(["aboutStart1", "aboutStart2", "aboutStart3", "aboutStart4", "aboutStart5"] as const).map((k) => (
          <li>{tr(k)}</li>
        ))}
      </ol>
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
      </main>
      <SiteFooter />
    </div>
  );
}

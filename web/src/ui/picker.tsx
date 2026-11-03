// SPDX-License-Identifier: GPL-3.0-or-later
import compat from "../../../firmware/compat.json";
import results from "../../../firmware/compat-results.json";
import { tr, type Key } from "../i18n";
import { formatSize } from "./download";

interface Seen {
  boots: boolean | null;
  desktop: boolean | null;
}

/** What the nightly run saw of this pinned firmware, and nothing it did not see (ADR-0017). */
function seen(id: string): string {
  const r = (results.results as Record<string, Seen | undefined>)[id];
  if (r?.boots && r.desktop) return tr("pickTested").replace("{date}", results.checked);
  return r?.boots === false || r?.desktop === false ? tr("pickFailing") : tr("notTested");
}

/** Where a firmware comes from: the pinned list, downloaded from its own host, or a file of the visitor's. */
export function Picker({
  ready,
  busy,
  heading,
  onUrl,
  onFile,
}: {
  ready: boolean;
  busy: boolean;
  heading: Key;
  onUrl: (url: string) => void;
  onFile: (file: File) => void;
}) {
  return (
    <section class="picker" aria-labelledby="picker-title">
      <h2 id="picker-title">{tr(heading)}</h2>
      {heading === "pickTitle" && <p class="lead">{tr("tagline")}</p>}
      <ul class="fw-list">
        {compat.firmwares.map((f) => (
          <li>
            <button type="button" class="fw" disabled={!ready || busy} onClick={() => onUrl(f.update_tgz.url)}>
              <span class="fw-name">{f.name}</span>
              <span class="fw-version">{f.version}</span>
              <span class="fw-size">{formatSize(f.update_tgz.size)}</span>
              <span class="fw-seen">{seen(f.id)}</span>
            </button>
          </li>
        ))}
      </ul>
      <p class="hint">
        {tr("pickFrom")} <a href="/compat">{tr("navCompat")}</a>
      </p>
      <label class="drop">
        <span>{ready ? tr("drop") : tr("starting")}</span>
        <input
          type="file"
          accept=".dfu,.tgz"
          disabled={!ready}
          onChange={(e) => {
            const input = e.currentTarget as HTMLInputElement;
            const f = input.files?.[0];
            input.value = "";
            if (f) onFile(f);
          }}
        />
        <span class="choose">{tr("choose")}</span>
        <span class="hint">{tr("privacy")}</span>
      </label>
    </section>
  );
}

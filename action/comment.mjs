// SPDX-License-Identifier: GPL-3.0-or-later
// The text for the pull request and the job summary. Crash messages and step names come from the firmware and
// the script under test, so they are treated as untrusted: no markup from them, and no mentions.

export const MARKER = "<!-- dryflash-action -->";

/** Inline text from something untrusted. */
export function plain(s) {
  return String(s)
    .replace(/[\r\n]+/g, " ")
    .replace(/[\\`*_~[\]<>|#]/g, "\\$&")
    .replace(/@/g, "@​");
}

const HEADLINE = {
  pass: "✅ Passed",
  fail: "❌ Failed",
  "emulator-limitation": "⚠️ Stopped by an emulator limitation",
};

/** @param {object} r the runner's --report JSON */
export function toMarkdown(r, runUrl) {
  const o = [MARKER, `### dryflash: ${HEADLINE[r.result] ?? plain(r.result)}`, ""];
  o.push(`\`${String(r.firmware).replace(/[`\r\n]/g, "")}\`, after ${(r.virtualMs / 1000).toFixed(1)} s of virtual time.`, "");
  if (r.report) {
    o.push(`**${plain(r.report.summary)}**`, "");
    o.push(`${plain(r.report.verdictLabel)}. This is a judgement, not a proof:`, "");
    for (const b of r.report.basis) o.push(`- ${plain(b)}`);
    o.push("");
    if (r.report.backtrace?.length) {
      o.push("<details><summary>Backtrace</summary>", "", "```");
      for (const f of r.report.backtrace) o.push(`${f.address} ${(f.symbol ?? "?").replace(/`/g, "")} (${f.source})`);
      o.push("```", "</details>", "");
    }
  }
  if (r.screen?.length) o.push("```", ...r.screen, "```", "");
  const failed = r.steps.filter((s) => s.status === "failed");
  const passed = r.steps.filter((s) => s.status === "passed").length;
  o.push(`${passed} of ${r.steps.length} steps passed.`, "");
  if (failed.length) {
    for (const s of failed) o.push(`- ${plain(s.name)}: ${plain(s.message ?? "failed")}`);
    o.push("");
  }
  if (runUrl) o.push(`[Screenshots, JUnit and crash report](${runUrl}) are in the run's artifact.`, "");
  o.push("<sub>A smoke test, not a hardware certification: passing here does not guarantee it works on a device.</sub>", "");
  return o.join("\n");
}

/** Comments on the pull request, replacing this action's earlier comment. Whether it could. */
export async function postComment({ api, repo, pr, token, body, fetchImpl = fetch }) {
  const headers = {
    authorization: `Bearer ${token}`,
    accept: "application/vnd.github+json",
    "content-type": "application/json",
    "user-agent": "dryflash-action",
  };
  const base = `${api}/repos/${repo}/issues`;
  let existing = null;
  for (let page = 1; page <= 10 && !existing; page++) {
    const res = await fetchImpl(`${base}/${pr}/comments?per_page=100&page=${page}`, { headers });
    if (!res.ok) return { ok: false, status: res.status };
    const rows = await res.json();
    existing = rows.find((c) => c.body?.startsWith(MARKER)) ?? null;
    if (rows.length < 100) break;
  }
  const res = await fetchImpl(existing ? `${base}/comments/${existing.id}` : `${base}/${pr}/comments`, {
    method: existing ? "PATCH" : "POST",
    headers,
    body: JSON.stringify({ body }),
  });
  return { ok: res.ok, status: res.status };
}

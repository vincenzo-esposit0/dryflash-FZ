// SPDX-License-Identifier: GPL-3.0-or-later
// The action's one step: runs the runner, then reports. It always ends well itself, so that the artifact is
// uploaded; the composite's last step fails the job with the runner's exit code.
import { spawnSync } from "node:child_process";
import { appendFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { postComment, toMarkdown } from "./comment.mjs";

const env = process.env;
const dir = mkdtempSync(join(env.RUNNER_TEMP ?? tmpdir(), "dryflash-"));
mkdirSync(dir, { recursive: true });
const report = join(dir, "report.json");
const junit = join(dir, "junit.xml");

function output(name, value) {
  if (env.GITHUB_OUTPUT) appendFileSync(env.GITHUB_OUTPUT, `${name}=${value}\n`);
}
output("dir", dir);
output("junit", junit);

const args = ["--yes", `--package=${env.DRYFLASH_PACKAGE}`, "dryflash", "run", env.DRYFLASH_FIRMWARE ?? "", "--screenshots", dir, "--junit", junit, "--report", report];
if (env.DRYFLASH_ELF) args.push("--elf", env.DRYFLASH_ELF);
if (env.DRYFLASH_SCRIPT) args.push("--script", env.DRYFLASH_SCRIPT);
if (env.DRYFLASH_TIMEOUT) args.push("--timeout", env.DRYFLASH_TIMEOUT);
const run = spawnSync("npx", args, { stdio: "inherit", shell: process.platform === "win32" });
const code = run.status ?? 3;
output("exit-code", code);

if (!existsSync(report)) {
  output("result", "error");
  console.log("::error::dryflash did not produce a result: see the log above.");
  process.exit(0);
}
const result = JSON.parse(readFileSync(report, "utf8"));
output("result", result.result);

const runUrl = env.GITHUB_RUN_ID ? `${env.GITHUB_SERVER_URL}/${env.GITHUB_REPOSITORY}/actions/runs/${env.GITHUB_RUN_ID}` : null;
const body = toMarkdown(result, runUrl);
if (env.GITHUB_STEP_SUMMARY) appendFileSync(env.GITHUB_STEP_SUMMARY, `${body}\n`);

const event = env.GITHUB_EVENT_PATH && existsSync(env.GITHUB_EVENT_PATH) ? JSON.parse(readFileSync(env.GITHUB_EVENT_PATH, "utf8")) : {};
const pr = event.pull_request?.number;
if (env.DRYFLASH_COMMENT === "true" && pr) {
  const posted = await postComment({
    api: env.GITHUB_API_URL ?? "https://api.github.com",
    repo: env.GITHUB_REPOSITORY,
    pr,
    token: env.DRYFLASH_TOKEN,
    body,
  }).catch((err) => ({ ok: false, status: String(err) }));
  // Pull requests from forks get a read-only token. The result is still in the job summary.
  if (!posted.ok) console.log(`::warning::Could not comment on the pull request (${posted.status}). It needs permission pull-requests: write; pull requests from forks do not get it.`);
}
process.exit(0);

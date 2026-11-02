// SPDX-License-Identifier: GPL-3.0-or-later
// The GitHub Action, as far as it can run outside GitHub: its real script, against a fake API.
import { spawn } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { MARKER, plain, postComment, toMarkdown } from "../../action/comment.mjs";
import { toBraille } from "../src/braille.mjs";

const ACTION = fileURLToPath(new URL("../../action/", import.meta.url));
const RUNNER = fileURLToPath(new URL("..", import.meta.url));
const dir = mkdtempSync(join(tmpdir(), "dryflash-action-"));

/** A DfuSe file (ST UM0391) that spins from the reset vector. */
function spinning() {
  const image = Buffer.alloc(0x200);
  image.writeUInt32LE(0x20030000, 0);
  image.writeUInt32LE(0x08000101, 4);
  image.writeUInt16LE(0xe7fe, 0x100);
  const u32 = (v) => {
    const b = Buffer.alloc(4);
    b.writeUInt32LE(v);
    return b;
  };
  const name = Buffer.alloc(255);
  const target = Buffer.concat([Buffer.from("Target\0"), u32(1), name, u32(8 + image.length), u32(1), u32(0x08000000), u32(image.length), image]);
  const body = Buffer.concat([Buffer.from("DfuSe\x01"), u32(11 + target.length), Buffer.from([1]), target, Buffer.from([0xff, 0xff, 0x11, 0xdf, 0x83, 0x04, 0x1a, 0x01]), Buffer.from("UFD\x10")]);
  let crc = 0xffffffff;
  for (const byte of body) {
    crc ^= byte;
    for (let i = 0; i < 8; i++) crc = (crc >>> 1) ^ (crc & 1 ? 0xedb88320 : 0);
  }
  const file = join(dir, "spin.dfu");
  writeFileSync(file, Buffer.concat([body, u32(crc >>> 0)]));
  return file;
}

function fakeGithub(status = 200) {
  const calls = [];
  const server = createServer((req, res) => {
    let data = "";
    req.on("data", (d) => (data += d));
    req.on("end", () => {
      calls.push({ method: req.method, url: req.url, auth: req.headers.authorization, body: data ? JSON.parse(data).body : null });
      res.writeHead(status, { "content-type": "application/json" });
      res.end(req.method === "GET" ? "[]" : "{}");
    });
  });
  return new Promise((resolve) => server.listen(0, "127.0.0.1", () => resolve({ server, calls, url: `http://127.0.0.1:${server.address().port}` })));
}

/** Runs the action's script as the composite would, without blocking the fake API in this process. */
function runAction(env) {
  return new Promise((resolve) => {
    const child = spawn(process.execPath, [join(ACTION, "run.mjs")], { env: { ...process.env, ...env }, stdio: "pipe" });
    let out = "";
    child.stdout.on("data", (d) => (out += d));
    child.stderr.on("data", (d) => (out += d));
    child.on("close", (status) => resolve({ status, out }));
  });
}

const outputs = (file) => Object.fromEntries(readFileSync(file, "utf8").trim().split("\n").map((l) => l.split(/=(.*)/s).slice(0, 2)));

describe("the action's script", () => {
  const start = async (name, script, api, extra = {}) => {
    const output = join(dir, `${name}.out`);
    const summary = join(dir, `${name}.md`);
    const event = join(dir, `${name}.json`);
    writeFileSync(output, "");
    writeFileSync(summary, "");
    writeFileSync(event, JSON.stringify({ pull_request: { number: 7 } }));
    const scriptFile = join(dir, `${name}.yaml`);
    writeFileSync(scriptFile, script);
    const r = await runAction({
      GITHUB_OUTPUT: output,
      GITHUB_STEP_SUMMARY: summary,
      GITHUB_EVENT_PATH: event,
      GITHUB_API_URL: api,
      GITHUB_REPOSITORY: "someone/firmware",
      GITHUB_SERVER_URL: "https://github.example",
      GITHUB_RUN_ID: "42",
      RUNNER_TEMP: dir,
      DRYFLASH_FIRMWARE: spinning(),
      DRYFLASH_SCRIPT: scriptFile,
      DRYFLASH_TIMEOUT: "60",
      DRYFLASH_COMMENT: "true",
      DRYFLASH_PACKAGE: RUNNER,
      DRYFLASH_TOKEN: "t0ken",
      ...extra,
    });
    return { status: r.status, stdout: r.out, out: outputs(output), summary: readFileSync(summary, "utf8") };
  };

  it("passes, comments once, and says so", async () => {
    const gh = await fakeGithub();
    const r = await start("pass", "- wait: 1s\n", gh.url);
    gh.server.close();
    expect(r.status).toBe(0);
    expect(r.out.result).toBe("pass");
    expect(r.out["exit-code"]).toBe("0");
    expect(existsSync(r.out.junit)).toBe(true);
    expect(r.summary).toContain("Passed");
    const post = gh.calls.find((c) => c.method === "POST");
    expect(post.url).toBe("/repos/someone/firmware/issues/7/comments");
    expect(post.auth).toBe("Bearer t0ken");
    expect(post.body.startsWith(MARKER)).toBe(true);
    expect(post.body).toContain("https://github.example/someone/firmware/actions/runs/42");
  }, 60_000);

  it("reports a hang and hands the exit code on, without failing itself", async () => {
    const gh = await fakeGithub();
    const r = await start("hang", "- wait: 10s\n", gh.url);
    gh.server.close();
    expect(r.status).toBe(0);
    expect(r.out.result).toBe("fail");
    expect(r.out["exit-code"]).toBe("1");
    const post = gh.calls.find((c) => c.method === "POST");
    expect(post.body).toContain("Hang: 5 s");
    expect(post.body).toContain("firmware bug");
  }, 60_000);

  it("a token that cannot comment is a warning, and the summary still has the result", async () => {
    const gh = await fakeGithub(403);
    const r = await start("fork", "- wait: 1s\n", gh.url);
    gh.server.close();
    expect(r.out.result).toBe("pass");
    expect(r.out["exit-code"]).toBe("0");
    expect(r.stdout).toContain("::warning::Could not comment");
    expect(r.summary).toContain("Passed");
  }, 60_000);

  it("a bad input is exit code 3 with no result", async () => {
    const gh = await fakeGithub();
    const r = await start("usage", "- wait: 1s\n", gh.url, { DRYFLASH_FIRMWARE: "/nonexistent.tgz" });
    gh.server.close();
    expect(r.out["exit-code"]).toBe("3");
    expect(r.out.result).toBe("error");
    expect(gh.calls).toHaveLength(0);
  }, 60_000);
});

describe("the comment", () => {
  const report = (over = {}) => ({ firmware: "fw.tgz", result: "fail", virtualMs: 1500, steps: [{ name: "x", status: "failed", message: "boom" }], ...over });

  it("does not let the firmware write markup or mention anyone", () => {
    const evil = "@everyone <img src=x> [a](http://b) `code` ```";
    expect(plain(evil)).not.toContain("@e");
    expect(plain(evil)).toContain("\\<img");
    expect(plain(evil)).toContain("\\[a\\]");
    expect(plain("two\nlines")).toBe("two lines");
    expect(toMarkdown(report({ firmware: "a`b.tgz" }))).toContain("`ab.tgz`");
    const md = toMarkdown(report({ report: { summary: evil, verdictLabel: "Looks like a firmware bug", basis: [evil], backtrace: [] } }));
    expect(md).not.toMatch(/(?<!\\)<img/);
    expect(md).not.toContain("@everyone");
  });

  it("says a verdict is a judgement, and what this test is not", () => {
    const md = toMarkdown(report({ report: { summary: "s", verdictLabel: "Could be either", basis: ["because"], backtrace: [{ address: "0x1", symbol: null, source: "pc" }] } }));
    expect(md).toContain("not a proof");
    expect(md).toContain("not a hardware certification");
    expect(md).toContain("- because");
    expect(md).toContain("0x1 ? (pc)");
  });

  it("replaces its earlier comment instead of adding another", async () => {
    const calls = [];
    const fetchImpl = async (url, init = {}) => {
      calls.push([init.method ?? "GET", url]);
      return { ok: true, status: 200, json: async () => [{ id: 1, body: "other" }, { id: 9, body: `${MARKER}\nold` }] };
    };
    const r = await postComment({ api: "https://api", repo: "o/r", pr: 3, token: "t", body: "new", fetchImpl });
    expect(r.ok).toBe(true);
    expect(calls.at(-1)).toEqual(["PATCH", "https://api/repos/o/r/issues/comments/9"]);
  });
});

describe("what is released", () => {
  it("the action fetches the release of the package's version", () => {
    const { version, name, private: isPrivate } = JSON.parse(readFileSync(join(RUNNER, "package.json"), "utf8"));
    expect(readFileSync(join(ACTION, "action.yml"), "utf8")).toContain(`/releases/download/v${version}/${name}-${version}.tgz`);
    expect(isPrivate, "not for npm").toBe(true);
  });
});

describe("the screen as text", () => {
  it("is 64 by 16, with the dots where the pixels are", () => {
    const frame = new Uint8Array(1024);
    frame[0] = 0x80; // top left
    const lines = toBraille(frame);
    expect(lines).toHaveLength(16);
    expect([...lines[0]]).toHaveLength(64);
    expect(lines[0][0]).toBe("⠁");
    expect(lines[0][1]).toBe("⠀");
    frame.fill(0xff);
    expect(toBraille(frame)[15][63]).toBe("⣿");
  });
});

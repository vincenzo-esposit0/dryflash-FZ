// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from "vitest";
import { fetchAllowed, handleProxy } from "./index";
import { MAX_BYTES, RATE_LIMIT_PER_MINUTE, RateLimiter, allowedOrigin, allowedRedirect, allowedStart } from "./rules";

const ASSET = "https://github.com/DarkFlippers/unleashed-firmware/releases/download/unlshd-094/update.tgz";
const SIGNED = "https://release-assets.githubusercontent.com/github-production/abc?sig=1";

describe("allowlist", () => {
  it("takes release assets of the listed repositories", () => {
    expect(allowedStart(ASSET)).not.toBeNull();
    expect(allowedStart(ASSET.replace("DarkFlippers", "darkflippers"))).not.toBeNull();
  });

  it.each([
    ["another repository", "https://github.com/evil/repo/releases/download/v1/a.tgz"],
    ["another host", "https://example.com/DarkFlippers/unleashed-firmware/releases/download/v/a"],
    ["a repository page", "https://github.com/DarkFlippers/unleashed-firmware"],
    ["source archives", "https://github.com/DarkFlippers/unleashed-firmware/archive/refs/tags/a.tgz"],
    ["plain http", ASSET.replace("https", "http")],
    ["a port", ASSET.replace("github.com", "github.com:444")],
    ["credentials", ASSET.replace("github.com", "x@github.com")],
    ["path tricks", "https://github.com/DarkFlippers/unleashed-firmware/releases/download/../../../../evil/x"],
    ["no URL", ""],
    ["internal address", "https://169.254.169.254/latest"],
  ])("refuses %s", (_n, url) => {
    expect(allowedStart(url)).toBeNull();
  });

  it("follows redirects only to GitHub's asset hosts", () => {
    expect(allowedRedirect(SIGNED)).not.toBeNull();
    expect(allowedRedirect("https://example.com/x")).toBeNull();
    expect(allowedRedirect("http://release-assets.githubusercontent.com/x")).toBeNull();
    expect(allowedRedirect("https://github.com/evil/repo/releases/download/v1/a")).toBeNull();
  });

  it("knows its own sites", () => {
    expect(allowedOrigin("https://app.dryflash.workers.dev")).toBe(true);
    expect(allowedOrigin("https://pr-3-app.dryflash.workers.dev")).toBe(true);
    expect(allowedOrigin("http://localhost:5173")).toBe(true);
    expect(allowedOrigin("https://evil.example")).toBe(false);
    expect(allowedOrigin("https://app.dryflash.workers.dev.evil.example")).toBe(false);
  });
});

describe("rate limiter", () => {
  it("limits per client per minute", () => {
    const r = new RateLimiter();
    for (let i = 0; i < RATE_LIMIT_PER_MINUTE; i++) expect(r.allow("a", 0)).toBe(true);
    expect(r.allow("a", 1)).toBe(false);
    expect(r.allow("b", 1)).toBe(true);
    expect(r.allow("a", 60_001)).toBe(true);
  });
});

const body = (n: number) => new Response(new Uint8Array(n), { headers: { "Content-Length": String(n) } });
const redirect = (to: string) => new Response(null, { status: 302, headers: { Location: to } });
const req = (url: string, headers: Record<string, string> = {}, method = "GET") =>
  new Request(`https://app.dryflash.workers.dev/proxy/fetch?url=${encodeURIComponent(url)}`, {
    headers: { "CF-Connecting-IP": crypto.randomUUID(), ...headers },
    method,
  });

describe("handleProxy", () => {
  const github = (async (u: string) => (u === ASSET ? redirect(SIGNED) : body(3))) as unknown as typeof fetch;

  it("serves an allowed asset, with provenance", async () => {
    const r = await handleProxy(req(ASSET), github);
    expect(r.status).toBe(200);
    expect(r.headers.get("X-Dryflash-Source")).toBe(ASSET);
    expect((await r.arrayBuffer()).byteLength).toBe(3);
  });

  it("gives CORS only to its own sites", async () => {
    const mine = await handleProxy(req(ASSET, { Origin: "https://pr-3-app.dryflash.workers.dev" }), github);
    expect(mine.headers.get("Access-Control-Allow-Origin")).toBe("https://pr-3-app.dryflash.workers.dev");
    const other = await handleProxy(req(ASSET, { Origin: "https://evil.example" }), github);
    expect(other.status).toBe(403);
    expect(other.headers.get("Access-Control-Allow-Origin")).toBeNull();
  });

  it("refuses cross-site requests, other methods, URLs and routes", async () => {
    expect((await handleProxy(req(ASSET, { "Sec-Fetch-Site": "cross-site" }), github)).status).toBe(403);
    expect((await handleProxy(req(ASSET, {}, "POST"), github)).status).toBe(405);
    expect((await handleProxy(req("https://example.com/a"), github)).status).toBe(403);
    expect((await handleProxy(new Request("https://x/proxy/other"), github)).status).toBe(404);
  });

  it("does not follow a redirect out of the allowlist", async () => {
    const evil = (async () => redirect("https://example.com/secret")) as unknown as typeof fetch;
    const r = await fetchAllowed(allowedStart(ASSET)!, evil);
    expect(r.status).toBe(502);
    expect((await handleProxy(req(ASSET), evil)).status).toBe(502);
  });

  it("gives up on a redirect loop", async () => {
    const loop = (async () => redirect(ASSET)) as unknown as typeof fetch;
    expect((await fetchAllowed(allowedStart(ASSET)!, loop)).status).toBe(502);
  });

  it("refuses what is too large or of unknown size", async () => {
    const big = (async () =>
      new Response(null, { headers: { "Content-Length": String(MAX_BYTES + 1) } })) as unknown as typeof fetch;
    expect((await handleProxy(req(ASSET), big)).status).toBe(413);
    const unknown = (async () => new Response("x")) as unknown as typeof fetch;
    expect((await handleProxy(req(ASSET), unknown)).status).toBe(502);
  });

  it("passes on upstream failures as such", async () => {
    const gone = (async () => new Response("no", { status: 404 })) as unknown as typeof fetch;
    expect((await handleProxy(req(ASSET), gone)).status).toBe(502);
  });

  it("answers from the cache without going upstream", async () => {
    const store = new Map<string, Response>();
    const cache = {
      match: async (k: Request) => store.get(k.url)?.clone(),
      put: async (k: Request, r: Response) => void store.set(k.url, r),
    } as unknown as Cache;
    let calls = 0;
    const counting = (async (u: string) => {
      calls++;
      return u === ASSET ? redirect(SIGNED) : body(3);
    }) as unknown as typeof fetch;
    const pending: Promise<unknown>[] = [];
    await (await handleProxy(req(ASSET), counting, cache, (p) => pending.push(p))).arrayBuffer();
    await Promise.all(pending);
    const before = calls;
    const again = await handleProxy(req(ASSET), counting, cache);
    expect(again.status).toBe(200);
    expect(calls).toBe(before);
  });
});

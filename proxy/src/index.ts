// SPDX-License-Identifier: GPL-3.0-or-later
// The single Worker of ADR-0003: static assets, plus `GET /proxy/fetch?url=` (ADR-0002), the one
// route that runs code. Never an open proxy: only allowlisted release assets, up to 64 MiB.
import { MAX_BYTES, MAX_REDIRECTS, RateLimiter, allowedOrigin, allowedRedirect, allowedStart } from "./rules";

interface Env {
  ASSETS: { fetch(request: Request): Promise<Response> };
}

const limiter = new RateLimiter();

function refuse(status: number, message: string, cors: Record<string, string>): Response {
  return new Response(message, { status, headers: { "Content-Type": "text/plain", ...cors } });
}

/** Follows redirects by hand, checking every hop against the allowlist. */
export async function fetchAllowed(start: URL, doFetch: typeof fetch = fetch): Promise<Response> {
  let url = start;
  for (let hop = 0; hop <= MAX_REDIRECTS; hop++) {
    const r = await doFetch(url.href, { redirect: "manual" });
    if (r.status < 300 || r.status >= 400) return r;
    const next = r.headers.get("Location");
    const target = next ? allowedRedirect(new URL(next, url).href) : null;
    if (!target) return new Response("redirect outside the allowlist", { status: 502 });
    url = target;
  }
  return new Response("too many redirects", { status: 502 });
}

export async function handleProxy(
  request: Request,
  doFetch: typeof fetch = fetch,
  cache: Pick<Cache, "match" | "put"> | null = null,
  waitUntil: (p: Promise<unknown>) => void = () => {},
  now = Date.now(),
): Promise<Response> {
  const origin = request.headers.get("Origin");
  const cors: Record<string, string> =
    origin && allowedOrigin(origin) ? { "Access-Control-Allow-Origin": origin, Vary: "Origin" } : {};
  const url = new URL(request.url);
  if (url.pathname !== "/proxy/fetch") return refuse(404, "not found", cors);
  if (request.method !== "GET") return refuse(405, "only GET", cors);
  // Same-origin requests carry no Origin; other sites are told no, and browsers would block them.
  if (origin ? !allowedOrigin(origin) : request.headers.get("Sec-Fetch-Site") === "cross-site") {
    return refuse(403, "this proxy serves dryflash only", cors);
  }
  const start = allowedStart(url.searchParams.get("url") ?? "");
  if (!start) return refuse(403, "URL not in the allowlist (ADR-0002)", cors);
  const client = request.headers.get("CF-Connecting-IP") ?? "unknown";
  if (!limiter.allow(client, now)) return refuse(429, "too many requests", cors);

  const key = new Request(`https://proxy.invalid/${encodeURIComponent(start.href)}`);
  const hit = await cache?.match(key);
  if (hit) return withCors(hit, cors);

  const upstream = await fetchAllowed(start, doFetch);
  if (!upstream.ok) return refuse(502, `upstream answered ${upstream.status}`, cors);
  const length = Number(upstream.headers.get("Content-Length") ?? NaN);
  if (!(length >= 0)) return refuse(502, "upstream did not state a size", cors);
  if (length > MAX_BYTES) return refuse(413, "larger than 64 MiB", cors);

  const response = new Response(upstream.body, {
    headers: {
      "Content-Type": "application/octet-stream",
      "Content-Length": String(length),
      "Cache-Control": "public, max-age=86400",
      "X-Dryflash-Source": start.href,
    },
  });
  if (cache) waitUntil(cache.put(key, response.clone()));
  return withCors(response, cors);
}

function withCors(r: Response, cors: Record<string, string>): Response {
  const out = new Response(r.body, r);
  for (const [k, v] of Object.entries(cors)) out.headers.set(k, v);
  return out;
}

export default {
  async fetch(request: Request, env: Env, ctx: { waitUntil(p: Promise<unknown>): void }): Promise<Response> {
    if (!new URL(request.url).pathname.startsWith("/proxy/")) return env.ASSETS.fetch(request);
    const cache = (globalThis as unknown as { caches: { default: Cache } }).caches.default;
    return handleProxy(request, fetch, cache, (p) => ctx.waitUntil(p));
  },
};

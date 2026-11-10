// SPDX-License-Identifier: GPL-3.0-or-later
// Getting firmware from a URL (ADR-0002): straight from its host, or through dryflash's allowlisted
// proxy when the host sends no CORS headers. The page always says which.

/** Same limit as the proxy. */
export const MAX_BYTES = 64 * 1024 * 1024;

export interface Fetched {
  bytes: ArrayBuffer;
  name: string;
  viaProxy: boolean;
}

/** Only https, or a path on this site; never `file:`, `blob:` or plain http. */
export function checkUrl(raw: string, base: string): URL {
  const u = new URL(raw, base);
  const local = u.origin === new URL(base).origin;
  if (u.protocol !== "https:" && !local) throw new Error(`only https URLs can be loaded: ${raw}`);
  return u;
}

export function fileName(u: URL): string {
  const last = u.pathname.split("/").pop();
  return last ? decodeURIComponent(last) : u.hostname;
}

async function read(r: Response, onProgress?: (done: number, total: number) => void): Promise<ArrayBuffer> {
  const total = Number(r.headers.get("Content-Length") ?? 0);
  if (total > MAX_BYTES) throw new Error("the file is larger than 64 MiB");
  if (!r.body) return r.arrayBuffer();
  const reader = r.body.getReader();
  const chunks: Uint8Array[] = [];
  let done = 0;
  for (;;) {
    const { done: end, value } = await reader.read();
    if (end) break;
    done += value.length;
    if (done > MAX_BYTES) {
      await reader.cancel();
      throw new Error("the file is larger than 64 MiB");
    }
    chunks.push(value);
    onProgress?.(done, total);
  }
  const out = new Uint8Array(done);
  let o = 0;
  for (const c of chunks) {
    out.set(c, o);
    o += c.length;
  }
  return out.buffer;
}

export async function fetchFirmware(
  raw: string,
  base: string,
  onProgress?: (done: number, total: number) => void,
  doFetch: typeof fetch = fetch,
): Promise<Fetched> {
  const u = checkUrl(raw, base);
  const name = fileName(u);
  let direct: Response;
  try {
    direct = await doFetch(u.href);
  } catch {
    try {
      // A host that echoes the Origin in Access-Control-Allow-Origin without `Vary: Origin`
      // (update.flipperzero.one) lets the browser's cache hand back a copy made for another
      // origin, which then fails CORS. Asking past the cache settles it.
      direct = await doFetch(u.href, { cache: "reload" });
    } catch {
      // A browser reports a missing CORS header as a network error: try the proxy.
      return { bytes: await read(await viaProxy(u, doFetch), onProgress), name, viaProxy: true };
    }
  }
  if (!direct.ok) throw new Error(`cannot download ${u.href}: ${direct.status}`);
  return { bytes: await read(direct, onProgress), name, viaProxy: false };
}

async function viaProxy(u: URL, doFetch: typeof fetch): Promise<Response> {
  const proxied = await doFetch(`/proxy/fetch?url=${encodeURIComponent(u.href)}`);
  if (!proxied.ok) {
    throw new Error(`cannot download ${u.href}: the host allows no direct download, and the proxy said ${proxied.status}: ${await proxied.text()}`);
  }
  // A static host without the Worker answers any path with the app's page.
  if ((proxied.headers.get("Content-Type") ?? "").startsWith("text/html")) {
    throw new Error(`cannot download ${u.href}: the host allows no direct download, and this site has no download proxy. Download the file and drop it on the page.`);
  }
  return proxied;
}

/** The parameters of a link (docs/EMBED.md). */
export interface LinkParams {
  fw: string;
  elf: string | null;
  ota: boolean;
  autoboot: boolean;
  device: string | null;
  frames: boolean;
}

export function parseLink(search: string): LinkParams | null {
  const p = new URLSearchParams(search);
  const fw = p.get("fw");
  if (!fw) return null;
  return {
    fw,
    elf: p.get("elf"),
    ota: p.get("mode") === "ota",
    autoboot: p.get("autoboot") === "1",
    device: p.get("device"),
    frames: p.get("frames") === "1",
  };
}

/** Name of the device a link's firmware goes on, unless the link names one. */
export const LINK_DEVICE = "From a link";

/** A link must ask first unless it asked for autoboot and overwrites nothing of the visitor's. */
export function linkMustAsk(link: LinkParams, target: { hasFirmware: boolean } | null): boolean {
  if (link.device && target?.hasFirmware) return true;
  return !link.autoboot;
}

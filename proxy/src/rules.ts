// SPDX-License-Identifier: GPL-3.0-or-later
// What the fetch proxy may touch (ADR-0002). Pure functions, so that they can be tested.

/** Repositories of firmware/compat.json whose release assets lack CORS headers. */
export const ALLOWED_REPOS = new Set(
  [
    "Next-Flip/Momentum-Firmware",
    "DarkFlippers/unleashed-firmware",
    "RogueMaster/flipperzero-firmware-wPlugins",
  ].map((r) => r.toLowerCase()),
);

/** Where GitHub sends release downloads. */
const REDIRECT_HOSTS = new Set(["release-assets.githubusercontent.com", "objects.githubusercontent.com"]);

export const MAX_BYTES = 64 * 1024 * 1024;
export const MAX_REDIRECTS = 5;
export const RATE_LIMIT_PER_MINUTE = 30;

function parse(raw: string): URL | null {
  try {
    const u = new URL(raw);
    if (u.protocol !== "https:" || u.port || u.username || u.password) return null;
    return u;
  } catch {
    return null;
  }
}

/** A URL a request may ask for: a release asset of an allowed repository, and nothing else. */
export function allowedStart(raw: string): URL | null {
  const u = parse(raw);
  if (!u || u.hostname !== "github.com") return null;
  const parts = u.pathname.split("/");
  // "", owner, repo, "releases", "download", tag, file
  if (parts.length !== 7 || parts[3] !== "releases" || parts[4] !== "download") return null;
  if (parts.slice(5).some((p) => !p || p === "." || p === "..")) return null;
  return ALLOWED_REPOS.has(`${parts[1]}/${parts[2]}`.toLowerCase()) ? u : null;
}

/** A redirect target that may be followed from an allowed URL. */
export function allowedRedirect(raw: string): URL | null {
  const u = parse(raw);
  if (!u) return null;
  return REDIRECT_HOSTS.has(u.hostname) ? u : allowedStart(raw);
}

/** The sites that may use the proxy from a browser: this project's Worker, previews included. */
export function allowedOrigin(origin: string): boolean {
  return (
    /^https:\/\/([a-z0-9-]+-)?app\.dryflash\.workers\.dev$/.test(origin) ||
    /^http:\/\/localhost:\d+$/.test(origin)
  );
}

/** Per-client request counter for one minute windows. Per isolate, so it limits abuse only roughly. */
export class RateLimiter {
  private windows = new Map<string, { start: number; count: number }>();

  allow(client: string, nowMs: number): boolean {
    const w = this.windows.get(client);
    if (!w || nowMs - w.start >= 60_000) {
      // Old windows go away here, so the map cannot grow without bound.
      if (this.windows.size > 10_000) this.windows.clear();
      this.windows.set(client, { start: nowMs, count: 1 });
      return true;
    }
    w.count++;
    return w.count <= RATE_LIMIT_PER_MINUTE;
  }
}

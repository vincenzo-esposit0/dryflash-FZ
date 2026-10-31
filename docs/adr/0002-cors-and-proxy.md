<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# ADR-0002 — Loading firmware by URL: CORS findings and proxy

- Status: **Accepted**
- Date: 2026-10-31
- Milestone: M0 (implementation in M5)

## Context

`?fw=<url>` must load firmware directly from its original host (BRIEF §9); we never host binaries
(BRIEF §11). Browsers can only read cross-origin responses that carry CORS headers.

## Findings (curl with an `Origin` header, 2026-10-31)

| Firmware | Host | CORS |
|---|---|---|
| Official 1.5.1 (`update_tgz`, `firmware_elf`, …) | `update.flipperzero.one/builds/firmware/…` | **Yes**: `Access-Control-Allow-Origin` echoes the request origin |
| Official release channel index | `update.flipperzero.one/firmware/directory.json` | Yes (same host) |
| Momentum `mntm-012` | GitHub release asset | **No** |
| Unleashed `unlshd-094` | GitHub release asset | **No** |
| RogueMaster `RM1024-1347-392a65c3` | GitHub release asset | **No** |

GitHub release downloads answer `302` from `github.com` **without** CORS headers, and the
redirect target (`release-assets.githubusercontent.com`, signed short-lived URL) has none either.
A browser `fetch` fails at the first hop. Official GitHub releases carry no binary assets at all;
official builds are only on `update.flipperzero.one`.

## Decision

1. The web app first tries a direct `fetch`. Official firmware works without a proxy.
2. For hosts without CORS, a **Cloudflare Worker** in `/proxy` with: one route
   (`GET /fetch?url=…`); allowlist of `github.com/<owner>/<repo>/releases/download/…` for repos
   listed in configuration (initially the three above) and their redirect target
   `release-assets.githubusercontent.com` (plus `objects.githubusercontent.com`, used historically);
   redirects followed only inside the allowlist; 64 MiB response cap; edge caching keyed by URL;
   per-IP rate limit; `Access-Control-Allow-Origin` only for dryflash origins. No other routes.
3. Local drag & drop never uses the network.
4. The proxy is listed in the UI ("loaded via dryflash proxy from <original URL>") so provenance stays visible.

## Consequences

- A Cloudflare account is required for the proxy regardless of where the static site is hosted (see ADR-0003).
- The allowlist must be updated when new firmware projects are added to `firmware/compat.json`.
- CI and the headless runner (Node) are not subject to CORS and fetch directly.

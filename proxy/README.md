<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# /proxy — the Cloudflare Worker (static assets + allowlisted fetch proxy)

`wrangler.jsonc` configures the single Worker of ADR-0003. Since M3 it serves `web/dist` as
static assets; deployment is described in [docs/DEPLOY.md](../docs/DEPLOY.md).

The fetch proxy itself is not implemented yet (milestone M5). Needed because GitHub release assets are served
without CORS headers (verified in M0, see `docs/adr/0002-cors-and-proxy.md`).

Design constraints: a single `GET /fetch?url=…` route, strict host/repository allowlist,
redirects followed only within the allowlist, 64 MiB cap, edge cache, per-IP rate limit,
`Access-Control-Allow-Origin` only for dryflash origins. Never an open proxy.

<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# /proxy — allowlisted fetch proxy (Cloudflare Worker)

Not implemented yet (milestone M5). Needed because GitHub release assets are served
without CORS headers (verified in M0, see `docs/adr/0002-cors-and-proxy.md`).

Design constraints: a single `GET /fetch?url=…` route, strict host/repository allowlist,
redirects followed only within the allowlist, 64 MiB cap, edge cache, per-IP rate limit,
`Access-Control-Allow-Origin` only for dryflash origins. Never an open proxy.

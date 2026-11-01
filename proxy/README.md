<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# /proxy — the Cloudflare Worker (static assets + allowlisted fetch proxy)

`wrangler.jsonc` configures the single Worker of ADR-0003. It serves `web/dist` as static assets;
deployment is described in [docs/DEPLOY.md](../docs/DEPLOY.md).

`src/index.ts` adds one route, because GitHub release assets are served without CORS headers
(`docs/adr/0002-cors-and-proxy.md`): `GET /proxy/fetch?url=<release asset>`. Rules, in `src/rules.ts`:
only release assets of the repositories listed there, redirects followed only to GitHub's asset hosts,
64 MiB at most, one day of edge cache, about 30 requests a minute per client (per isolate, so a rough
limit), and only dryflash sites may use it from a browser. Never an open proxy.

When a firmware project is added to `firmware/compat.json`, add its repository to the allowlist.

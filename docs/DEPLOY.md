<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# Deployment (Cloudflare Worker with static assets)

Decision and rationale: [ADR-0003](adr/0003-hosting.md). One Worker, named `app`, serves
`web/dist`; its configuration is `proxy/wrangler.jsonc`. Response headers (CSP, caching) come
from `web/public/_headers`. GitHub Actions deploys it after every CI job passed
(`.github/workflows/ci.yml`, job `deploy`):

| Event | Result |
|---|---|
| push to `main` | `wrangler deploy` → production, `https://app.<account-subdomain>.workers.dev` |
| pull request from this repository | `wrangler versions upload --preview-alias pr-<n>` → `https://pr-<n>-app.<account-subdomain>.workers.dev` |
| pull request from a fork | no deploy (no secrets) |

## One-time setup (maintainer)

1. **workers.dev subdomain.** Cloudflare dashboard → *Workers & Pages* → the account's
   `workers.dev` subdomain (proposed: `dryflash`). Shown on the Workers overview page.
2. **API token.** Dashboard → *My Profile* → *API Tokens* → *Create Token* → template
   **Edit Cloudflare Workers**. Scope it to this account only (Account Resources → the account;
   Zone Resources → all zones is not needed: no custom domain yet). Create it and copy it once.
3. **Account ID.** Dashboard → *Workers & Pages* overview, right column, *Account ID*.
4. **GitHub secrets.** Repository → *Settings* → *Secrets and variables* → *Actions* →
   *New repository secret*: `CLOUDFLARE_API_TOKEN` (the token) and `CLOUDFLARE_ACCOUNT_ID`.
   Never paste the token anywhere else (chat, issues, commits).

The first run on `main` creates the Worker. Without the secrets the `deploy` job fails with an
explicit error; every other job is unaffected.

## Local check

```sh
scripts/build-wasm.sh && npm run build
npx wrangler dev --config proxy/wrangler.jsonc   # http://localhost:8787, same runtime and headers
```

`wrangler login` is only needed for manual deploys from a workstation; CI uses the token.

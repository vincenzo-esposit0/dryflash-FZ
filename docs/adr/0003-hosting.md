<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# ADR-0003 — Hosting: one Cloudflare Worker with static assets

- Status: **Accepted** (maintainer, 2026-10-31); URL since M3: https://app.dryflash.workers.dev
- Date: 2026-10-31

## Context

BRIEF §14 q1/q2: custom domain or default URL; GitHub Pages or Cloudflare Pages. The site is a
pure static build without COOP/COEP requirements (no SharedArrayBuffer), so any static host works.
ADR-0002 shows a Cloudflare Worker proxy is needed anyway for three of the four target firmwares.

The M0 plan first recommended Cloudflare **Pages**. Checking Cloudflare's documentation on
2026-10-31 changed that: the Pages docs now open with "Start new projects with Workers", Workers
is described as "Cloudflare's primary platform", and Pages features (Git builds, previews) exist
on Workers as static assets + Workers Builds. Pages is not deprecated, but it is not where new
features land.

## Decision

Deploy **one Cloudflare Worker with static assets** that serves both parts of the product:

- `web/dist` as static assets. Static asset requests are free and unlimited, and do not count
  towards the Workers Free plan's 100 000 requests/day (Cloudflare "Billing and limitations").
- The `/proxy/*` route (ADR-0002) as Worker code, selected with `run_worker_first = ["/proxy/*"]`.
  Same origin as the app, so the proxy needs no CORS for itself, and only proxy calls consume Worker requests.

Default URL: `<worker>.<account-subdomain>.workers.dev`. Proposed: account subdomain `dryflash`
and Worker name `app` → `app.dryflash.workers.dev`. Availability of an account subdomain cannot be
checked from outside (DNS answers NXDOMAIN for every bare account subdomain); the dashboard
confirms it when it is chosen. A custom domain can be attached later without code changes
(the build uses a relative `base`).

Free plan limits that matter (Cloudflare limits pages, 2026-10-31): 25 MiB per asset (the
WASM core is far below), 20 000 files per version, 100 000 Worker requests/day (proxy only).

## Deployment from GitHub (decided in M3)

Two supported ways; the recommended one is (a):

- **(a) GitHub Actions deploys with Wrangler.** CI already builds Rust → WASM and the web app and
  runs the tests; a deploy job runs `wrangler deploy` on `main` and `wrangler versions upload`
  with a preview alias for pull requests. Needs a scoped API token stored as a GitHub secret.
  Cloudflare gets no access to the repository, and nothing deploys unless the tests pass.
- **(b) Workers Builds.** Cloudflare's GitHub App watches the repository and builds in Cloudflare's
  build environment (Free: 3 000 build minutes/month, 1 concurrent build, 20-minute timeout).
  It would need the pinned Rust toolchain and the wasm target installed in Cloudflare's build
  image on every build, and it builds independently of our CI results.

## Consequences

- The maintainer creates the Cloudflare account and chooses the `workers.dev` subdomain.
- In M3 the maintainer creates an API token (template "Edit Cloudflare Workers") and stores it,
  with the account ID, as GitHub repository secrets `CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ACCOUNT_ID`.
  Tokens are never pasted into chat, issues or commits.
- `/proxy` contains the Worker script and `wrangler` configuration for the single Worker.

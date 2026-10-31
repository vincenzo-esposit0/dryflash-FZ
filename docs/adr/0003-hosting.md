<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# ADR-0003 — Hosting: Cloudflare Pages

- Status: **Proposed** — the maintainer approved the plan; the default URL is still open (see below)
- Date: 2026-10-31

## Context

BRIEF §14 q1/q2: custom domain or default URL; GitHub Pages or Cloudflare Pages. The site is a
pure static build without COOP/COEP requirements (no SharedArrayBuffer), so both hosts work.
ADR-0002 shows a Cloudflare Worker proxy is needed anyway for three of the four target firmwares.

## Decision

Host the static site on **Cloudflare Pages**, next to the `/proxy` Worker: one account, one
deployment pipeline, preview deployments per pull request (useful from M3), brotli served
automatically, and the proxy can live on the same origin (`/proxy/*` route) so it needs no CORS of its own.

## Open point

The M0 plan recommended both "the default GitHub Pages URL for now" (q1) and "Cloudflare Pages"
(q2). These are inconsistent: Cloudflare Pages' default URL is `<project>.pages.dev`
(e.g. `dryflash.pages.dev`, availability not checked). Options:

1. Cloudflare Pages with its default `*.pages.dev` URL (recommended, no domain cost), or
2. GitHub Pages at `vincenzo-esposit0.github.io/dryflash-FZ` plus a separately deployed Worker
   for the proxy (two hosts, proxy on a different origin).

To be confirmed by the maintainer before the M3 preview deployment. Nothing in M0–M2 depends on it:
the build uses a relative `base`, so it works under any path.

## Consequences

- CI gets a deploy job in M3 (needs a Cloudflare API token as a repository secret, set by the maintainer).

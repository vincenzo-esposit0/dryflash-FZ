<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# ADR-0004 — UI framework: Preact

- Status: **Accepted** (recommendation in the approved M0 plan)
- Date: 2026-10-31

## Context

BRIEF §3/§14 q3: Preact or Svelte, small bundle, static output, TypeScript.

## Decision

**Preact 11** with Vite and TypeScript (strict), JSX via `@preact/preset-vite`.

Reasons: ~4 KiB gzip runtime; plain TypeScript/JSX with no extra compiler or template language,
so components, the worker protocol types and the embed API share one type system; the large
React-ecosystem knowledge base applies (hooks, testing patterns); signals available if fine-grained
updates are needed for the LCD/status panels. The LCD itself is drawn to a `<canvas>` outside the
virtual DOM, so framework render speed is not on the hot path.

Svelte would be equally viable; it was not chosen because its compiler and `.svelte` files add a
second language to a TypeScript code base with little benefit for this UI's size.

## Consequences

- M0 scaffold: `web/` with Vite 8, Preact 11, TypeScript 7, Vitest 5 (exact versions pinned).
  Current bundle: 6.0 KiB gzip JS, 0.4 KiB CSS.

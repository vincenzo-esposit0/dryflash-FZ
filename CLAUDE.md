# CLAUDE.md — dryflash

**dryflash** — an emulator for Flipper Zero firmware that runs entirely in the browser.
"Try any Flipper Zero firmware in your browser before flashing it to your real device."
It executes the real, unmodified release binaries (`.dfu` / `.tgz`) via instruction-level
emulation of the STM32WB55RG (target `f7`). CPU2 is replaced by a behavioural stub.

## Session start

At the start of every session read `docs/BRIEF.md`, `docs/ROADMAP.md` and
`docs/BRINGUP_LOG.md`, then continue from the next unfinished milestone.
`docs/BRIEF.md` is the full project brief and the source of truth for scope.

## Working rules (BRIEF §0)

1. **Plan first.** No code before `docs/ARCHITECTURE.md` and `docs/ROADMAP.md` are approved by the maintainer.
2. **Milestones (BRIEF §12).** Each ends with passing tests, an updated `docs/BRINGUP_LOG.md`,
   a short demo instruction and a commit. Stop and report at the end of each milestone.
3. **Never fake success.** Stubbed peripherals are declared stubbed in code, in the bring-up
   digest and in the UI ("Simulated" / "Stubbed" / "Not emulated"). No hard-coded screens,
   no pre-rendered frames, no "pretend boot".
4. **Firmware source is ground truth.** Never guess a pin, bus, address or register: look it up
   in the pinned firmware tag and in RM0434 / PM0214 / ARMv7-M ARM, and cite file/section in a code comment.
5. **Record non-obvious decisions** as ADRs in `docs/adr/NNNN-title.md`.
6. **If the brief is technically wrong**, say so, propose the fix and update the docs. Never silently work around it.

Communication with the maintainer is in Italian; code, comments, commits and docs are in English.

## Stack (BRIEF §3)

- Core: Rust → `wasm32-unknown-unknown` + `wasm-bindgen`; deterministic, no threads.
- Host: Web Worker running the WASM core; `postMessage` + transferable `ArrayBuffer`s.
  **No SharedArrayBuffer** (must run on static hosts without COOP/COEP).
- Frontend: Vite + TypeScript + Preact (ADR-0004); xterm.js for the CLI.
- Storage: OPFS, IndexedDB fallback.
- Headless: Node.js CLI reusing the same WASM core.
- Tests: `cargo test`, Vitest, Playwright.

## Useful commands

- `tests/build-fw.sh` — build the bare-metal test firmwares.
- `node scripts/fetch-firmware.mjs <id> [--elf]` — pinned firmware into `firmware/cache/` (git-ignored); the official
  firmware source tree for reference lives in `firmware/cache/src/` (sparse clone of the pinned tag).
- `cargo test --workspace --release -- --include-ignored` — all tests, including booting the pinned firmware.
- `cargo run --release -p dryflash-core --example boot -- <dfu> --elf <elf>` — boot milestones + bring-up digest.
- `python tests/cpu-fuzz/fuzz.py target/release/cpu-exec <cases> <seed> [--corpus f]` — differential fuzzing vs Unicorn.

## Licence rule

GPL-3.0-or-later. Do not replace `LICENSE`. Every source file starts with
`SPDX-License-Identifier: GPL-3.0-or-later`; `license = "GPL-3.0-or-later"` in every
`Cargo.toml` and `package.json`. Ported code keeps its copyright notices and is credited in `CREDITS.md`.
Never commit or host firmware binaries; never brand the product as "Flipper …".

## Repo layout

```
/core            Rust crate: cpu/, bus/, mem/, periph/ (stm32wb55), board/ (f7 devices), loader/, trace/, crash/
/tests/cpu-bench ADR-0001 benchmark firmware, harness and Unicorn differential check
/core-wasm       wasm-bindgen bindings
/runner          Node CLI (headless), uses core-wasm
/web             Vite app (emulator UI, embed, compat, about)
/proxy           Cloudflare Worker: serves web/dist as static assets + allowlisted CORS proxy (ADR-0003)
/action          GitHub Action
/firmware        compat.json (pinned URLs + hashes) — NO binaries committed
/tests           integration + E2E + golden screenshots
/docs            BRIEF, ARCHITECTURE, ROADMAP, BRINGUP_LOG, EMBED, adr/
```

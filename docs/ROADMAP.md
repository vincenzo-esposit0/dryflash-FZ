# dryflash — Roadmap

Status: **approved 2026-10-31**; M0, M1 and M2 done. Milestones follow BRIEF §12. Each milestone ends with: passing tests,
updated `docs/BRINGUP_LOG.md`, a short demo instruction, a commit, and a stop-and-report.

Legend: ☐ todo · ◐ in progress · ☑ done

---

## M0 — Spike + plan ☑ (2026-10-31)

- ☑ `docs/BRIEF.md`, `CLAUDE.md`
- ☑ `docs/ARCHITECTURE.md`, `docs/ROADMAP.md` (approved 2026-10-31)
- ☑ BRIEF §14 answers → ADR-0003 (Cloudflare Worker + static assets; final URL when the account exists), ADR-0004 (Preact), ADR-0005 (pinned versions), ADR-0006 (no analytics)
- ☑ **ADR-0001 CPU engine**: custom Rust interpreter; verified against Unicorn 2.1.4 (522 079 identical steps);
  153–165 MIPS in Chrome/Safari Web Workers; Unicorn→WASM excluded (no upstream interpreter, GPL-2.0, size)
- ☑ ADR-0002 CORS + proxy: official host OK, GitHub release assets need the proxy
- ☑ Repo scaffold: Cargo workspace (`core`, `core-wasm`, spike harness), `runner` (honest stub), `web` (Vite + Preact),
  `proxy`/`action` placeholders, `firmware/compat.json`, SPDX headers, licence fields, `CREDITS.md`, `README.md`
- ☑ CI skeleton: fmt, clippy, tests, wasm build, Node benchmark, web test/build, SPDX check, `cargo deny` (not run on GitHub yet)
- ☑ `docs/BRINGUP_LOG.md` started

**Demo:** see `docs/BRINGUP_LOG.md` § M0.

## M1 — CPU core + memory + DFU loader ☑ (2026-10-31)

- ☑ Bus + memory map (flash + boot alias, SRAM1, SRAM2a/b, OTP, engineering bytes, peripheral blocks from the CMSIS header)
- ☑ Full Thumb/Thumb-2 + DSP + FPv4-SP decoder: 219 464 / 219 464 instructions of firmware 1.5.1
- ☑ Exception model (entry/return, FP frames, priorities, faults, escalation, lockup), MRS/MSR/CPS
- ☑ PPB model: SCB, NVIC, SysTick, MPU (PMSAv7), FPU control, DWT CYCCNT, CoreDebug
- ☑ ARM-exact FP semantics and exception flags (ADR-0008); eager FP stacking (ADR-0007)
- ☑ DfuSe loader (suffix CRC, CPU2-region guard) + ELF symbols; `Machine` with WFI fast-forward and SYSRESETREQ
- ☑ Bring-up digest (unimplemented registers by block/offset with first PC, unmapped accesses, events) — ADR-0009
- ☑ Differential fuzzing vs Unicorn (random + firmware-corpus encodings) in CI; exception conformance firmware

**DoD met:** pinned official `.dfu` executes from the reset vector through `SystemInit` and `main`
into `furi_hal_init_early`; tests green; digest shows the next gaps (BRINGUP_LOG § M1).

## M2 — System bring-up ☑ (2026-10-31)

- ☑ Peripheral register files from ST's SVD (ADR-0010); NVIC interrupt lines; virtual time base
  (ns) and peripheral events on the CPU loop and WFI fast-forward
- ☑ RCC (clock tree, ready flags, reset flags, backup domain), GPIO, EXTI, I2C, SPI, USART/LPUART,
  RTC (calendar, backup registers), HSEM, IPCC, RNG (deterministic), LPTIM1/2 (tickless idle)
- ☑ CPU2: not emulated, firmware's "C2 startup failed" path (ADR-0011; superseded in M3 by ADR-0012)
- ☑ Board: BQ27220 fuel gauge, BQ25896 charger on I2C1; OTP provisioned with board data
- ☑ Furi crash record decoding (RTC BKP4R) and boot-loop detection
- ☑ Boot test: reset vector → `flipper_init`, then 10 s with no fault or reset
- → M4: FLASH controller program/erase + option bytes (only the updater writes flash in 1.5.1)
- → M3: wasm speed back to the M0 level; IWDG (status in BRINGUP_LOG M3 CP2)
- → backlog: detailed instruction cycle model

**DoD met:** Furi kernel starts, services start, no boot loop (BRINGUP_LOG § M2).

## M3 — First visible MVP

- ☑ ST7567, buttons + EXTI, LP5562 backlight/LED
- ☑ CPU2 as a FUS without wireless stack; virtual device with Legacy sleep (ADR-0012)
- ☑ `core-wasm` bindings + Web Worker host (slices, 1× pacing, Turbo, transferable frames, minimum press time)
- ☑ Minimal web UI: drag & drop `.dfu`, LCD canvas, keyboard + on-screen buttons, fidelity table, honest-positioning text, disclaimer, virtual-device setting, firmware log
- ☑ Deployed: https://app.dryflash.workers.dev (one Cloudflare Worker, CI deploy after all checks, docs/DEPLOY.md)

**DoD:** Desktop rendered in the browser, menus navigable with the keyboard, preview URL live.

## M4 — SD card + `.tgz`

- ☑ FLASH controller: unlock, program, page erase, option bytes (moved from M2)
- ☑ SD SPI protocol, FAT32 card (ADR-0013), saved in the browser (OPFS, IndexedDB fallback)
- ☑ `.tgz` + `update.fuf`; resources as `resources.ths` (heatshrink + tar) or `resources.tar.gz`, unpacked to `/ext`, also from the web app
- ☑ Apps (`.fap`) launch from SD
- ☑ Real OTA install mode via the firmware's own updater (RAM execution, option bytes); radio stack left out (ADR-0014)

**DoD:** official `.tgz` quick-installs and runs apps from SD; Real OTA install succeeds on the official firmware.

## M5 — Web app complete

- ☑ Virtual devices: flash, option bytes, card and settings per device; export/import, factory reset
- ☑ Full layout (BRIEF §9): toolbar, SD file manager, crash report, events
- ☑ Screenshot, GIF/WebM recording
- ☑ `?fw=` / `?elf=` / `mode` / `autoboot` / `device` URL loading + `/proxy`
- ☑ `/embed` + embed API + `docs/EMBED.md` + "Try in browser" badge; `/compat` (by hand until M6) and `/about`
- ☑ Pixel-art device modelled on the real one, tabs modelled on qFlipper's use (ADR-0015)
- ☑ i18n (en, it), accessibility pass (axe: WCAG A and AA), keyboard use
- ☑ E2E suite on Chromium, Firefox and WebKit (`tests/e2e`)
- ◐ Firmware Hub's "Try in emulator" link: this side is ready; the link belongs to the Hub, in the maintainer's hands

**DoD:** E2E suite green on Chrome/Firefox/WebKit; Hub "Try in emulator" link works.

## M6 — Crash detection + headless

- ☑ Full crash report (§7), symbolication via ELF, emulator-gap vs firmware-bug labelling (ADR-0016)
- ☑ Hang detection, and the independent watchdog
- ☑ `dryflash run`, YAML scripts, default smoke test, JUnit, screenshots, exit codes; released on GitHub, not on npm (maintainer's decision)
- ☑ CC1101 and ST25R3916 stubs (BRIEF §5.2), which the smoke test needs
- ◐ GitHub Action with PR comment (`action/`); `v1` follows the first release (docs/RELEASE.md)
- ☐ Nightly compatibility matrix + `/compat` page (Official, Momentum, Unleashed, RogueMaster)

**DoD:** Action usable from another repo; `/compat` generated by CI.

## M7 — Senses & CLI

- ☐ Speaker → WebAudio (muted by default), vibro, LED, battery/USB controls
- ☐ USB FS device + CDC → xterm.js CLI (stretch: protobuf RPC)

**DoD:** CLI usable in the browser; audio/vibro/LED visible and controllable.

## M8 (stretch) — Virtual signals

- ☐ `.sub` raw → CC1101 RX, `.nfc` → ST25R3916 virtual tag, `.ir` → IR RX

---

## Pinned versions (ADR-0005, hashes in `firmware/compat.json`)

| Firmware | Pinned | Source host | CORS |
|---|---|---|---|
| Official | `1.5.1` (+ "latest release" channel) | update.flipperzero.one | yes |
| Momentum | `mntm-012` | GitHub releases | no → proxy |
| Unleashed | `unlshd-094` | GitHub releases | no → proxy |
| RogueMaster | `RM1024-1347-392a65c3` | GitHub releases | no → proxy |

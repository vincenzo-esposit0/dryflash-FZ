# dryflash — Roadmap

Status: **approved 2026-10-31**; M0 and M1 done. Milestones follow BRIEF §12. Each milestone ends with: passing tests,
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

## M2 — System bring-up

- ☐ RCC first: enable/reset registers that latch (the M1 boot loop is `furi_check` in `furi_hal_bus_enable`), ready flags, clock tree for timer rates
- ☐ PWR, FLASH controller (+ option bytes, ACR), HSEM, IPCC, SYSCFG/EXTI (digest order: PWR, RCC, GPIO, SYSCFG, EXTI, FLASH, RTC)
- ☐ CPU2 behavioural stub (ADR: secure-enclave behaviour)
- ☐ OTP provisioning from `furi_hal_version` layout
- ☐ RTC + backup registers (survive soft reset), IWDG, RNG (seeded), CRC, DWT CYCCNT, MPU
- ☐ Timers/LPTIM, EXTI, GPIO, DMA/DMAMUX, I2C, SPI cores; board devices initialise (stubs declared)
- ☐ Virtual-time scheduler + WFI fast-forward; determinism test
- ☐ Reset/boot-loop detection; decode the Furi crash record in RTC backup registers
- ☐ Cycle model (instruction timings, exception entry/exit) and per-instruction cost back to M0 level in wasm

**DoD:** Furi kernel starts, services start, no boot loop; digest shows no unexpected gaps on the boot path.

## M3 — First visible MVP

- ☐ ST7567, buttons + EXTI, LP5562 backlight/LED
- ☐ `core-wasm` bindings + Web Worker host (slices, throttle, Turbo, transferable frames)
- ☐ Minimal web UI: drag & drop `.dfu`, LCD canvas, keyboard + on-screen buttons, fidelity table, honest-positioning text, disclaimer
- ☐ Deploy preview (host per §14 answer)

**DoD:** Desktop rendered in the browser, menus navigable with the keyboard, preview URL live.

## M4 — SD card + `.tgz`

- ☐ SD SPI protocol, FAT32 image (OPFS / IndexedDB fallback)
- ☐ `.tgz` + `update.fuf`; resources as `resources.ths` (heatshrink + tar) or `resources.tar.gz`, unpacked to `/ext`
- ☐ Apps (`.fap`) launch from SD
- ☐ Real OTA install mode via the firmware's own updater (RAM execution, option bytes)

**DoD:** official `.tgz` quick-installs and runs apps from SD; Real OTA install succeeds on the official firmware.

## M5 — Web app complete

- ☐ Full layout and panels (BRIEF §9), profiles + export/import + factory reset
- ☐ Screenshot, GIF/WebM recording
- ☐ `?fw=` / `?elf=` / `mode` / `autoboot` / `device` URL loading + `/proxy` deployed
- ☐ `/embed` + embed API + `docs/EMBED.md` + "Try in browser" badge
- ☐ i18n (en, it), accessibility pass, reduced motion

**DoD:** E2E suite green on Chrome/Firefox/WebKit; Hub "Try in emulator" link works.

## M6 — Crash detection + headless

- ☐ Full crash report (§7), symbolication via ELF, emulator-gap vs firmware-bug labelling
- ☐ Hang detection
- ☐ `npx dryflash run`, YAML scripts, default smoke test, JUnit, screenshots, exit codes
- ☐ GitHub Action `action@v1` with PR comment
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

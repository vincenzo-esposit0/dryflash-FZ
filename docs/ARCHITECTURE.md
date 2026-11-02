# dryflash — Architecture

Status: **approved 2026-10-31**, updated with M0 findings. Scope and requirements live in [`BRIEF.md`](BRIEF.md);
this document describes *how* we build it. Decisions marked **(ADR)** get a record in `docs/adr/`.
Values marked **(verify)** must be confirmed against the pinned firmware tag / RM0434 before
code depends on them (rule 4).

---

## 1. Verified facts (inspected 2026-10-31)

These come from inspecting real artifacts, not from memory. They shape the design below.

| Fact | Source |
|---|---|
| Latest official release is **1.5.1** (2026-10-15); RC `1.5.1-rc`; dev builds daily | `https://update.flipperzero.one/firmware/directory.json` |
| Official GitHub releases carry **no assets**; binaries are served from `update.flipperzero.one/builds/firmware/<ver>/` (`update_tgz`, `full_bin`, `firmware_elf`, `appsymbols_tgz`, …) | GitHub API + `directory.json` |
| `update.flipperzero.one` **sends `Access-Control-Allow-Origin`** (echoes the Origin) → direct browser `fetch` works | `curl -I -H Origin:` on `flipper-z-f7-update-1.5.1.tgz` |
| GitHub release assets (Momentum, Unleashed, RogueMaster) answer with a **302 without CORS headers**, and the `release-assets.githubusercontent.com` target has **no CORS headers** either → browser `fetch` fails, a proxy is required | `curl -I` on `unlshd-094` asset |
| `.tgz` layout: `f7-update-<ver>/{update.fuf, firmware.dfu, updater.bin, radio.bin, resources.ths, splash.bin}` | official 1.5.1 package (sha256 `44d0045d…fa52`) |
| `update.fuf` is a Flipper Format text file, `Version: 2`, `Target: 7`, with `Loader`, `Loader CRC`, `Firmware`, `Radio`, `Radio address`, `Radio version`, `Radio CRC`, `Resources`, `OB reference/mask/write mask`, `Splashscreen` | same |
| `firmware.dfu`: DfuSe v1, **1 target** ("Flipper Zero F7"), **1 element at `0x08000000`, 771 284 bytes** (ends `0x080BC4D4`); initial SP `0x20030000`, reset vector `0x08011651` | parsed with a throw-away script |
| `updater.bin` is linked to **run from SRAM1** (SP `0x20030000`, vectors at `0x2000xxxx`) | vector table of `updater.bin` |
| Radio stack in 1.5.1 is placed at **`0x080D7000`** (from `Radio address`), not "around `0x080C0000`" | `update.fuf` |
| Resources are **`resources.ths`** (heatshrink-compressed tar; verify framing in the firmware's update tooling) in Official and Unleashed, but **`resources.tar.gz`** in Momentum and RogueMaster, which also ship `firstboot.bin` | the four pinned packages (ADR-0005) |
| All four pinned packages: one DFU element at `0x08000000`, `Radio address` `0x080D7000`; Momentum's image ends at `0x080D6164`, 3.7 KiB below the CPU2 region | same |

### 1.1 Corrections to the brief (rule 6)

1. **CPU2 region is not fixed.** The wireless-stack base depends on the stack size and comes from
   `Radio address` in `update.fuf` (1.5.1: `0x080D7000`) and from the option bytes (`SFSA`).
   The loader must not hard-code `0x080C0000`; the flash model protects everything from `SFSA`
   upwards, with `SFSA` taken from the OB reference in `update.fuf` (verify the bit field in RM0434 §3.10) or a default per profile.
2. **The resources archive format varies.** `.ths` (heatshrink + tar) or `.tar.gz`, as named by
   `Resources:` in `update.fuf`. The loader needs a heatshrink decoder (ISC, GPL-compatible), gzip and tar.
3. **Real OTA mode runs `updater.bin` from RAM.** The CPU must support executing from SRAM with
   self-modifying-code safety (decode-cache invalidation on RAM writes), and the flash controller
   must model option bytes enough for the updater's OB check (it compares OB against `OB reference/mask`).
4. **Unicorn → WASM is not an upstream option.** Unicorn 2.1.4 has no TCG interpreter, so it cannot
   target WASM without unofficial forks; it is declared GPLv2 without "or later", so it cannot be
   shipped in our GPL-3.0-or-later bundle. It remains a native, dev-only differential-test reference (ADR-0001).

---

## 2. System overview

```
┌──────────────────────────── Browser tab ────────────────────────────┐
│ UI thread (Preact)                                                   │
│  device SVG + LCD canvas · panels · toolbar · i18n · embed bridge    │
│        ▲ frames / events / status        │ input / commands          │
│        │  (postMessage, transferables)   ▼                           │
│ Web Worker ───────────────────────────────────────────────────────── │
│  host-glue (TS): slice scheduler, throttle/turbo, OPFS I/O, fetch    │
│        │ wasm-bindgen API                                            │
│  dryflash-core (Rust → WASM)                                         │
│   cpu · bus · mem · periph(stm32wb55) · board(f7) · sched · trace    │
│   crash · loader                                                     │
└──────────────────────────────────────────────────────────────────────┘
Node runner (CLI / GitHub Action) ── same core-wasm, file-system storage
Cloudflare Worker /proxy ── allowlisted fetch for hosts without CORS
```

The core is a pure, deterministic library: no clocks, no I/O, no threads. Everything that touches
the outside world (time, storage, network, audio) lives in the host glue, which differs between
browser worker and Node runner but drives the same `Machine` API.

---

## 3. Rust core (`/core`)

### 3.1 Crate layout

| Module | Responsibility | Status |
|---|---|---|
| `cpu/` | ARMv7E-M interpreter: `decode`, `exec`, `dsp`, `fp` (ARM-exact FP, ADR-0008), `exception` (B1.5), `decode_cache` | M1 |
| `sysctl/` | Internal PPB owned by the CPU: SCB, NVIC, SysTick, `mpu` (PMSAv7), FPU control, DWT, CoreDebug (ADR-0009) | M1 |
| `bus/` | `SystemBus`: memories, flash boot alias, `map` (blocks from the CMSIS header); unmodelled blocks read as zero and are reported | M1 (peripheral dispatch M2) |
| `trace/digest` | Bring-up digest: unimplemented registers by block/offset, unmapped accesses, events | M1 |
| `loader/` | `dfu` (DfuSe), `elf` (symbols only); `.tgz` + `update.fuf`, heatshrink/tar/gzip in M4 | M1 / M4 |
| `machine` | `Machine`: CPU + bus + decode cache; run loop with WFI fast-forward and SYSRESETREQ | M1 |
| `periph/` | Register files for every SVD peripheral (`svd.rs` generated, `regfile`); behavioural models: RCC, GPIO, EXTI, I2C, SPI, USART/LPUART, RTC, HSEM, IPCC, RNG, LPTIM; NVIC interrupt lines; virtual time base (ns) and peripheral event scheduling | M2 (more in M3–M7) |
| `board/` | f7 devices: BQ27220, BQ25896, OTP board data (M2); ST7567, buttons, LP5562 (M3); SD (M4); CC1101, ST25R3916, … | M2–M8 |
| `fs/` | FAT32 image creation and file injection | M4 |
| `crash` | Furi crash record (RTC BKP4R) decoding (M2); fault capture, hang detection, report model (M6) | M2 / M6 |
| `snapshot/` | Machine state save/restore | later |

Every peripheral and device implements a common trait:

```rust
trait Device {
    fn read(&mut self, offset: u32, size: AccessSize, ctx: &mut Ctx) -> u32;
    fn write(&mut self, offset: u32, value: u32, size: AccessSize, ctx: &mut Ctx);
    fn fidelity(&self) -> Fidelity;           // Emulated | Partial | Stubbed + description
}
```

`Ctx` gives access to the scheduler, IRQ lines, DMA requests, GPIO nets and the trace sink.
Registers that a device does not implement are routed through a shared "unimplemented" handler
that returns a documented reset value and records `(peripheral, offset, r/w, count, first PC)`
for the bring-up digest. This is how the digest drives peripheral work (BRIEF §8).

### 3.2 CPU engine

**Custom Rust interpreter** (ADR-0001): verified instruction-by-instruction against Unicorn on the
benchmark, 153–165 MIPS in Chrome/Safari Web Workers (≥ 2.2× real time at 100 % CPU load).
The PPB (SCB, NVIC, SysTick, DWT, MPU, FPU control at `0xE000_E000`) is handled by the core
(`sysctl/`), not by the system bus.

- Decode once, execute many: the decode cache stores a compact `Op` enum per halfword address.
  Flash is mostly immutable, so the hit rate is near 100 %; RAM code (the updater) is handled by
  page generation counters.
- Thumb-16, Thumb-32, DSP (SIMD, saturating, `SMLAxy`…), FPv4-SP (single precision, VFP register
  file, FPSCR, lazy stacking), IT blocks, `LDREX/STREX` with a local monitor cleared on exception entry.
- Exceptions: priority-based preemption and nesting, EXC_RETURN handling for MSP/PSP and FP frames,
  fault escalation and lockup. Tail-chaining is done as return + re-entry (same architectural state).
  Run stops (`Stop`): budget, sleep (WFI/WFE with nothing pending), reset request, lockup,
  watchpoint, unimplemented encoding, BKPT when acting as a halting debugger.
- Cycle model: deterministic approximate cycle counts per instruction class (good enough for
  DWT-based delays and timer rates; not cycle-exact).
- Hooks: PC breakpoints (sorted set checked only when non-empty), symbol breakpoints (`furi_crash`)
  resolved from the ELF, single-step.

### 3.3 Timing and execution loop

- Virtual time in CPU cycles at 64 MHz (`u64`). Peripherals schedule events on the timeline;
  the CPU runs until the next event or the slice budget.
- **WFI fast-forward**: when the CPU executes `WFI`/`WFE` with nothing pending, jump directly to
  the next timeline event. FreeRTOS tickless idle (verify `configUSE_TICKLESS_IDLE` in the f7 target)
  makes this the main performance lever.
- `Machine::run(budget_cycles) -> RunOutcome { cycles, idle_cycles, events, stop_reason }`.
  The host calls it in slices (~4–8 ms wall time), throttled to 1× real time, or unthrottled in Turbo.
- Determinism: no host time reads in the core; RNG seeded from config; input is timestamped in
  virtual time by the host before injection. Same firmware + script + seed ⇒ identical trace hash
  (tested in CI).

### 3.4 Flash, OTP and option bytes

M2 status: OTP is provisioned with board data (`board/otp.rs`, `FuriHalVersionOTPv2` layout).
The FLASH controller is still a register file: program/erase and option bytes move to M4 because
in firmware 1.5.1 only the updater (and the app API) programs the internal flash — settings live on
the SD card (correction to BRIEF §5.1, rule 6).

- Flash controller model implements unlock keys, page erase (4 KiB pages — verify RM0434),
  double-word programming, busy/EOP flags, error flags, and **write protection from `SFSA`**
  so CPU1 cannot overwrite the (absent) CPU2 stack region.
- Writes persist to the device profile's flash image (dirty-page tracking; host flushes pages).
- OTP is provisioned from the virtual-device settings using the exact layout of
  `furi_hal_version` (`targets/f7/furi_hal/furi_hal_version.c` — verify struct, magic and version).
- Option bytes: initialised from the `OB reference` of the installed package, readable through
  FLASH registers, writable only through the documented sequence (needed for Real OTA).

---

## 4. Firmware loading

| Input | Handling |
|---|---|
| `.dfu` | Parse DfuSe prefix/targets/elements/suffix, validate suffix CRC32, place elements. Reject elements that overlap the protected CPU2 region. |
| `.tgz` | gunzip + tar (in Rust, `flate2` with the `miniz_oxide` backend + `tar`), parse `update.fuf`, verify `Loader CRC`, load `firmware.dfu`, decompress `resources.ths` → write into the SD image under `/ext`, record `radio.bin` metadata (version, address, CRC) and **do not execute it**. |
| `.bin` | Raw image at a user-provided base address (advanced). |
| `.elf` | Symbols + DWARF line tables for crash symbolication only. |

**Quick install**: write the DFU elements into flash, provision OTP/OB, unpack resources into the SD
image, reset. **Real OTA install** (M4+): copy the package to `/ext/update/<package>/` on the SD
image, set the RTC backup-register boot mode that requests an update (verify in
`furi_hal_rtc` / `update_operation`) and reset, so the firmware's own updater does the job.

Fetching by URL happens in the host glue: direct `fetch` first; if it fails with a CORS/network error
and the host is in the allowlist, retry through `/proxy`. Local drag & drop never touches the network.

---

## 5. CPU2

**Decision (ADR-0012, supersedes ADR-0011): CPU2 is a behavioural FUS without a wireless stack.**
CPU2's code (ST's encrypted FUS and stack) is not executed. A model in `core/src/bus/cpu2.rs`
answers the mailbox the way a device whose wireless stack was removed does:

1. On `PWR_CR4.C2BOOT` it writes the FUS device-info table (validity keyword, FUS 1.2.0, stack
   version 0) where the reference table points and posts the `SHCI_SUB_EVT_CODE_READY` system
   event with `FUS_FW_RUNNING`. The firmware logs "C2 boot completed, mode: FUS" and
   "Stack isn't installed!"; Bluetooth stays off, `furi_hal_bt_is_alive()` is true.
2. System commands are answered synchronously (CPU1 spins on the IPCC flag): `FUS_GET_STATE` →
   idle; every other command fails (status 0x01) — no factory keys ("No Factory Keys Found" popup),
   no user key storage, no `SetSystemClock`.
3. Released event buffers (IPCC channel 4) are taken back at once.
4. The virtual device ships with Settings → System → Sleep Method = **Legacy** (RTC backup
   register), because the default STOP2 path needs the stack's `SetSystemClock` and otherwise
   spins without WFI.

Why not "no CPU2": with CPU2 dead, `furi_hal_crypto_enclave_load_key` leaves without releasing
its mutex and `desktop_srv` deadlocks on the next key slot (BRINGUP_LOG M3).

---

## 6. Board devices

Wiring (pins, SPI/I2C buses, chip-selects, IRQ lines) is taken from
`targets/f7/furi_hal/furi_hal_resources.c`, `furi_hal_spi_config.c`, `furi_hal_i2c_config.c` of
the pinned tag and cited in each device file. Every device declares its fidelity.
Initial fidelity targets:

| Device | Target fidelity | Milestone |
|---|---|---|
| ST7567 LCD | Emulated (command set used by `u8g2`/firmware, 1-bit framebuffer, contrast, inversion) | M3 |
| Buttons (Up/Down/Left/Right/OK/Back) | Emulated (active levels per `furi_hal_resources`) | M3 |
| LP5562 LED + backlight | Emulated | M3 |
| BQ27220 / BQ25896 | Partial (plausible register set, UI controls) | M2 init, M7 UI |
| microSD (SPI mode) | Emulated (CMD0/8/55/41/58/9/10/12/13/17/18/24/25, CRC optional) | M4 |
| CC1101 | Stubbed (register file + strobes, no RX signal) → Partial in M8 | M2 init |
| ST25R3916 | Stubbed (init + "no tag") → Partial in M8 | M2 init |
| RFID 125 kHz, iButton | Stubbed ("no tag", "no key") | M2 init |
| IR RX/TX | Partial (TX decoded to event log) | M7 |
| Speaker (TIM16 PWM) | Partial (frequency/duty → WebAudio) | M7 |
| Vibro, GPIO header | Emulated (pin state) | M7 |
| USB FS device (CDC) | Partial (enough for CLI) | M7 |

External SPI flash: none expected on f7 — verify in `furi_hal_resources.c`.

---

## 7. Crash, hang and fault detection

Implemented in `crash/`, fed by CPU and peripheral hooks:

- **Faults**: on HardFault/MemManage/BusFault/UsageFault entry, capture R0–R12, MSP, PSP, LR, PC,
  xPSR, CFSR, HFSR, MMFAR, BFAR and the stacked frame. Decide "firmware fault" vs "emulator gap"
  (e.g. a BusFault caused by an unmapped peripheral we don't model is an emulator gap).
- **`furi_crash`/`furi_check`**: ELF symbol breakpoint when available; otherwise detect the firmware's
  own crash path (crash message pointer + reason saved in RTC backup registers before reset — verify
  in `furi/core/check.c` and `furi_hal_rtc`) and decode the message from flash.
- **Resets**: IWDG, `SYSRESETREQ`, lockup. **Boot loop** = N resets within T virtual seconds (configurable).
- **Hangs**: not in WFI and no LCD change / no progress for a configurable virtual time, or a tight
  loop detected by the PC ring buffer.
- **Emulator gaps**: unmapped accesses and unimplemented registers are reported separately.

The report model serialises to JSON and Markdown; the UI adds the last LCD frame as PNG.

---

## 8. Host glue and worker protocol (`/core-wasm`, `/web/src/worker`)

`core-wasm` exposes a small API: `Machine::new(config)`, `load_*`, `run(budget)`, `press/release`,
`frame()` (128×64 → 1 KiB packed), `drain_events()`, `digest()`, `crash_report()`,
`flash_dirty_pages()`, `sd_read_block / sd_write_block` callbacks, debug accessors.

Worker ↔ UI messages (versioned, TypeScript discriminated unions):

- UI → worker: `init`, `loadFirmware {bytes|url, mode}`, `input {button, down}`, `control
  {pause|resume|reset|turbo}`, `setDeviceSettings`, `debug {...}`.
- Worker → UI: `status`, `frame {ArrayBuffer}` (only on change), `events` (LED, vibro, IR, GPIO,
  audio params), `bootMilestone {booted|desktopReached}`, `crash {report}`, `digest`, `progress`.

Storage: OPFS per virtual device at `/devices/<id>/{flash.bin, sd.img, ob.bin, device.json}`.
The worker uses `FileSystemSyncAccessHandle` for SD block I/O (synchronous, worker-only);
IndexedDB fallback keeps a sparse block map in memory and flushes dirty blocks between slices.
SD images are created sparse (default size configurable, e.g. 256 MiB — **ADR**).

No SharedArrayBuffer anywhere.

---

## 9. Web app (`/web`)

- Vite + TypeScript + **Preact** (ADR-0004).
- Routes: `/`, `/embed`, `/compat`, `/about` (static, hash- or path-based depending on host — ADR).
- Device illustration: original SVG shell (no Flipper Devices artwork), LCD canvas with integer scaling,
  backlight tone driven by LP5562 state.
- Panels: Firmware, Virtual device, SD file manager, Event log, Hardware fidelity, Crash report,
  Debug (developer mode). Bottom sheets on mobile.
- Recording: screenshot (PNG 4×), GIF/WebM (MediaRecorder on a canvas stream; GIF via a small encoder in a worker).
- i18n: `en` default, `it`; all strings in locale files.
- Embed API over `postMessage` with origin-aware messaging; documented in `docs/EMBED.md`.
- WASM lazy-loaded after first paint with a progress bar; brotli-precompressed assets where supported.

---

## 10. Headless runner (`/runner`) and GitHub Action (`/action`)

- `dryflash run <fw> [--elf] [--script smoke.yaml] [--timeout] [--junit] [--screenshots]`.
- Same `core-wasm` build loaded in Node; storage on the local file system (temp dir per run).
- YAML script engine: `wait_for_desktop`, `press`, `long_press`, `wait`, `expect_screen`
  (pixel-exact or tolerance), `expect_no_crash`. Built-in default smoke test.
- "Desktop reached" detection: **ADR** — candidates are (1) ELF symbol hit in the desktop service,
  (2) a golden-frame match, (3) a firmware-independent heuristic (stable frame after boot + menu
  opens on OK). No hard-coded per-firmware screens in the emulator itself.
- Exit codes 0/1/2/3 as in the brief. The Action is a composite action calling the runner.

---

## 11. CORS proxy (`/proxy`)

Required: GitHub release assets have no CORS headers (verified, ADR-0002). Hosting: ADR-0003. Cloudflare Worker with:
a single route `GET /fetch?url=…`; strict allowlist (`github.com/<owner>/<repo>/releases/download/…`
for configured repos + `release-assets.githubusercontent.com` / `objects.githubusercontent.com`
redirect targets + configured extra hosts); redirect following only within the allowlist; 64 MiB cap;
edge cache keyed by URL; per-IP rate limit; adds `Access-Control-Allow-Origin` for our origins only.
Not an open proxy. The official host doesn't need it.

---

## 12. Testing strategy

| Level | What | Tool |
|---|---|---|
| CPU unit | Per-encoding tests (Thumb-16/32, DSP, FPU, IT, exceptions, LDREX/STREX, unaligned) | `cargo test` |
| CPU differential | Random instruction sequences vs native Unicorn (dev-dependency only, not shipped) | `cargo test --features diff-fuzz` |
| Peripheral | Register behaviour from RM0434 | `cargo test` |
| Loader | Real packages fetched by CI from `firmware/compat.json` (hash-checked, cached, never committed) | `cargo test` |
| Integration | Boot pinned official release to Desktop; golden screenshots of Desktop + main menu; determinism hash | runner |
| Frontend | Components, worker protocol | Vitest |
| E2E | File/URL load, navigation, crash path, persistence, embed API | Playwright |

CI (GitHub Actions): build core + wasm + web, tests, nightly compatibility matrix → `/compat` data.

---

## 13. Risks

| Risk | Mitigation |
|---|---|
| Interpreter too slow in Safari / mobile | Decode cache, WFI fast-forward, measure in the spike; snapshots to skip boot |
| CPU2 stub insufficient for BT init | Prefer graceful "BT unavailable" path; prior-art bring-up log |
| Custom firmwares touch peripherals the official one doesn't | Bring-up digest per firmware in the nightly matrix |
| Licence contamination | Only GPL-compatible deps; Unicorn only as non-distributed test tool; `cargo deny` licence check in CI |
| Trademark | Nominative wording only; original artwork; footer disclaimer |

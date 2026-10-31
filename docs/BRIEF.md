# PROMPT FOR CLAUDE CODE — dryflash: browser-based Flipper Zero emulator

> Paste everything below this line into a **new** Claude Code session opened in the cloned `dryflash-FZ` repository (it may contain only `LICENSE` and `README.md`).

---

## 0. Role and working mode

You are a senior embedded-systems and emulation engineer (ARM Cortex-M, STM32, Rust → WebAssembly) and a senior frontend engineer. You will build, from scratch, **dryflash** — an **open-source Flipper Zero emulator that runs entirely in the browser**, with no backend and no hardware. GitHub repository: `dryflash-FZ`.

Working rules — follow them for the whole project:

0. **Persist this brief first.** Save this entire prompt verbatim as `docs/BRIEF.md`, then create a concise `CLAUDE.md` at the repo root containing: the project one-liner, the working rules of this §0, the stack summary (§3), the repo layout (§15), the licence rule (§3) and the instruction "at the start of every session read `docs/BRIEF.md`, `docs/ROADMAP.md` and `docs/BRINGUP_LOG.md`, then continue from the next unfinished milestone". Future sessions will rely on these files, not on chat history.
1. **Start in plan mode.** Before writing code, read this whole brief, then produce `docs/ARCHITECTURE.md` and `docs/ROADMAP.md` and ask me the open questions listed in §14. Do not start Milestone 1 until I approve the plan.
2. **Work in milestones (§12).** Each milestone ends with: passing tests, an updated `docs/BRINGUP_LOG.md`, a short demo instruction, and a commit. Stop and report at the end of each milestone.
3. **Never fake success.** If a peripheral is stubbed, it must be declared as stubbed in code, in the bring-up digest, and in the UI ("Simulated" / "Stubbed" / "Not emulated"). No hard-coded screens, no pre-rendered frames, no "pretend boot".
4. **The official firmware source is the ground truth** for memory map, pins, buses and device wiring. Never guess a pin, bus, address or register: look it up in the pinned firmware tag and in the ST reference manual, and cite the file/section in a code comment.
5. **Record every non-obvious decision** as an ADR in `docs/adr/NNNN-title.md`.
6. If something in this brief turns out to be technically wrong once you inspect the real firmware, tell me, propose the fix, and update the docs. Don't silently work around it.

---

## 1. Product vision

**Name:** **dryflash** (always lowercase in code, CLI and npm package). GitHub repository: `dryflash-FZ`. The name comes from "dry run" + "flash": try a firmware *before* you flash it.
Never brand the product as "Flipper …" (trademark of Flipper Devices Inc.). Use nominative wording only: "dryflash — an emulator for Flipper Zero firmware". GitHub topics: `flipper-zero`, `emulator`, `stm32`, `stm32wb55`, `webassembly`, `rust`, `firmware`.

**One-liner:** "Try any Flipper Zero firmware in your browser before flashing it to your real device."

**Primary users:**
- People who don't own a Flipper Zero and want to explore firmwares and apps.
- Owners who want to try a custom firmware (Official, Momentum, Unleashed, RogueMaster, etc.) before flashing it.
- Firmware and app developers who want a **smoke test** that their build boots, navigates and doesn't crash — interactively and in CI.

**Core user flow (the MVP must do exactly this):**
1. User opens the emulator (directly, or from the Firmware Hub via a "Try in emulator" button: `<HUB_URL>` = https://vincenzo-esposit0.github.io/flipper-zero-firmware-hub/).
2. User loads a firmware: drag & drop a local file, OR the URL already carries it (`?fw=<url>`).
3. The emulator "installs" it on a virtual device, boots it, and shows the real firmware's own LCD output.
4. User navigates with keyboard / on-screen buttons / touch.
5. If the firmware crashes, hangs, boot-loops or faults, the emulator stops, shows a clear crash report, and offers to download it.

**Honest positioning (must appear in the UI and README):** this is a *smoke test*, not a hardware certification. It catches boot failures, crashes, faults, hangs and broken UI flows. It does not reproduce real RF, real NFC/RFID physics, BLE, power behaviour or exact timing. Passing in the emulator ≠ guaranteed to work on hardware.

---

## 2. Hard technical requirement: run the real, unmodified binaries

The emulator must execute the **real released binaries** (the same files a user would flash with qFlipper / Flipper Lab), not a recompiled "host build". This is the whole point: users test the exact artifact they would install.

Therefore the architecture is an **instruction-level emulation of the STM32WB55RG** (Flipper Zero hardware target `f7`):
- CPU1: ARM Cortex-M4F, ARMv7E-M, Thumb/Thumb-2, DSP extensions, FPv4-SP single-precision FPU, 64 MHz.
- CPU2: Cortex-M0+ running ST's proprietary FUS + wireless stack (encrypted, closed). **CPU2 is NOT emulated** — it is replaced by a behavioural stub (see §5.3).

Out of scope: compiling firmware to WASM from source (other projects do that; it doesn't test the user's binary).

---

## 3. Technology stack

| Layer | Choice | Notes |
|---|---|---|
| Emulator core | **Rust**, compiled to `wasm32-unknown-unknown` with `wasm-bindgen` | No `std` dependencies that block WASM; deterministic; no threads required |
| Runtime host | **Web Worker** running the WASM core | UI thread never blocks |
| Worker ↔ UI | `postMessage` with transferable `ArrayBuffer`s | **Do not require SharedArrayBuffer** (see §9 — must run on static hosts without COOP/COEP headers) |
| Frontend | **Vite + TypeScript + Preact** (or Svelte — propose in ADR) | Small bundle; static output |
| Terminal | xterm.js | For the emulated USB-CDC CLI (Milestone 7) |
| Storage | OPFS (Origin Private File System), IndexedDB fallback | Flash image + SD card image per virtual device |
| Headless runner | Node.js CLI reusing the same WASM core | For CI (§10) |
| Tests | `cargo test` (core), Vitest (frontend), Playwright (E2E) | |
| License | **GPL-3.0-or-later** — the `LICENSE` file is already in the repo, do not replace it | Firmware is GPL-3.0; also allows learning from existing GPL emulators (§13). Put `SPDX-License-Identifier: GPL-3.0-or-later` in a header comment of every source file; set `license = "GPL-3.0-or-later"` in Cargo.toml and package.json |

**Phase-0 spike (mandatory, write the result as ADR-0001):** compare
(a) a custom Rust ARMv7E-M interpreter, versus
(b) Unicorn Engine 2 compiled with Emscripten (TCG JIT is not available in WASM, so it would run in interpreter mode).
Criteria: correctness on Thumb-2 + FPU, speed (MIPS in Chrome and Safari), WASM size, debuggability, determinism, licence. My expectation is (a) wins; prove it with a micro-benchmark, don't assume.

---

## 4. Firmware input formats

Implement a `firmware-loader` module (Rust, unit-tested with real files):

1. **`.dfu`** — DfuSe container. Parse all targets/elements, validate suffix + CRC, place elements at their addresses. The application lives at `0x08000000`; the top region of flash (around `0x080C0000` and above — verify) is reserved for CPU2's FUS/wireless stack and must be left untouched. Verify layout byte-for-byte against the official package of the pinned release.
2. **`.tgz` update package** (the format most custom firmwares distribute). Parse the manifest (`update.fuf` or equivalent — inspect the real package), extract the firmware `.dfu`, the resources archive (unpack to the virtual SD at `/ext`), and ignore/record the radio stack binary.
3. **Raw `.bin`** at a user-provided base address (advanced option).
4. **Optional `.elf`** from the same release, used only for symbolication of crash reports (function names + source lines), never for execution.

**Two install modes in the UI:**
- **Quick install** (default): write the DFU directly into the virtual flash and boot.
- **Real OTA install** (advanced, Milestone 4+): copy the `.tgz` content into `/ext/update/<package>/` on the virtual SD and let the firmware's **own updater** perform the update, exactly like a real device. This tests the update package itself, which is a very valuable check for firmware maintainers.

**Firmware compatibility targets** (pin exact versions in `firmware/compat.json`, fetch in CI, never commit binaries to the repo):
Official (pinned release + latest), Momentum, Unleashed, RogueMaster. Any firmware for target `f7` should be loadable.

---

## 5. Emulated hardware

### 5.1 Memory map (verify every value against RM0434 and the firmware linker scripts)
- Flash 1 MiB @ `0x08000000` — with a **flash controller model** supporting program/erase so the firmware's own settings, storage and OTA writes persist.
- SRAM1 @ `0x20000000`, SRAM2a/SRAM2b @ `0x20030000`/`0x20038000` (sizes per RM0434).
- **OTP area** @ `0x1FFF7000`: must be **provisioned with valid board data** (hardware version, target `f7`, body colour, region, device name). Real firmware checks this; dev boards without it fail. Expose these as "virtual device settings" in the UI with sensible defaults. Derive the exact OTP format from `furi_hal_version` in the firmware source.
- Peripheral space, System Memory (ROM bootloader) region stubbed as needed.

### 5.2 Core and system peripherals (CPU1)
NVIC (with priorities, tail-chaining, late arrival as far as the firmware needs), SCB (VTOR, fault status registers, SYSRESETREQ), SysTick, **DWT CYCCNT** (Furi uses it for delays), MPU (enough to not break firmware; enforce if the firmware relies on it for stack guards), FPU with lazy stacking, `WFI`/`WFE`/`SEV`.

### 5.3 STM32WB55 peripherals
Model at register level, only as deep as the firmware actually uses (drive this by the bring-up digest, §8):
RCC (clock-ready flags, clock tree enough for timers to compute correct rates), PWR, FLASH, **HSEM**, **IPCC**, GPIOA–H, EXTI, SPI1/SPI2, I2C1/I2C3, DMA1/DMA2 + DMAMUX, TIM1/TIM2/TIM16/TIM17, LPTIM1/LPTIM2, RTC (+ backup registers, which survive soft reset), IWDG, RNG, CRC, ADC (battery/temperature reads), COMP (used by RFID), USART1/LPUART1, USB FS device, AES1/AES2/PKA (stub or implement as the firmware requires).

**CPU2 behavioural stub (critical):** emulate the IPCC mailbox and the shared-SRAM2 structures CPU2 normally fills (FUS/wireless-stack info tables, version info, system-ready events) so CPU1 believes a radio stack is present and either boots BT normally with the radio "idle", or takes its graceful "BT unavailable" path — whichever the firmware supports without crashing. Investigate what the firmware reads (`furi_hal_bt`, the `ble_glue` / shci code in the firmware source) and synthesise exactly that. Secure-enclave crypto requests routed through CPU2 must get well-formed responses (decide in an ADR: emulate key storage, or return a defined failure the firmware handles).

### 5.4 Board-level devices (wired exactly as in `targets/f7/furi_hal/` of the pinned firmware tag — `furi_hal_resources.c`, `furi_hal_spi_config.c`, `furi_hal_i2c_config.c` and friends)
| Device | Interface | Required behaviour |
|---|---|---|
| ST7567 128×64 mono LCD | SPI | Full command set used by firmware; produce a 1-bit framebuffer; contrast/inversion |
| 5 buttons + Back | GPIO + EXTI | Real active levels (check which are active-low), debouncing left to firmware |
| LP5562 RGB LED + backlight driver | I2C | LED colour & backlight level shown in UI |
| BQ27220 fuel gauge | I2C | Plausible battery data; UI slider to set charge % and charging state |
| BQ25896 charger | I2C | Plausible status registers; "USB connected" toggle |
| Speaker | TIM16 PWM | Convert to audio via WebAudio (Milestone 7); mute by default |
| Vibro motor | GPIO | Visual shake + `navigator.vibrate` on mobile |
| microSD card | SPI mode | Full SD SPI protocol (CMD0/8/55/41/58/17/24/…); backed by a FAT32 image in OPFS |
| CC1101 Sub-GHz | SPI | Register file + strobes so drivers init; RX returns no signal by default |
| ST25R3916 NFC | SPI | Enough for init and "no tag" field polling |
| RFID 125 kHz | COMP + TIM | Init path; "no tag" |
| IR receiver / IR LED | GPIO/TIM | Init; IR TX decoded and shown in an event log |
| iButton | GPIO 1-Wire | Init; "no key" |
| GPIO header | GPIO | Visual pin-state panel (input/output, level) |
| External SPI flash (if present on target) | SPI | Verify in firmware source; model if used |

**Status registry:** every device declares `Emulated | Partial | Stubbed` plus a one-line description. The UI shows this table ("Hardware fidelity").

---

## 6. Execution engine and timing

- Interpreter loop with **virtual time** driven by cycle counts; peripherals schedule events on a min-heap timeline.
- **WFI fast-forward:** when the CPU is in WFI/idle and no interrupt is pending, jump virtual time to the next scheduled event. Furi/FreeRTOS idles most of the time; this is the main performance lever.
- Throttle virtual time to wall-clock for interactive use (1× real-time), with a "Turbo" toggle (unthrottled) used for boot and headless tests.
- Run in slices (e.g. ~4–8 ms of wall time per slice) inside the worker; between slices, flush LCD frames (only on change) and drain input/events.
- Determinism: same firmware + same input script + same seed ⇒ identical execution. RNG peripheral seeded from a configurable seed.
- Performance targets (measure and record in BRINGUP_LOG): boot to Desktop in < 10 s on a mid-range laptop in Chrome; UI responsive at ≥ 30 LCD fps; usable on recent Safari/Firefox; acceptable on a recent mobile phone.

---

## 7. Crash, hang and fault detection (the "does it brick?" feature)

Detect and report, with timestamps in virtual time:
- Entry into HardFault / MemManage / BusFault / UsageFault: capture R0–R12, SP (MSP/PSP), LR, PC, xPSR, CFSR, HFSR, MMFAR, BFAR, and the stacked exception frame.
- Firmware-level crashes (`furi_crash` / `furi_check` failures): detect via ELF symbol breakpoint when an ELF is provided; otherwise via the firmware's own crash path (reset with crash data left in RTC backup registers — inspect the firmware to find out exactly how it records it) and decode the message if possible.
- Watchdog resets (IWDG), `SYSRESETREQ`, and **boot loops** (N resets within T seconds).
- **Hangs:** no LCD change and no input-driven progress for a configurable time while not in WFI, or stuck in a tight loop.
- Unmapped memory accesses and accesses to unimplemented registers (these are emulator gaps, report them separately from firmware bugs).

**Crash report** (UI panel + downloadable JSON + Markdown): firmware name/version/hash, what happened, registers, symbolised backtrace (if ELF), last 64 executed PCs (ring buffer), last LCD frame as PNG, input history, list of stubbed devices touched just before the crash. Must clearly label whether the cause looks like a **firmware bug** or an **emulator limitation**.

---

## 8. Bring-up tooling (build this early — it drives everything else)

- **Bring-up digest** printed after every run: unmapped accesses (count + address), reads/writes to unimplemented registers grouped by peripheral and offset, resets and their causes, faults, time to first LCD frame, time to Desktop.
- Debug panel in the UI (developer mode): registers, disassembly around PC, memory viewer, peripheral register viewer, breakpoints on PC, single-step, trace on/off.
- Optional GDB remote-serial-protocol stub (stretch goal) so developers can attach `arm-none-eabi-gdb` via the Node runner.

---

## 9. Web app (UX/UI)

**Pages/routes**
- `/` — emulator.
- `/embed` — chrome-less emulator for iframes.
- `/compat` — compatibility matrix (firmware × version × status: Boots / Reaches Desktop / Apps launch / Crash), generated by CI.
- `/about` — how it works, hardware fidelity table, disclaimer, credits, licences.

**Main screen layout**
- Centre: an **original device illustration** (SVG) containing the LCD canvas and the buttons. Do NOT copy Flipper Devices' industrial design, artwork or mascot; design an original, clearly distinct shell that still makes the 128×64 screen + D-pad + OK + Back layout instantly understandable.
- LCD: crisp pixel scaling (integer scale, `image-rendering: pixelated`), authentic orange backlight tone, backlight level from the LP5562 model, optional "LCD ghosting" effect toggle.
- Left/right panels (collapsible; bottom sheet on mobile): Firmware (load, info, install mode), Virtual device (OTP settings, battery, USB), SD card file manager (browse, upload `.sub/.nfc/.ir/.fap/...`, download, format), Event log (IR TX, LED, vibro, GPIO), Hardware fidelity, Crash report.
- Toolbar: Power/Reset, Reboot to DFU (if modelled), Pause/Resume, Turbo, Screenshot (PNG at 4× scale), **Record GIF/WebM** (for sharing on social — important for community growth), Mute, Fullscreen.

**Input**
- Keyboard: arrows = D-pad, Enter/Space = OK, Esc/Backspace = Back; long-press supported by holding.
- Mouse/touch on the illustrated buttons, with press/long-press.
- Gamepad API (stretch).

**Loading firmware by URL**
- `?fw=<url>&elf=<url>&mode=quick|ota&autoboot=1&device=<profile>`.
- **CORS:** many release hosts (notably GitHub release assets) may not send CORS headers, so a direct `fetch` from the browser can fail. In the Phase-0 spike, verify this for each target firmware host. If needed, implement a tiny **Cloudflare Worker proxy** in `/proxy` with: strict domain allowlist (github.com release assets + configured hosts), max size (e.g. 64 MB), caching, rate limiting, no other routes. Never build an open proxy. Local file drag & drop must always work without the proxy.

**Hosting**
- Pure static build deployable on GitHub Pages or Cloudflare Pages. Must not need COOP/COEP headers (no SharedArrayBuffer requirement).
- WASM served compressed (brotli where host supports it), lazy-loaded after first paint; show a progress bar.

**Persistence**
- Multiple "virtual devices" (profiles), each with its own flash image + SD image + OTP settings, stored in OPFS. Export/import a device as a single file. "Factory reset" per device.

**Embed API (for the Firmware Hub and third parties)**
- `/embed?fw=...` in an iframe.
- `postMessage` events out: `ready`, `booted`, `desktopReached`, `crashed` (with summary), `frame` (optional, throttled). Commands in: `load`, `press`, `reset`, `screenshot`. Document in `docs/EMBED.md` with a copy-paste snippet.
- A "**Try in browser**" README badge (SVG) + markdown snippet that firmware/app maintainers can add to their repos, linking to `/?fw=<their release url>`.

**i18n:** English default, Italian included; all strings in locale files.

**Accessibility:** keyboard-only usable, focus states, ARIA labels on buttons, reduced-motion respected.

**Visual style:** dark UI (background near `#050505`, consistent with the Firmware Hub), monospace accent font, one accent colour. Clean, technical, not "hacker cliché".

---

## 10. Headless runner and CI integration (adoption driver for developers)

- `npx dryflash run <firmware.tgz|.dfu> [--elf f.elf] [--script smoke.yaml] [--timeout 60] [--junit out.xml] [--screenshots dir/]`
- Input script format (YAML): steps like `wait_for_desktop`, `press: [down, ok]`, `long_press: back`, `wait: 2s`, `expect_screen: golden.png` (pixel-exact or tolerance), `expect_no_crash`.
- Built-in **default smoke test**: boot → reach Desktop → open main menu → open each top-level app category → back out → no crash/hang.
- Exit codes: 0 pass, 1 firmware failure, 2 emulator limitation hit, 3 usage error.
- Publish a **GitHub Action** in `/action` (usable as `uses: vincenzo-esposit0/dryflash-FZ/action@v1`) that runs the smoke test on a firmware build artifact and comments the result + screenshots on PRs.
- Our own CI: build core + web, run unit/E2E tests, run the compatibility matrix nightly against the pinned + latest releases, publish `/compat`.

---

## 11. Quality, testing and safety

- **CPU correctness:** per-instruction unit tests for every implemented encoding (Thumb-16, Thumb-32, DSP, FPU); **differential fuzzing** against Unicorn (native, in tests only) on random instruction sequences; condition flags, IT blocks, exceptions, unaligned access rules, exclusive monitors (LDREX/STREX).
- **Peripheral tests:** register-level tests derived from RM0434 behaviour.
- **Integration:** boot the pinned official release to Desktop in CI; golden screenshots of Desktop and main menu.
- **E2E (Playwright):** load firmware from file and URL, navigate, crash report path, persistence across reload, embed API.
- **Sandboxing:** the emulator never uses WebUSB/WebSerial/WebBluetooth and never talks to real hardware in v1 (no "flash to my device" button — out of scope and a liability). The emulated firmware has no network access.
- **Do not host firmware binaries** in our repo or on our domain; always fetch from the original source (or the user's file). Show the firmware's own licence and source link.
- Disclaimer in footer: "Not affiliated with or endorsed by Flipper Devices Inc. Flipper Zero is a trademark of its respective owner. For educational and authorised security research only."

---

## 12. Milestones (stop and report after each)

| # | Goal | Definition of done |
|---|---|---|
| M0 | Spike + plan | ADR-0001 (CPU engine), CORS findings per firmware host, ARCHITECTURE.md, ROADMAP.md, repo scaffold, CI skeleton |
| M1 | CPU core + memory + DFU loader | Pinned official `.dfu` loaded; executes from reset vector into `SystemInit`; instruction tests green; bring-up digest works |
| M2 | System bring-up | RCC/PWR/FLASH/HSEM/IPCC/CPU2 stub/OTP/RTC/timers: Furi kernel starts, services start, no boot loop |
| M3 | **First visible MVP** | ST7567 + buttons + EXTI + LP5562: Desktop rendered in the browser, menus navigable with keyboard. Deployed preview URL |
| M4 | SD card + `.tgz` | SD over SPI with FAT32 image; resources unpacked; apps launch from SD; Real OTA install mode works on at least the official firmware |
| M5 | Web app complete | Full UI from §9, persistence, profiles, screenshot/GIF, embed API + docs, `?fw=` loading (+ proxy if needed), i18n |
| M6 | Crash detection + headless | §7 and §10 done; GitHub Action published; `/compat` page generated by CI for Official, Momentum, Unleashed, RogueMaster |
| M7 | Senses & CLI | Speaker via WebAudio, vibro, LED, battery/USB controls; USB-CDC → xterm.js CLI (and, stretch, protobuf RPC) |
| M8 (stretch) | Virtual signals | Inject `.sub` raw recordings into CC1101 RX, virtual NFC tags from `.nfc` into ST25R3916, IR RX from `.ir` files — so apps can be tested end-to-end with recorded data |

---

## 13. References (read before designing; cite in code)

- Official firmware: `github.com/flipperdevices/flipperzero-firmware` — pin a tag (e.g. the release the official `.dfu` comes from). Key paths: `targets/f7/` (furi_hal, linker scripts), `furi/`, `lib/drivers/`, `applications/services/`.
- ST: **RM0434** (STM32WB55 reference manual), STM32WB55 datasheet, **PM0214** (Cortex-M4 programming manual), ARM **ARMv7-M Architecture Reference Manual (DDI 0403)**.
- Device datasheets: ST7567, LP5562, BQ27220, BQ25896, CC1101, ST25R3916, SD Physical Layer Simplified Spec (SPI mode).
- Prior art to study for bring-up pitfalls (both GPL-3.0, Renode-based, desktop-only):
  - `github.com/tITAZ314/flipper-emu` — boots unmodified official `.dfu` to the Desktop; documents the CPU2/IPCC, HSEM and OTP pitfalls in its bring-up log.
  - `github.com/d4rks1d33/Flipper-Zero-Emulator` — full-system emulator with persistent flash for OTA testing.
  If you port any code or models from them, keep their copyright notices, credit them in `CREDITS.md`, and keep our project GPL-compatible.

---

## 14. Questions to ask me before Milestone 1

1. Custom domain, or the default GitHub Pages URL (`vincenzo-esposit0.github.io/dryflash-FZ`) for now?
2. Hosting: GitHub Pages or Cloudflare Pages? (Cloudflare makes the CORS proxy trivial.)
3. Preact or Svelte for the UI?
4. Which exact firmware versions to pin for the first compatibility matrix?
5. Do I want analytics? If yes, privacy-friendly only (e.g. Plausible/Umami), no cookies.

---

## 15. Repository layout (proposal — adjust in ARCHITECTURE.md if needed)

```
/core            Rust crate: cpu/, bus/, mem/, periph/ (stm32wb55), board/ (f7 devices), loader/, trace/, crash/
/core-wasm       wasm-bindgen bindings
/runner          Node CLI (headless), uses core-wasm
/web             Vite app (emulator UI, embed, compat, about)
/proxy           Cloudflare Worker (only if CORS requires it)
/action          GitHub Action
/firmware        compat.json (pinned URLs + hashes) — NO binaries committed
/tests           integration + E2E + golden screenshots
/docs            ARCHITECTURE, ROADMAP, BRINGUP_LOG, EMBED, adr/
```

Start now with §0 rule 0 (persist `docs/BRIEF.md` + `CLAUDE.md`), then rule 1: read, plan, ask the §14 questions.

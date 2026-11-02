<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# dryflash — Bring-up log

One entry per milestone (newest last): what runs, what is stubbed, measurements, known gaps.
Performance numbers state host, browser versions and method so they can be compared over time.

---

## M0 — Spike + plan (2026-10-31)

### What runs

- `core/src/cpu`: ARMv7E-M interpreter, user-mode subset (integer ISA, IT, multiply/divide,
  LDREX/STREX, FPv4-SP data processing, transfers and load/store) with a decoded-instruction cache.
- A bare-metal Thumb-2 benchmark (`tests/cpu-bench/fw`) runs to completion natively, in Node and in
  Chrome, Safari and Firefox (main thread and Web Worker), with identical result blocks everywhere.
- No firmware boots yet: there is no exception model, no peripheral, no loader.

### Correctness

- Differential check against native Unicorn 2.1.4: 522 079 / 522 079 trace steps identical
  (R0–R14, PC, NZCV, S0–S31 before every instruction). Command:

  ```sh
  tests/cpu-bench/build-fw.sh
  cargo run --release -p dryflash-cpu-bench -- tests/cpu-bench/bench-fw.bin 1 --trace /tmp/trace.bin
  python3 -m venv /tmp/uc && /tmp/uc/bin/pip install unicorn==2.1.4
  /tmp/uc/bin/python tests/cpu-bench/unicorn_diff.py tests/cpu-bench/bench-fw.bin /tmp/trace.bin 1 8000474
  ```

- `cargo test --workspace`: 7 CPU unit tests + 2 benchmark integration tests green.

### Performance (ADR-0001)

Host: Apple M5 laptop, macOS. Workload: 200 benchmark rounds, 106 357 887 instructions, best of 5.

| Host | MIPS (main) | MIPS (worker) |
|---|---|---|
| Native release build | 170–220 | — |
| Node 23.7 | 157 | — |
| Chrome 156 | 159 | 153 |
| Safari 27.0.1 | 172 | 165 |
| Firefox 147 | 133 | 140 |

Not yet measured: mobile, and anything with peripherals or WFI fast-forward.

Size: benchmark harness wasm stripped 46 KiB, 16 KiB brotli. Web scaffold: 6.0 KiB gzip JS.

### Firmware facts verified (see ADR-0002, ADR-0005)

- Official builds live on `update.flipperzero.one` (CORS OK); GitHub release assets have no CORS.
- All four pinned packages: one DFU element at `0x08000000`, radio stack at `0x080D7000`.
- Resources are `resources.ths` (Official, Unleashed) or `resources.tar.gz` (Momentum, RogueMaster).

### Stubbed / not emulated

Everything except the CPU subset above. In the spike harness only flash and SRAM1 are mapped, and
SCB->CPACR is handled by the CPU (ADR-0001, consequences).

### Known gaps carried to M1

- Exceptions (entry/return, EXC_RETURN, priorities), PPB model (NVIC, SCB, SysTick, DWT, MPU).
- MRS/MSR, SSAT/USAT, parallel add/subtract, halfword/dual DSP multiplies, SXTB16/UXTB16, SEL, QADD
  family, half-precision and fixed-point VCVT, FPSCR exception flags, FZ/DN modes.
- Differential fuzzing on random instruction sequences (only one program is compared so far).

### Demo

```sh
tests/cpu-bench/build-fw.sh
cargo run --release -p dryflash-cpu-bench -- tests/cpu-bench/bench-fw.bin 200
cargo build --release --target wasm32-unknown-unknown -p dryflash-cpu-bench --lib
node tests/cpu-bench/node-bench.mjs target/wasm32-unknown-unknown/release/dryflash_cpu_bench.wasm tests/cpu-bench/bench-fw.bin 200
# Browsers: (cd tests/cpu-bench && node serve.mjs ../../target/wasm32-unknown-unknown/release/dryflash_cpu_bench.wasm bench-fw.bin /tmp/results.jsonl)
# then open http://127.0.0.1:8765/ in each browser; results are printed by the server.
```

---

## M1 — CPU core, memory map, DFU loader (2026-10-31)

### What runs

- **Official firmware 1.5.1** (`firmware.dfu`, pinned, sha256-checked) loads into a 1 MiB flash
  model, resets through the vector table at address 0 (flash alias, `VTOR = 0` as set by
  `SystemInit`) and runs the real boot path:

  | Cycle | Reached |
  |---|---|
  | 0 | `Reset_Handler` (vector 0x08011651) |
  | 3 | `SystemInit` |
  | 39 535 | `__libc_init_array` |
  | 39 578 | `main` → `furi_init` |
  | 43 128 | `furi_hal_init_early` → `furi_hal_clock_init_early` |
  | 48 014 | `__furi_crash_implementation` ("furi_check failed", called from `furi_hal_bus_enable`) |

  The crash is expected: RCC is not modelled yet, so the enable/reset-state checks in
  `furi_hal_bus_enable` (targets/f7/furi_hal/furi_hal_bus.c:177–203) cannot hold. Furi records
  the crash in RTC backup register `RTC+0x060` (message pointer 0x080A3BCA → "furi_check failed")
  and resets with SYSRESETREQ: **boot loop**, ~1 100 resets per virtual second. This is the
  starting point of M2.
- Decoder coverage: **219 464 / 219 464** instructions of the 1.5.1 ELF decode
  (`cargo run --example decode_coverage`).
- Exception model: entry/return with basic and extended (FP) frames, EXC_RETURN, MSP/PSP banking,
  CONTROL, PRIMASK/BASEPRI/FAULTMASK, priorities with PRIGROUP, preemption and nesting, SVC,
  PendSV, SysTick, WFI/WFE with fast-forward, fault escalation (UsageFault, BusFault, MemManage →
  HardFault, lockup), CFSR/HFSR/MMFAR/BFAR, MPU (PMSAv7, used by Furi's NULL guard).
- PPB: SCB, NVIC (63 IRQs, 4 priority bits), SysTick, MPU, FPU control, DWT CYCCNT, CoreDebug.

### Correctness

- Exception conformance firmware (`tests/exc-fw`, 13 self-checks against DDI 0403E): all pass.
  A mutation check (CCR.STKALIGN forced to 0) is caught by test T11.
- Differential fuzzing vs Unicorn 2.1.4 (`tests/cpu-fuzz/fuzz.py`): 240 000 single-instruction
  cases with random registers, flags, FP registers and RAM; 88 608 on defined encodings, **0
  unexplained mismatches**. Divergences are classified and counted, never hidden:
  - dryflash follows the Cortex-M4 where Unicorn is permissive: unaligned LDM/STRD/VLDR fault,
    NOCP for coprocessors other than CP10/CP11, double-precision/NEON/ARMv8 FP UNDEFINED;
  - UNPREDICTABLE encodings (PC/SP misuse, should-be-zero bits) may differ: architecture allows it;
  - harness timing: branches to unmapped memory fault at the next step in dryflash.
- The fuzzer found and fixed in M1: an out-of-range FP register index (D16+ decoded on an FPU
  that only has D0–D15), NaN propagation (ADR-0008), FPSub NaN handling, FLDMX forms.
- Integration test: the 1.5.1 boot path order and 1 M further cycles without CPU emulator gaps
  or unmapped accesses (`core/tests/boot_official.rs`).

### Performance (Apple M5, best of runs)

| Workload | Native | Node 23.7 (wasm) |
|---|---|---|
| CPU benchmark (M0 workload) | 165–199 MIPS | ~140 MIPS (M0: 158) |
| Official 1.5.1 boot loop, full system bus + digest | 131 MIPS | not measured |

The wasm loss (~11 %) is the cost of the full exception model per instruction; profiling showed
it is all in the run loop and `exec` after moving PPB/MPU/fault paths out of line (#[cold]) and
forcing `dp`/`exec_mem`/`cond` inline. Browsers were not re-measured in M1.

### Stubbed / not emulated

All STM32WB55 peripherals (RCC, PWR, FLASH controller, GPIO, EXTI, RTC, …): read-as-zero,
write-ignored, reported in the digest (ADR-0009). OTP not provisioned (erased). System-memory
ROM not provided. CPU2 absent.

### Known gaps

- FP: arithmetic always rounds to nearest (FPSCR.RMode honoured by VCVT only); lazy FP stacking
  is done eagerly (ADR-0007: FPCCR.LSPACT/FPCAR not updated).
- Cycle model: 1 cycle per instruction; exception entry/exit take no extra cycles (M2).
- Debug: no halting debug, DebugMonitor, DWT comparators, ITM/TPIU.
- Stacking errors escalate directly to HardFault (no MSTKERR/STKERR-specific handler choice).
- WFE wakes on the event register and pending exceptions only (no SEVONPEND).

### Demo

```sh
tests/build-fw.sh
node scripts/fetch-firmware.mjs official --elf
cargo run --release -p dryflash-core --example boot -- \
  firmware/cache/official/f7-update-1.5.1/firmware.dfu \
  --elf firmware/cache/official/flipper-z-f7-firmware-1.5.1.elf --cpu2-start 0x080D7000 --cycles 64000000
cargo test --workspace --release -- --include-ignored
# Differential fuzzing (dev tools in a venv: unicorn==2.1.4 capstone==5.0.9):
cargo build --release -p dryflash-cpu-bench --bin cpu-exec
python tests/cpu-fuzz/fuzz.py target/release/cpu-exec 20000 1
```

---

## M2 — System bring-up (2026-10-31)

### Checkpoint 1 (2026-10-31) — no more boot loop

- Peripheral register files generated from ST's SVD (ADR-0010); RCC modelled (ready flags,
  clock switch, reset flags, backup domain, clock tree: 64 MHz from HSE/PLL as Furi sets it).
- The `furi_check` in `furi_hal_bus_enable` passes; boot reaches `furi_hal_rtc_init_early` and
  starts drawing with u8g2. 0 resets, 0 unmapped accesses.

### Checkpoint 2 (2026-10-31) — FreeRTOS scheduler running

- NVIC sampling of peripheral interrupt lines; peripheral event hook in the CPU loop;
  virtual time base in nanoseconds that follows HCLK1 changes.
- Models: GPIO (IDR from outputs and pulls), EXTI (pending/mask/software trigger), I2C master
  (instant transfers, absent devices NACK), SPI master with FIFOs, USART1/LPUART1 (instant TX
  captured, TEACK/REACK), RTC (write protection, init mode, running BCD calendar, backup registers
  kept across system resets).
- Boot path now: … → `furi_run` → `vTaskStartScheduler` → `prvPortStartFirstTask` (PSP) →
  `furi_hal_init` → `furi_hal_clock_init` → `furi_hal_interrupt_init` → `furi_hal_power_init`.
- With no fuel gauge (BQ27220) and charger (BQ25896) on I2C1, `furi_hal_power_init` waits
  2 × 4 s + 2 × 2.02 s by design (furi_hal_power.c:75–98): ~12 s virtual. Device models are needed.
- Then `furi_hal_bt_init` crashes on `furi_check(LL_HSEM_1StepLock(…) == 0)` (furi_hal_bt.c:56):
  HSEM is not modelled yet — next checkpoint (CPU2: HSEM, IPCC, shared SRAM2).
- Speed: 153–160 MIPS native on the boot path (busy-wait delays dominate).
- Nothing has been printed on USART1 yet at that point.

### Checkpoint 3 (2026-10-31) — services running, CPU2 absent, stable

- HSEM (1-step/2-step locks, free interrupts) and IPCC (CPU1 side of the mailboxes) modelled.
  CPU2 is not emulated (ADR-0011): the firmware takes its own "C2 startup failed" path.
- RNG: deterministic xorshift64* with a configurable seed (BRIEF §6 determinism).
- LPTIM1/LPTIM2 with events on the virtual timeline: Furi's tickless idle (LPTIM1 on LSE)
  sleeps in WFI and wakes on CMPM, so idle time is fast-forwarded.
- Board devices on I2C1: BQ27220 fuel gauge (control subcommands, security keys, data memory
  with checksum, CFG UPDATE; starts unprovisioned, so the firmware provisions it) and BQ25896
  charger (register file). `furi_hal_power_init` no longer waits 12 s: `furi_hal_bt_init` is
  reached at 2.4 s virtual (the remaining time is the gauge provisioning sequence).
- Boot path: … → `furi_hal_bt_init` → `flipper_init` (services) → idle with periodic GUI
  redraws (u8g2/canvas). 60 s virtual: no fault, no reset, no unmapped access; the CPU sleeps
  ~90 % of the time; 60 s virtual run in 5.7 s wall (Apple M5, native).
- Still register file only: PWR (pull-up/down control), SYSCFG, FLASH, DBGMCU, COMP, USB,
  TIM1, TIM17. Nothing has been printed on USART1 yet.

### Closing (2026-10-31) — definition of done met

**Furi kernel starts, services start, no boot loop.** The official 1.5.1 image boots through
`flipper_init` and runs its services; over 60 s of virtual time there is no fault, no reset and
no unmapped access (CI checks 10 s).

| Milestone | Virtual time |
|---|---|
| `Reset_Handler` → `main` | 0.6 ms |
| FreeRTOS scheduler started (`prvPortStartFirstTask`) | 7.9 ms |
| `furi_hal_power_init` | 9.3 ms |
| `furi_hal_bt_init` (after fuel-gauge provisioning) | 2.41 s |
| `flipper_init` (services) | 2.42 s |

- Furi crash records are decoded: on SYSRESETREQ the machine reads the fault-message pointer
  that `__furi_crash_implementation` leaves in RTC BKP4R (`FuriHalRtcRegisterFaultData`) and the
  string in flash. Boot loops stop the run (default: 5 resets within 30 s virtual).
- OTP provisioned with board data in the `FuriHalVersionOTPv2` layout (target 7, version 12,
  region World, name "dryflash"); these are the virtual-device settings of BRIEF §5.1.
- ~~USART1 stays silent by design~~ **Wrong — corrected in M3.** The boot tool printed
  `usart[0]`, which was LPUART1: peripheral models were indexed in address order. USART1 does
  carry the firmware log from the first `furi_hal_*_init` (see M3).

### Performance (Apple M5, native)

| Workload | M2 |
|---|---|
| CPU benchmark | 187–190 MIPS native, ~138 MIPS Node/wasm |
| Official boot, active phases (I2C/SPI/peripheral polling) | 80–160 MIPS |
| 60 s virtual incl. idle (CPU sleeps ~90 %) | 6.2 s wall (~10× real time) |

### Stubbed / not emulated (fidelity table: `Periphs::fidelity()`)

- CPU2 (radio core): not emulated, no Bluetooth (ADR-0011).
- Register file only (SVD reset values, latched writes): PWR, SYSCFG, FLASH controller, DBGMCU,
  COMP, USB, TIM1/2/16/17, DMA/DMAMUX, ADC, AES, PKA, CRC, LCD, TSC, SAI, QUADSPI, CRS, VREFBUF,
  WWDG, IWDG.
- I2C/SPI transfers are instant; SPI devices (display, SD, radios) are not attached yet (M3/M4).
- Oscillators are ready at once; clock gating is not enforced.

### Moved out of M2 (with reasons)

- **FLASH program/erase and option bytes → M4.** In 1.5.1 only the updater
  (applications/system/updater) and the app API program the internal flash; settings live on the
  SD card. BRIEF §5.1 expects flash writes for "settings, storage and OTA": for this firmware
  only OTA applies (rule 6 note, also in ARCHITECTURE §3.4).
- **Detailed cycle model → backlog.** Virtual time is 1 cycle per instruction plus WFI
  fast-forward; instruction timings only affect timing fidelity, which the brief does not promise.
- **wasm speed back to the M0 level (~158 MIPS) → M3**, where the browser runner is built and
  measured.
- **IWDG → M3.** Not enabled by the firmware in the observed boot (register file only).

### Demo

```sh
node scripts/fetch-firmware.mjs official --elf
cargo run --release -p dryflash-core --example boot -- \
  firmware/cache/official/f7-update-1.5.1/firmware.dfu \
  --elf firmware/cache/official/flipper-z-f7-firmware-1.5.1.elf --cpu2-start 0x080D7000 --cycles 3840000000
cargo test --workspace --release -- --include-ignored
```

## M3 — First visible MVP

### Checkpoint 1 — display, buttons, LEDs in the core

- **ST7567 on SPI2** (board/st7567.rs): 132×9-page RAM, page/column addressing, start line,
  SEG/COM reverse, inverse/all-on, contrast, reset on a falling RST edge. A byte is routed to the
  controller when CS (PC11) is low; A0 (PB1) selects data/command (furi_hal_resources.c,
  furi_hal_spi_config.c). The firmware's default orientation is ADC normal + COM reverse (`A0`,
  `C8`), x offset 0, so glass pixel (x, y) is SEG x, line y.
- **LP5562 at 0x30 on I2C1** (board/lp5562.rs): registers, LED map, chip enable, the three
  program engines evaluated lazily against virtual time (ramp/wait/set PWM/branch/end as used
  by `lp5562_execute_ramp`/`_blink`). White is the LCD backlight.
- **Buttons** (board::Button): Up PB10, Down PC6, Right PB12, Left PB11, Back PC13 active low
  with pull-ups; OK PH3 active high with pull-down. A press drives the pin; level changes raise
  EXTI edges on the line's port selected by `SYSCFG_EXTICRx`.
- **Bug fixed (since M2): SPI and USART models were indexed by address**, so `spi[0]` was SPI2
  and `usart[0]` LPUART1 — the USART1 and LPUART1 NVIC lines were swapped and the boot tool
  printed the wrong port. Fixed indices now (SPI1/SPI2, USART1/LPUART1).
- **USART1 log** is now visible (3.2 KB in 10 s): HAL init, gauge data-memory provisioning on a
  fresh BQ27220 ("Incorrect configuration data, reset needed" → "Updating data memory", as on a
  new board), "Boot mode 0, starting services", "Startup complete", animation
  `L1_NoSd_128x49`, "C2 startup failed".
- **Result:** at 5 s the LCD shows the Desktop with the "Need an SD card" animation, status bar
  and battery; backlight 252/255. `boot --png` writes a 4× screenshot.
- **Input path verified to the GUI:** OK → PH3 → EXTI3 → `EXTI3_IRQHandler` → `input_isr` →
  input service debounce → `furi_pubsub_publish` (Press, Short, Release) →
  `view_dispatcher_input_callback`.

### Finding: without CPU2 the Desktop deadlocks (ADR-0011 assumption broken)

`desktop_srv` never reaches `view_dispatcher_run`, so the Desktop never handles input. It
blocks in `furi_hal_crypto_enclave_verify` → `furi_hal_crypto_enclave_load_key`
(targets/f7/furi_hal/furi_hal_crypto.c): the function takes `furi_hal_crypto_mutex`, and when
`furi_hal_bt_is_alive()` is false it leaves through `break` **without releasing the mutex**; the
next key slot waits on the same non-recursive mutex forever. On hardware CPU2 is always alive,
so this path is never taken there. ADR-0011 ("the firmware supports a missing CPU2") is
therefore wrong for the UI.

The firmware does support a CPU2 that is alive but has no usable radio stack
(furi_hal_bt.c: "Unsupported radio stack — Don't stop SHCI for crypto enclave support"):
`ble_glue_is_alive()` is true once the C2 ready system event arrives
(`ble_sys_user_event_callback`), and FUS key commands get SHCI responses.

### Checkpoint 1b — CPU2 as a FUS without wireless stack (ADR-0012)

- Transport layer read from the pinned `lib/stm32wb_copro` (commit `133182d`, fetched into
  `firmware/cache/src/stm32wb_copro`; the sparse firmware clone has no submodules):
  reference table at `SRAM2A_BASE + IPCCDBA*4` (IPCCDBA reads 0 → `TL_RefTable` at 0x20030000),
  system command/response on IPCC channel 2 (CPU1 spins on the flag), system events on the queue
  in `TL_SysTable`, buffer release on channel 4.
- Model (`core/src/bus/cpu2.rs`): on C2BOOT writes the FUS device-info table and posts the ready
  event (`FUS_FW_RUNNING`); `FUS_GET_STATE` → idle; every other SHCI command → status 0x01.
- Firmware log now: "Core2 started", "C2 boot completed, mode: FUS",
  "Core2: FUS: 1.2.0 … Stack: 0.0.0", "Stack isn't installed!", "Radio stack start failed",
  "Secure Enclave verification failed: total 0, valid 0". SHCI opcodes seen: 0xFC59
  (`FUS_LOAD_USR_KEY`), and 0xFC79 (`SetSystemClock`) until Legacy sleep was preset.
- **Sleep:** with CPU2 alive the idle task uses STOP2, which asks CPU2 for `SetSystemClock`
  (stack ≥ 1.20 only); the FUS refuses, `furi_hal_power_deep_sleep` returns before WFI and idle
  spins (640 M instructions in 640 M cycles, 45 MIPS, slower than real time). The virtual device
  now ships with Sleep Method = Legacy in the RTC backup domain (maintainer decision, ADR-0012):
  plain WFI, fast-forward works again.
- **Result:** the firmware is interactive. At 5 s: "No Factory Keys Found / Secure Enclave is
  damaged" popup (the firmware's own); Back → Desktop ("Need an SD card" animation); OK → main
  menu (Apps, Sub-GHz, 125 kHz RFID, NFC, …); Down moves the selection. No faults, no unmapped
  accesses, no resets.

### Performance (Apple M5, native)

| Workload | M2 | M3 CP1 | M3 CP1b |
|---|---|---|---|
| Official 1.5.1, 10 s virtual | — | 97 MIPS, 2.5 s wall | 135 MIPS, 1.2 s wall (163 M instructions) |

### Demo

```sh
node scripts/fetch-firmware.mjs official --elf
cargo run --release -p dryflash-core --example boot -- \
  firmware/cache/official/f7-update-1.5.1/firmware.dfu \
  --elf firmware/cache/official/flipper-z-f7-firmware-1.5.1.elf --cpu2-start 0x080D7000 --png screen.png
cargo test --workspace --release -- --include-ignored
```

### Checkpoint 2 — wasm bindings, Web Worker, web app

- `core-wasm` (wasm-bindgen 0.2.130, pinned together with the CLI): `Emulator` with `load_dfu`,
  `power_on`, `run`, `set_button`, `frame` (1 KiB, 1 bpp), `lights`, `take_log`,
  `fidelity_json`, `crashes_json`, `digest_text`, `set_legacy_sleep`. `scripts/build-wasm.sh`
  generates bindings for `web/` and `runner/` (git-ignored); CI builds them before the web job.
- Worker (`web/src/emu/worker.ts`): slices of ≤ 12 ms wall time, steps of ≤ 5 ms virtual;
  1× pacing locked to the wall clock with a 250 ms backlog cap (no catch-up bursts); Turbo runs
  flat out. Frames are posted only when the display changed, as transferable buffers.
- **Minimum press time (40 ms virtual).** A key tap whose press and release reach the worker
  between two slices was invisible to the firmware: the input service debounces over
  `INPUT_DEBOUNCE_TICKS` = 4 ms (furi_hal_resources.h). Releases are now held back until the
  button has been down 40 ms of virtual time.
- Web app: LCD canvas (backlight tint from the LP5562 white channel), on-screen keys and
  keyboard (arrows, Enter/Space = OK, Backspace/Esc = Back), drag & drop or file picker for
  `.dfu` (file stays in the browser), pause/resume, power cycle, Turbo, the Sleep-method
  setting, firmware log, hardware fidelity table, positioning text and disclaimer; EN/IT.
- Verified in a browser (built-in Chromium pane, dev server): official 1.5.1 boots to the
  popup at 1× real time; Esc → Desktop, Enter → menu, ↓ ↓ → NFC, on-screen Back → Desktop;
  Turbo ≈ 10× on the idle Desktop; no console errors; 375 px layout without horizontal scroll.
- Production build: 349 KB wasm (123 KB gzip), 24 KB JS, 8 KB worker.

### wasm speed (Node 23 / V8, Apple M5)

| Workload | M0 | M2 | M3 CP2 |
|---|---|---|---|
| ADR-0001 CPU benchmark | 158 MIPS | ~138 MIPS | 137 MIPS |
| Official 1.5.1, 10 s virtual (wasm, `core-wasm`) | — | — | 95 MIPS, 1.7 s wall (5.8× real time) |

The benchmark has not returned to the M0 figure (peripheral dispatch and the M2 bus model sit on
the hot path). It does not limit M3: the real firmware runs at 5.8× real time in V8. Kept open
as a performance item rather than chased now.

### IWDG

Not enabled by firmware 1.5.1: no IWDG register is accessed in 60 s virtual from power-on (boot
tool digest, Desktop idle); it stays register file only and is listed as such in the fidelity table. To be
modelled when a firmware starts it.

### Checkpoint 3 — deployment

- `proxy/wrangler.jsonc`: Worker `app`, static assets from `web/dist`, `workers.dev` and preview
  URLs on; wrangler 4.156.0 pinned in the `proxy` workspace.
- `web/public/_headers`: CSP (`script-src 'self' 'wasm-unsafe-eval'`, `worker-src 'self'`, no
  inline scripts or styles), `nosniff`, `no-referrer`, immutable caching for hashed assets; no
  COOP/COEP.
- Verified locally with `wrangler dev` (workerd): the page loads, the wasm core initialises under
  the CSP (fidelity table filled from the core), headers present, `_headers` not served (404).
- CI job `deploy` (after rust, cpu-fuzz, web, licences): `main` → `wrangler deploy`; pull requests
  from this repository → preview version `pr-<n>`. Needs the repository secrets of
  docs/DEPLOY.md; the preview URL is recorded here once live.
- First CI run (PR #1): token and account accepted, all checks green, but `wrangler versions
  upload` refuses a Worker that does not exist yet. Only `wrangler deploy` creates it, and that
  publishes production, reserved for `main`. The preview step now reports this one case as a
  warning; PR #1 therefore has no preview, and the first live check is production after merge.

### M3 closed

- Live: **https://app.dryflash.workers.dev** (first deploy from `5263cf1`, version
  `ae064fe7-e3b2-40fd-b6cd-18d8c870173b`).
- Checked on the live site from the built-in browser: HTTP 200, CSP and other headers present, no
  COOP/COEP, wasm core initialised under the CSP (fidelity table filled), no console errors.
- The built-in browser cannot drop files, so the firmware itself was run on the live site by the
  maintainer: official 1.5.1 `firmware.dfu` dropped on the page, popup, Desktop and menu navigation
  with the keyboard work. Definition of done met.
- Pull request previews work: PR #2 published https://pr-2-app.dryflash.workers.dev, checked in the
  browser (wasm core under the CSP, no console errors).

Known limits at the end of M3: no SD card or `.tgz` (M4), no Bluetooth and no factory keys
(ADR-0012), wasm CPU benchmark at 137 MIPS vs 158 in M0.

### Demo

Open https://app.dryflash.workers.dev and drop a `firmware.dfu` on the page (official update
package, after `node scripts/fetch-firmware.mjs official`:
`firmware/cache/official/f7-update-1.5.1/firmware.dfu`). Esc closes the popup, Enter opens the menu.

## M4 — SD card + `.tgz`

### Checkpoint 1 — SD card over SPI and DMA

- **The SD driver moves blocks through DMA** (`furi_hal_spi_bus_trx_dma`, DMA2 channels 6/7, requests
  8/9 through DMAMUX channels 12/13, IRQ 60/61; values from the pinned `stm32wbxx_ll_dmamux.h` and
  `stm32wb55xx.h`), so DMA stopped being a register file: DMA1/DMA2 have flags, IFCR and TC
  interrupts, and SPI requests run instantly. Other requests stay idle with the channel enabled.
- `board/sdcard.rs`: SDHC in SPI mode (CMD0/8/9/10/12/13/16/17/18/24/25/55/ACMD41/58), CS PC12, card
  detect PC10 pulled low by an inserted card. Sparse store, 1 GiB by default (ADR-0013).
- `board/fat32.rs`: MBR + empty FAT32 volume.
- **Chip selects now mean "output driven low".** The handles leave CS in analog mode when idle; on
  the board that floats high, but `levels()` read 0, which would have sent SD traffic to the display.
- Firmware log with a card: "card detected", "card mounted", state files created under `/int`
  ("State saved"), animation `L0_NoDb_128x51` (no resources on the card yet). After a power cycle
  with the same card the Dolphin state is found again. Commands outside the model: none seen.

### Checkpoint 2 — update packages, resources on the card, apps

- `loader/package.rs`: gzip (CRC checked), tar (ustar, GNU long names; paths with `..` refused),
  heatshrink data stream (`HSDS`), `update.fuf`. `board/fat32.rs` gained a volume writer and
  reader: long file names, directories that grow, replacing files, FSInfo.
- **Quick install** = firmware from the package into flash, resources into the card's root (`/ext`).
  Files from an older install stay on the card (only the updater's Manifest cleanup removes them).
- Independent check: `fsck_msdos` (and `fsck.vfat` in CI) finds no errors on cards built from all
  four pinned packages.

| Package | Resources | Files | Boot, 15 s virtual |
|---|---|---|---|
| official 1.5.1 | `.ths` | 1074 | card mounted; app test below |
| momentum mntm-012 | `.tar.gz` | 2150 | card mounted, no crash, no unmapped access |
| unleashed unlshd-094 | `.ths` | 1284 | card mounted, no crash, no unmapped access |
| roguemaster rm-420 | `.tar.gz` | 2071 | card mounted, no crash, no unmapped access |

The custom firmwares were only booted to the Desktop's animations; their apps were not exercised.

- **Apps run from the card.** Through the official firmware's own menus: Apps → `/ext` listing (long
  lower-case names shown correctly) → apps → Games → `snake_game.fap`: "Loaded in 18ms", the game
  draws (`core/tests/apps_official.rs`). The Sub-GHz menu entry loads `subghz.fap` too and then
  reports what the device lacks: "Unable to load decryption key" (no enclave keys), external
  CC1101 "Init failed".
- With resources the Desktop plays a real animation (`L1_Waves_128x50`) instead of `L1_NoSd`.

### Checkpoint 3 — card and packages in the web app

- The page takes `.tgz` and `.dfu`, told apart by content. A package does a quick install and the
  page says how many files went on the card. A card is always inserted: new and empty on the first
  visit, otherwise the saved one. "Erase card" asks first, then replaces it and restarts the device.
- `web/src/emu/cardstore.ts`: blocks saved about once a second and when the page is hidden. OPFS
  with sync access handles (`devices/default/sd.idx` + `sd.dat`), IndexedDB where they are missing.
  If neither opens (for instance another tab holds the card) the page says the card is **not saved**.
  Only the card persists: the firmware has to be loaded again after a reload (profiles are M5).
- Bugs found while wiring this up: loading a firmware reset the Sleep-method setting and would have
  dropped the card; a rejected image now leaves the running machine alone.
- **Slices no longer depend on timers.** With the pane hidden the emulator ran at 0.06× (Chromium
  throttles timers of background pages). Pending work now continues through a `MessageChannel`,
  which also removes the 4 ms pause per slice that nested `setTimeout(0)` imposed on Turbo.
- Verified in a browser (dev server, official package, OPFS): quick install of 1074 files, 3.1 MB on
  the card. After reloading with only the bare `.dfu`, the Dolphin state is found again and
  `L1_Waves_128x50` plays from the saved resources. After "Erase card": a new card, no saved state,
  `L0_NoDb_128x51`. Not tested in a browser: the IndexedDB fallback, and two tabs at once.
- wasm size with the package code: 454 KB.

### Checkpoint 4 — FLASH controller and Real OTA

- `periph/flash.rs`: unlock sequences, page erase, 64-bit and fast programming with the sequence and
  "already programmed" errors, option bytes. The virtual device's option bytes are the `OB reference`
  of the packages, identical in all four pinned ones. Sector protection is not enforced.
- `SYSCFG_MEMRMP` maps SRAM1 at address 0: how the updater runs from RAM. Written flash invalidates
  decoded instructions. Flash now survives a power cycle in the web app (it used to be reloaded from
  the image, which would have undone an update).
- `package::stage_ota` + `Machine::start_ota`: the package in `/ext/update/<name>/`, the `.fupdate`
  pointer, boot mode PreUpdate, reset. Radio entries emptied (ADR-0014).
- **Unleashed → official, firmware's own updater** (`core/tests/ota_official.rs`, also run in the
  browser with OPFS): `/int` backup, reset; `1.5.1 (RAM)` runs from SRAM1; option bytes validated
  (stage 8), DFU validated, flash written and read back (9–11), reset; `/int` restored (12), old
  resources removed via Manifest (13–14), resources unpacked (15), splash (16), reset; normal boot of
  1.5.1. 25 s virtual. The test compares flash with the package's image and every resource file with
  the package: all identical. No crash, no unmapped access.
- **A wrong first attempt, caught by the byte comparison:** removing the `Radio*` lines made the
  update look successful while skipping resources and option bytes, because `update_manifest_init`
  reads keys in order and a missing key hides the later ones. The keys now stay with empty values,
  the way `scripts/update.py` writes a package without radio.
- The updater clears `FuriHalRtcFlagLegacySleep`; with the device setting on, it is set again when
  the machine resets into PostUpdate (ADR-0014).
- Speed during the update in the browser: 46 MIPS, 0.72× while unpacking.
- Seen once, not reproduced: `RangeError: Invalid typed array length: 144115188075855870` on the first
  page load right after restarting the dev server with a freshly rebuilt wasm; two later reloads and
  the whole OTA were clean. Probably a stale pairing of `dryflash.js` and the `.wasm` on the dev server
  (production assets are hashed). Keep an eye on it.
- **Not tested:** a package's radio stage; OTA of Momentum and RogueMaster; option bytes that do not
  match (the repair path). 

### Demo

```sh
node scripts/fetch-firmware.mjs official --elf && node scripts/fetch-firmware.mjs unleashed
cargo run --release -p dryflash-core --example ota -- \
  firmware/cache/unleashed/flipper-z-f7-update-unlshd-094.tgz \
  firmware/cache/official/flipper-z-f7-update-1.5.1.tgz 60
cargo test --workspace --release -- --include-ignored
```

In the web app: load a `.tgz`, tick "Install packages with the firmware's own updater", load another.

### M4 closed

- Live at https://app.dryflash.workers.dev since `4d50b03`.
- **Checked by the maintainer on the PR #3 preview**, with the firmware running on the deployed Worker:
  quick install of a package, an app from the card, the card still there after a reload, Real OTA
  from Unleashed to the official firmware. Definition of done met, with the radio stage left out by
  decision (ADR-0014).
- Checked on the live site from the built-in browser: the page, the wasm core under the CSP, the card
  saved in the browser, no console errors.
- `fsck.vfat` in CI caught what `fsck_msdos` had not: a boot-sector label with no entry in the root
  directory. Cards prepared earlier keep that harmless inconsistency.

Known limits at the end of M4: no Bluetooth and no factory keys (ADR-0012); radio stage of packages
not tested (ADR-0014); only the card persists, the firmware is loaded again after a reload (M5);
OTA of Momentum and RogueMaster, the option-byte repair path, the IndexedDB fallback and two tabs at
once not tested; wasm CPU benchmark still below its M0 figure.

## M5 — Web app complete

### Checkpoint 1 — virtual devices

- `web/src/emu/devices.ts`: each device has its flash image, option bytes, card and settings, in
  OPFS (`devices/<id>/`), IndexedDB where sync handles are missing, memory if neither opens. The
  folder of the version before profiles becomes "Device 1" with its card intact.
- **A reload now resumes the device:** its flash is restored and the firmware starts by itself. Flash
  is saved when installed and whenever the core reports writes, so an update persists too.
- Export/import: one gzip file (`.dryflash`) with settings, flash, option bytes and card blocks.
  Importing always creates a new device. Factory reset wipes firmware, card and settings, keeping the
  label and the device's ID.
- Settings per device: OTP name, region and colour, UID64 (random per device), Sleep method, simulated
  battery (charge, charging, USB).
- Worker: `ready` now follows the device being up, and messages arriving during a switch wait instead
  of being dropped (a file dropped right after page load used to be lost).
- Verified in a browser (dev server, OPFS): install; reload resumes 1.5.1 with its card; a second
  device starts empty and the first keeps its firmware; export (6.1 MB) and import give identical flash
  and card; delete removes the folder; factory reset empties the device. Not tested in a browser: the
  IndexedDB backend, a device open in two tabs, and the firmware actually showing changed settings
  (unit tests check they reach the OTP, UID and battery models).

### Checkpoint 2 — device, panels, file manager, recording

- Layout: an original device drawing (SVG shell, its own proportions, no Flipper Devices artwork) with
  the real canvas and buttons on top; the LCD is always a whole multiple of 128×64. Collapsible panels:
  firmware, devices, card, crash report, events, log, fidelity.
- SD file manager: browse, upload, download, new folder, delete, erase. **Changes restart the device**:
  FatFs keeps state in RAM, so the card must not change under a running firmware, and restarting makes
  it mount the new contents. Reading while it runs shows the card as it is at that moment.
- Screenshot (PNG, 4×), video (WebM with VP9 where available, MP4 otherwise; from a 4× canvas), GIF (own
  LZW encoder, 2× scale, stops after 30 s), fullscreen, optional LCD ghosting.
- Crash report panel: Furi's decoded records as they happen, not only when the emulator stops.
- **Bugs found:** `prompt()` throws in the desktop app's browser and is blocked in sandboxed iframes, so
  questions are now a `<dialog>` in the page. After a quick reload the old worker can still hold the OPFS
  locks, and the page silently fell back to an unsaved card: opening is retried for 3 seconds. An
  on-screen key with focus swallowed the arrow keys: it now passes them on.
- Verified in a browser (dev server): upload → the firmware boots again and the card still mounts →
  download gives the same bytes → delete; PNG is 512×256; the browser's own decoder accepts the GIF
  (256×128); a WebM comes out; 375 px layout has no horizontal scroll. LZW is also tested against a
  standard decoder, past the 4096-code table reset.
- **Not done or not tested:** fullscreen and ghosting were only exercised without looking at the result;
  bottom sheets on mobile (panels collapse instead); Mute (no audio before M7); Reboot to DFU (not
  modelled); Gamepad API (stretch). Firefox and WebKit come with the E2E suite in checkpoint 4.

### Checkpoint 3 — links, proxy, pages, embed

- `proxy/src`: `GET /proxy/fetch?url=` (ADR-0002). Only release assets of the three repositories without
  CORS; each redirect checked, up to GitHub's asset hosts; 64 MiB; one day of edge cache; about 30
  requests a minute per client (per isolate: a rough limit, no binding needed); only dryflash sites from a
  browser. Everything else runs no code: `run_worker_first` covers `/proxy/*` alone.
- Links (`docs/EMBED.md`): `fw`, `elf`, `mode`, `autoboot`, `device`. **A link cannot overwrite a visitor's
  device by surprise:** its firmware goes on a device called "From a link", and naming a device that has a
  firmware always asks. Without `autoboot` it asks too. The page says where the file came from and whether
  through the proxy. `connect-src` now allows https for this.
- `/embed`: only the device, frameable by anyone (its own CSP; the other pages stay `frame-ancestors
  'self'`), keeps nothing and cannot reach saved devices. API in `docs/EMBED.md`: `ready`, `booted`,
  `desktopReached`, `crashed`, `frame`, `screenshot`, `error` out; `load`, `press`, `reset`, `screenshot` in,
  accepted only from the parent window. `booted` and `desktopReached` read the firmware's log, which is
  provisional until the M6 decision.
- `/compat`: the pinned firmwares with what was seen working, from `firmware/compat-results.json`, written
  by hand and saying so; the M6 nightly replaces it. `/about`: how it works, limits, fidelity table from the
  core, credits. "Try in browser" badge: `web/public/badge.svg`.
- Verified on the real Worker with `wrangler dev`: proxy download identical to the pinned package (same
  SHA-256); wrong host and origin 403, other route 404; `/embed` and only `/embed` frameable; link with
  `autoboot` loads Unleashed through the proxy onto "From a link" and leaves "Device 1" alone; without it, the
  question appears; `/compat`, `/about`. Embed in a real iframe: milestones, tap, `frame`, screenshot, reset.
- Expected noise: when a host has no CORS the browser logs the failed direct attempt before the proxy works.
- **Not tested:** the proxy deployed on Cloudflare (edge cache, rate limit, `CF-Connecting-IP`); `elf` and
  `mode=ota` through a link; embedding from another origin (the test frame was same-origin).

### Checkpoint 3b — pixel-art device and tabs (ADR-0015)

- `web/src/ui/shell.ts` rasterizes the device from a model, seen from the front on the page's dark background: thick
  body, bevels, chunky art pixels (2 face units), "FL1PP3R" by the maintainer's choice. The real canvas and `<button>`s
  sit on the face at a whole-number scale. Stacked on phones. Tried and dropped at the maintainer's request: a flat
  drawing tilted by CSS, then an angled view rasterized through a projective map on a sky-blue stage.
- The page follows how qFlipper is used, not its graphics: tabs for device information and virtual
  devices, update (quick install or Real OTA), files, backup, diagnostics. The panels of checkpoint 2 are
  gone. Tabs follow the ARIA pattern, with arrow keys.
- **The `RangeError: Invalid typed array length: 144115188075855870` came back**, once, on the first load after
  starting the dev server with a wasm rebuilt since the last one (now "in promise"). Fast navigation and
  reloads did not produce it, and the page worked normally afterwards. Still only the stale `.js`/`.wasm`
  guess: cause not proven. The E2E suite runs on the production build and fails on console errors.

### Checkpoint 4 — accessibility and end-to-end tests

- `tests/e2e`: Playwright against the production build on the real Worker (`wrangler dev`), 22 tests per engine at this checkpoint:
  package install and boot, keyboard, drawn keys with the mouse, reload resume, files, screenshot and GIF, export and
  import, new device and dialogs, links, proxy refusals, framing headers, `/compat`, `/about`, `/embed` in a frame, phone
  width, axe on `/`, `/compat`, `/about` (WCAG A and AA) in English and Italian, tabs and drawn keys by keyboard.
- **Every test fails on an uncaught error, a console error or a CSP violation.** One exception, by the violation's own
  details: WebKit reports `style-src-attr` for the internals of a `<select>`, with no effect on rendering, and not always naming the element. Chromium and Firefox stay strict on it. The CSP was not
  relaxed.
- Result here: **44 of 44 on Chromium and WebKit.** Playwright's Firefox does not start on macOS 27 ("Could not find profile
  folder", from the binary alone), so Firefox runs in the CI job `e2e`, on which `deploy` now depends.
- Bugs these tests found: in private modes where OPFS is refused the page went straight to an unsaved device though
  IndexedDB worked (it now tries OPFS, IndexedDB, memory; WebKit's run covers the IndexedDB backend, including a reload and an
  import); the first render overflowed phones until the first resize; the `ResizeObserver` updated even with nothing to
  change, which WebKit reports as an error; `<html lang>` stayed `en` over Italian text.
- The `RangeError` of checkpoint 3b did not appear in any run on the production build. Those runs could not have seen it if it was raised in the worker: see "On production" below.
- **Not tested:** a screen reader; OPFS in WebKit (its test contexts refuse it); Mobile Safari and Chrome on real phones;
  fullscreen; Real OTA in the suite (`core/tests/ota_official.rs` covers the core). Touch targets at 2×, computed from the
  models: 48–50 px on the pad, 44–56 px for Back.
- **Definition of done:** the E2E part depends on Firefox in CI. "Hub 'Try in emulator' link works" cannot be checked from this
  repository: `/?fw=`, the badge and `/embed` are ready, and the maintainer is connecting the link.

### On production

Merged through PR #5. The merge's first run on main failed on the WebKit `style-src-attr` flake, so `deploy` was
skipped; the re-run passed and deployed `c31a952`. The flake's fix is PR #8.

Checked against https://app.dryflash.workers.dev, 2026-11-01:

- Official 1.5.1 through `?fw=` (fetched directly, no proxy) goes on the "From a link" device and boots to the "No Factory
  Keys Found" popup; after a reload the saved device boots again. Chromium and WebKit installed the same package from a file and booted it.
- `/`, `/compat`, `/about`, `/embed` load with no page errors, 8 cold loads each on Chromium and WebKit for `/`; WebKit shows
  only the known stylesheet CSP reports. `/embed` alone has `frame-ancestors *`.
- A proxy request for a host outside the allowlist gets 403.
- **Not checked on production:** a successful download through the proxy (done on the pr-5 preview, with an identical
  SHA-256 and a cache hit on the second download); Firefox (CI only); the Hub's link.

**The `RangeError` reached production.** `Invalid typed array length: 144115188075855870` appeared once in the desktop app's
browser pane, on the first production load in a tab that had been running a firmware from the dev server. That rules out the guess
of a stale `.js`/`.wasm` pair: production's assets are hashed. It did not come back in about 7 more loads in a new pane, including a
firmware running and then navigating away. The cause is **not known**. The page was still on "Loading" 4 s into that load, while
loads usually take under a second, and it was not looked at again before the next navigation, so it may have been stuck.

What it did show is a gap: exceptions in the worker reached nothing. `start()` runs with no handler, so a failed start left
"Loading" on screen with no message, and Playwright's `pageerror` does not report workers, so "every test fails on an uncaught
error" did not cover them. The worker now reports uncaught exceptions; the page logs them with their stack (`dryflash worker:` in the
console) and shows them, and the embed API passes them on as `error`. A new test blocks the `.wasm` and expects the alert and the console
line, and it fails without the change. This does not fix the `RangeError`: if it comes back, it now has a stack and fails the suite.

Result here: **46 of 46 on Chromium and WebKit** (23 tests per engine).

**M5 is not closed:** the Hub's "Try in emulator" link is still to be confirmed by the maintainer.

## M6 — Crash detection + headless

### Checkpoint 1 — crash report, hang detection, watchdog

Everything from BRIEF §7 now ends in a report, and the emulator stops there. Decisions in ADR-0016.

- **Crashes.** `__furi_crash_implementation` (furi/core/check.c) prints `[CRASH][task] message`, r0–r11 and lr on USART1 before
  resetting, and HardFault, BusFault, UsageFault and MemManage all go that way (furi_hal_interrupt.c). The core reads that record,
  so it works without an ELF. Builds that halt instead of resetting are found by their "System halted" line.
- **Faults.** The first fault is recorded before its exception is taken: registers, CFSR, BFAR, MMFAR, 256 stack words, last 64 PCs.
- **Hangs.** No WFI, display change or input for 5 s in one 256-byte span of code, or for 30 s anywhere. Off when the firmware's
  Sleep Method is not Legacy, because that idle loop spins here (ADR-0012).
- **Watchdog.** IWDG is modelled (`periph/iwdg.rs`), with RCC's reset flag; the window is not enforced. Firmware 1.5.1 never starts it.
- **Report.** JSON and Markdown from the core, the display as PNG, and a panel in Diagnostics. A backtrace from a stack scan, checked
  against BL/BLX call sites and named with the ELF. A verdict with the reasons it rests on.
- **With an ELF**, the core stops briefly on entering Furi's crash code. Without one, a `furi_check` report's last 64 PCs are Furi's
  own printing, and the report says so.
- A crash now stops the emulator (BRIEF §1); Resume continues into the reboot, as the device would.

Tests: `core/tests/crash_report.rs` has small firmwares for hang, fault, undefined instruction and watchdog, and the official
firmware with a fault, a branch into Furi's crash code and a hang patched into `loader_show_menu`, reached by pressing OK. Boot,
SD, apps and real OTA still pass with the detector on. Official, Momentum, Unleashed and RogueMaster ran 2 virtual minutes idle on the
Desktop with no false alarm. E2E: 50 of 50 on Chromium and WebKit, including the panel, its three downloads and axe; Firefox in CI only.
`cpu-bench`, 5 runs each: 111–162 MIPS before the PC ring and 166–177 after, so no cost shows above this machine's noise.

- A first verdict called every crash "unclear": PWR and SYSCFG are touched all the time in the idle path. Only registers first
  touched in the last second now count against the firmware being at fault; the rest are still listed.
- **Limits.** A task blocked for ever while the idle task sleeps is not a hang by §7's definition, and is not detected: M3's Desktop
  deadlock would not have been. A script's timeout has to catch that (next checkpoint). The verdict is a heuristic and has only been
  tried on the cases above. The report's own text stays in English in the Italian UI.
- **Not tested:** a boot loop or a Furi halt on real firmware (synthetic only for the loop; parsing only for the halt); the watchdog on any
  real firmware; a crash in the browser with an ELF loaded.

Demo: `cargo test --release -p dryflash-core --test crash_report -- --include-ignored`. On the page, drop a `.dfu` that faults: the E2E
test `crash report` builds one.

### Checkpoint 2 — headless runner

`dryflash run` (`runner/`) drives the same wasm core under Node, in virtual time, with no throttling.

- **Scripts** (BRIEF §10): `wait_for_desktop`, `press`, `long_press`, `wait`, `expect_screen` with a tolerance, `expect_no_crash`, plus
  `expect_screen_change`, `expect_log`, `wait_for_log`, `screenshot` and `walk_menu`. Checked before anything runs. `runner/README.md` has them all.
- **Exit codes:** 0 pass, 1 firmware failure, 2 emulator limitation, 3 usage. 1 and 2 follow the report's verdict (ADR-0016).
- **Output:** JUnit with a test case per step, `--screenshots` with `crash.json`, `crash.md`, `crash.png` and `digest.txt`, `--report` as JSON.
- **Default smoke test:** boot, Desktop, every main-menu entry, back out, no crash or hang. Menus differ too much to assume a shape: a list on
  Official and Unleashed, a carousel on Momentum, a grid on RogueMaster, and on some of them Back lands on the Desktop. So it looks
  at the screen, moving down and then right until it sees a screen it has seen.

| Firmware | Result | Entries opened | Virtual | Wall |
| --- | --- | --- | --- | --- |
| Official 1.5.1 | pass | 10 | 32.1 s | 6.5 s |
| Momentum mntm-012 | pass | 11 | 40.5 s | 9.0 s |
| Unleashed 094 | pass | 12 | 37.8 s | 9.0 s |
| RogueMaster RM1024 | pass | 15 | 54.0 s | 18.4 s |

Every opened entry's screen was different on all four, and I looked at the screens of Official, Unleashed and RogueMaster. Two runs of the official
firmware end at the same virtual time with the same final screen.

**The first walk crashed the official firmware, and the report blamed the firmware.** Closing Sub-GHz failed a `furi_check` in `furi_hal_subghz_sleep`, because SPI1
had no chips: `[FuriHalSubGhz] Init Fail`, as at every boot since M2. The report named the function at once, but its verdict was wrong, since no register gap was
involved. Both are fixed:

- **CC1101 and ST25R3916 are now stubs**, as BRIEF §5.2 always asked and ARCHITECTURE placed in M2: `board/cc1101.rs` and `board/st25r3916.rs`, from the firmware's
  drivers. They answer so the drivers start, and there is no signal, field, tag or NFC interrupt line. At boot the log now says `Init OK` and no longer `Wrong chip id`.
- **A stubbed chip used in the last 100 ms counts against blaming the firmware**, whenever it was first used.

Sub-GHz on Official shows "Firmware update needed" and U2F shows "Certificate error". Both are the firmware's own answers to a device with no region data and no keys,
not emulator failures. The smoke test only opens and closes these.

Tests: 32 in `runner/test`, running the real command for each exit code; all Rust tests including real firmware; E2E 50 of 50 on Chromium and WebKit. CI has a new `runner`
job with the smoke test on Official, and `deploy` waits for it.

- **Not tested:** the runner on Windows; on Linux until CI runs it; Read screens in Sub-GHz or NFC, which the stubs may not carry: NFC polling in particular has no interrupt line.
  The smoke test only proves the apps open and close.
- **Not on npm** at this checkpoint. Use `node runner/bin/dryflash.mjs` after `scripts/build-wasm.sh`. (Checkpoint 3 settles how it is released.)

Demo:

    scripts/build-wasm.sh
    node runner/bin/dryflash.mjs run firmware/cache/official/flipper-z-f7-update-1.5.1.tgz --screenshots out --junit out/junit.xml

The merge's run on main failed in `cpu-fuzz`, seed 25, so `deploy` was skipped: `strh r4, [r0, r2]` with an address of 0xE002C629. Each register was random and outside the
PPB, which the generator avoids, but their sum was inside it. dryflash models the PPB and this Unicorn harness maps nothing there, so there was nothing to compare, and the CPU was
not at fault. The fuzzer now counts that as a known case, from the address Unicorn reports. Re-running would not have helped: the seed is the run number, and a re-run keeps it.

### Checkpoint 3 — GitHub Action and npm package

- **Action** (`action/`): runs the runner, comments on the pull request, writes the job summary, uploads the artifact and fails the job on any non-zero exit code. Later runs
  replace the comment. Inputs reach its script through the environment, and text from the firmware goes into the comment escaped, with no mentions.
- **BRIEF §10 asks for screenshots in the comment. That is not possible as written:** the REST API cannot attach images to a comment, and `data:` images are not rendered. Hosting them
  would mean pushing to a branch of the user's repository. So the comment has the last screen as Braille text, 64×16, and the PNGs are in the artifact, one click away.
- **Package:** `dryflash` 0.1.0, 15 files, 233 kB, as a file on a GitHub Release and not on npm, by the maintainer's decision: no account or token beyond GitHub's. `npx` runs it from that URL, so
  BRIEF §10's command is now `dryflash run`, reached with `npx --package=<url>`. `"private": true` prevents an accidental `npm publish`. `prepack` refuses to pack without the core and copies the licence.
- **Releases** (`docs/RELEASE.md`, `.github/workflows/release.yml`): a version tag runs the tests and creates the release with the package attached.

Tests: the action's real script against a fake GitHub API for pass, hang, a read-only token and a bad input; comment escaping; 41 in `runner/test`. CI's `runner` job runs `uses: ./action` with this commit's package.

- **Not done yet:** the first release, the `v1` tag, and the milestone's definition of done, using the action from another repository. Until the first release, the action's default package does not exist.
- **Not tested:** `release.yml`, which can only run for real; a pull request from a fork, except with the fake API's 403; the comment as GitHub renders it, until this CI runs.

### Device skins

The body now follows the device's Color setting at once: white, black or transparent (ADR-0015, addendum), and it stays after a reload. New devices start white. The firmware still reads the colour from OTP at the next power-on,
as before. The board seen through the transparent body is decoration. E2E: the setting changes the drawing and survives a reload, on Chromium and WebKit. Checked by eye at desktop and phone widths.

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

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

## M2 — System bring-up (in progress)

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

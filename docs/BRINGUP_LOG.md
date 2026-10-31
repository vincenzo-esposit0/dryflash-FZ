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

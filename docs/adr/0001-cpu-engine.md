<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# ADR-0001 — CPU engine: custom Rust ARMv7E-M interpreter

- Status: **Accepted**
- Date: 2026-10-31
- Milestone: M0

## Context

dryflash must execute the real, unmodified STM32WB55 (Cortex-M4F) firmware in the browser,
deterministically, without SharedArrayBuffer (BRIEF §2, §3, §6). BRIEF §3 asks to compare:

- **(a)** a custom Rust ARMv7E-M interpreter compiled to `wasm32-unknown-unknown`, versus
- **(b)** Unicorn Engine 2 compiled with Emscripten in interpreter mode (no JIT in WASM).

Criteria: correctness on Thumb-2 + FPU, speed (MIPS in Chrome and Safari), WASM size,
debuggability, determinism, licence.

## What was measured

**Workload.** `tests/cpu-bench/fw` is a bare-metal Rust `no_std` program built for
`thumbv7em-none-eabihf` (LLVM, `-Os`, Cortex-M4). Per round it runs: CRC-32 over 4 KiB,
16×16 integer matrix multiply, insertion sort of 256 halfwords, 512 iterations of splitmix64
(64-bit multiplies, ADDS/ADC), and 512 iterations of an FPv4-SP loop
(VMUL/VADD/VDIV/VCMP/VCVT/VMRS). Its code uses ~70 distinct mnemonics, including IT blocks,
LDRD/STRD, UMULL, MLA/MLS, MOVW/MOVT. It writes a result block and stops at `BKPT #0`.
200 rounds = 106 357 887 retired instructions.

**Engine (a).** `core/src/cpu`: decode-once interpreter with a per-halfword decoded-instruction
cache for flash, a `Bus` trait monomorphised into the run loop, and FP on host `f32`.
Spike scope: user-mode integer ISA, IT, multiply/divide, LDREX/STREX, FPv4-SP data processing
and transfers. Exceptions and the remaining DSP encodings are M1 work; unimplemented encodings
stop execution (`Stop::Unimplemented`) and are never skipped.

**Correctness.** `tests/cpu-bench/unicorn_diff.py` runs the same image in native Unicorn 2.1.4
(Cortex-M4 model) and compares R0–R14, PC, NZCV and S0–S31 **before every instruction**:
**522 079 / 522 079 steps identical** for one round. (Unicorn does not report instructions
skipped inside IT blocks; 16 894 such steps were aligned, with all registers equal.)
Result blocks match for 1 and 200 rounds, natively and in every browser below.

**Speed** (Apple M5 laptop, macOS, 200 rounds, best of 5, MIPS = retired instructions / wall time):

| Host | Main thread | Web Worker |
|---|---|---|
| Native (aarch64, `--release`) | 170–220 | — |
| Node 23.7 (V8) | 157 | — |
| Chrome 156 | 159 | 153 |
| Safari 27.0.1 | 172 | 165 |
| Firefox 147 | 133 | 140 |

The first run in each browser is 20–40 % slower (tier-up); steady-state numbers are shown.
A busy Cortex-M4 at 64 MHz retires at most ~64 M instructions/s, so (a) runs at **≥ 2.2× real
time with the CPU 100 % busy** in all three engines, before WFI fast-forward (ARCHITECTURE §3.3)
and before any optimisation. Peripheral dispatch will reduce this in M2; it will be re-measured
and recorded in `docs/BRINGUP_LOG.md` at every milestone. Mobile is not measured yet (M3 preview).

**Size.** Benchmark harness (interpreter + spike bus), stripped: **46 KiB raw, 16 KiB brotli**.

## Option (b): Unicorn 2 → WebAssembly

Findings, verified 2026-10-31:

1. **Upstream Unicorn 2.1.4 has no interpreter backend.** `qemu/tcg/` contains only native code
   generators (no TCI), so upstream cannot be built for WASM. All WASM builds found are
   unofficial forks that backport TCI patches from the unmaintained unicorn.js
   (e.g. `elliott-wen/unicorn.wasm`, `petabyt/unicorn-wasm`). Depending on such a fork for the
   core of the product is a maintenance risk.
2. **Size.** The prebuilt ARM-only `unicorn_wasm_arm.wasm` in `elliott-wen/unicorn.wasm` is
   723 240 bytes uncompressed, before any STM32 peripheral model.
3. **Licence.** Unicorn is declared "GPLv2" with no "or later" grant (README, COPYING); we treat
   it as GPL-2.0-only, which **cannot be combined with and distributed in our GPL-3.0-or-later
   bundle**. This alone excludes shipping (b).
4. **Speed was not measured.** It would require building an unofficial fork with Emscripten
   and running third-party code; given points 1 and 3 the result could not change the decision.
   If the maintainer wants the number for completeness, it can be done as a separate task.
   For context only: native Unicorn with its JIT runs the same workload at ~440 MIPS — JIT is
   not available in WASM.

## Decision

Use **(a)**, the custom Rust interpreter. Unicorn stays a **development-only** reference for
differential testing (native, never distributed, never a runtime dependency).

## Consequences

- We own ISA correctness. Mitigation: per-encoding unit tests plus continuous differential
  fuzzing against native Unicorn (BRIEF §11), starting in M1.
- Full control of determinism (no host time, explicit FP semantics), debugging hooks
  (PC ring buffer, breakpoints, trace) and binary size.
- FP uses host IEEE `f32`. Still to model for exactness (M1): FPSCR cumulative exception flags,
  flush-to-zero (FZ), default-NaN (DN) and NaN propagation rules, VMLA double rounding — all
  covered by differential tests.
- The PPB (SCB, NVIC, SysTick, DWT, MPU, FPU control) belongs to the core, not to the system bus.
  In the spike only SCB->CPACR is intercepted; M1 routes the whole PPB to `sysctl/`.

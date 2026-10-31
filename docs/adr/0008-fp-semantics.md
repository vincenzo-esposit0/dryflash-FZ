<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# ADR-0008 — Floating point follows the ARM pseudocode, not the host

- Status: **Accepted**
- Date: 2026-10-31
- Milestone: M1

## Context

The M0 spike used host `f32` arithmetic directly. Differential fuzzing in M1 showed two problems:

1. **Wrong results.** ARM processes NaNs in a defined operand order and FPNeg flips the sign of a
   NaN (e.g. VNMLA = FPAdd(FPNeg(d), FPNeg(FPMul(n, m)))), while VSUB's FPSub processes NaNs on the
   original operands. Host propagation gave different NaN signs/payloads.
2. **Nondeterminism.** WebAssembly does not specify the sign or payload of NaNs produced by
   arithmetic, so results could differ between Chrome, Safari, Firefox and a native runner.

## Decision

`core/src/cpu/fp.rs` implements FPProcessNaNs (signalling first, operand order, FPSCR.DN), the
default NaN for invalid operations, flush-to-zero (FPSCR.FZ, IDC/UFC), and the cumulative
exception flags (IOC, DZC, OFC, UFC, IXC, IDC). Every instruction is composed exactly as its
pseudocode (FPAdd/FPSub/FPMul/FPDiv/FPSqrt/FPMulAdd plus FPNeg). The host only computes
operations whose inputs are not NaN; any NaN result is replaced by the ARM default NaN.
Inexactness is computed exactly (products in f64, TwoSum for sums) and only when it can still
change FPSCR, because the flags are cumulative.

## Consequences

- After this change, 240 000 differential fuzz cases against Unicorn 2.1.4 (88 608 of them on
  architecturally defined encodings) show no unexplained mismatch, FPSCR flags included.
- Still not modelled: FPSCR.RMode for arithmetic (round-to-nearest only; VCVT honours RMode).
  None of the target firmwares changes RMode; it is listed as a known gap.

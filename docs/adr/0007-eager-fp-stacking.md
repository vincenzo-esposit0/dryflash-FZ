<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# ADR-0007 — FP context stacking is eager, not lazy

- Status: **Accepted**
- Date: 2026-10-31
- Milestone: M1

## Context

With FPCCR.ASPEN = LSPEN = 1 (reset values, kept by Furi/FreeRTOS), a Cortex-M4 that takes an
exception while CONTROL.FPCA = 1 *reserves* space for S0–S15 and FPSCR in the extended frame
(EXC_RETURN bit 4 = 0) but only writes them when the handler executes its first FP instruction
(lazy state preservation, DDI 0403E B1.5.7, FPCCR.LSPACT/FPCAR).

## Decision

dryflash writes the extended frame at exception entry and restores it at return (eager), while
keeping the exact frame layout (0x68 bytes, 8-byte alignment, EXC_RETURN encodings).

## Rationale

- Memory contents seen by software after the handler's first FP instruction are identical, and
  that is the only point at which software can observe them (the FreeRTOS CM4F port saves
  S16–S31 itself in PendSV and relies on the hardware for S0–S15).
- Lazy stacking needs a per-instruction "first FP use" hook plus FPCAR/LSPACT state; eager
  stacking keeps the interpreter loop simpler and deterministic.

## Consequences

- FPCCR.LSPACT is never set and FPCAR is not updated. Software that reads them would see a
  difference; none of the target firmwares does (verify again if a crash report points there).
- Covered by the exception conformance firmware (tests/exc-fw, test T3).

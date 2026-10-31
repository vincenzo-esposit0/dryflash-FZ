<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# ADR-0009 — Unmodelled hardware: read-as-zero, write-ignored, always reported

- Status: **Accepted**
- Date: 2026-10-31
- Milestone: M1

## Context

Bring-up is driven by what the firmware touches (BRIEF §8). Before a peripheral has a model the
emulator must do *something* on access, without faking success (rule 3).

## Decision

1. **Peripheral blocks and the system-memory ROM** without a model read as zero and ignore
   writes (QEMU's "unimplemented device" semantics). Every access is recorded in the bring-up
   digest with block, offset, read/write counts, first PC and last value written.
2. **Unmapped addresses** raise a precise BusFault, as on hardware, and are recorded separately.
3. **Direct writes to flash/OTP** (without the FLASH controller, M2) are ignored and recorded.
4. **The internal PPB (0xE000_0000–0xE003_FFFF) is part of the CPU** (`sysctl`): SCB, NVIC,
   SysTick, MPU, FPU control, DWT, CoreDebug. Unimplemented PPB registers also read as zero and
   are reported. The external PPB (DBGMCU at 0xE004_2000, …) goes to the system bus.
5. **DHCSR.C_DEBUGEN reads 0**: dryflash does not present itself as an attached debugger, so
   firmware takes its non-debug paths (`furi_crash` resets instead of halting on BKPT).
6. **Encodings the decoder does not implement stop execution** (`Stop::Unimplemented`) instead
   of being treated as NOPs or faults.

## Consequences

- Polling loops on unmodelled ready flags spin or fail visibly; the digest shows exactly which
  register is involved (e.g. M1: `furi_check` in `furi_hal_bus_enable` because RCC enable/reset
  bits read as zero). This is the work list for M2.
- Read-as-zero is not a hardware value; reports label these accesses as emulator gaps.

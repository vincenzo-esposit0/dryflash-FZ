<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# ADR-0010 — Peripheral registers come from the SVD; behaviour is added per peripheral

- Status: **Accepted** (refines ADR-0009)
- Date: 2026-10-31
- Milestone: M2

## Context

ADR-0009 made every unmodelled peripheral register read as zero. That is not what hardware
does: registers have documented reset values and latch what software writes, and much early
init code is "write a configuration, read it back, modify it". Writing those register maps by
hand for ~50 peripherals is slow and error-prone. RM0434 could not be fetched automatically
(the ST site blocks scripted downloads), but ST's SVD description of the chip ships in the
pinned firmware tag: `scripts/debug/STM32WB55_CM4.svd` (v1.9) in flipperzero-firmware 1.5.1.

## Decision

1. `scripts/gen-svd-tables.py` generates `core/src/periph/svd.rs` from that SVD: for every
   register, its offset, reset value, readable bits and writable bits. Only hardware facts are
   extracted; the source is credited in CREDITS.md.
2. Every SVD peripheral on the system bus gets a `RegFile` with those reset values and masks.
3. A behavioural model (RCC first) owns its `RegFile` and adds side effects: ready flags,
   status bits, write-1-to-clear, interrupts, resets, clocks.
4. Accesses to registers without behaviour are reported in the digest as **"register file
   only"**; offsets with no register at all stay read-as-zero/write-ignored and are reported as
   before (ADR-0009). The hardware-fidelity table lists such peripherals as *Stubbed*.

## Consequences

- Correct reset values everywhere from day one; polling loops on status bits that only
  hardware sets still spin, and the digest shows exactly which register.
- Regenerating the tables for a new SVD version is a one-line command; diffs are reviewable.
- Register names in the digest come from the SVD (`I2C1.ISR` instead of `I2C1+0x018`).

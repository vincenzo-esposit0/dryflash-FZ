<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# ADR-0016 — A crash stops the emulator; hangs and verdicts are explicit heuristics

- Status: **Accepted**
- Date: 2026-11-02
- Milestone: M6

## Context

BRIEF §1 says the emulator stops and shows a report when firmware crashes, hangs, boot-loops or faults. §7 lists what to detect and
asks for a label: firmware bug or emulator limitation. Until now a Furi crash was recorded and the machine rebooted, as a device does.

Three things cannot be decided exactly: whether a firmware is hung, what a fault's cause was, and what happened just before a
`furi_check`, since Furi's crash code prints its whole record first and fills any short history of PCs.

## Decision

1. **A crash stops the run, after the firmware's own reset.** `RunExit::Crashed` returns with the report taken before the reset; running
   again continues into the reboot and the firmware's crash scene. A fault alone does not stop it: the firmware's handler runs, because it is
   what produces Furi's record. The fault's state is kept and joins the report if a crash, reset or hang follows within 2 s, or while the
   handler is still running.
2. **A hang is no WFI, no display change and no input** for 5 s of virtual time within 256 bytes of code (interrupts aside), or for 30 s
   anywhere. It is off when the firmware's Sleep Method is not Legacy, whose idle loop spins with no radio stack (ADR-0012).
3. **The verdict is a judgement with its reasons attached.** Firmware bug, emulator limitation or "could be either", with a `basis` list in the
   report. An unimplemented encoding is the emulator's. An undefined instruction could be either. A tight loop that keeps touching a register
   with no behaviour is probably the emulator's. A register with no behaviour first touched in the last second leaves it open. Otherwise it looks
   like the firmware's. The UI says it is not a proof.
4. **Crash code is watched only with an ELF.** Without one the record comes from the log, and the report says its PCs are Furi's printing.

## Rationale

- Stopping follows the brief and keeps the evidence on screen; Resume keeps the device's real behaviour one click away.
- Reading the UART record works on any firmware built from Furi, with no symbols.
- Timers on WFI cannot produce false alarms from a firmware that is idle: 2 virtual minutes on the four pinned firmwares and a full real OTA raised none.

## Consequences

- A task blocked for ever while the idle task sleeps is not detected. Input scripts catch that with timeouts (BRIEF §10).
- The thresholds and verdict rules are guesses to revisit with real crashes. A wrong verdict misleads, so the reasons must stay visible.
- `RunExit` has two more variants, and hosts must treat them as stops.

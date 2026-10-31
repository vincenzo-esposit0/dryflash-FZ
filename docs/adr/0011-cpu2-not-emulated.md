<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# ADR-0011 — CPU2 is not emulated; the firmware's "C2 startup failed" path is used

- Status: **Accepted**
- Date: 2026-10-31
- Milestone: M2

## Context

The STM32WB55's CPU2 (Cortex-M0+) runs ST's encrypted FUS and wireless stack. BRIEF §5.3 asks
for a behavioural stub so that CPU1 "either boots BT normally with the radio idle, or takes its
graceful BT unavailable path — whichever the firmware supports without crashing".

Firmware 1.5.1 (targets/f7/ble_glue/ble_glue.c) initialises the transport layer
(`TL_Init`, `shci_init`, `TL_MM_Init`, `TL_Enable` → `HW_IPCC_Enable`, which sets C2BOOT) and
then `ble_glue_wait_for_c2_start(timeout)` waits for the `VS_HCI_C2_Ready` system event. On
timeout it logs "C2 startup failed", sets `BleGlueStatusBroken` and returns false; the rest of
the system keeps running without Bluetooth.

## Decision

- HSEM and IPCC are modelled faithfully on the CPU1 side (1-step/2-step locks, channel
  occupancy, RX/TX interrupts), so CPU1 code that uses them behaves as on hardware.
- **No CPU2 model**: nobody consumes CPU1→CPU2 mailbox messages and no system event is sent.
  The firmware takes its own "C2 startup failed" path. The hardware-fidelity table lists CPU2 as
  *Stubbed — not emulated, no Bluetooth*.
- `Ipcc::cpu2_set/cpu2_clear` exist for a future CPU2 model.

## Rationale

A stub that makes CPU1 believe a radio stack is running must answer the full SHCI and BLE
HCI/ACI command set the stack is started with (`ble_app_init`, GAP/GATT setup, …) with
plausible payloads — a large surface whose correctness cannot be checked against the closed
stack. The firmware already supports a missing CPU2, so this path satisfies the brief without
faking success.

## Consequences

- Bluetooth features are unavailable in the emulator; the UI must say so (fidelity table).
- Secure-enclave key storage (routed through CPU2) is unavailable; firmware code that needs it
  takes its own error paths. Revisit if a target firmware crashes there.
- A radio-idle CPU2 model remains possible later (stretch), behind the same IPCC interface.

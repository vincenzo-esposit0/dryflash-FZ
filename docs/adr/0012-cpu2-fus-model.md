<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# ADR-0012 — CPU2 is a behavioural FUS with no wireless stack; the virtual device uses Legacy sleep

- Status: **Accepted** (supersedes ADR-0011)
- Date: 2026-10-31
- Milestone: M3

## Context

ADR-0011 left CPU2 absent, relying on the firmware's "C2 startup failed" path. In M3, with the
display and buttons working, the Desktop never reacted to input: `desktop_srv` deadlocks in
`furi_hal_crypto_enclave_verify`. `furi_hal_crypto_enclave_load_key`
(targets/f7/furi_hal/furi_hal_crypto.c, firmware 1.5.1) takes `furi_hal_crypto_mutex` and, when
`furi_hal_bt_is_alive()` is false, leaves through `break` without releasing it; the next key slot
waits on the same non-recursive mutex forever. Real devices always have a live CPU2, so the
firmware never meets this path on hardware.

The firmware does handle a CPU2 that is alive but has no usable radio stack: `ble_glue` accepts a
ready event from the FUS (`FUS_FW_RUNNING`), `ble_glue_force_c2_mode` reports
"Stack isn't installed!" when the stack version is 0.0, and `furi_hal_bt_start_radio_stack` keeps
SHCI running "for crypto enclave support". This is the state of a real device whose wireless
stack has been deleted.

## Decision

1. **CPU2 is modelled as ST's FUS with no wireless stack** (`core/src/bus/cpu2.rs`). It follows
   the transport layer in the pinned `lib/stm32wb_copro` (`tl_mbox.c`, `shci_tl.c`, `shci.c`,
   `mbox_def.h`, `tl.h`) and Flipper's `hw_ipcc.c`/`ble_glue.c`:
   - on `PWR_CR4.C2BOOT`: write `MB_FUS_DeviceInfoTable_t` (validity keyword `0xA94656B9`,
     FUS 1.2.0, stack fields 0, the device's UID64, device ID 0x6495) where the reference
     table's first pointer points, and post `SHCI_SUB_EVT_CODE_READY`/`FUS_FW_RUNNING` in the
     system spare buffer on the system event queue (IPCC channel 2);
   - system commands are answered synchronously at the IPCC write that posts them (CPU1 spins on
     the flag in `HW_IPCC_SYS_SendCmd`): `FUS_GET_STATE` → idle; **every other command fails with
     status 0x01**, including key load/store/unload (no factory keys, no user key storage) and
     wireless-stack commands such as `SetSystemClock`;
   - buffers released on channel 4 are taken back and the channel cleared;
   - BLE commands (channel 1) are never taken; the first one is reported in the digest.
   Every distinct SHCI opcode is reported once in the bring-up digest.
2. **The virtual device ships with Sleep Method = Legacy.** On power-on the RTC backup domain
   holds Furi's header (magic 0x10F1, version 0) and `SystemReg.flags = FuriHalRtcFlagLegacySleep`,
   all other fields at the defaults `furi_hal_rtc_reset_registers` writes. This is the user
   setting Settings → System → Sleep Method (applications/settings/system/system_settings.c);
   the firmware can change it like any other setting. It is configurable in `SystemBus`
   (`legacy_sleep`) and is shown in the UI's virtual-device settings.

## Rationale

- A FUS-only CPU2 is a real, supported device state with a small and fully documented command
  set (AN5185 and shci.h). Answering failure where the device has nothing (keys, stack) produces
  the firmware's own messages ("No Factory Keys Found — Secure Enclave is damaged") instead of a
  fake success.
- Emulating the wireless stack would mean answering the full BLE HCI/ACI command set with
  invented payloads; not justified for M3 (BRIEF §5.3 allows the "BT unavailable" outcome).
- Without a stack, default sleep (STOP2) asks CPU2 to switch the system clock (`SetSystemClock`,
  stack ≥ 1.20 only, targets/f7/furi_hal/furi_hal_clock.c). The FUS refuses, the clock stays on
  the PLL and `furi_hal_power_deep_sleep` returns without WFI: the idle task spins. That is what a
  stackless device does, but it halves emulation speed (45 vs 135 MIPS native, below real time)
  because WFI fast-forward never triggers. Legacy sleep (plain WFI) is a user-visible firmware
  setting with the same functional behaviour, so it is used as the virtual device's default.

## Consequences

- The Desktop, menus and apps run; Bluetooth is off ("Radio stack start failed"); apps that
  need the secure enclave (factory or user keys) report their own errors.
- First boot shows the firmware's "No Factory Keys Found" popup; Back closes it.
- The fidelity table lists CPU2 as *Stubbed — behavioural FUS, no wireless stack*.
- A future wireless-stack model can replace the FUS model behind the same mailbox code.

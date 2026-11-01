<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# ADR-0014 — Real OTA runs the firmware's own updater, without installing the radio stack

- Status: **Accepted** by the maintainer; it qualifies the M4 definition of done
- Date: 2026-11-01
- Milestone: M4

## Context

BRIEF §4 and the M4 definition of done ask for a Real OTA: copy the package to `/ext/update/`, let
the firmware's own updater do the update, "exactly like a real device". Official packages contain a
radio stack. With CPU2 modelled as a FUS without a stack (ADR-0012), the updater's radio stage cannot
finish: it writes `radio.bin` to flash, asks the FUS to install it, and expects to come back with
that stack running. Faking it would need a CPU2 that claims a stack it does not run; a firmware that
then boots normally calls `ble_app_init`, which fails, `ble_glue_stop` frees the glue,
`furi_hal_bt_is_alive()` turns false and the Desktop deadlocks (the ADR-0012 bug).

## Decision

- The package is staged with its radio entries emptied, exactly how `scripts/update.py` writes a
  package without a radio (`Radio:` empty, address, version and CRC zero; the keys must stay, because
  `update_manifest_init` reads them in order). Everything else is byte-identical to the package.
- The firmware's updater then does all of it for real, from its own code: `/int` backup, loader CRC
  check, execution from RAM, option-byte validation, DFU validation, flash programming and read-back,
  `/int` restore, old-resource cleanup via Manifest, resource unpack, splash.
- The UI says the radio stack is not installed. **The radio stage of a package is not tested.**
- Staging ends where `update_operation_prepare` does: the `/ext/.fupdate` pointer and boot mode
  PreUpdate, then a reset. The checks that function does are repeated by the updater itself.
- The virtual device's option bytes are the `OB reference` of the packages (identical in all four
  pinned ones), as on a production unit.

## Consequences

- Real OTA checks firmware, option bytes and resources of a package; radio problems are not caught.
- The updater clears `FuriHalRtcFlagLegacySleep`. The device setting holds: when the machine resets
  into boot mode PostUpdate with the setting on, the flag is set again (ADR-0012).
- If a wireless-stack model ever exists, the radio stage can be enabled behind the same staging code.

<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# Credits

dryflash is an independent project. It is not affiliated with or endorsed by Flipper Devices Inc.
Flipper Zero is a trademark of its respective owner.

## Reference material

- Flipper Zero firmware — <https://github.com/flipperdevices/flipperzero-firmware> (GPL-3.0). Ground truth for the memory map, board wiring and update package format.
- ARM: ARMv7-M Architecture Reference Manual (DDI 0403E), Cortex-M4 programming manual (PM0214).
- ST: RM0434 (STM32WB55 reference manual) and the STM32WB55 datasheet.

## Prior art studied (no code ported so far)

- tITAZ314/flipper-emu (GPL-3.0) — Renode-based bring-up of the official firmware.
- d4rks1d33/Flipper-Zero-Emulator (GPL-3.0) — full-system emulator with persistent flash.

## Hardware descriptions

- STMicroelectronics `STM32WB55_CM4.svd` v1.9, as shipped in flipperzero-firmware 1.5.1
  (`scripts/debug/`). `core/src/periph/svd.rs` is generated from it (register offsets, reset
  values, access masks only) by `scripts/gen-svd-tables.py`.

## Development tools (not distributed)

- Unicorn Engine 2 (GPL-2.0) — reference CPU for differential testing only (ADR-0001).

## Libraries (distributed with the site)

- xterm.js (`@xterm/xterm` 6.0.0, `@xterm/addon-fit` 0.11.0), MIT licence — the CLI terminal.

## Reference material for USB

- libusb_stm32 (Apache-2.0), the USB stack inside the firmware (`lib/libusb_stm32`), read to model the USB peripheral as its
  STM32WB55 driver uses it (ADR-0021). No code ported.

## Fonts (distributed with the site)

- JetBrains Mono, Share Tech Mono, IBM Plex Sans (SIL Open Font License 1.1), through the `@fontsource` npm packages.

If code or models are ported from any project, its copyright notice is kept and it is listed here.

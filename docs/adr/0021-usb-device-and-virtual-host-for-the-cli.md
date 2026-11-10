<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# ADR-0021 — The CLI runs over an emulated USB device and a virtual host

- Status: **Accepted**
- Date: 2026-11-10
- Milestone: M7 checkpoint 4

## Context

BRIEF §5 asks for "USB FS device + CDC → xterm.js CLI". In firmware 1.5.1 the CLI only runs on the USB serial port:
`applications/services/cli/cli_vcp.c` switches USB to `usb_cdc_single` and starts a shell when the computer raises DTR
(`SET_CONTROL_LINE_STATE`). USART1 carries only the log.

The USB driver is libusb_stm32 (`lib/libusb_stm32`, submodule at 6ca2857, `usbd_stm32wb55_devfs.c`) under
`targets/f7/furi_hal/furi_hal_usb.c` and `furi_hal_usb_cdc.c`. The CDC data endpoints are **double-buffered bulk**
endpoints (`USB_EPTYPE_BULK | USB_EPTYPE_DBLBUF`), EP0 has 8-byte packets, both USB interrupts call `usbd_poll`, and SOF
interrupts are on.

The options were:

1. catch the shell above the driver (`cli_vcp`'s pipe, or `furi_hal_cdc_send` / `furi_hal_cdc_receive`) and feed it bytes;
2. model the USB peripheral and put a host on the other end of the cable.

## Decision

**Model the peripheral, and add a virtual host.**

- **USB device** (`core/src/periph/usb.rs`): the eight USB_EPnR with their bit types (rc_w0, toggle, read/write,
  read-only); CNTR; ISTR with CTR, DIR and EP_ID computed from the endpoints; FNR, DADDR, BTABLE, BCDR.DPPU; the 1 KiB
  packet memory at 0x4000_6C00 with its buffer table; the clock and reset from RCC (APB1ENR1/APB1RSTR1 bit 26). The bus
  side — SETUP, OUT and IN transactions, bus reset, start of frame, suspend, wakeup — is called by the host. Everything
  goes to the USB_LP interrupt (20); the firmware handles USB_HP the same way.
- **Virtual host** (`core/src/board/usb_host.rs`): the computer at the other end. With the page's USB cable in and
  D+ pulled up, it waits 100 ms, resets the bus for 10 ms, then sends a start of frame every millisecond and enumerates:
  device descriptor at address 0, SET_ADDRESS(1), device and configuration descriptors, product and serial strings,
  SET_CONFIGURATION(1). It finds the CDC ACM function in the configuration descriptor. Opening the port in the page sends
  SET_LINE_CODING (115200 8N1) and SET_CONTROL_LINE_STATE with DTR and RTS; closing it drops DTR. Bulk OUT carries what
  is typed; bulk IN is polled while the port is open. Three milliseconds of idle bus (no cable, or no pull-up) flag
  SUSP, as USB 2.0 §7.1.7.6 has it.
- **Timing**: transactions are not bit-timed. The host runs at each frame and 10 µs after the firmware writes a USB
  register, and moves one packet per endpoint at a time until the device NAKs.
- **Page**: a **CLI** tab with xterm.js 6.0.0 and its fit addon (MIT). The connection state and the device's product and
  serial strings come from the host. Connections and disconnections go to the Events panel. One terminal for the page,
  so its scrollback survives switching tabs.

Option 1 was rejected for the same reason as ADR-0020's option 1: it would show what the CLI meant to send, not what the
firmware's USB stack produces. It would also break on every firmware that changes the shell's internals.

### Double-buffered bulk endpoints

RM0434 says only that a double-buffered endpoint's status is "masked" as NAK when DTOG and SW_BUF are equal. It does not say
what a read of the register shows then. The driver that runs on the chip settles it:

- `ep_read` reads a double-buffered **OUT** endpoint only while STAT_RX reads VALID, including when both buffers are full.
  So the peripheral leaves STAT_RX alone and NAKs on the bus.
- `ep_write` writes a double-buffered **IN** endpoint only while STAT_TX reads NAK, and only toggles SW_BUF afterwards. It
  never sets VALID again. So on the chip the register must read NAK while no buffer is ready, and a filled buffer must still go
  out.

Modelled accordingly. For OUT, STAT_RX is unchanged and the token is NAKed. For IN, the peripheral sets STAT_TX to NAK when it
has nothing to send, and a NAK it set itself does not hold back a buffer the application has filled. A NAK written by
software does hold it back.

## Consequences

- The firmware's whole USB stack runs: enumeration, its reconnect timer (`furi_hal_usb_thread`), suspend and wakeup
  callbacks, the CDC callbacks and the CLI's pipe. Other USB modes (HID, U2F, the dual CDC used by qFlipper's RPC) use the
  same peripheral. They are not exercised by a host yet.
- With the cable in, the device gets 1000 SOF interrupts a second, as on the hardware. The emulator stays at 1.00× speed in
  the browser.
- Not modelled: isochronous endpoints, LPM, battery-charging detection (BCDR reads back what was written, so the driver's
  `connect()` reports "no data contact", which it ignores), data-toggle errors, the USB_HP line. The host does not send
  protobuf RPC (qFlipper's protocol), which stays a stretch goal.

## Inline styles in the CSP

xterm.js inserts `<style>` elements: the scrollbar, the cell size, and the colour theme (`DomRenderer._injectCss`). It
also sets `style` attributes on the cells it renders. It has no option for a nonce, and that content is computed at run
time, so hashes cannot cover it. With `style-src 'self'` the browser blocks all of it. The terminal still worked, but without
the firmware's ANSI colours, and every page load logged CSP violations.

Decision (maintainer, 2026-11-10): the main page gets `style-src 'self' 'unsafe-inline'` in `web/public/_headers`.
`script-src` is unchanged, so no inline script runs. `/embed` has no terminal and keeps `style-src 'self'`. The risk this
adds is CSS injection, which needs markup injection first, and Preact escapes text. The rejected alternative was a terminal
of our own in place of xterm.js, which BRIEF §3 names.

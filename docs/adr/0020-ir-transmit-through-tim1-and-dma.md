<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# ADR-0020 — IR transmit runs the firmware's own TIM1 and DMA path

- Status: **Accepted**
- Date: 2026-11-09
- Milestone: M7 checkpoint 3

## Context

BRIEF §5 asks for "IR TX decoded and shown in an event log". The firmware (1.5.1, `targets/f7/furi_hal/furi_hal_infrared.c`)
does not drive the IR LED from code. `furi_hal_infrared_async_tx_start` sets things up so that:

- TIM1 counts at the carrier frequency, with channel 3N on PB9 (alternate function 1) in PWM mode 2;
- two DMA2 channels are both requested by TIM1's update event (DMAMUX `LL_DMAMUX_REQ_TIM1_UP`, 0x19). At each update, channel 1
  writes CCMR2 from a buffer of "carrier on" and "forced inactive" modes, and channel 2 writes RCR from a buffer of period counts;
- the next update comes RCR + 1 carrier periods later, so a mark or space lasts that long;
- the buffers are double: the channel 2 half-transfer interrupt refills the other buffer, and transfer-complete swaps them.
  After the last one, `furi_hal_infrared_tx_dma_terminate` stops the counter.

Until now TIM1 was a register file only, DMA moved data only for SPI, and the DMA model raised only the transfer-complete interrupt.

The two options were:

1. catch the firmware above the HAL, at the encoder or at `furi_hal_infrared_async_tx_start`, and read the timings from its buffers;
2. model what the HAL uses, then watch the pin.

## Decision

**Model the hardware path and watch the pin.**

- **TIM1** (`core/src/periph/tim1.rs`) runs update events on the virtual timeline from PSC, ARR and the repetition counter.
  ARR, PSC and RCR take effect at an update, as their preload does in the chip. It also models EGR.UG, SR.UIF (rc_w0) and the
  update DMA request. Channel 3N is computed from CCER, BDTR.MOE and the OC3M mode, either PWM with a frequency and duty or
  forced. CNT is not readable, and there are no capture/compare events and no TIM1 interrupts: the IR driver uses none of them.
- **DMA** serves TIM1's update requests one item at a time, on every enabled channel requested by TIM1_UP. A narrower source is
  zero-extended. CNDTR counts down. HTIF is set when half the programmed count has moved and TCIF at the end. Every flag with its
  enable (TC, HT, TE) raises the channel's interrupt.
- **The bus** runs TIM1's update events one by one, because each one's DMA rewrites RCR, which sets the time of the next. The
  write to TIM1 carries the update's own time.
- **The IR LED** (`core/src/board/infrared.rs`) is channel 3N whenever PB9 is in alternate function 1. Its on/off edges become
  a transmission: the carrier and duty of the first mark, then mark, space, …, mark in µs. A transmission ends when the timer
  stops; the space the firmware sends only to finish its buffer is left out.
- **The page** lists each transmission in the Events panel. "Save as .ir" downloads it as a raw entry of a Flipper `.ir`
  file, so it can be put on a device's card and replayed from the Infrared app. A download rather than the clipboard: writing
  to the clipboard needs a permission that an embedding page or a strict browser may not give.

Option 1 was rejected because it would show what the firmware meant to send, not what the HAL, timer and DMA produce. It would
need firmware symbols, and it would work only for firmwares that keep those functions.

## Consequences

- The firmware runs its own interrupt-driven refill. A transmission longer than one buffer (200 timings) goes through the same
  half-transfer and transfer-complete path as on the device.
- The timings are whole carrier periods, as the firmware rounds them (`furi_hal_infrared_tx_fill_buffer`). An NEC preamble of
  9000 µs comes out as 8999 µs at 38 kHz.
- Not modelled: protocol names (the event shows raw timings; the Infrared app on the device can decode a copied signal); the
  external IR module output on PA7 (TIM1 channel 1N), because the virtual board has no module there; IR receive (M8).
- 125 kHz RFID also uses TIM1 with DMA (`furi_hal_rfid.c`). It now gets update events and DMA too, which it did not before.
  Nothing reads its output yet.

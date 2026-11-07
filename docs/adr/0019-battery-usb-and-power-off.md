<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# ADR-0019 — Battery, USB and power-off follow the charger's registers

- Status: **Accepted**
- Date: 2026-11-07
- Milestone: M7 checkpoint 2

## Context

Until M6 the page set four things about the battery: charge, voltage (derived), "charging" and "USB plugged in". The
charger model returned zeros for its ADC registers, so the firmware's Battery Info showed 0 V on USB, and "charging" could be
ticked with no cable, a state the real charger never reports. Power → Power OFF ended in Furi's halt, which the emulator
reported as a crash, though on the device it is an ordinary power-off.

The firmware (1.5.1) does the following:

- `furi_hal_power_is_charging` reads the charger's CHRG_STAT; the power service lights the LED red while charging and green
  when done (`power.c`, `power_check_charging_state`). Sub-GHz suppresses charging by clearing CHG_CONFIG.
- `furi_hal_power_off` sets BATFET_DIS. With USB in, the system stays up, so the firmware halts on "It's now safe to
  unplug the USB cable" (`power_handle_shutdown`). With a flat battery, the power service powers off after a 90 s warning
  if the gauge raises SYSDWN and VBUS is below 4 V (`power_check_low_battery`).

TI's BQ25896 datasheet (SLUSC76C, May 2018) gives the reset values (§9.4), ADC scales (§9.4.15–9.4.19) and ship mode
(§9.2.10): BATFET off cuts the battery from the system, and plugging in an adapter, clearing BATFET_DIS, REG_RST or the QON
pin turns it back on.

## Decision

- **The page sets only what a person controls:** charge in percent and whether a USB cable is plugged in. Whether the
  battery charges is computed from USB, CHG_CONFIG and the charge (done at 100 %), and the gauge reports the same.
- **The charger's ADC registers** are computed from that state with the datasheet's scales; the reset values are the
  datasheet's. Read-only registers ignore writes; reads past REG14 return 0xFF (§9.2.16).
- **Ship mode is power-off.** When BATFET_DIS is set and USB is out, the machine is off: nothing runs, virtual time still
  passes, the LCD, backlight and LEDs are dark and the speaker is silent. RAM is lost. The Furi halt that precedes it with
  USB in is not a crash, and neither is the idle wait.
- **Power comes back** when USB is plugged in (a power-on reset, as on the charger) or with the page's **Power on**, the same
  power-on reset as the existing restart. How the real device's button reaches the charger (QON or otherwise) is not in the
  firmware source, so the page does not claim which button it is.
- **SYSDWN at 0 %.** The real gauge raises it below a voltage in its data memory that the firmware does not provision, so
  the value is unknown here. 0 % is a choice of the emulator, made so a flat battery shows the firmware's own warning.

## Consequences

- Battery Info shows plausible VBUS, battery voltage, current and temperature from both chips; neither is a physics model.
- The old per-device setting `charging` is ignored. Stored devices keep their charge and USB settings.
- Not modelled: the charger's I2C watchdog, faults, OTG boost, input-current optimisation, BATFET_DLY's delay, the
  gauge's own voltage thresholds and anything that changes the charge over time.

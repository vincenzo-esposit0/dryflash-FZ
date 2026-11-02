<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# dryflash runner

Smoke-tests a Flipper Zero firmware build with no hardware, by running the real binary in the dryflash emulator.
It is a smoke test, not a hardware certification: passing here does not guarantee it works on a device.

```bash
dryflash run <firmware.tgz|.dfu> [--elf f.elf] [--script smoke.yaml] [--timeout 300] [--junit out.xml] [--screenshots dir/] [--report out.json] [--update-goldens]
```

Times are virtual: the same firmware and script give the same result however fast the machine is. `--timeout` is in seconds
of virtual time, for the whole script.

| Exit code | Meaning |
| --- | --- |
| 0 | Passed |
| 1 | Firmware failure: a crash, a hang, a check that did not hold, or time ran out |
| 2 | Emulator limitation: the crash report says the emulator looks responsible |
| 3 | Usage error: nothing was judged |

The label on a crash is a judgement, with its reasons in the report (ADR-0016), and exit codes 1 and 2 follow it.

## With no script

The default smoke test boots, reaches the Desktop, opens every entry of the main menu, backs out of each, and fails on any crash or
hang. It works out the menu's shape from the screen, so it handles lists, grids and carousels.

## Scripts

```yaml
- wait_for_desktop                 # the firmware's log says so; closes the "No Factory Keys" popup
- press: [down, ok]                # one after the other; each is a short press plus 400 ms
- long_press: back
- wait: 2s                         # or 500ms
- expect_screen: golden.png        # relative to the script; PNG of 128×64 or a whole multiple
- expect_screen: { file: golden.png, tolerance: 1% }   # or a number of pixels
- expect_screen_change             # differs from the end of the step before
- expect_log: some text            # already in the firmware's log
- wait_for_log: some text          # up to 60 s
- screenshot: name                 # into --screenshots
- walk_menu                        # the default smoke test's walk, from the Desktop
- expect_no_crash
```

`--update-goldens` writes the current screen to each `expect_screen` file instead of comparing.

With `--screenshots`, a stop also leaves `crash.json`, `crash.md` and `crash.png`, and every run leaves `digest.txt`: what the firmware touched
that the emulator does not model. `--elf` names functions in the report's backtrace, and gives it the instructions leading to a `furi_check`.

## What it cannot see

No radio signal, NFC field, Bluetooth or factory keys: those apps open and show what a device without them would. A task blocked for ever
while the rest of the firmware idles is not a hang to the detector, so give your script steps that would notice.

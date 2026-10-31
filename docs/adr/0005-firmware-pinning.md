<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# ADR-0005 — Pinned firmware versions and the no-binaries rule

- Status: **Accepted** (recommendation in the approved M0 plan)
- Date: 2026-10-31

## Decision

`firmware/compat.json` pins, by URL, SHA-256 and size (verified 2026-10-31):

| Firmware | Version | Resources archive |
|---|---|---|
| Official | `1.5.1` (+ ELF for symbolication) | `resources.ths` |
| Momentum | `mntm-012` | `resources.tar.gz` |
| Unleashed | `unlshd-094` | `resources.ths` |
| RogueMaster | `RM1024-1347-392a65c3` | `resources.tar.gz` |

The nightly matrix (M6) additionally tests the "latest release" of each project, resolved at run
time (official: `directory.json` channel `release`; others: GitHub latest release), and records
the resolved version and hash in the published `/compat` data.

Binaries are downloaded by CI and tests into a cache (`firmware/cache/`, git-ignored), verified
against the pinned hash, and never committed or re-hosted.

## Facts observed in these packages (inputs for the loader, M1/M4)

- All four: `update.fuf` `Version: 2`, `Target: 7`, one DfuSe target with one element at
  `0x08000000`, initial SP `0x20030000`, `Radio address: 0x080D7000`, identical `Radio version` bytes
  `01 14 00 00 02 03` (field meaning to be taken from the firmware's update tooling, not guessed).
- Application images end between `0x080BC4D4` (Official) and `0x080D6164` (Momentum): only 3.7 KiB
  below the CPU2 region. The flash model must enforce the `SFSA` protection boundary exactly.
- Momentum and RogueMaster ship `resources.tar.gz` and an extra `firstboot.bin`; Official and
  Unleashed ship heatshrink-compressed `resources.ths`. The loader follows the `Resources:` key in
  `update.fuf` and supports both formats.
- All four projects are GPL-3.0 (GitHub licence metadata); the UI shows each firmware's licence and source link.

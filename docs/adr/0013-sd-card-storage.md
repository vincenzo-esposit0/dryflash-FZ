<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# ADR-0013 — The SD card is a sparse block store owned by the core; the host persists dirty blocks

- Status: **Accepted**
- Date: 2026-11-01
- Milestone: M4

## Context

ARCHITECTURE §8 planned `sd_read_block`/`sd_write_block` callbacks into the worker, backed by
`FileSystemSyncAccessHandle`. That makes every card access cross the wasm boundary, ties the core
to a host mechanism with an IndexedDB fallback that cannot be synchronous, and breaks the core's
determinism and its native tests. The SD driver (`furi_hal_sd.c`) only uses single-block commands,
and real usage of a card is far smaller than its capacity.

## Decision

- `core/src/board/sdcard.rs` holds a **sparse block store** in memory: unwritten and all-zero blocks
  read as zero and take no memory. The default virtual card is **1 GiB**.
- The host loads the stored blocks at start and persists `take_dirty()` blocks between slices
  (OPFS, IndexedDB fallback; checkpoint 3 of M4). The core never calls out.
- A new card is prepared by `core/src/board/fat32.rs`: MBR, one FAT32 partition at 1 MiB, 4 KiB
  clusters, like a real card, so the firmware's own FatFs mounts it and an exported image opens on
  a PC. No formatting step is asked of the user.
- Card access and DMA transfers are instantaneous in virtual time.

## Consequences

- Memory grows with the data on the card, not with its capacity.
- Writes reach persistent storage after the slice, not at the instant of the write: closing the tab
  mid-slice can lose at most the last slice, which a real card pulled mid-write can also do.
- Export needs only the stored blocks; a whole-card image is sparse where the host allows it.

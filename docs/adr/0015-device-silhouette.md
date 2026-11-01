<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# ADR-0015 — Pixel-art device modelled on the real one, with "FL1PP3R" on the body

- Status: **Accepted** by the maintainer, who owns this decision and its risk
- Date: 2026-11-01
- Milestone: M5
- Changes: BRIEF §9, "Main screen layout"

## Context

BRIEF §9 asked for an original, clearly distinct shell, not copying Flipper Devices' industrial design.
The maintainer asked for a pixel-art device based on a picture of the real one, so that the site feels
like using the real thing, and for an organisation of the page modelled on how qFlipper is used. A first
version with a flat shell tilted by CSS and "dryflash" on the body was rejected as too far from the picture.
The risk was explained: a look-alike of a commercial product can raise trade-dress and trademark questions
and can make a site look official.

## Decision

- The device is pixel art in the style of the maintainer's reference picture: angled, a thick body, bevels,
  screen on the left, a round D-pad on the right, an orange LCD, on a sky-blue stage. It is rasterized by code
  from a model (`web/src/ui/shell.ts`), with art pixels aligned to the screen; the real LCD canvas and buttons
  sit on the same face through the same projective map. A front view is one tick away, and phones get it
  stacked.
- **The body says "FL1PP3R"**, by the maintainer's explicit choice (`BODY_NAME`, one constant). The maintainer
  was told that this does not avoid the trademark and may read as a deliberate imitation of the name.
- **The product stays dryflash** in its name, title, logo text, URLs and documentation. No mascot, and no artwork
  or files from Flipper Devices or from the picture, whose rights are unknown. The "not affiliated" disclaimer
  stands.
- The page takes from qFlipper how it is used (device information, update, files, backup), not its
  graphics or files.

## Consequences

- Should Flipper Devices object, the word and the shape must change: both are in `shell.ts` alone.
- The disclaimer is on every page except the bare embed frame, where the host page is responsible.

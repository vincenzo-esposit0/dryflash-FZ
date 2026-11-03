<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# ADR-0018: The site takes the look of the Firmware Hub

**Status:** Accepted

## Context

The first version of the page stacked everything in one column: at 1024×768 the only way to load a firmware was about
1080 px down, below a row of disabled buttons. The maintainer also asked for the visual style of the Firmware Hub, the
catalogue that links here, so the two read as neighbours.

## Decision

- **Tokens, not code.** Colours, fonts, type scale and square edges come from the Hub's `tailwind.config.js` and are restated as CSS
  variables in `web/src/style.css`. No Hub code is copied and Tailwind is not added.
- **Fonts are served by dryflash**, through the `@fontsource` npm packages (OFL-1.1, credited in `CREDITS.md`), not from
  Google Fonts as on the Hub. The CSP allows no other host, and the site's promise is that nothing about a visit goes
  elsewhere. Vite inlines nothing (`assetsInlineLimit: 0`), because the CSP has no `data:` for fonts.
- **The firmware comes first.** An empty device shows the pinned list from `firmware/compat.json`, one click each, with
  what the nightly run saw (ADR-0017). A firmware is still downloaded from its own host or through the proxy (ADR-0002),
  never hosted. Dropping a file works anywhere on the page.
- **Two columns from 960 px:** the device on the left, on the right the firmware list or, with a firmware running, its
  controls. The panels go below at the full width of both. Below 960 px, one column with the list first.
- **IT / EN switch, as on the Hub.** The choice is kept in `localStorage`; without one the browser's language decides.
  Switching redraws the page rather than reloading it, so a running firmware does not restart.
- **Nothing on the page claims more than is true.** The screen stays empty until the firmware draws on it. "Starting the
  firmware" ends at its first pixels, the one signal every firmware gives. The terminal line above the device states the
  loaded file and is decoration, hidden from screen readers; it is not a console.

## Consequences

- A few hundred KB of fonts in the build, of which a visit loads four files, about 90 KB.
- Style changes on the Hub do not reach this site by themselves.
- The Hub's sound and CRT toggles are not carried over.
- Text from the core (fidelity notes, crash reports) stays in English in both languages: it is the single source for the
  files people download and for the table, and a copy per language would drift. The Italian page says so.

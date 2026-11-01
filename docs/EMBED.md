<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# Embedding dryflash

## "Try in browser" badge

For a firmware or app repository's README. Replace the `fw` value with the URL of your release's
update package (`.tgz`) or `.dfu`, URL-encoded:

```markdown
[![Try in browser](https://app.dryflash.workers.dev/badge.svg)](https://app.dryflash.workers.dev/?fw=https%3A%2F%2Fexample.com%2Fupdate.tgz)
```

## Loading by URL

| Parameter | Meaning |
|---|---|
| `fw` | URL of a `.tgz` or `.dfu` (https) |
| `elf` | URL of the firmware's ELF, for symbols in the bring-up digest |
| `mode` | `quick` (default) or `ota`: the running firmware's own updater (needs a firmware on the device) |
| `autoboot` | `1`: start without asking |
| `device` | Name of the visitor's virtual device to use |

A link can never overwrite a visitor's device by surprise. Without `device`, firmware goes on a device
called "From a link". With `device` naming one that already has a firmware, the page always asks,
whatever `autoboot` says.

Files are fetched directly from their host. Where the host sends no CORS headers (GitHub release assets)
the page falls back to dryflash's proxy, which only serves release assets of the projects in its allowlist
(`proxy/src/rules.ts`; ask to be added) and says so on the page. dryflash hosts no firmware.

## The embed page

```html
<iframe src="https://app.dryflash.workers.dev/embed?fw=https%3A%2F%2Fexample.com%2Fupdate.tgz"
        width="560" height="620" title="dryflash" style="border:0"></iframe>
```

The frame shows only the device. It keeps nothing: its state lasts as long as the frame, and it cannot
reach the visitor's saved devices.

### Messages out

Posted to the parent window, each with `source: "dryflash"`:

| `type` | Fields | When |
|---|---|---|
| `ready` | `version` | The emulator is up |
| `booted` | | The firmware logged that its services started |
| `desktopReached` | | The firmware selected a Desktop animation (provisional, see below) |
| `crashed` | `summary` | Furi recorded a crash, or the emulator stopped |
| `frame` | `bits` (ArrayBuffer) | Only with `&frames=1`, at most 5 per second. 1 KiB: rows top to bottom, MSB leftmost, 1 = dark |
| `screenshot` | `png` (ArrayBuffer) | Answer to the command |
| `error` | `message` | Loading or running failed |

### Commands in

Accepted only from the parent window:

```js
const frame = document.querySelector("iframe").contentWindow;
frame.postMessage({ type: "load", url: "https://example.com/update.tgz" }, "*");
frame.postMessage({ type: "press", button: "ok" }, "*"); // a tap; `down: true/false` to hold
frame.postMessage({ type: "reset" }, "*");
frame.postMessage({ type: "screenshot" }, "*");
```

`button` is one of `up`, `down`, `left`, `right`, `ok`, `back`. `load` also takes `bytes` (ArrayBuffer)
and `name` instead of `url`.

`booted` and `desktopReached` come from the firmware's log, which custom firmwares could change. The
firmware-independent detection is decided in M6.

## Licence

GPL-3.0-or-later. Not affiliated with or endorsed by Flipper Devices Inc.

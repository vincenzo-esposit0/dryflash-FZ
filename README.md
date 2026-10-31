<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# dryflash

**dryflash — an emulator for Flipper Zero firmware that runs entirely in your browser.**
Try any Flipper Zero firmware before flashing it to your real device: dryflash executes the
real, unmodified release binaries (`.dfu` / update `.tgz`) on an instruction-level model of the
STM32WB55, with no backend and no hardware.

> **Status: pre-alpha (milestone M2).** In the emulator core the official firmware boots to its
> services and runs stably (no display, input or SD card yet; no web UI yet). See [docs/ROADMAP.md](docs/ROADMAP.md).

## What it is — and what it is not

dryflash is a **smoke test**, not a hardware certification. It catches boot failures, crashes,
faults, hangs and broken UI flows. It does **not** reproduce real RF, NFC/RFID physics, BLE,
power behaviour or exact timing. Passing in the emulator does not guarantee the firmware works
on hardware.

## Repository layout

| Path | Contents |
|---|---|
| `core/` | Rust emulator core (CPU, bus, peripherals, board devices, loaders) |
| `core-wasm/` | WebAssembly bindings |
| `runner/` | Headless Node CLI (`npx dryflash run …`) — M6 |
| `web/` | Static web app (Vite + Preact) |
| `proxy/` | Allowlisted CORS proxy (Cloudflare Worker) — M5 |
| `action/` | GitHub Action — M6 |
| `firmware/compat.json` | Pinned firmware packages (URLs + hashes; binaries are never committed) |
| `tests/` | Integration tests and the CPU benchmark |
| `docs/` | Brief, architecture, roadmap, bring-up log, ADRs |

## Development

Requirements: `rustup` (the toolchain is pinned in `rust-toolchain.toml`), Node.js ≥ 20.

```sh
tests/build-fw.sh                              # bare-metal test firmwares (benchmark, exception model)
node scripts/fetch-firmware.mjs official --elf # pinned official firmware, hash-checked, into firmware/cache/
cargo test --workspace -- --include-ignored    # core tests, incl. booting the official firmware
cargo install --locked wasm-bindgen-cli --version 0.2.130
scripts/build-wasm.sh                          # wasm core + JS bindings for web/ and runner/
npm install && npm test                        # web tests
npm run dev -w web                             # emulator at http://localhost:5173 (drop a .dfu on the page)
npm run build                                  # static web build in web/dist
```

Bring-up tool: `cargo run --release -p dryflash-core --example boot -- <firmware.dfu> --elf <firmware.elf>`
prints the boot milestones and the bring-up digest. See [docs/BRINGUP_LOG.md](docs/BRINGUP_LOG.md).
```

## Licence

GPL-3.0-or-later. See [LICENSE](LICENSE) and [CREDITS.md](CREDITS.md).

Not affiliated with or endorsed by Flipper Devices Inc. Flipper Zero is a trademark of its
respective owner. For educational and authorised security research only.

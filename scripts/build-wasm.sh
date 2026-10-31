#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Builds core-wasm and generates the JS bindings used by the web app and the runner.
# Needs: rustup target wasm32-unknown-unknown, wasm-bindgen-cli matching core-wasm/Cargo.toml
#   cargo install --locked wasm-bindgen-cli --version 0.2.130
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build --release --target wasm32-unknown-unknown -p dryflash-core-wasm
wasm=target/wasm32-unknown-unknown/release/dryflash_core_wasm.wasm
wasm-bindgen --target web --out-dir web/src/wasm --out-name dryflash "$wasm"
wasm-bindgen --target web --out-dir runner/wasm --out-name dryflash "$wasm"
ls -l web/src/wasm/dryflash_bg.wasm

#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# Builds the ADR-0001 benchmark firmware and extracts tests/cpu-bench/bench-fw.bin.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
cd "$here/fw"
cargo build --release
objcopy=$(find "$(rustc --print sysroot)" -name llvm-objcopy | head -n 1)
"$objcopy" -O binary target/thumbv7em-none-eabihf/release/dryflash-cpu-bench-fw "$here/bench-fw.bin"
echo "wrote $here/bench-fw.bin"

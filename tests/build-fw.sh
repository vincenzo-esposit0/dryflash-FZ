#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# Builds the bare-metal test firmwares (thumbv7em-none-eabihf) and extracts raw images:
#   tests/cpu-bench/bench-fw.bin  — ADR-0001 CPU benchmark
#   tests/exc-fw.bin              — exception-model conformance firmware
set -eu
here=$(cd "$(dirname "$0")" && pwd)
objcopy=$(find "$(rustc --print sysroot)" -name llvm-objcopy | head -n 1)
build() {
  (cd "$here/$1" && cargo build --release)
  "$objcopy" -O binary "$here/$1/target/thumbv7em-none-eabihf/release/$2" "$3"
  echo "wrote $3"
}
build cpu-bench/fw dryflash-cpu-bench-fw "$here/cpu-bench/bench-fw.bin"
build exc-fw dryflash-exc-fw "$here/exc-fw.bin"

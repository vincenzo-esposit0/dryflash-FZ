#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# Extracts the distinct instruction encodings of an ELF's code into a fuzz corpus.
# usage: corpus-from-elf.sh firmware.elf > corpus.txt
set -eu
objdump=$(find "$(rustc --print sysroot)" -name llvm-objdump | head -n 1)
"$objdump" -d --mcpu=cortex-m4 "$1" \
  | cut -f1 \
  | sed -nE 's/^ *[0-9a-f]+: ([0-9a-f]{4})( ([0-9a-f]{4}))? *$/\1\3/p' \
  | sort -u

// SPDX-License-Identifier: GPL-3.0-or-later
//! Runs the benchmark firmware to completion and checks its result block.
//!
//! Expected values were produced by this interpreter and cross-checked instruction by
//! instruction against Unicorn 2.1.4 with `tests/cpu-bench/unicorn_diff.py` (ADR-0001).
//! Build the image first with `tests/build-fw.sh`.

use dryflash_core::cpu::Stop;
use dryflash_cpu_bench::Bench;

fn image() -> Vec<u8> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../bench-fw.bin");
    std::fs::read(path).unwrap_or_else(|e| panic!("{path}: {e} — run tests/build-fw.sh first"))
}

#[test]
fn one_round_matches_reference() {
    let mut b = Bench::new(&image(), 1);
    assert!(matches!(b.run(), Stop::Breakpoint { imm: 0, .. }));
    assert_eq!(b.cpu.instret, 522_079);
    assert_eq!(
        b.results(),
        [
            0xd0f1a5e0, 1, 0x614183ee, 0xc687a65b, 0xbf98fbe5, 0x729255f9, 0xa24fddd8, 0x43a06584, 1
        ]
    );
}

#[test]
fn many_rounds_match_reference() {
    let mut b = Bench::new(&image(), 200);
    assert!(matches!(b.run(), Stop::Breakpoint { imm: 0, .. }));
    assert_eq!(b.cpu.instret, 106_357_887);
    assert_eq!(
        b.results()[2..7],
        [0xb94e3fec, 0x2aa31ebe, 0xbfb48601, 0x25734436, 0x85a42546]
    );
}

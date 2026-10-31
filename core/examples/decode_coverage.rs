// SPDX-License-Identifier: GPL-3.0-or-later
//! Decoder coverage over a real firmware image.
//!
//! Reads `llvm-objdump -d --mcpu=cortex-m4 --no-show-raw-insn=false` style lines
//! (` addr: hhhh [hhhh] \tmnemonic\toperands`) from stdin, decodes every instruction and
//! prints the mnemonics whose encodings decode to `Op::Unimplemented`, most frequent first.
//!
//! usage: llvm-objdump -d --mcpu=cortex-m4 fw.elf | cargo run --example decode_coverage

use dryflash_core::cpu::decode::decode;
use dryflash_core::cpu::op::Op;
use std::collections::BTreeMap;
use std::io::BufRead;

fn main() {
    let mut total = 0u64;
    let mut missing: BTreeMap<String, (u64, String)> = BTreeMap::new();
    for line in std::io::stdin().lock().lines() {
        let line = line.unwrap();
        let Some((_, rest)) = line.split_once(": ") else {
            continue;
        };
        let mut parts = rest.splitn(2, '\t');
        let raw = parts.next().unwrap_or("").trim();
        let asm = parts.next().unwrap_or("").trim();
        let hws: Vec<u16> = raw
            .split_whitespace()
            .filter_map(|h| u16::from_str_radix(h, 16).ok())
            .collect();
        if hws.is_empty()
            || asm.starts_with('.')
            || asm.starts_with('<')
            || raw.split_whitespace().any(|h| h.len() != 4)
        {
            continue;
        }
        total += 1;
        let dec = decode(hws[0], *hws.get(1).unwrap_or(&0));
        if matches!(dec.op, Op::Unimplemented { .. }) {
            let mnemonic = asm.split_whitespace().next().unwrap_or("?").to_string();
            let e = missing.entry(mnemonic).or_insert((0, String::new()));
            e.0 += 1;
            if e.1.is_empty() {
                e.1 = format!("{raw}  {asm}");
            }
        }
    }
    let mut v: Vec<_> = missing.into_iter().collect();
    v.sort_by_key(|x| std::cmp::Reverse(x.1.0));
    let unimpl: u64 = v.iter().map(|x| x.1.0).sum();
    println!(
        "instructions: {total}, unimplemented: {unimpl} in {} mnemonics",
        v.len()
    );
    for (m, (n, ex)) in v {
        println!("{n:7}  {m:10} e.g. {ex}");
    }
}

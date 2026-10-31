// SPDX-License-Identifier: GPL-3.0-or-later
//! Native runner for the ADR-0001 spike.
//!
//! `cpu-bench <bench-fw.bin> [rounds]` — run and report MIPS and the result block.
//! `cpu-bench <bench-fw.bin> <rounds> --trace <out>` — write the architectural state before
//! every instruction (PC, R0–R14, NZCV, S0–S31) for differential checks against Unicorn.

use dryflash_core::cpu::Stop;
use dryflash_cpu_bench::Bench;
use std::io::Write;
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let image = std::fs::read(&args[1]).expect("read firmware image");
    let rounds: u32 = args.get(2).map(|s| s.parse().unwrap()).unwrap_or(1);
    let mut b = Bench::new(&image, rounds);

    if args.get(3).map(String::as_str) == Some("--trace") {
        let mut out = std::io::BufWriter::new(std::fs::File::create(&args[4]).unwrap());
        loop {
            let c = &b.cpu;
            let nzcv = (c.n as u32) << 3 | (c.z as u32) << 2 | (c.c as u32) << 1 | c.v as u32;
            let mut rec = vec![c.pc()];
            rec.extend_from_slice(&c.r[..15]);
            rec.push(nzcv);
            rec.extend_from_slice(&c.s);
            for w in rec {
                out.write_all(&w.to_le_bytes()).unwrap();
            }
            match b.cpu.run(&mut b.bus, &mut b.cache, 1) {
                Stop::Budget => {}
                stop => {
                    eprintln!("stop: {stop:x?} after {} instructions", b.cpu.instret);
                    return;
                }
            }
        }
    }

    let t = Instant::now();
    let stop = b.run();
    let secs = t.elapsed().as_secs_f64();
    let n = b.cpu.instret;
    println!("stop       {stop:x?}");
    println!("instret    {n}");
    println!("time       {secs:.3} s");
    println!("MIPS       {:.1}", n as f64 / secs / 1e6);
    println!("results    {:08x?}", b.results());
}

// SPDX-License-Identifier: GPL-3.0-or-later
//! Single-instruction executor for differential fuzzing (tests/cpu-fuzz/fuzz.py).
//!
//! Each stdin line: `code r0..r14 pc(ignored) apsr s0..s31 fpscr seed` (hex words, space-separated).
//! The code is placed at 0x08000000; SRAM1 is filled from `seed` exactly as the Python
//! side fills Unicorn's memory. Exactly one instruction is executed. Output line:
//! `OK r0..r15 apsr s0..s31 fpscr t ramhash` or `FAULT <kind>` / `STOP <reason>`.

use dryflash_core::cpu::{Bus, Cpu, CpuEvent, DecodeCache, Size, Stop};
use dryflash_cpu_bench::SpikeBus;
use std::io::{BufRead, Write};

/// Window compared after each instruction (start of SRAM1).
const RAM_WINDOW: usize = 0x10000;

fn fill_ram(ram: &mut [u8], mut seed: u64) {
    // xorshift64*, mirrored in fuzz.py.
    for chunk in ram[..RAM_WINDOW].chunks_mut(8) {
        seed ^= seed >> 12;
        seed ^= seed << 25;
        seed ^= seed >> 27;
        let v = seed.wrapping_mul(0x2545_F491_4F6C_DD1D);
        chunk.copy_from_slice(&v.to_le_bytes()[..chunk.len()]);
    }
    ram[RAM_WINDOW..].fill(0);
}

fn fnv1a(data: &[u8]) -> u64 {
    data.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| {
        (h ^ b as u64).wrapping_mul(0x100_0000_01b3)
    })
}

fn main() {
    let stdin = std::io::stdin();
    let mut out = std::io::BufWriter::new(std::io::stdout());
    let mut bus = SpikeBus::new(&[]);
    for line in stdin.lock().lines() {
        let line = line.unwrap();
        let w: Vec<u64> = line
            .split_whitespace()
            .map(|x| u64::from_str_radix(x, 16).unwrap())
            .collect();
        let code = w[0] as u32;
        bus.flash[..8].fill(0xFF);
        if code > 0xFFFF {
            bus.flash[0..2].copy_from_slice(&((code >> 16) as u16).to_le_bytes());
            bus.flash[2..4].copy_from_slice(&(code as u16).to_le_bytes());
        } else {
            bus.flash[0..2].copy_from_slice(&(code as u16).to_le_bytes());
        }
        fill_ram(&mut bus.ram, w[51]);
        let mut cpu = Cpu::new();
        cpu.halt_on_bkpt = true;
        cpu.sys.cpacr = 0xF << 20;
        for i in 0..15 {
            cpu.r[i] = w[1 + i] as u32;
        }
        cpu.r[15] = 0x0800_0000;
        let apsr = w[17] as u32;
        cpu.n = apsr >> 31 != 0;
        cpu.z = (apsr >> 30) & 1 != 0;
        cpu.c = (apsr >> 29) & 1 != 0;
        cpu.v = (apsr >> 28) & 1 != 0;
        cpu.q = (apsr >> 27) & 1 != 0;
        cpu.ge = ((apsr >> 16) & 0xF) as u8;
        for i in 0..32 {
            cpu.s[i] = w[18 + i] as u32;
        }
        cpu.fpscr = w[50] as u32 & 0xF7C0_009F;
        let mut cache = DecodeCache::new(0x0800_0000, 16);
        let stop = cpu.run(&mut bus, &mut cache, 1);
        if let Some(CpuEvent::Fault { fault, .. }) = cpu.events.first() {
            writeln!(out, "FAULT {fault:?}").unwrap();
            continue;
        }
        match stop {
            Stop::Budget => {}
            other => {
                writeln!(out, "STOP {other:?}").unwrap();
                continue;
            }
        }
        let mut fields: Vec<u32> = cpu.r.to_vec();
        fields.push(cpu.apsr());
        fields.extend_from_slice(&cpu.s);
        fields.push(cpu.fpscr);
        fields.push(cpu.t as u32);
        let mut s = String::from("OK");
        for f in fields {
            s.push_str(&format!(" {f:x}"));
        }
        let _ = bus.read(0x2000_0000, Size::Word);
        writeln!(out, "{s} {:x}", fnv1a(&bus.ram[..RAM_WINDOW])).unwrap();
    }
}

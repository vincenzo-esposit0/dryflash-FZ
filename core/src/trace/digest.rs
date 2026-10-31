// SPDX-License-Identifier: GPL-3.0-or-later
//! Bring-up digest (BRIEF §8): what the firmware touched that the emulator does not model.
//!
//! Emulator gaps are reported here and never hidden: unmapped accesses, reads/writes to
//! unimplemented registers grouped by block and offset, faults and resets.

use crate::bus::map::block_of;
use std::collections::BTreeMap;
use std::fmt::Write;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AccessStat {
    pub reads: u64,
    pub writes: u64,
    /// PC of the first instruction that made this access.
    pub first_pc: u32,
    pub last_write: Option<u32>,
}

impl AccessStat {
    fn record(&mut self, write: bool, value: Option<u32>, pc: u32) {
        if self.reads == 0 && self.writes == 0 {
            self.first_pc = pc;
        }
        if write {
            self.writes += 1;
            self.last_write = value;
        } else {
            self.reads += 1;
        }
    }

    pub fn total(&self) -> u64 {
        self.reads + self.writes
    }
}

/// Free-form timeline entry (resets, faults, milestones), timestamped in cycles.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DigestEvent {
    pub cycles: u64,
    pub what: String,
}

#[derive(Clone, Debug, Default)]
pub struct Digest {
    /// Accesses to addresses with nothing mapped (BusFault on hardware).
    pub unmapped: BTreeMap<u32, AccessStat>,
    /// Accesses to registers the emulator does not implement, keyed by address
    /// (read-as-zero, write-ignored).
    pub unimplemented: BTreeMap<u32, AccessStat>,
    pub events: Vec<DigestEvent>,
}

impl Digest {
    pub fn unmapped(&mut self, addr: u32, write: bool, value: Option<u32>, pc: u32) {
        self.unmapped.entry(addr).or_default().record(write, value, pc);
    }

    pub fn unimplemented(&mut self, addr: u32, write: bool, value: Option<u32>, pc: u32) {
        self.unimplemented.entry(addr).or_default().record(write, value, pc);
    }

    pub fn event(&mut self, cycles: u64, what: impl Into<String>) {
        self.events.push(DigestEvent {
            cycles,
            what: what.into(),
        });
    }

    /// Human-readable name of an address: `RCC+0x05C`, `PPB 0xE000EF34`, or the address.
    pub fn name_of(addr: u32) -> String {
        if let Some(b) = block_of(addr) {
            return format!("{}+0x{:03X}", b.name, addr - b.base);
        }
        if (0xE000_0000..0xE010_0000).contains(&addr) {
            return format!("PPB 0x{addr:08X}");
        }
        format!("0x{addr:08X}")
    }

    /// Renders the digest. `symbol` maps a PC to a function name when an ELF is available.
    pub fn render(&self, symbol: &dyn Fn(u32) -> Option<String>, max_rows: usize) -> String {
        let mut out = String::new();
        let sym = |pc: u32| symbol(pc).map(|s| format!(" ({s})")).unwrap_or_default();

        let _ = writeln!(
            out,
            "== Unimplemented registers (read-as-zero, write-ignored): {}",
            self.unimplemented.len()
        );
        // Group per block, blocks ordered by total accesses.
        let mut blocks: BTreeMap<String, Vec<(u32, AccessStat)>> = BTreeMap::new();
        for (&addr, st) in &self.unimplemented {
            let block = block_of(addr)
                .map(|b| b.name.to_string())
                .unwrap_or_else(|| "PPB/other".into());
            blocks.entry(block).or_default().push((addr, *st));
        }
        let mut blocks: Vec<_> = blocks.into_iter().collect();
        blocks.sort_by_key(|(_, v)| std::cmp::Reverse(v.iter().map(|(_, s)| s.total()).sum::<u64>()));
        for (name, mut regs) in blocks {
            let total: u64 = regs.iter().map(|(_, s)| s.total()).sum();
            let _ = writeln!(out, "  {name}: {} registers, {total} accesses", regs.len());
            regs.sort_by_key(|(_, s)| std::cmp::Reverse(s.total()));
            for (addr, st) in regs.iter().take(max_rows) {
                let last = st
                    .last_write
                    .map(|v| format!(" last write 0x{v:08X}"))
                    .unwrap_or_default();
                let _ = writeln!(
                    out,
                    "    {:<18} r {:>8} w {:>6}  first pc 0x{:08X}{}{last}",
                    Self::name_of(*addr),
                    st.reads,
                    st.writes,
                    st.first_pc,
                    sym(st.first_pc)
                );
            }
        }
        let _ = writeln!(out, "== Unmapped accesses (bus errors): {}", self.unmapped.len());
        for (addr, st) in self.unmapped.iter().take(max_rows) {
            let _ = writeln!(
                out,
                "    0x{addr:08X}  r {:>6} w {:>6}  first pc 0x{:08X}{}",
                st.reads,
                st.writes,
                st.first_pc,
                sym(st.first_pc)
            );
        }
        let _ = writeln!(out, "== Events: {}", self.events.len());
        for e in &self.events {
            let _ = writeln!(out, "    @{:>12} cycles  {}", e.cycles, e.what);
        }
        out
    }
}

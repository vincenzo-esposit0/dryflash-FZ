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
    /// Cycle counter at the first and the last access, to within one run slice.
    pub first_cycles: u64,
    pub last_cycles: u64,
}

impl AccessStat {
    fn record(&mut self, write: bool, value: Option<u32>, pc: u32, now: u64) {
        self.last_cycles = now;
        if self.reads == 0 && self.writes == 0 {
            self.first_pc = pc;
            self.first_cycles = now;
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

/// A block the emulator has no behavioural model for, touched recently.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GapTouch {
    pub block: String,
    pub registers: Vec<String>,
    /// One of them had not been touched before `new_since`: code that had not run until now.
    pub new: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Digest {
    /// Accesses to addresses with nothing mapped (BusFault on hardware).
    pub unmapped: BTreeMap<u32, AccessStat>,
    /// Accesses to addresses with no register (read-as-zero, write-ignored), keyed by address.
    pub unimplemented: BTreeMap<u32, AccessStat>,
    /// Accesses to registers that exist in the SVD but have no behavioural model: values
    /// are latched with the right reset value and write mask (ADR-0010).
    pub latched: BTreeMap<u32, AccessStat>,
    pub events: Vec<DigestEvent>,
    /// Cycle counter at the start of the current run slice.
    pub now: u64,
}

impl Digest {
    pub fn unmapped(&mut self, addr: u32, write: bool, value: Option<u32>, pc: u32) {
        self.unmapped
            .entry(addr)
            .or_default()
            .record(write, value, pc, self.now);
    }

    pub fn unimplemented(&mut self, addr: u32, write: bool, value: Option<u32>, pc: u32) {
        self.unimplemented
            .entry(addr)
            .or_default()
            .record(write, value, pc, self.now);
    }

    pub fn latched(&mut self, addr: u32, write: bool, value: Option<u32>, pc: u32) {
        self.latched.entry(addr).or_default().record(write, value, pc, self.now);
    }

    pub fn event(&mut self, cycles: u64, what: impl Into<String>) {
        self.events.push(DigestEvent {
            cycles,
            what: what.into(),
        });
    }

    /// What was touched since `since` (cycles) with no behavioural model behind it, by block.
    pub fn gaps_since(&self, since: u64, new_since: u64) -> Vec<GapTouch> {
        let mut out: BTreeMap<String, GapTouch> = BTreeMap::new();
        let unmodelled = self.unimplemented.iter().chain(self.latched.iter());
        for (addr, st) in unmodelled.filter(|(_, st)| st.last_cycles >= since && st.total() > 0) {
            let block = block_of(*addr).map_or_else(|| Self::name_of(*addr), |b| b.name.to_string());
            let row = out.entry(block.clone()).or_insert(GapTouch {
                block,
                registers: Vec::new(),
                new: false,
            });
            row.new |= st.first_cycles >= new_since;
            row.registers.push(Self::name_of(*addr));
        }
        out.into_values().collect()
    }

    /// Human-readable name of an address: `RCC.CR`, `RCC+0x0A0`, `PPB 0xE000EF34`, or the address.
    pub fn name_of(addr: u32) -> String {
        for d in crate::periph::svd::ALL {
            if let Some(r) = d.regs.iter().find(|r| d.base + r.offset as u32 == addr & !3) {
                let byte = if addr & 3 != 0 {
                    format!("+{}", addr & 3)
                } else {
                    String::new()
                };
                return format!("{}.{}{byte}", d.name, r.name);
            }
        }
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
        let sym: &dyn Fn(u32) -> String = &sym;

        render_grouped(
            &mut out,
            "Register file only (SVD reset values, writes latched, no behaviour)",
            &self.latched,
            &sym,
            max_rows,
        );
        render_grouped(
            &mut out,
            "No register at this offset (read-as-zero, write-ignored)",
            &self.unimplemented,
            &sym,
            max_rows,
        );
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

/// One digest section: accesses grouped per block, blocks and registers by access count.
fn render_grouped(
    out: &mut String,
    title: &str,
    map: &BTreeMap<u32, AccessStat>,
    sym: &dyn Fn(u32) -> String,
    max_rows: usize,
) {
    let _ = writeln!(out, "== {title}: {} registers", map.len());
    let mut blocks: BTreeMap<String, Vec<(u32, AccessStat)>> = BTreeMap::new();
    for (&addr, st) in map {
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
                "    {:<22} r {:>8} w {:>6}  first pc 0x{:08X}{}{last}",
                Digest::name_of(*addr),
                st.reads,
                st.writes,
                st.first_pc,
                sym(st.first_pc)
            );
        }
    }
}

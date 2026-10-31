// SPDX-License-Identifier: GPL-3.0-or-later
//! STM32WB55 peripherals as seen by CPU1.
//!
//! Every peripheral in the SVD gets a register file with real reset values and write masks
//! (ADR-0010). Peripherals with a behavioural model (RCC, …) add side effects on top; for
//! all others, accesses are latched and reported in the digest as "register file only".
//! Offsets with no register at all keep read-as-zero / write-ignored semantics (ADR-0009).

pub mod rcc;
pub mod regfile;
#[rustfmt::skip]
pub mod svd;

use crate::cpu::Size;
use crate::trace::digest::Digest;
use regfile::{PeriphDesc, RegFile};

/// Why the system is being reset (selects which state survives).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResetKind {
    /// Power-on/brown-out: everything resets, including the backup domain.
    PowerOn,
    /// SYSRESETREQ from software.
    Software,
    /// Independent watchdog timeout.
    Iwdg,
}

/// Clock frequencies derived from the RCC configuration, in Hz.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Clocks {
    pub sysclk: u32,
    /// HCLK1: CPU1, AHB1/2/3, SysTick (processor clock).
    pub hclk1: u32,
    pub pclk1: u32,
    pub pclk2: u32,
}

/// Per-access context handed to peripheral models.
pub struct Ctx<'a> {
    pub now: u64,
    pub digest: &'a mut Digest,
    pub pc: u32,
}

/// Core peripherals in the SVD that live in the CPU's PPB model, not on the bus.
const CORE_BLOCKS: &[&str] = &[
    "SCB_ACTRL",
    "STK",
    "NVIC",
    "SCB",
    "FPU_CPACR",
    "MPU",
    "NVIC_STIR",
    "FPU",
];

pub struct Periphs {
    pub rcc: rcc::Rcc,
    /// Register-file-only peripherals, by SVD order.
    pub generic: Vec<RegFile>,
    /// (base, end, slot): slot 0 = RCC, n ≥ 1 = generic[n-1]. Sorted by base.
    map: Vec<(u32, u32, usize)>,
    pub clocks: Clocks,
    /// NVIC input levels, bit n = IRQ n.
    pub irq_lines: u64,
    pub irq_dirty: bool,
}

impl Default for Periphs {
    fn default() -> Self {
        Self::new()
    }
}

impl Periphs {
    pub fn new() -> Self {
        let mut descs: Vec<&'static PeriphDesc> = svd::ALL.iter().filter(|d| !CORE_BLOCKS.contains(&d.name)).collect();
        descs.sort_by_key(|d| d.base);
        let mut generic = Vec::new();
        let mut map = Vec::new();
        for (i, d) in descs.iter().enumerate() {
            // A block extends to the next block or its last register, within the 1 KiB stride.
            let last = d.regs.iter().map(|r| r.offset as u32 + 4).max().unwrap_or(4);
            let next = descs.get(i + 1).map(|n| n.base).unwrap_or(u32::MAX);
            let end = (d.base + last.max(0x400)).min(next);
            let slot = if d.name == "RCC" {
                0
            } else {
                generic.push(RegFile::new(d));
                generic.len()
            };
            map.push((d.base, end, slot));
        }
        let rcc = rcc::Rcc::new();
        let clocks = rcc.clocks();
        Periphs {
            rcc,
            generic,
            map,
            clocks,
            irq_lines: 0,
            irq_dirty: false,
        }
    }

    pub fn reset(&mut self, kind: ResetKind) {
        self.rcc.reset(kind);
        for g in &mut self.generic {
            g.reset();
        }
        self.clocks = self.rcc.clocks();
        self.irq_lines = 0;
        self.irq_dirty = true;
    }

    #[inline]
    fn slot(&self, addr: u32) -> Option<(u32, usize)> {
        let i = self.map.partition_point(|m| m.0 <= addr);
        let &(base, end, slot) = self.map.get(i.checked_sub(1)?)?;
        (addr < end).then_some((base, slot))
    }

    fn regfile(&mut self, slot: usize) -> &mut RegFile {
        if slot == 0 {
            &mut self.rcc.regs
        } else {
            &mut self.generic[slot - 1]
        }
    }

    /// Reads a peripheral register; `None` if `addr` is not in a peripheral block.
    pub fn read(&mut self, addr: u32, size: Size, ctx: &mut Ctx) -> Option<u32> {
        let (base, slot) = self.slot(addr)?;
        let off = addr - base;
        let word_off = off & !3;
        let word = match slot {
            0 => self.rcc.read(word_off, ctx, addr),
            _ => {
                let rf = &self.generic[slot - 1];
                match rf.read(word_off) {
                    Some(v) => {
                        ctx.digest.latched(addr, false, None, ctx.pc);
                        Some(v)
                    }
                    None => None,
                }
            }
        };
        let Some(word) = word else {
            ctx.digest.unimplemented(addr, false, None, ctx.pc);
            return Some(0);
        };
        let shift = (off & 3) * 8;
        Some(match size {
            Size::Byte => (word >> shift) & 0xFF,
            Size::Half => (word >> shift) & 0xFFFF,
            Size::Word => word,
        })
    }

    /// Writes a peripheral register; `false` if `addr` is not in a peripheral block.
    pub fn write(&mut self, addr: u32, value: u32, size: Size, ctx: &mut Ctx) -> bool {
        let Some((base, slot)) = self.slot(addr) else {
            return false;
        };
        let off = addr - base;
        let word_off = off & !3;
        // Sub-word writes update their byte lanes of the stored word.
        let value = match size {
            Size::Word => value,
            _ => {
                let shift = (off & 3) * 8;
                let mask = if size == Size::Byte { 0xFFu32 } else { 0xFFFF } << shift;
                let old = self.regfile(slot).get(word_off);
                (old & !mask) | ((value << shift) & mask)
            }
        };
        let handled = match slot {
            0 => {
                let h = self.rcc.write(word_off, value, ctx, addr);
                self.clocks = self.rcc.clocks();
                h
            }
            _ => {
                let ok = self.generic[slot - 1].write(word_off, value);
                if ok {
                    ctx.digest.latched(addr, true, Some(value), ctx.pc);
                }
                ok
            }
        };
        if !handled {
            ctx.digest.unimplemented(addr, true, Some(value), ctx.pc);
        }
        true
    }

    /// Hardware fidelity table (BRIEF §5.4 "status registry").
    pub fn fidelity(&self) -> Vec<(&'static str, crate::bus::Fidelity, &'static str)> {
        let mut v = vec![("RCC", rcc::FIDELITY.0, rcc::FIDELITY.1)];
        for g in &self.generic {
            v.push((
                g.desc.name,
                crate::bus::Fidelity::Stubbed,
                "register file only (reset values, latched writes)",
            ));
        }
        v
    }
}

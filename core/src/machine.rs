// SPDX-License-Identifier: GPL-3.0-or-later
//! The emulated system: CPU1 + system bus + decoded-instruction cache, plus the run loop
//! that advances virtual time (WFI fast-forward, ARCHITECTURE §3.3) and handles resets.

use crate::bus::SystemBus;
use crate::bus::map::{FLASH_BASE, FLASH_SIZE};
use crate::cpu::{Bus, Cpu, CpuEvent, DecodeCache, Stop};
use crate::loader::dfu::{Dfu, Element};
use crate::periph::ResetKind;
use crate::trace::digest::Digest;
use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadError {
    /// Element outside main flash.
    OutsideFlash {
        address: u32,
        len: usize,
    },
    /// Element overlapping the region reserved for CPU2's wireless stack.
    OverlapsCpu2 {
        address: u32,
        len: usize,
        cpu2_start: u32,
    },
    NoElements,
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::OutsideFlash { address, len } => {
                write!(f, "element at 0x{address:08X} ({len} bytes) is outside main flash")
            }
            LoadError::OverlapsCpu2 {
                address,
                len,
                cpu2_start,
            } => write!(
                f,
                "element at 0x{address:08X} ({len} bytes) overlaps the CPU2 region from 0x{cpu2_start:08X}"
            ),
            LoadError::NoElements => write!(f, "image has no elements"),
        }
    }
}

impl std::error::Error for LoadError {}

/// Why [`Machine::run`] returned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunExit {
    /// The requested number of cycles elapsed.
    Budget,
    /// The CPU sleeps and nothing is scheduled that could wake it up.
    IdleForever { pc: u32 },
    /// The CPU stopped for another reason (watchpoint, lockup, emulator gap, …).
    Stopped(Stop),
}

pub struct Machine {
    pub cpu: Cpu,
    pub bus: SystemBus,
    cache: DecodeCache,
    /// Number of system resets since power-on (boot-loop detection, M6).
    pub resets: u32,
}

impl Default for Machine {
    fn default() -> Self {
        Self::new()
    }
}

fn earliest(a: Option<u64>, b: Option<u64>) -> Option<u64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.min(y)),
        (x, None) => x,
        (None, y) => y,
    }
}

impl Machine {
    pub fn new() -> Self {
        Machine {
            cpu: Cpu::new(),
            bus: SystemBus::new(),
            cache: DecodeCache::new(FLASH_BASE, FLASH_SIZE),
            resets: 0,
        }
    }

    /// Writes image elements into main flash ("Quick install", BRIEF §4). `cpu2_start`
    /// is the first address reserved for CPU2 (from the package's `Radio address` or the
    /// option bytes); nothing may be written there.
    pub fn load_elements(&mut self, elements: &[Element], cpu2_start: Option<u32>) -> Result<(), LoadError> {
        if elements.is_empty() {
            return Err(LoadError::NoElements);
        }
        for e in elements {
            let (address, len) = (e.address, e.data.len());
            let end = address as u64 + len as u64;
            if address < FLASH_BASE || end > (FLASH_BASE + FLASH_SIZE) as u64 {
                return Err(LoadError::OutsideFlash { address, len });
            }
            if let Some(c2) = cpu2_start
                && end > c2 as u64
            {
                return Err(LoadError::OverlapsCpu2 {
                    address,
                    len,
                    cpu2_start: c2,
                });
            }
        }
        for e in elements {
            let off = (e.address - FLASH_BASE) as usize;
            self.bus.flash[off..off + e.data.len()].copy_from_slice(&e.data);
            self.cache.invalidate(e.address, e.data.len() as u32);
        }
        Ok(())
    }

    /// Loads every element of every target of a DfuSe image.
    pub fn load_dfu(&mut self, dfu: &Dfu, cpu2_start: Option<u32>) -> Result<(), LoadError> {
        let elements: Vec<Element> = dfu.targets.iter().flat_map(|t| t.elements.iter().cloned()).collect();
        self.load_elements(&elements, cpu2_start)
    }

    /// Power-on reset: RAM cleared, CPU reset through the vector table at address 0
    /// (VTOR resets to 0; main flash is aliased there).
    pub fn power_on(&mut self) {
        self.bus.power_on_clear();
        self.bus.now = self.cpu.cycles;
        self.bus.reset_peripherals(ResetKind::PowerOn);
        self.resets = 0;
        self.reset_cpu("power-on reset");
    }

    fn reset_cpu(&mut self, why: &str) {
        let cycles = self.cpu.cycles;
        self.bus.digest.event(cycles, why.to_string());
        if let Err(e) = self.cpu.reset(&mut self.bus, 0) {
            self.bus
                .digest
                .event(cycles, format!("reset vector fetch failed at 0x{:08X}", e.addr));
        }
    }

    pub fn digest(&self) -> &Digest {
        &self.bus.digest
    }

    /// Runs for up to `cycles` of virtual time.
    pub fn run(&mut self, cycles: u64) -> RunExit {
        let end = self.cpu.cycles + cycles;
        let exit = loop {
            let budget = end.saturating_sub(self.cpu.cycles);
            if budget == 0 {
                break RunExit::Budget;
            }
            self.bus.now = self.cpu.cycles;
            match self.cpu.run(&mut self.bus, &mut self.cache, budget) {
                Stop::Budget => break RunExit::Budget,
                Stop::Sleep { pc } => match earliest(self.cpu.next_event(), self.bus.next_event()) {
                    // WFI fast-forward: jump to the next scheduled event.
                    Some(t) if t <= end => self.cpu.cycles = self.cpu.cycles.max(t),
                    Some(_) => {
                        self.cpu.cycles = end;
                        break RunExit::Budget;
                    }
                    None => break RunExit::IdleForever { pc },
                },
                Stop::ResetRequest { pc } => {
                    self.resets += 1;
                    self.bus.now = self.cpu.cycles;
                    self.bus.reset_peripherals(ResetKind::Software);
                    self.reset_cpu(&format!("SYSRESETREQ from pc 0x{pc:08X}"));
                }
                other => break RunExit::Stopped(other),
            }
        };
        self.drain_cpu_events();
        exit
    }

    fn drain_cpu_events(&mut self) {
        for ev in self.cpu.events.drain(..) {
            match ev {
                CpuEvent::Fault {
                    cycles,
                    fault,
                    exception,
                    pc,
                } => self.bus.digest.event(
                    cycles,
                    format!("fault {fault:?} → exception {exception} at pc 0x{pc:08X}"),
                ),
                CpuEvent::Escalated { cycles, from, pc } => self.bus.digest.event(
                    cycles,
                    format!("exception {from} escalated to HardFault at pc 0x{pc:08X}"),
                ),
            }
        }
    }
}

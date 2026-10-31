// SPDX-License-Identifier: GPL-3.0-or-later
//! ARMv7E-M (Cortex-M4F) instruction-level interpreter with the M-profile exception model.
//!
//! The CPU owns the Private Peripheral Bus (`crate::sysctl`): SCB, NVIC, SysTick, MPU,
//! FPU control, DWT. Everything else goes through the [`Bus`] trait.
//! Encodings that are not implemented stop execution with [`Stop::Unimplemented`]; they
//! are never skipped (rule 3: never fake success).

pub mod decode;
mod decode_cache;
mod dsp;
mod exception;
mod exec;
mod fp;
pub mod op;

pub use decode_cache::DecodeCache;

use crate::sysctl::{MpuAccess, Sysctl};
use op::Op;

/// Memory access width.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Size {
    Byte = 1,
    Half = 2,
    Word = 4,
}

/// A failed bus access (unmapped address, write to ROM, …).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BusError {
    pub addr: u32,
    pub write: bool,
}

/// The CPU's view of the system bus. Implementations decide memory map and side effects.
pub trait Bus {
    fn read(&mut self, addr: u32, size: Size) -> Result<u32, BusError>;
    fn write(&mut self, addr: u32, value: u32, size: Size) -> Result<(), BusError>;
    /// Instruction fetch of one halfword (no side effects).
    fn fetch16(&mut self, addr: u32) -> Result<u16, BusError>;
    /// Called before data accesses with the PC of the accessing instruction, so the bus
    /// can attribute unimplemented-register accesses in the bring-up digest.
    #[inline]
    fn set_access_pc(&mut self, _pc: u32) {}
    /// A PPB register that `sysctl` does not implement was accessed (digest only).
    fn ppb_unimplemented(&mut self, _addr: u32, _write: bool, _pc: u32) {}
}

/// Why [`Cpu::run`] returned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    /// The cycle budget was used up.
    Budget,
    /// `BKPT #imm` with `halt_on_bkpt` set (the emulator acting as a halting debugger).
    Breakpoint { imm: u8, pc: u32 },
    /// `WFI`/`WFE` with nothing to wake up for. The instruction has retired; the caller
    /// advances virtual time (WFI fast-forward) and calls `run` again.
    Sleep { pc: u32 },
    /// An encoding that is not implemented yet (emulator gap, not a firmware bug).
    Unimplemented { raw: u32, pc: u32 },
    /// AIRCR.SYSRESETREQ: the system must be reset.
    ResetRequest { pc: u32 },
    /// The processor locked up (fault while handling HardFault/NMI, DDI 0403E B1.5.15).
    Lockup { pc: u32 },
    /// A PC watchpoint was reached (before executing the instruction).
    Watch { pc: u32 },
}

/// Synchronous fault causes (DDI 0403E B1.5.14, CFSR bits).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    /// Precise data bus error.
    BusData {
        addr: u32,
    },
    /// Instruction prefetch bus error.
    BusFetch,
    /// MPU data access violation.
    MemData {
        addr: u32,
    },
    /// MPU / XN instruction access violation.
    MemFetch,
    Undefined,
    InvalidState,
    InvalidPc,
    NoCoprocessor,
    Unaligned {
        addr: u32,
    },
    DivideByZero,
}

/// A recorded exception or fault, drained by the machine into the bring-up digest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CpuEvent {
    Fault {
        cycles: u64,
        fault: Fault,
        exception: u8,
        pc: u32,
    },
    Escalated {
        cycles: u64,
        from: u8,
        pc: u32,
    },
}

/// Outcome of executing one instruction other than normal completion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Trap {
    Fault(Fault),
    Svc,
    Bkpt(u8),
    /// Branch to an EXC_RETURN value in Handler mode.
    ExcReturn(u32),
    Sleep,
    Unimplemented(u32),
    ResetRequest,
}

/// Architectural CPU state.
#[derive(Clone, Debug)]
pub struct Cpu {
    /// R0–R15. `r[13]` is the current stack pointer (see `msp`/`psp`). While an
    /// instruction executes, `r[15]` holds its address + 4 (the value it reads as PC);
    /// between instructions it holds the next PC.
    pub r: [u32; 16],
    pub n: bool,
    pub z: bool,
    pub c: bool,
    pub v: bool,
    pub q: bool,
    /// APSR.GE[3:0] (DSP extension).
    pub ge: u8,
    /// ITSTATE (IT[7:0]); 0 when not in an IT block.
    pub it: u8,
    /// EPSR.T. Clearing it makes the next instruction raise INVSTATE.
    pub t: bool,
    /// IPSR: current exception number, 0 in Thread mode.
    pub ipsr: u16,
    /// Banked stack pointers; the inactive one is valid, the active one lives in `r[13]`.
    pub msp: u32,
    pub psp: u32,
    /// CONTROL: nPRIV (bit 0), SPSEL (bit 1), FPCA (bit 2).
    pub control: u32,
    pub primask: bool,
    pub faultmask: bool,
    pub basepri: u8,
    /// S0–S31 as raw bits.
    pub s: [u32; 32],
    pub fpscr: u32,
    /// Local exclusive monitor (address of the last LDREX), cleared by CLREX/STREX/exceptions.
    pub exclusive: Option<u32>,
    /// Event register for WFE/SEV.
    pub event: bool,
    /// System Control Space and the rest of the PPB.
    pub sys: Sysctl,
    /// Retired instructions.
    pub instret: u64,
    /// Virtual time in core clock cycles (1 per instruction until the M2 cycle model).
    pub cycles: u64,
    /// BKPT halts (debugger semantics) instead of escalating to HardFault.
    pub halt_on_bkpt: bool,
    /// PC watchpoints, sorted.
    pub watch: Vec<u32>,
    pub events: Vec<CpuEvent>,

    branched: bool,
    /// Re-evaluate pending exceptions before the next instruction.
    exc_check: bool,
    /// PC of the instruction being executed (for fault attribution).
    cur_pc: u32,
    /// Instruction-fetch permission window [lo, hi) known to be executable.
    exec_lo: u32,
    exec_hi: u32,
    mpu_bounds: Vec<u32>,
    watch_resume: Option<u32>,
}

impl Default for Cpu {
    fn default() -> Self {
        Self::new()
    }
}

/// CONTROL bits.
pub const CONTROL_NPRIV: u32 = 1;
pub const CONTROL_SPSEL: u32 = 2;
pub const CONTROL_FPCA: u32 = 4;

impl Cpu {
    pub fn new() -> Self {
        Cpu {
            r: [0; 16],
            n: false,
            z: false,
            c: false,
            v: false,
            q: false,
            ge: 0,
            it: 0,
            t: true,
            ipsr: 0,
            msp: 0,
            psp: 0,
            control: 0,
            primask: false,
            faultmask: false,
            basepri: 0,
            s: [0; 32],
            fpscr: 0,
            exclusive: None,
            event: false,
            sys: Sysctl::new(),
            instret: 0,
            cycles: 0,
            halt_on_bkpt: false,
            watch: Vec::new(),
            events: Vec::new(),
            branched: false,
            exc_check: true,
            cur_pc: 0,
            exec_lo: 0,
            exec_hi: 0,
            mpu_bounds: Vec::new(),
            watch_resume: None,
        }
    }

    /// Power-on/system reset (DDI 0403E B1.5.5 TakeReset): SP_main from vector 0,
    /// PC from vector 1, everything else to reset values. `cycles` keeps counting so
    /// virtual time is monotonic across resets.
    pub fn reset<B: Bus>(&mut self, bus: &mut B, vtor: u32) -> Result<(), BusError> {
        let cycles = self.cycles;
        let instret = self.instret;
        let halt = self.halt_on_bkpt;
        let watch = std::mem::take(&mut self.watch);
        let debugger = self.sys.debugger_attached;
        *self = Cpu::new();
        self.cycles = cycles;
        self.instret = instret;
        self.halt_on_bkpt = halt;
        self.watch = watch;
        self.sys.debugger_attached = debugger;
        self.sys.systick.reset_time(cycles);
        self.sys.vtor = vtor;
        self.r[13] = bus.read(vtor, Size::Word)? & !3;
        let pc = bus.read(vtor + 4, Size::Word)?;
        self.r[14] = 0xFFFF_FFFF;
        self.t = pc & 1 != 0;
        self.r[15] = pc & !1;
        self.mpu_changed();
        Ok(())
    }

    pub fn pc(&self) -> u32 {
        self.r[15]
    }

    pub fn handler_mode(&self) -> bool {
        self.ipsr != 0
    }

    pub fn privileged(&self) -> bool {
        self.handler_mode() || self.control & CONTROL_NPRIV == 0
    }

    /// Main stack pointer, regardless of which one is active.
    pub fn sp_main(&self) -> u32 {
        if self.using_psp() { self.msp } else { self.r[13] }
    }

    pub fn sp_process(&self) -> u32 {
        if self.using_psp() { self.r[13] } else { self.psp }
    }

    fn using_psp(&self) -> bool {
        !self.handler_mode() && self.control & CONTROL_SPSEL != 0
    }

    /// Switches the active stack pointer after a change of mode or CONTROL.SPSEL.
    /// `was_psp` is the selection before the change.
    fn reselect_sp(&mut self, was_psp: bool) {
        let now_psp = self.using_psp();
        if was_psp == now_psp {
            return;
        }
        if was_psp {
            self.psp = self.r[13];
            self.r[13] = self.msp;
        } else {
            self.msp = self.r[13];
            self.r[13] = self.psp;
        }
    }

    /// APSR as read by MRS: N Z C V Q in bits 31..27, GE in 19..16.
    pub fn apsr(&self) -> u32 {
        (self.n as u32) << 31
            | (self.z as u32) << 30
            | (self.c as u32) << 29
            | (self.v as u32) << 28
            | (self.q as u32) << 27
            | (self.ge as u32) << 16
    }

    fn set_apsr_flags(&mut self, v: u32) {
        self.n = v >> 31 != 0;
        self.z = (v >> 30) & 1 != 0;
        self.c = (v >> 29) & 1 != 0;
        self.v = (v >> 28) & 1 != 0;
        self.q = (v >> 27) & 1 != 0;
    }

    /// Full xPSR as stacked on exception entry: APSR | IPSR | EPSR (T, IT).
    pub fn xpsr(&self) -> u32 {
        self.apsr()
            | self.ipsr as u32
            | (self.t as u32) << 24
            | ((self.it as u32) & 0x3) << 25
            | ((self.it as u32) >> 2) << 10
    }

    /// Pends external interrupt `irq` (called by peripherals from M2 on).
    pub fn set_irq_pending(&mut self, irq: usize) {
        self.sys.pending[16 + irq] = true;
        self.exc_check = true;
    }

    /// Next internal timer event (SysTick wrap), for WFI fast-forward.
    pub fn next_event(&self) -> Option<u64> {
        self.sys.systick.next_wrap
    }

    /// Recomputes cached MPU-derived state after MPU or privilege changes.
    pub(crate) fn mpu_changed(&mut self) {
        self.mpu_bounds = self.sys.mpu.boundaries();
        self.exec_lo = 0;
        self.exec_hi = 0;
    }

    /// Instruction fetch permission for `pc`; on success caches the surrounding window
    /// where the MPU and default-map permissions cannot change.
    fn fetch_allowed(&mut self, pc: u32) -> bool {
        let use_default = self.mpu_default_map();
        if !self
            .sys
            .mpu
            .allowed(pc, MpuAccess::Exec, self.privileged(), use_default)
        {
            return false;
        }
        let i = self.mpu_bounds.partition_point(|&b| b <= pc);
        self.exec_lo = if i == 0 { 0 } else { self.mpu_bounds[i - 1] };
        self.exec_hi = self.mpu_bounds.get(i).copied().unwrap_or(u32::MAX);
        true
    }

    /// HardFault/NMI/FAULTMASK use the default map unless MPU_CTRL.HFNMIENA (B3.5.2).
    fn mpu_default_map(&self) -> bool {
        self.sys.mpu.ctrl & 2 == 0 && self.execution_priority() < 0
    }

    /// Runs until `budget` cycles have elapsed or a [`Stop`] condition occurs.
    pub fn run<B: Bus>(&mut self, bus: &mut B, cache: &mut DecodeCache, budget: u64) -> Stop {
        let end = self.cycles + budget;
        loop {
            while let Some(w) = self.sys.systick.next_wrap {
                if self.cycles < w {
                    break;
                }
                if self.sys.systick_wrap(w) {
                    self.sys.pending[crate::sysctl::EXC_SYSTICK] = true;
                    self.exc_check = true;
                }
            }
            if self.exc_check {
                self.exc_check = false;
                if let Some(n) = self.preempting_exception() {
                    let ret = self.r[15];
                    if let Err(stop) = self.take_exception(bus, n, ret) {
                        return stop;
                    }
                }
            }
            if self.cycles >= end {
                return Stop::Budget;
            }
            let limit = end.min(self.sys.systick.next_wrap.unwrap_or(u64::MAX));
            while self.cycles < limit && !self.exc_check {
                if let Some(stop) = self.step(bus, cache) {
                    return stop;
                }
            }
        }
    }

    /// Executes one instruction (or takes the fault it raises).
    #[inline]
    fn step<B: Bus>(&mut self, bus: &mut B, cache: &mut DecodeCache) -> Option<Stop> {
        let pc = self.r[15];
        if !self.watch.is_empty() && self.watch.binary_search(&pc).is_ok() {
            if self.watch_resume == Some(pc) {
                self.watch_resume = None;
            } else {
                self.watch_resume = Some(pc);
                return Some(Stop::Watch { pc });
            }
        }
        if !self.t {
            return self.fault_at(bus, Fault::InvalidState, pc);
        }
        if (pc < self.exec_lo || pc >= self.exec_hi) && !self.fetch_allowed(pc) {
            return self.fault_at(bus, Fault::MemFetch, pc);
        }
        let dec = match cache.lookup(bus, pc) {
            Ok(d) => d,
            Err(_) => return self.fault_at(bus, Fault::BusFetch, pc),
        };
        self.cur_pc = pc;
        self.r[15] = pc.wrapping_add(4);
        self.branched = false;
        let it_before = self.it;
        let result = if self.it == 0 {
            self.exec(bus, dec.op, pc)
        } else {
            let r = if self.cond(self.it >> 4) {
                self.exec(bus, dec.op, pc)
            } else {
                Ok(())
            };
            if !matches!(dec.op, Op::It { .. }) {
                self.advance_it();
            }
            r
        };
        let next = pc.wrapping_add(dec.size as u32);
        match result {
            Ok(()) => {
                if !self.branched {
                    self.r[15] = next;
                }
                self.retire();
                None
            }
            Err(trap) => self.handle_trap(bus, trap, pc, next, it_before),
        }
    }

    #[inline]
    fn retire(&mut self) {
        self.instret += 1;
        self.cycles += 1;
    }

    fn handle_trap<B: Bus>(&mut self, bus: &mut B, trap: Trap, pc: u32, next: u32, it_before: u8) -> Option<Stop> {
        match trap {
            Trap::Fault(f) => {
                // The faulting instruction does not retire; IT state is that of the instruction.
                self.it = it_before;
                self.fault_at(bus, f, pc)
            }
            Trap::Svc => {
                self.r[15] = next;
                self.retire();
                self.svc(bus, next)
            }
            Trap::Bkpt(imm) => {
                self.it = it_before;
                self.r[15] = pc;
                if self.halt_on_bkpt {
                    return Some(Stop::Breakpoint { imm, pc });
                }
                // No halting debugger and DebugMonitor disabled: escalate (B1.5.14, C1.4).
                self.sys.hfsr |= crate::sysctl::HFSR_DEBUGEVT;
                self.take_hardfault(bus, pc)
            }
            Trap::ExcReturn(value) => {
                self.retire();
                self.exception_return(bus, value)
            }
            Trap::Sleep => {
                self.r[15] = next;
                self.retire();
                if self.wakeup_pending() {
                    None
                } else {
                    Some(Stop::Sleep { pc })
                }
            }
            Trap::Unimplemented(raw) => {
                self.it = it_before;
                self.r[15] = pc;
                Some(Stop::Unimplemented { raw, pc })
            }
            Trap::ResetRequest => {
                self.r[15] = next;
                self.retire();
                Some(Stop::ResetRequest { pc })
            }
        }
    }

    #[inline]
    fn advance_it(&mut self) {
        if self.it & 0x7 == 0 {
            self.it = 0;
        } else {
            self.it = (self.it & 0xE0) | ((self.it << 1) & 0x1F);
        }
    }

    /// ConditionPassed (DDI 0403E A7.3.1).
    #[inline(always)]
    pub fn cond(&self, cond: u8) -> bool {
        let r = match cond >> 1 {
            0 => self.z,
            1 => self.c,
            2 => self.n,
            3 => self.v,
            4 => self.c && !self.z,
            5 => self.n == self.v,
            6 => self.n == self.v && !self.z,
            _ => return true,
        };
        if cond & 1 == 1 { !r } else { r }
    }
}

#[cfg(test)]
mod tests;

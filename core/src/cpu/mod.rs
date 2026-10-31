// SPDX-License-Identifier: GPL-3.0-or-later
//! ARMv7E-M (Cortex-M4F) instruction-level interpreter.
//!
//! Scope as of M0 (ADR-0001 spike): Thumb/Thumb-2 integer ISA, IT blocks, the
//! multiply/divide subset, LDREX/STREX, and the FPv4-SP data-processing,
//! transfer and load/store instructions. Exception entry/return, NVIC, MPU and the
//! remaining DSP encodings land in M1/M2. Anything not implemented stops execution
//! with [`Stop::Unimplemented`]; it is never skipped.

pub mod decode;
mod decode_cache;
mod exec;
pub mod op;

pub use decode_cache::DecodeCache;

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
}

/// Why [`Cpu::run`] returned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    /// The instruction budget was used up.
    Budget,
    /// `BKPT #imm` was executed (debug halt).
    Breakpoint { imm: u8, pc: u32 },
    /// `WFI` was executed; the caller decides how to advance time.
    Wfi { pc: u32 },
    /// The decoder or executor met an encoding that is not implemented yet.
    Unimplemented { raw: u32, pc: u32 },
    /// A condition that would raise an exception (fault, SVC, UDF) — exception
    /// entry is not modelled before M1, so execution stops here.
    Exception { kind: ExceptionKind, pc: u32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExceptionKind {
    BusFault {
        addr: u32,
        write: bool,
    },
    /// UsageFault NOCP: FP instruction while CPACR denies access.
    NoCoprocessor,
    /// UsageFault INVSTATE: interworking branch to an address with bit 0 clear.
    InvalidState,
    /// UsageFault UNDEFINSTR.
    Undefined,
    /// UsageFault UNALIGNED: LDM/STM/LDRD/STRD/LDREX/STREX on an unaligned address.
    Unaligned {
        addr: u32,
    },
    /// UsageFault DIVBYZERO (only when CCR.DIV_0_TRP is set).
    DivideByZero,
    Svc {
        imm: u8,
    },
    /// EXC_RETURN branch: needs the exception model (M1).
    ExceptionReturn {
        value: u32,
    },
}

/// Architectural CPU state.
#[derive(Clone, Debug)]
pub struct Cpu {
    /// R0–R15. While an instruction executes, `r[15]` holds its address + 4 (the
    /// value an instruction reads as PC); between instructions it holds the next PC.
    pub r: [u32; 16],
    pub n: bool,
    pub z: bool,
    pub c: bool,
    pub v: bool,
    pub q: bool,
    /// ITSTATE (IT[7:0]); 0 when not in an IT block.
    pub it: u8,
    /// S0–S31 as raw bits.
    pub s: [u32; 32],
    pub fpscr: u32,
    /// CPACR CP10/CP11 full access. Set by the SCB model; FP instructions fault without it.
    pub fp_enabled: bool,
    pub primask: bool,
    pub faultmask: bool,
    pub basepri: u8,
    /// Local exclusive monitor (address of the last LDREX), cleared by CLREX/STREX.
    pub exclusive: Option<u32>,
    /// Retired instruction count (also the cycle approximation until M2 adds a cycle model).
    pub instret: u64,
    branched: bool,
}

impl Default for Cpu {
    fn default() -> Self {
        Self::new()
    }
}

impl Cpu {
    pub fn new() -> Self {
        Cpu {
            r: [0; 16],
            n: false,
            z: false,
            c: false,
            v: false,
            q: false,
            it: 0,
            s: [0; 32],
            fpscr: 0,
            fp_enabled: false,
            primask: false,
            faultmask: false,
            basepri: 0,
            exclusive: None,
            instret: 0,
            branched: false,
        }
    }

    /// Reset as in DDI 0403E B1.5.5: SP from vector 0, PC from vector 1 (Thumb bit required).
    pub fn reset<B: Bus>(&mut self, bus: &mut B, vtor: u32) -> Result<(), BusError> {
        *self = Cpu::new();
        self.r[13] = bus.read(vtor, Size::Word)? & !3;
        let pc = bus.read(vtor + 4, Size::Word)?;
        self.r[14] = 0xFFFF_FFFF;
        self.r[15] = pc & !1;
        Ok(())
    }

    pub fn pc(&self) -> u32 {
        self.r[15]
    }

    /// APSR as read by MRS (N Z C V Q in bits 31..27).
    pub fn apsr(&self) -> u32 {
        (self.n as u32) << 31
            | (self.z as u32) << 30
            | (self.c as u32) << 29
            | (self.v as u32) << 28
            | (self.q as u32) << 27
    }

    /// Executes up to `budget` instructions.
    pub fn run<B: Bus>(&mut self, bus: &mut B, cache: &mut DecodeCache, budget: u64) -> Stop {
        let end = self.instret + budget;
        while self.instret < end {
            let pc = self.r[15];
            let dec = match cache.lookup(bus, pc) {
                Ok(d) => d,
                Err(e) => {
                    return Stop::Exception {
                        kind: ExceptionKind::BusFault {
                            addr: e.addr,
                            write: false,
                        },
                        pc,
                    };
                }
            };
            self.r[15] = pc.wrapping_add(4);
            self.branched = false;
            let stop = if self.it == 0 {
                self.exec(bus, dec.op, pc)
            } else {
                let cond = self.it >> 4;
                let r = if self.cond(cond) {
                    self.exec(bus, dec.op, pc)
                } else {
                    None
                };
                if !matches!(dec.op, Op::It { .. }) {
                    self.advance_it();
                }
                r
            };
            if let Some(stop) = stop {
                self.r[15] = pc;
                return stop;
            }
            if !self.branched {
                self.r[15] = pc.wrapping_add(dec.size as u32);
            }
            self.instret += 1;
        }
        Stop::Budget
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
    #[inline]
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

use op::Op;

#[cfg(test)]
mod tests;

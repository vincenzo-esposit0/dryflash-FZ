// SPDX-License-Identifier: GPL-3.0-or-later
//! Instruction semantics. Pseudocode names (AddWithCarry, Shift_C, …) follow
//! DDI 0403E A2.2 and the per-instruction pages of A7.7.

use super::op::*;
use super::{Bus, BusError, Cpu, ExceptionKind, Size, Stop};

type R = Option<Stop>;

/// SCB->CPACR (PM0214 §4.6.1). Until the PPB model lands in M1 the CPU owns this one register.
const CPACR: u32 = 0xE000_ED88;

/// AddWithCarry (A2.2.1): returns (result, carry_out, overflow).
#[inline]
fn add_with_carry(x: u32, y: u32, carry: bool) -> (u32, bool, bool) {
    let (r1, c1) = x.overflowing_add(y);
    let (r, c2) = r1.overflowing_add(carry as u32);
    let v = ((x ^ r) & (y ^ r)) >> 31 != 0;
    (r, c1 || c2, v)
}

/// Shift_C (A7.4.2) for a decoded immediate shift.
#[inline]
fn shift_c(value: u32, sh: Shift, carry_in: bool) -> (u32, bool) {
    let n = sh.amt as u32;
    match sh.ty {
        _ if n == 0 => (value, carry_in),
        ShiftType::Lsl => lsl_c(value, n),
        ShiftType::Lsr => lsr_c(value, n),
        ShiftType::Asr => asr_c(value, n),
        ShiftType::Ror => ror_c(value, n),
        ShiftType::Rrx => ((carry_in as u32) << 31 | value >> 1, value & 1 != 0),
    }
}

#[inline]
fn lsl_c(v: u32, n: u32) -> (u32, bool) {
    match n {
        0 => (v, false),
        1..=31 => (v << n, (v >> (32 - n)) & 1 != 0),
        32 => (0, v & 1 != 0),
        _ => (0, false),
    }
}

#[inline]
fn lsr_c(v: u32, n: u32) -> (u32, bool) {
    match n {
        0 => (v, false),
        1..=31 => (v >> n, (v >> (n - 1)) & 1 != 0),
        32 => (0, v >> 31 != 0),
        _ => (0, false),
    }
}

#[inline]
fn asr_c(v: u32, n: u32) -> (u32, bool) {
    match n {
        0 => (v, false),
        1..=31 => (((v as i32) >> n) as u32, (v >> (n - 1)) & 1 != 0),
        _ => (((v as i32) >> 31) as u32, v >> 31 != 0),
    }
}

#[inline]
fn ror_c(v: u32, n: u32) -> (u32, bool) {
    let r = v.rotate_right(n % 32);
    (r, r >> 31 != 0)
}

/// FPSCR NZCV after a comparison (A7.7.227 VCMP).
fn fp_compare_flags(a: f32, b: f32) -> u32 {
    if a.is_nan() || b.is_nan() {
        0b0011
    } else if a == b {
        0b0110
    } else if a < b {
        0b1000
    } else {
        0b0010
    }
}

/// Rounds to integral according to FPSCR.RMode (A2.5.2): RN, RP, RM, RZ.
fn fp_round(x: f32, fpscr: u32) -> f32 {
    match (fpscr >> 22) & 3 {
        0 => x.round_ties_even(),
        1 => x.ceil(),
        2 => x.floor(),
        _ => x.trunc(),
    }
}

impl Cpu {
    #[inline]
    fn set_nz(&mut self, r: u32) {
        self.n = r >> 31 != 0;
        self.z = r == 0;
    }

    #[inline]
    fn want_flags(&self, s: SetFlags) -> bool {
        match s {
            SetFlags::No => false,
            SetFlags::Yes => true,
            SetFlags::OutsideIt => self.it == 0,
        }
    }

    /// BranchWritePC (A2.3.1).
    #[inline]
    fn branch(&mut self, target: u32) {
        self.r[15] = target & !1;
        self.branched = true;
    }

    /// BXWritePC / LoadWritePC (A2.3.1): bit 0 selects Thumb state, which is mandatory
    /// on ARMv7-M; values 0xFxxxxxxx are EXC_RETURN in handler mode (M1).
    #[inline]
    fn bx_write_pc(&mut self, target: u32) -> R {
        if target >> 28 == 0xF {
            return Some(Stop::Exception {
                kind: ExceptionKind::ExceptionReturn { value: target },
                pc: 0,
            });
        }
        if target & 1 == 0 {
            return Some(Stop::Exception {
                kind: ExceptionKind::InvalidState,
                pc: 0,
            });
        }
        self.branch(target);
        None
    }

    /// Register write from a data-processing instruction (ALUWritePC for R15).
    #[inline]
    fn write_alu(&mut self, rd: u8, value: u32) {
        if rd == 15 {
            self.branch(value);
        } else {
            self.r[rd as usize] = value;
        }
    }

    #[inline]
    fn load<B: Bus>(&mut self, bus: &mut B, addr: u32, size: Size) -> Result<u32, Stop> {
        if addr == CPACR && size == Size::Word {
            return Ok(if self.fp_enabled { 0xF << 20 } else { 0 });
        }
        bus.read(addr, size).map_err(fault)
    }

    #[inline]
    fn store<B: Bus>(&mut self, bus: &mut B, addr: u32, value: u32, size: Size) -> Result<(), Stop> {
        if addr == CPACR && size == Size::Word {
            // CP10 and CP11 must both be "full access" (0b11) for FP instructions.
            self.fp_enabled = (value >> 20) & 0xF == 0xF;
            return Ok(());
        }
        bus.write(addr, value, size).map_err(fault)
    }

    #[inline]
    fn sf(&self, i: u8) -> f32 {
        f32::from_bits(self.s[i as usize])
    }

    #[inline]
    fn set_sf(&mut self, i: u8, v: f32) {
        self.s[i as usize] = v.to_bits();
    }

    /// Executes one decoded instruction. `pc` is its address; `r[15]` already reads as pc + 4.
    pub(super) fn exec<B: Bus>(&mut self, bus: &mut B, op: Op, pc: u32) -> R {
        let stop = match self.exec_inner(bus, op, pc) {
            Ok(()) => return None,
            Err(s) => s,
        };
        // Fill in the faulting PC for stops raised by helpers that don't know it.
        Some(match stop {
            Stop::Exception { kind, .. } => Stop::Exception { kind, pc },
            Stop::Unimplemented { raw, .. } => Stop::Unimplemented { raw, pc },
            other => other,
        })
    }

    fn exec_inner<B: Bus>(&mut self, bus: &mut B, op: Op, pc: u32) -> Result<(), Stop> {
        match op {
            Op::DpImm {
                op,
                s,
                rd,
                rn,
                imm,
                carry,
            } => {
                let c = match carry {
                    ImmCarry::Unchanged => self.c,
                    ImmCarry::Clear => false,
                    ImmCarry::Set => true,
                };
                let x = self.r[rn as usize];
                self.dp(op, s, rd, x, imm, c);
            }
            Op::DpReg { op, s, rd, rn, rm, sh } => {
                let (y, c) = shift_c(self.r[rm as usize], sh, self.c);
                let x = self.r[rn as usize];
                self.dp(op, s, rd, x, y, c);
            }
            Op::ShiftReg { ty, s, rd, rn, rm } => {
                let v = self.r[rn as usize];
                let n = self.r[rm as usize] & 0xFF;
                let (r, c) = if n == 0 {
                    (v, self.c)
                } else {
                    match ty {
                        ShiftType::Lsl => lsl_c(v, n),
                        ShiftType::Lsr => lsr_c(v, n),
                        ShiftType::Asr => asr_c(v, n),
                        _ => ror_c(v, n),
                    }
                };
                self.write_alu(rd, r);
                if self.want_flags(s) {
                    self.set_nz(r);
                    self.c = c;
                }
            }
            Op::Movw { rd, imm } => self.r[rd as usize] = imm as u32,
            Op::Movt { rd, imm } => {
                let r = &mut self.r[rd as usize];
                *r = (*r & 0xFFFF) | (imm as u32) << 16;
            }
            Op::Adr { rd, off } => self.r[rd as usize] = (self.r[15] & !3).wrapping_add(off as u32),
            Op::Bfx {
                signed,
                rd,
                rn,
                lsb,
                width,
            } => {
                let (lsb, width) = (lsb as u32, width as u32);
                if lsb + width > 32 {
                    return Err(unimpl(op));
                }
                let v = self.r[rn as usize] << (32 - lsb - width);
                self.r[rd as usize] = if signed {
                    ((v as i32) >> (32 - width)) as u32
                } else {
                    v >> (32 - width)
                };
            }
            Op::Bfi { rd, rn, lsb, msb } => {
                if msb < lsb {
                    return Err(unimpl(op));
                }
                let width = (msb - lsb + 1) as u32;
                let mask = (u32::MAX >> (32 - width)) << lsb;
                let src = if rn == 15 { 0 } else { self.r[rn as usize] << lsb };
                let r = &mut self.r[rd as usize];
                *r = (*r & !mask) | (src & mask);
            }
            Op::Extend { kind, rd, rn, rm, rot } => {
                let v = self.r[rm as usize].rotate_right(rot as u32);
                let e = match kind {
                    ExtendKind::Sxtb => v as u8 as i8 as i32 as u32,
                    ExtendKind::Sxth => v as u16 as i16 as i32 as u32,
                    ExtendKind::Uxtb => v & 0xFF,
                    ExtendKind::Uxth => v & 0xFFFF,
                };
                let add = if rn == 15 { 0 } else { self.r[rn as usize] };
                self.r[rd as usize] = add.wrapping_add(e);
            }
            Op::Rev { kind, rd, rm } => {
                let v = self.r[rm as usize];
                self.r[rd as usize] = match kind {
                    RevKind::Rev => v.swap_bytes(),
                    RevKind::Rev16 => (v & 0xFF00_FF00) >> 8 | (v & 0x00FF_00FF) << 8,
                    RevKind::Revsh => (v as u16).swap_bytes() as i16 as i32 as u32,
                    RevKind::Rbit => v.reverse_bits(),
                    RevKind::Clz => v.leading_zeros(),
                };
            }
            Op::Mul { s, rd, rn, rm } => {
                let r = self.r[rn as usize].wrapping_mul(self.r[rm as usize]);
                self.r[rd as usize] = r;
                if self.want_flags(s) {
                    self.set_nz(r);
                }
            }
            Op::Mla { sub, rd, rn, rm, ra } => {
                let p = self.r[rn as usize].wrapping_mul(self.r[rm as usize]);
                let a = self.r[ra as usize];
                self.r[rd as usize] = if sub { a.wrapping_sub(p) } else { a.wrapping_add(p) };
            }
            Op::LongMul {
                kind,
                rdlo,
                rdhi,
                rn,
                rm,
            } => {
                let (n, m) = (self.r[rn as usize], self.r[rm as usize]);
                let acc = (self.r[rdhi as usize] as u64) << 32 | self.r[rdlo as usize] as u64;
                let r: u64 = match kind {
                    LongMulKind::Umull => n as u64 * m as u64,
                    LongMulKind::Smull => (n as i32 as i64 * m as i32 as i64) as u64,
                    LongMulKind::Umlal => (n as u64 * m as u64).wrapping_add(acc),
                    LongMulKind::Smlal => ((n as i32 as i64 * m as i32 as i64) as u64).wrapping_add(acc),
                    LongMulKind::Umaal => {
                        n as u64 * m as u64 + self.r[rdhi as usize] as u64 + self.r[rdlo as usize] as u64
                    }
                };
                self.r[rdlo as usize] = r as u32;
                self.r[rdhi as usize] = (r >> 32) as u32;
            }
            Op::Div { signed, rd, rn, rm } => {
                let (n, m) = (self.r[rn as usize], self.r[rm as usize]);
                // CCR.DIV_0_TRP is 0 out of reset: division by zero returns 0 (A7.7.127).
                self.r[rd as usize] = if m == 0 {
                    0
                } else if signed {
                    (n as i32).wrapping_div(m as i32) as u32
                } else {
                    n / m
                };
            }
            Op::Mem { kind, rt, rn, mode } => self.exec_mem(bus, kind, rt, rn, mode)?,
            Op::Dual {
                load,
                rt,
                rt2,
                rn,
                off,
                index,
                wback,
            } => {
                let base = if rn == 15 { self.r[15] & !3 } else { self.r[rn as usize] };
                let offset_addr = base.wrapping_add(off as u32);
                let addr = if index { offset_addr } else { base };
                if addr & 3 != 0 {
                    return Err(exc(ExceptionKind::Unaligned { addr }));
                }
                if load {
                    let lo = self.load(bus, addr, Size::Word)?;
                    let hi = self.load(bus, addr.wrapping_add(4), Size::Word)?;
                    self.r[rt as usize] = lo;
                    self.r[rt2 as usize] = hi;
                } else {
                    self.store(bus, addr, self.r[rt as usize], Size::Word)?;
                    self.store(bus, addr.wrapping_add(4), self.r[rt2 as usize], Size::Word)?;
                }
                if wback {
                    self.r[rn as usize] = offset_addr;
                }
            }
            Op::Multi {
                load,
                rn,
                list,
                wback,
                db,
            } => {
                let count = list.count_ones();
                let base = self.r[rn as usize];
                let start = if db { base.wrapping_sub(4 * count) } else { base };
                if start & 3 != 0 {
                    return Err(exc(ExceptionKind::Unaligned { addr: start }));
                }
                let mut addr = start;
                let mut new_pc = None;
                for i in 0..16u8 {
                    if list & (1 << i) == 0 {
                        continue;
                    }
                    if load {
                        let v = self.load(bus, addr, Size::Word)?;
                        if i == 15 {
                            new_pc = Some(v);
                        } else {
                            self.r[i as usize] = v;
                        }
                    } else {
                        self.store(bus, addr, self.r[i as usize], Size::Word)?;
                    }
                    addr = addr.wrapping_add(4);
                }
                if wback && !(load && list & (1 << rn) != 0) {
                    self.r[rn as usize] = if db { start } else { base.wrapping_add(4 * count) };
                }
                if let Some(s) = new_pc.and_then(|t| self.bx_write_pc(t)) {
                    return Err(s);
                }
            }
            Op::Ldrex { rt, rn, imm, size } => {
                let addr = self.r[rn as usize].wrapping_add(imm as u32);
                let sz = mem_size(size);
                if addr & (size as u32 - 1) != 0 {
                    return Err(exc(ExceptionKind::Unaligned { addr }));
                }
                self.r[rt as usize] = self.load(bus, addr, sz)?;
                self.exclusive = Some(addr);
            }
            Op::Strex { rd, rt, rn, imm, size } => {
                let addr = self.r[rn as usize].wrapping_add(imm as u32);
                if addr & (size as u32 - 1) != 0 {
                    return Err(exc(ExceptionKind::Unaligned { addr }));
                }
                if self.exclusive == Some(addr) {
                    self.store(bus, addr, self.r[rt as usize], mem_size(size))?;
                    self.r[rd as usize] = 0;
                } else {
                    self.r[rd as usize] = 1;
                }
                self.exclusive = None;
            }
            Op::Clrex => self.exclusive = None,
            Op::Tb { half, rn, rm } => {
                let base = self.r[rn as usize];
                let idx = self.r[rm as usize];
                let off = if half {
                    self.load(bus, base.wrapping_add(idx << 1), Size::Half)?
                } else {
                    self.load(bus, base.wrapping_add(idx), Size::Byte)?
                };
                let target = self.r[15].wrapping_add(off << 1);
                self.branch(target);
            }
            Op::B { cond, off } => {
                if self.cond(cond) {
                    let target = self.r[15].wrapping_add(off as u32);
                    self.branch(target);
                }
            }
            Op::Bl { off } => {
                self.r[14] = self.r[15] | 1;
                let target = self.r[15].wrapping_add(off as u32);
                self.branch(target);
            }
            Op::Bx { rm, link } => {
                let target = self.r[rm as usize];
                if link {
                    self.r[14] = pc.wrapping_add(2) | 1;
                }
                if let Some(s) = self.bx_write_pc(target) {
                    return Err(s);
                }
            }
            Op::Cbz { nz, rn, off } => {
                if (self.r[rn as usize] != 0) == nz {
                    let target = self.r[15].wrapping_add(off as u32);
                    self.branch(target);
                }
            }
            Op::It { state } => self.it = state,
            // WFE/SEV/YIELD: no event model before M2; they behave as NOP.
            Op::Hint { kind: HintKind::Wfi } => return Err(Stop::Wfi { pc }),
            Op::Hint { .. } => {}
            Op::Bkpt { imm } => return Err(Stop::Breakpoint { imm, pc }),
            Op::Svc { imm } => return Err(exc(ExceptionKind::Svc { imm })),
            Op::Udf { .. } => return Err(exc(ExceptionKind::Undefined)),
            Op::Cps {
                disable,
                primask,
                faultmask,
            } => {
                if primask {
                    self.primask = disable;
                }
                if faultmask {
                    self.faultmask = disable;
                }
            }
            Op::Mrs { .. } | Op::Msr { .. } => return Err(unimpl(op)), // Needs the special-register model (M1).
            Op::Undecoded | Op::Unimplemented { .. } => return Err(unimpl(op)),
            _ => {
                if !self.fp_enabled {
                    return Err(exc(ExceptionKind::NoCoprocessor));
                }
                self.exec_fp(bus, op)?;
            }
        }
        Ok(())
    }

    #[inline]
    fn dp(&mut self, op: DpOp, s: SetFlags, rd: u8, x: u32, y: u32, shifter_carry: bool) {
        let (r, c, v, write) = match op {
            DpOp::And => (x & y, shifter_carry, self.v, true),
            DpOp::Bic => (x & !y, shifter_carry, self.v, true),
            DpOp::Orr => (x | y, shifter_carry, self.v, true),
            DpOp::Orn => (x | !y, shifter_carry, self.v, true),
            DpOp::Eor => (x ^ y, shifter_carry, self.v, true),
            DpOp::Mov => (y, shifter_carry, self.v, true),
            DpOp::Mvn => (!y, shifter_carry, self.v, true),
            DpOp::Tst => (x & y, shifter_carry, self.v, false),
            DpOp::Teq => (x ^ y, shifter_carry, self.v, false),
            DpOp::Add => {
                let (r, c, v) = add_with_carry(x, y, false);
                (r, c, v, true)
            }
            DpOp::Adc => {
                let (r, c, v) = add_with_carry(x, y, self.c);
                (r, c, v, true)
            }
            DpOp::Sbc => {
                let (r, c, v) = add_with_carry(x, !y, self.c);
                (r, c, v, true)
            }
            DpOp::Sub => {
                let (r, c, v) = add_with_carry(x, !y, true);
                (r, c, v, true)
            }
            DpOp::Rsb => {
                let (r, c, v) = add_with_carry(!x, y, true);
                (r, c, v, true)
            }
            DpOp::Cmp => {
                let (r, c, v) = add_with_carry(x, !y, true);
                (r, c, v, false)
            }
            DpOp::Cmn => {
                let (r, c, v) = add_with_carry(x, y, false);
                (r, c, v, false)
            }
        };
        if write {
            self.write_alu(rd, r);
        }
        if self.want_flags(s) {
            self.set_nz(r);
            self.c = c;
            self.v = v;
        }
    }

    fn exec_mem<B: Bus>(&mut self, bus: &mut B, kind: MemKind, rt: u8, rn: u8, mode: AddrMode) -> Result<(), Stop> {
        let (addr, wb) = match mode {
            AddrMode::Imm { off, index, wback } => {
                let base = self.r[rn as usize];
                let offset_addr = base.wrapping_add(off as u32);
                (
                    if index { offset_addr } else { base },
                    if wback { Some(offset_addr) } else { None },
                )
            }
            AddrMode::Reg { rm, shift } => (self.r[rn as usize].wrapping_add(self.r[rm as usize] << shift), None),
            AddrMode::Literal { off } => ((self.r[15] & !3).wrapping_add(off as u32), None),
        };
        if kind.is_load() {
            let v = match kind {
                MemKind::Ldr => self.load(bus, addr, Size::Word)?,
                MemKind::Ldrh => self.load(bus, addr, Size::Half)?,
                MemKind::Ldrsh => self.load(bus, addr, Size::Half)? as u16 as i16 as i32 as u32,
                MemKind::Ldrb => self.load(bus, addr, Size::Byte)?,
                _ => self.load(bus, addr, Size::Byte)? as u8 as i8 as i32 as u32,
            };
            if let Some(w) = wb {
                self.r[rn as usize] = w;
            }
            if rt == 15 {
                if let Some(s) = self.bx_write_pc(v) {
                    return Err(s);
                }
            } else {
                self.r[rt as usize] = v;
            }
        } else {
            let size = match kind {
                MemKind::Str => Size::Word,
                MemKind::Strh => Size::Half,
                _ => Size::Byte,
            };
            self.store(bus, addr, self.r[rt as usize], size)?;
            if let Some(w) = wb {
                self.r[rn as usize] = w;
            }
        }
        Ok(())
    }

    fn exec_fp<B: Bus>(&mut self, bus: &mut B, op: Op) -> Result<(), Stop> {
        match op {
            Op::VDp { op, d, n, m } => {
                let (a, b, acc) = (self.sf(n), self.sf(m), self.sf(d));
                let r = match op {
                    VOp::Add => a + b,
                    VOp::Sub => a - b,
                    VOp::Mul => a * b,
                    VOp::Nmul => -(a * b),
                    VOp::Div => a / b,
                    VOp::Mla => acc + a * b,
                    VOp::Mls => acc - a * b,
                    VOp::Nmla => -acc - a * b,
                    VOp::Nmls => -acc + a * b,
                    VOp::Fma => a.mul_add(b, acc),
                    VOp::Fms => (-a).mul_add(b, acc),
                    VOp::Fnma => (-a).mul_add(b, -acc),
                    VOp::Fnms => a.mul_add(b, -acc),
                };
                self.set_sf(d, r);
            }
            Op::VUn { op, d, m } => {
                let x = self.s[m as usize];
                self.s[d as usize] = match op {
                    VUnOp::Mov => x,
                    VUnOp::Abs => x & 0x7FFF_FFFF,
                    VUnOp::Neg => x ^ 0x8000_0000,
                    VUnOp::Sqrt => f32::from_bits(x).sqrt().to_bits(),
                };
            }
            Op::VMovImm { d, bits } => self.s[d as usize] = bits,
            Op::VCmp { d, m, .. } => {
                let b = match m {
                    Some(m) => self.sf(m),
                    None => 0.0,
                };
                self.fpscr = (self.fpscr & 0x0FFF_FFFF) | fp_compare_flags(self.sf(d), b) << 28;
            }
            Op::VCvt { kind, d, m } => {
                let x = self.s[m as usize];
                self.s[d as usize] = match kind {
                    VCvtKind::FromS32 => (x as i32 as f32).to_bits(),
                    VCvtKind::FromU32 => (x as f32).to_bits(),
                    // Rust float→int casts saturate and map NaN to 0, as FPToFixed does (A2.5.3).
                    VCvtKind::ToS32 { rz } => {
                        let f = f32::from_bits(x);
                        (if rz { f } else { fp_round(f, self.fpscr) }) as i32 as u32
                    }
                    VCvtKind::ToU32 { rz } => {
                        let f = f32::from_bits(x);
                        (if rz { f } else { fp_round(f, self.fpscr) }) as u32
                    }
                };
            }
            Op::VMovCore { to_core, rt, sn } => {
                if to_core {
                    self.r[rt as usize] = self.s[sn as usize];
                } else {
                    self.s[sn as usize] = self.r[rt as usize];
                }
            }
            Op::VMov2 { to_core, rt, rt2, sm } => {
                if to_core {
                    self.r[rt as usize] = self.s[sm as usize];
                    self.r[rt2 as usize] = self.s[sm as usize + 1];
                } else {
                    self.s[sm as usize] = self.r[rt as usize];
                    self.s[sm as usize + 1] = self.r[rt2 as usize];
                }
            }
            Op::Vmrs { rt } => {
                if rt == 15 {
                    let f = self.fpscr;
                    self.n = f >> 31 != 0;
                    self.z = (f >> 30) & 1 != 0;
                    self.c = (f >> 29) & 1 != 0;
                    self.v = (f >> 28) & 1 != 0;
                } else {
                    self.r[rt as usize] = self.fpscr;
                }
            }
            // Writable FPSCR bits on the M4: NZCV, QC is absent, AHP DN FZ RMode (PM0214 §4.6.4).
            Op::Vmsr { rt } => self.fpscr = self.r[rt as usize] & 0xF7C0_009F,
            Op::VLdSt {
                load,
                double,
                d,
                rn,
                off,
            } => {
                let base = if rn == 15 { self.r[15] & !3 } else { self.r[rn as usize] };
                let addr = base.wrapping_add(off as u32);
                let words = if double { 2 } else { 1 };
                for i in 0..words {
                    let a = addr.wrapping_add(4 * i);
                    let reg = d as usize + i as usize;
                    if load {
                        self.s[reg] = self.load(bus, a, Size::Word)?;
                    } else {
                        self.store(bus, a, self.s[reg], Size::Word)?;
                    }
                }
            }
            Op::VLdStM {
                load,
                d,
                count,
                rn,
                wback,
                db,
            } => {
                let base = self.r[rn as usize];
                let start = if db { base.wrapping_sub(4 * count as u32) } else { base };
                for i in 0..count as u32 {
                    let a = start.wrapping_add(4 * i);
                    let reg = d as usize + i as usize;
                    if reg >= 32 {
                        return Err(unimpl(op));
                    }
                    if load {
                        self.s[reg] = self.load(bus, a, Size::Word)?;
                    } else {
                        self.store(bus, a, self.s[reg], Size::Word)?;
                    }
                }
                if wback {
                    self.r[rn as usize] = if db { start } else { base.wrapping_add(4 * count as u32) };
                }
            }
            _ => return Err(unimpl(op)),
        }
        Ok(())
    }
}

#[inline]
fn mem_size(size: u8) -> Size {
    match size {
        1 => Size::Byte,
        2 => Size::Half,
        _ => Size::Word,
    }
}

fn fault(e: BusError) -> Stop {
    exc(ExceptionKind::BusFault {
        addr: e.addr,
        write: e.write,
    })
}

fn exc(kind: ExceptionKind) -> Stop {
    Stop::Exception { kind, pc: 0 }
}

fn unimpl(op: Op) -> Stop {
    let raw = match op {
        Op::Unimplemented { raw } => raw,
        _ => 0,
    };
    Stop::Unimplemented { raw, pc: 0 }
}

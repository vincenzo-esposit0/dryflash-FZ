// SPDX-License-Identifier: GPL-3.0-or-later
//! Instruction semantics. Pseudocode names (AddWithCarry, Shift_C, …) follow
//! DDI 0403E A2.2 and the per-instruction pages of A7.7.

use super::fp::{self, Fp};
use super::op::*;
use super::{Bus, CONTROL_FPCA, CONTROL_NPRIV, CONTROL_SPSEL, Cpu, Fault, Size, Trap};
use crate::sysctl::{MpuAccess, PpbEffect};

type R = Result<(), Trap>;

/// Internal PPB (DDI 0403E B3.1): ITM, DWT, FPB, SCS. Handled by `sysctl`, not the bus.
/// The external PPB from 0xE004_0000 (TPIU, ETM, vendor DBGMCU) goes to the bus.
const PPB_START: u32 = 0xE000_0000;
const PPB_END: u32 = 0xE004_0000;

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

    /// BXWritePC / LoadWritePC (A2.3.1, B1.5.8): in Handler mode 0xFxxxxxxx is an
    /// EXC_RETURN; otherwise bit 0 sets EPSR.T, and T = 0 faults (INVSTATE) when the
    /// next instruction executes — with the target as the stacked PC, as on hardware.
    #[inline]
    fn bx_write_pc(&mut self, target: u32) -> R {
        if self.handler_mode() && target >> 28 == 0xF {
            return Err(Trap::ExcReturn(target));
        }
        self.t = target & 1 != 0;
        self.branch(target);
        Ok(())
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

    // The hot path of every data access stays inlined; PPB, MPU and error handling are
    // out of line (#[cold]): in WebAssembly a call per memory access costs ~30 % speed.
    #[inline(always)]
    pub(crate) fn load<B: Bus>(&mut self, bus: &mut B, addr: u32, size: Size) -> Result<u32, Trap> {
        if (PPB_START..PPB_END).contains(&addr) {
            return self.ppb_read(bus, addr, size);
        }
        if self.sys.mpu.ctrl & 1 != 0 {
            self.mpu_check(addr, MpuAccess::Read)?;
        }
        bus.set_access_pc(self.cur_pc);
        let r = match bus.read(addr, size) {
            Ok(v) => Ok(v),
            Err(e) => Err(bus_fault(e.addr)),
        };
        // Peripheral accesses (including read-to-clear) may change the interrupt lines.
        if bus.take_irq_dirty() {
            self.exc_check = true;
        }
        r
    }

    #[inline(always)]
    pub(crate) fn store<B: Bus>(&mut self, bus: &mut B, addr: u32, value: u32, size: Size) -> R {
        if (PPB_START..PPB_END).contains(&addr) {
            return self.ppb_write(bus, addr, value, size);
        }
        if self.sys.mpu.ctrl & 1 != 0 {
            self.mpu_check(addr, MpuAccess::Write)?;
        }
        bus.set_access_pc(self.cur_pc);
        let r = match bus.write(addr, value, size) {
            Ok(()) => Ok(()),
            Err(e) => Err(bus_fault(e.addr)),
        };
        // Peripheral accesses (including read-to-clear) may change the interrupt lines.
        if bus.take_irq_dirty() {
            self.exc_check = true;
        }
        r
    }

    #[cold]
    #[inline(never)]
    fn mpu_check(&self, addr: u32, access: MpuAccess) -> R {
        if !self
            .sys
            .mpu
            .allowed(addr, access, self.privileged(), self.mpu_default_map())
        {
            return Err(Trap::Fault(Fault::MemData { addr }));
        }
        Ok(())
    }

    /// PPB accesses always use the default memory map and are privileged-only, except
    /// STIR with CCR.USERSETMPEND (B3.2.2).
    #[cold]
    #[inline(never)]
    fn ppb_read<B: Bus>(&mut self, bus: &mut B, addr: u32, size: Size) -> Result<u32, Trap> {
        if !self.privileged() {
            return Err(Trap::Fault(Fault::BusData { addr }));
        }
        match self.sys.read_sized(addr, size as u32, self.cycles, self.ipsr) {
            Ok(v) => Ok(v),
            Err(u) => {
                // Unimplemented PPB register: reads as zero, reported in the digest.
                bus.ppb_unimplemented(u.addr, false, self.cur_pc);
                Ok(0)
            }
        }
    }

    #[cold]
    #[inline(never)]
    fn ppb_write<B: Bus>(&mut self, bus: &mut B, addr: u32, value: u32, size: Size) -> R {
        let stir = addr == 0xE000_EF00 && self.sys.ccr & 2 != 0;
        if !self.privileged() && !stir {
            return Err(Trap::Fault(Fault::BusData { addr }));
        }
        match self.sys.write_sized(addr, value, size as u32, self.cycles) {
            Ok(PpbEffect::None) => {}
            Ok(PpbEffect::ExceptionStateChanged) => self.exc_check = true,
            Ok(PpbEffect::MpuChanged) => self.mpu_changed(),
            Ok(PpbEffect::ResetRequest) => return Err(Trap::ResetRequest),
            Err(u) => bus.ppb_unimplemented(u.addr, true, self.cur_pc),
        }
        Ok(())
    }

    /// ExecuteFPCheck (B1.5.11 / A6.?): CPACR access, then automatic FP context activation.
    fn fp_check(&mut self) -> R {
        if !self.sys.fp_access(self.privileged()) {
            return Err(Trap::Fault(Fault::NoCoprocessor));
        }
        if self.sys.fpccr & (1 << 31) != 0 && self.control & CONTROL_FPCA == 0 {
            // New FP context: FPSCR takes the FPDSCR defaults (AHP, DN, FZ, RMode).
            self.fpscr = (self.fpscr & !0x07C0_0000) | (self.sys.fpdscr & 0x07C0_0000);
            self.control |= CONTROL_FPCA;
        }
        Ok(())
    }

    /// Executes one decoded instruction. `pc` is its address; `r[15]` already reads as pc + 4.
    pub(super) fn exec<B: Bus>(&mut self, bus: &mut B, op: Op, pc: u32) -> R {
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
                if m == 0 && self.sys.ccr & (1 << 4) != 0 {
                    return Err(Trap::Fault(Fault::DivideByZero));
                }
                // With CCR.DIV_0_TRP = 0 division by zero returns 0 (A7.7.127).
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
                    return Err(Trap::Fault(Fault::Unaligned { addr }));
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
                    return Err(Trap::Fault(Fault::Unaligned { addr: start }));
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
                if let Some(t) = new_pc {
                    self.bx_write_pc(t)?;
                }
            }
            Op::Ldrex { rt, rn, imm, size } => {
                let addr = self.r[rn as usize].wrapping_add(imm as u32);
                let sz = mem_size(size);
                if addr & (size as u32 - 1) != 0 {
                    return Err(Trap::Fault(Fault::Unaligned { addr }));
                }
                self.r[rt as usize] = self.load(bus, addr, sz)?;
                self.exclusive = Some(addr);
            }
            Op::Strex { rd, rt, rn, imm, size } => {
                let addr = self.r[rn as usize].wrapping_add(imm as u32);
                if addr & (size as u32 - 1) != 0 {
                    return Err(Trap::Fault(Fault::Unaligned { addr }));
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
                self.bx_write_pc(target)?;
            }
            Op::Cbz { nz, rn, off } => {
                if (self.r[rn as usize] != 0) == nz {
                    let target = self.r[15].wrapping_add(off as u32);
                    self.branch(target);
                }
            }
            Op::It { state } => self.it = state,
            Op::Hint { kind } => match kind {
                HintKind::Wfi => return Err(Trap::Sleep),
                // WFE: sleep unless the event register is set, which it consumes (B1.5.18).
                HintKind::Wfe => {
                    if self.event {
                        self.event = false;
                    } else {
                        return Err(Trap::Sleep);
                    }
                }
                // Single core: SEV only sets our own event register.
                HintKind::Sev => self.event = true,
                HintKind::Nop | HintKind::Yield | HintKind::Barrier => {}
            },
            Op::Bkpt { imm } => return Err(Trap::Bkpt(imm)),
            Op::Svc { .. } => return Err(Trap::Svc),
            Op::Udf { .. } => return Err(Trap::Fault(Fault::Undefined)),
            Op::NoCp => return Err(Trap::Fault(Fault::NoCoprocessor)),
            Op::Cps {
                disable,
                primask,
                faultmask,
            } => {
                // CPS is ignored in unprivileged code (B5.2.1).
                if self.privileged() {
                    if primask {
                        self.primask = disable;
                    }
                    // FAULTMASK cannot be set at execution priority -1 or above (HardFault/NMI).
                    if faultmask && (!disable || self.execution_priority() > -1) {
                        self.faultmask = disable;
                    }
                    self.exc_check = true;
                    self.mpu_changed();
                }
            }
            Op::Mrs { rd, sysm } => self.r[rd as usize] = self.mrs(sysm),
            Op::Msr { rn, sysm, mask } => self.msr(sysm, mask, self.r[rn as usize]),
            Op::Undecoded | Op::Unimplemented { .. } => return Err(unimpl(op)),
            Op::Sat { .. }
            | Op::Sat16 { .. }
            | Op::Qarith { .. }
            | Op::Sel { .. }
            | Op::Parallel { .. }
            | Op::SmulXY { .. }
            | Op::SmulWY { .. }
            | Op::SmlaD { .. }
            | Op::Smmla { .. }
            | Op::Usad8 { .. }
            | Op::SmlalXY { .. }
            | Op::SmlalD { .. }
            | Op::Extend16 { .. }
            | Op::Pkh { .. } => self.exec_dsp(op),
            _ => {
                self.fp_check()?;
                self.exec_fp(bus, op)?;
            }
        }
        Ok(())
    }

    /// MRS (B5.2.2).
    fn mrs(&self, sysm: u8) -> u32 {
        let privileged = self.privileged();
        match sysm {
            // xPSR views: APSR in [31:27]+GE, IPSR in [8:0]; EPSR reads as zero.
            0..=7 => {
                let mut v = 0;
                if sysm & 1 != 0 {
                    v |= self.ipsr as u32;
                }
                if sysm & 4 == 0 {
                    v |= self.apsr();
                }
                v
            }
            8 if privileged => self.sp_main(),
            9 if privileged => self.sp_process(),
            16 => self.primask as u32,
            17 | 18 => self.basepri as u32,
            19 => self.faultmask as u32,
            20 => self.control & 7,
            _ => 0,
        }
    }

    /// MSR (B5.2.3).
    fn msr(&mut self, sysm: u8, mask: u8, v: u32) {
        let privileged = self.privileged();
        match sysm {
            0..=3 => {
                if mask & 2 != 0 {
                    self.set_apsr_flags(v);
                }
                if mask & 1 != 0 {
                    self.ge = ((v >> 16) & 0xF) as u8;
                }
            }
            8 if privileged => {
                if self.using_psp() {
                    self.msp = v & !3;
                } else {
                    self.r[13] = v & !3;
                }
            }
            9 if privileged => {
                if self.using_psp() {
                    self.r[13] = v & !3;
                } else {
                    self.psp = v & !3;
                }
            }
            16 if privileged => self.primask = v & 1 != 0,
            17 if privileged => self.basepri = (v as u8) & 0xF0,
            18 if privileged => {
                let new = (v as u8) & 0xF0;
                if new != 0 && (new < self.basepri || self.basepri == 0) {
                    self.basepri = new;
                }
            }
            19 if privileged => {
                if self.execution_priority() > -1 {
                    self.faultmask = v & 1 != 0;
                }
            }
            20 if privileged => {
                let was_psp = self.using_psp();
                let mut c = (self.control & !CONTROL_NPRIV) | (v & CONTROL_NPRIV);
                if !self.handler_mode() {
                    c = (c & !CONTROL_SPSEL) | (v & CONTROL_SPSEL);
                }
                c = (c & !CONTROL_FPCA) | (v & CONTROL_FPCA);
                self.control = c;
                self.reselect_sp(was_psp);
                self.mpu_changed();
            }
            _ => {}
        }
        self.exc_check = true;
    }

    #[inline(always)]
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

    #[inline(always)]
    fn exec_mem<B: Bus>(&mut self, bus: &mut B, kind: MemKind, rt: u8, rn: u8, mode: AddrMode) -> R {
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
        // CCR.UNALIGN_TRP makes unaligned word/halfword accesses fault (B3.2.8).
        let align = match kind {
            MemKind::Ldr | MemKind::Str => 3,
            MemKind::Ldrh | MemKind::Ldrsh | MemKind::Strh => 1,
            _ => 0,
        };
        if addr & align != 0 && self.sys.ccr & (1 << 3) != 0 {
            return Err(Trap::Fault(Fault::Unaligned { addr }));
        }
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
                self.bx_write_pc(v)?;
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

    fn exec_fp<B: Bus>(&mut self, bus: &mut B, op: Op) -> R {
        match op {
            Op::VDp { op, d, n, m } => {
                // Compositions follow the A7.7 pseudocode, e.g. VNMLA is
                // FPAdd(FPNeg(d), FPNeg(FPMul(n, m))), so NaN signs and flags match.
                let (sd, sn, sm) = (self.s[d as usize], self.s[n as usize], self.s[m as usize]);
                let mut fp = Fp { fpscr: &mut self.fpscr };
                let r = match op {
                    VOp::Add => fp.add(sn, sm),
                    VOp::Sub => fp.sub(sn, sm),
                    VOp::Mul => fp.mul(sn, sm),
                    VOp::Nmul => fp::neg(fp.mul(sn, sm)),
                    VOp::Div => fp.div(sn, sm),
                    VOp::Mla => {
                        let p = fp.mul(sn, sm);
                        fp.add(sd, p)
                    }
                    VOp::Mls => {
                        let p = fp.mul(sn, sm);
                        fp.add(sd, fp::neg(p))
                    }
                    VOp::Nmla => {
                        let p = fp.mul(sn, sm);
                        fp.add(fp::neg(sd), fp::neg(p))
                    }
                    VOp::Nmls => {
                        let p = fp.mul(sn, sm);
                        fp.add(fp::neg(sd), p)
                    }
                    VOp::Fma => fp.mul_add(sd, sn, sm),
                    VOp::Fms => fp.mul_add(sd, fp::neg(sn), sm),
                    VOp::Fnma => fp.mul_add(fp::neg(sd), fp::neg(sn), sm),
                    VOp::Fnms => fp.mul_add(fp::neg(sd), sn, sm),
                };
                self.s[d as usize] = r;
            }
            Op::VUn { op, d, m } => {
                let x = self.s[m as usize];
                self.s[d as usize] = match op {
                    VUnOp::Mov => x,
                    VUnOp::Abs => x & 0x7FFF_FFFF,
                    VUnOp::Neg => x ^ 0x8000_0000,
                    VUnOp::Sqrt => Fp { fpscr: &mut self.fpscr }.sqrt(x),
                };
            }
            Op::VMovImm { d, bits } => self.s[d as usize] = bits,
            Op::VCmp { e, d, m } => {
                let b = m.map(|m| self.s[m as usize]).unwrap_or(0);
                let a = self.s[d as usize];
                let nzcv = Fp { fpscr: &mut self.fpscr }.compare(a, b, e);
                self.fpscr = (self.fpscr & 0x0FFF_FFFF) | nzcv << 28;
            }
            Op::VCvt { kind, d, m } => {
                let x = self.s[m as usize];
                let rmode = (self.fpscr >> 22) & 3;
                let round = move |v: f64| match rmode {
                    0 => v.round_ties_even(),
                    1 => v.ceil(),
                    2 => v.floor(),
                    _ => v.trunc(),
                };
                let mut fp = Fp { fpscr: &mut self.fpscr };
                self.s[d as usize] = match kind {
                    VCvtKind::FromS32 => fp.cvt_from_int(x, false),
                    VCvtKind::FromU32 => fp.cvt_from_int(x, true),
                    VCvtKind::ToS32 { rz } => {
                        if rz {
                            fp.cvt_to_int(x, false, f64::trunc)
                        } else {
                            fp.cvt_to_int(x, false, round)
                        }
                    }
                    VCvtKind::ToU32 { rz } => {
                        if rz {
                            fp.cvt_to_int(x, true, f64::trunc)
                        } else {
                            fp.cvt_to_int(x, true, round)
                        }
                    }
                };
            }
            Op::VCvtFixed {
                to_fixed,
                unsigned,
                half,
                d,
                frac_bits,
            } => {
                let scale = 2f64.powi(frac_bits as i32);
                let bits = if half { 16 } else { 32 };
                let x = self.s[d as usize];
                let mut fpscr = self.fpscr;
                self.s[d as usize] = if to_fixed {
                    // FPToFixed with round towards zero, saturating (A2.5.9).
                    let x = if fpscr & (1 << 24) != 0 && x & 0x7F80_0000 == 0 && x & 0x7F_FFFF != 0 {
                        fpscr |= fp::IDC;
                        x & 0x8000_0000
                    } else {
                        x
                    };
                    let f = f32::from_bits(x) as f64;
                    let (lo, hi) = if unsigned {
                        (0.0, ((1u64 << bits) - 1) as f64)
                    } else {
                        (-((1i64 << (bits - 1)) as f64), ((1i64 << (bits - 1)) - 1) as f64)
                    };
                    let v = (f * scale).trunc();
                    let out = if f.is_nan() {
                        fpscr |= fp::IOC;
                        0.0
                    } else if v < lo || v > hi {
                        fpscr |= fp::IOC;
                        v.clamp(lo, hi)
                    } else {
                        if v != f * scale {
                            fpscr |= fp::IXC;
                        }
                        v
                    };
                    if unsigned {
                        out as u32
                    } else if half {
                        out as i32 as i16 as i32 as u32
                    } else {
                        out as i32 as u32
                    }
                } else {
                    let int = match (half, unsigned) {
                        (true, true) => (x & 0xFFFF) as f64,
                        (true, false) => (x as u16 as i16) as f64,
                        (false, true) => x as f64,
                        (false, false) => (x as i32) as f64,
                    };
                    let r = (int / scale) as f32;
                    if r as f64 != int / scale {
                        fpscr |= fp::IXC;
                    }
                    r.to_bits()
                };
                self.fpscr = fpscr;
            }
            Op::VCvtHalf { to_half, top, d, m } => {
                if to_half {
                    let x = self.s[m as usize];
                    let (h, flags) = f32_to_f16(f32::from_bits(x));
                    self.fpscr |= flags;
                    let shift = if top { 16 } else { 0 };
                    let keep = !(0xFFFFu32 << shift);
                    self.s[d as usize] = (self.s[d as usize] & keep) | (h as u32) << shift;
                } else {
                    let src = self.s[m as usize];
                    let h = if top { src >> 16 } else { src } as u16;
                    // A signalling half NaN is quietened and raises IOC.
                    if h & 0x7C00 == 0x7C00 && h & 0x3FF != 0 && h & 0x200 == 0 {
                        self.fpscr |= fp::IOC;
                    }
                    self.s[d as usize] = f16_to_f32(h).to_bits();
                }
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

/// IEEE half → single (FPSCR.AHP = 0). Every half value is exact in single precision.
pub(crate) fn f16_to_f32(h: u16) -> f32 {
    let sign = ((h >> 15) as u32) << 31;
    let exp = ((h >> 10) & 0x1F) as u32;
    let frac = (h & 0x3FF) as u32;
    let bits = match (exp, frac) {
        (0, 0) => sign,
        (0, f) => {
            // Subnormal half: normalise.
            let shift = f.leading_zeros() - 21;
            let f = (f << shift) & 0x3FF;
            sign | ((127 - 15 + 1 - shift) << 23) | (f << 13)
        }
        (0x1F, 0) => sign | 0x7F80_0000,
        (0x1F, f) => sign | 0x7FC0_0000 | (f << 13),
        (e, f) => sign | ((e + 127 - 15) << 23) | (f << 13),
    };
    f32::from_bits(bits)
}

/// IEEE single → half (FPSCR.AHP = 0), round to nearest even. Returns the half value
/// and the FPSCR exception flags raised.
pub(crate) fn f32_to_f16(x: f32) -> (u16, u32) {
    let b = x.to_bits();
    let sign = ((b >> 16) & 0x8000) as u16;
    let exp = ((b >> 23) & 0xFF) as i32;
    let mant = b & 0x7F_FFFF;
    if exp == 0xFF {
        if mant == 0 {
            return (sign | 0x7C00, 0);
        }
        // NaN: quiet, keep the top payload bits; a signalling NaN raises IOC.
        let flags = if mant & 0x40_0000 == 0 { fp::IOC } else { 0 };
        return (sign | 0x7E00 | (mant >> 13) as u16, flags);
    }
    let e = exp - 127 + 15;
    if e >= 0x1F {
        return (sign | 0x7C00, fp::OFC | fp::IXC);
    }
    let (m, shift) = if e <= 0 {
        if e < -10 {
            let flags = if b & 0x7FFF_FFFF != 0 { fp::UFC | fp::IXC } else { 0 };
            return (sign, flags);
        }
        (mant | 0x80_0000, (14 - e) as u32)
    } else {
        (mant, 13)
    };
    let mut h = m >> shift;
    let rem = m & ((1 << shift) - 1);
    let halfway = 1 << (shift - 1);
    if rem > halfway || (rem == halfway && h & 1 != 0) {
        h += 1;
    }
    let out = if e > 0 {
        // Normal: a mantissa carry correctly bumps the exponent (possibly to Inf).
        sign | (((e as u32) << 10) + h) as u16
    } else {
        sign | h as u16
    };
    let mut flags = 0;
    if rem != 0 {
        flags |= fp::IXC;
        if out & 0x7C00 == 0 {
            flags |= fp::UFC;
        }
    }
    if out & 0x7FFF == 0x7C00 {
        flags |= fp::OFC | fp::IXC;
    }
    (out, flags)
}

#[inline]
fn mem_size(size: u8) -> Size {
    match size {
        1 => Size::Byte,
        2 => Size::Half,
        _ => Size::Word,
    }
}

#[cold]
fn bus_fault(addr: u32) -> Trap {
    Trap::Fault(Fault::BusData { addr })
}

pub(crate) fn unimpl(op: Op) -> Trap {
    match op {
        Op::Unimplemented { raw } => Trap::Unimplemented(raw),
        _ => Trap::Unimplemented(0),
    }
}

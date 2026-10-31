// SPDX-License-Identifier: GPL-3.0-or-later
//! Pre-decoded instruction representation.
//!
//! Instructions are decoded once into an [`Op`] and cached per halfword address
//! (see `decode_cache.rs`). Field names follow the ARMv7-M Architecture Reference
//! Manual (DDI 0403E), chapter A7 and A7.7 for the FP extension.

/// When an instruction updates APSR.NZCV.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetFlags {
    No,
    Yes,
    /// 16-bit encodings: set flags only when not inside an IT block (DDI 0403E A7.3.3).
    OutsideIt,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DpOp {
    And,
    Bic,
    Orr,
    Orn,
    Eor,
    Mov,
    Mvn,
    Tst,
    Teq,
    Add,
    Adc,
    Sbc,
    Sub,
    Rsb,
    Cmp,
    Cmn,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShiftType {
    Lsl,
    Lsr,
    Asr,
    Ror,
    /// Rotate right with extend by one bit (ROR #0 encoding).
    Rrx,
}

/// A decoded immediate shift (`DecodeImmShift`, DDI 0403E A7.4.2): `amt` is already 1..=32
/// for LSR/ASR #0, and 0 only for LSL #0.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shift {
    pub ty: ShiftType,
    pub amt: u8,
}

impl Shift {
    pub const NONE: Shift = Shift {
        ty: ShiftType::Lsl,
        amt: 0,
    };
}

/// Carry produced by `ThumbExpandImm_C` (DDI 0403E A5.3.2): unchanged, or a fixed value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImmCarry {
    Unchanged,
    Clear,
    Set,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemKind {
    Ldr,
    Ldrh,
    Ldrsh,
    Ldrb,
    Ldrsb,
    Str,
    Strh,
    Strb,
}

impl MemKind {
    pub fn is_load(self) -> bool {
        !matches!(self, MemKind::Str | MemKind::Strh | MemKind::Strb)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddrMode {
    /// `[Rn, #off]`, `[Rn, #off]!` or `[Rn], #off`. `off` already carries the U bit sign.
    Imm { off: i32, index: bool, wback: bool },
    /// `[Rn, Rm, LSL #shift]`.
    Reg { rm: u8, shift: u8 },
    /// `[PC, #off]` with `Align(PC, 4)` as base.
    Literal { off: i32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExtendKind {
    Sxtb,
    Sxth,
    Uxtb,
    Uxth,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RevKind {
    Rev,
    Rev16,
    Revsh,
    Rbit,
    Clz,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LongMulKind {
    Umull,
    Smull,
    Umlal,
    Smlal,
    Umaal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HintKind {
    Nop,
    Yield,
    Wfe,
    Wfi,
    Sev,
    /// DMB/DSB/ISB: no architectural effect in a single-core, in-order model.
    Barrier,
}

/// FP data-processing operations with three operands (DDI 0403E A6.4.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VOp {
    Add,
    Sub,
    Mul,
    Nmul,
    Div,
    /// VMLA: d = d + n*m (rounded twice).
    Mla,
    Mls,
    Nmla,
    Nmls,
    /// VFMA: d = d + n*m (fused).
    Fma,
    Fms,
    Fnma,
    Fnms,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VUnOp {
    Mov,
    Abs,
    Neg,
    Sqrt,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VCvtKind {
    /// Signed/unsigned integer in Sm to f32 in Sd.
    FromS32,
    FromU32,
    /// f32 in Sm to integer in Sd; `rz` = round towards zero, otherwise FPSCR.RMode.
    ToS32 {
        rz: bool,
    },
    ToU32 {
        rz: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    /// Cache slot not decoded yet.
    Undecoded,
    /// Encoding not implemented (or UNDEFINED); `raw` is hw1 << 16 | hw2 (or hw1).
    Unimplemented {
        raw: u32,
    },
    Udf {
        imm: u8,
    },

    DpImm {
        op: DpOp,
        s: SetFlags,
        rd: u8,
        rn: u8,
        imm: u32,
        carry: ImmCarry,
    },
    DpReg {
        op: DpOp,
        s: SetFlags,
        rd: u8,
        rn: u8,
        rm: u8,
        sh: Shift,
    },
    /// LSL/LSR/ASR/ROR by register.
    ShiftReg {
        ty: ShiftType,
        s: SetFlags,
        rd: u8,
        rn: u8,
        rm: u8,
    },
    Movw {
        rd: u8,
        imm: u16,
    },
    Movt {
        rd: u8,
        imm: u16,
    },
    /// `ADR`: Align(PC, 4) + off.
    Adr {
        rd: u8,
        off: i32,
    },
    Bfx {
        signed: bool,
        rd: u8,
        rn: u8,
        lsb: u8,
        width: u8,
    },
    /// BFI, or BFC when `rn == 15`.
    Bfi {
        rd: u8,
        rn: u8,
        lsb: u8,
        msb: u8,
    },
    /// Extend with optional add (`rn == 15` means no add).
    Extend {
        kind: ExtendKind,
        rd: u8,
        rn: u8,
        rm: u8,
        rot: u8,
    },
    Rev {
        kind: RevKind,
        rd: u8,
        rm: u8,
    },

    Mul {
        s: SetFlags,
        rd: u8,
        rn: u8,
        rm: u8,
    },
    Mla {
        sub: bool,
        rd: u8,
        rn: u8,
        rm: u8,
        ra: u8,
    },
    LongMul {
        kind: LongMulKind,
        rdlo: u8,
        rdhi: u8,
        rn: u8,
        rm: u8,
    },
    Div {
        signed: bool,
        rd: u8,
        rn: u8,
        rm: u8,
    },

    Mem {
        kind: MemKind,
        rt: u8,
        rn: u8,
        mode: AddrMode,
    },
    Dual {
        load: bool,
        rt: u8,
        rt2: u8,
        rn: u8,
        off: i32,
        index: bool,
        wback: bool,
    },
    /// LDM/STM/PUSH/POP. `db` = decrement before, otherwise increment after.
    Multi {
        load: bool,
        rn: u8,
        list: u16,
        wback: bool,
        db: bool,
    },
    Ldrex {
        rt: u8,
        rn: u8,
        imm: u16,
        size: u8,
    },
    Strex {
        rd: u8,
        rt: u8,
        rn: u8,
        imm: u16,
        size: u8,
    },
    Clrex,
    Tb {
        half: bool,
        rn: u8,
        rm: u8,
    },

    /// Conditional or unconditional branch; `cond == 14` means always.
    B {
        cond: u8,
        off: i32,
    },
    Bl {
        off: i32,
    },
    Bx {
        rm: u8,
        link: bool,
    },
    Cbz {
        nz: bool,
        rn: u8,
        off: u8,
    },
    It {
        state: u8,
    },
    Hint {
        kind: HintKind,
    },
    Bkpt {
        imm: u8,
    },
    Svc {
        imm: u8,
    },
    Cps {
        disable: bool,
        primask: bool,
        faultmask: bool,
    },
    Mrs {
        rd: u8,
        sysm: u8,
    },
    Msr {
        rn: u8,
        sysm: u8,
        mask: u8,
    },

    VDp {
        op: VOp,
        d: u8,
        n: u8,
        m: u8,
    },
    VUn {
        op: VUnOp,
        d: u8,
        m: u8,
    },
    VMovImm {
        d: u8,
        bits: u32,
    },
    /// VCMP/VCMPE; `m == None` compares with +0.0.
    VCmp {
        e: bool,
        d: u8,
        m: Option<u8>,
    },
    VCvt {
        kind: VCvtKind,
        d: u8,
        m: u8,
    },
    /// VMOV between a core register and a single-precision register.
    VMovCore {
        to_core: bool,
        rt: u8,
        sn: u8,
    },
    /// VMOV between two core registers and two consecutive S registers (or one D register).
    VMov2 {
        to_core: bool,
        rt: u8,
        rt2: u8,
        sm: u8,
    },
    Vmrs {
        rt: u8,
    },
    Vmsr {
        rt: u8,
    },
    /// VLDR/VSTR; `d` is an S index; `double` transfers S[d], S[d+1].
    VLdSt {
        load: bool,
        double: bool,
        d: u8,
        rn: u8,
        off: i32,
    },
    /// VLDM/VSTM/VPUSH/VPOP; `d` is the first S index, `count` in words.
    VLdStM {
        load: bool,
        d: u8,
        count: u8,
        rn: u8,
        wback: bool,
        db: bool,
    },
}

/// A cached decoded instruction: the operation plus its encoded size (2 or 4 bytes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Decoded {
    pub op: Op,
    pub size: u8,
}

impl Decoded {
    pub const EMPTY: Decoded = Decoded {
        op: Op::Undecoded,
        size: 0,
    };
}

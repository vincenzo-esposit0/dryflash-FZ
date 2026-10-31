// SPDX-License-Identifier: GPL-3.0-or-later
//! Thumb / Thumb-2 / FPv4-SP decoder.
//!
//! Section references are to the ARMv7-M Architecture Reference Manual, DDI 0403E:
//! A5.2 (16-bit encodings), A5.3 (32-bit encodings), A6 (FP encodings).
//! Encodings not implemented yet decode to [`Op::Unimplemented`] so the bring-up
//! digest can report them; they are never silently treated as NOPs.

use super::op::*;

/// True when `hw1` is the first halfword of a 32-bit instruction (A5.1).
#[inline]
pub fn is_32bit(hw1: u16) -> bool {
    (hw1 >> 11) >= 0b11101
}

#[inline]
fn bits(v: u32, hi: u32, lo: u32) -> u32 {
    (v >> lo) & ((1 << (hi - lo + 1)) - 1)
}

#[inline]
fn bit(v: u32, n: u32) -> bool {
    (v >> n) & 1 != 0
}

#[inline]
fn sign_extend(v: u32, width: u32) -> i32 {
    let s = 32 - width;
    ((v << s) as i32) >> s
}

/// DecodeImmShift (A7.4.2).
fn decode_imm_shift(ty: u32, imm5: u32) -> Shift {
    match ty {
        0 => Shift {
            ty: ShiftType::Lsl,
            amt: imm5 as u8,
        },
        1 => Shift {
            ty: ShiftType::Lsr,
            amt: if imm5 == 0 { 32 } else { imm5 as u8 },
        },
        2 => Shift {
            ty: ShiftType::Asr,
            amt: if imm5 == 0 { 32 } else { imm5 as u8 },
        },
        _ => {
            if imm5 == 0 {
                Shift {
                    ty: ShiftType::Rrx,
                    amt: 1,
                }
            } else {
                Shift {
                    ty: ShiftType::Ror,
                    amt: imm5 as u8,
                }
            }
        }
    }
}

/// ThumbExpandImm_C (A5.3.2). Returns `None` for UNPREDICTABLE encodings.
fn thumb_expand_imm(imm12: u32) -> Option<(u32, ImmCarry)> {
    if bits(imm12, 11, 10) == 0 {
        let b = imm12 & 0xFF;
        let v = match bits(imm12, 9, 8) {
            0 => b,
            1 => {
                if b == 0 {
                    return None;
                }
                b << 16 | b
            }
            2 => {
                if b == 0 {
                    return None;
                }
                b << 24 | b << 8
            }
            _ => {
                if b == 0 {
                    return None;
                }
                b << 24 | b << 16 | b << 8 | b
            }
        };
        Some((v, ImmCarry::Unchanged))
    } else {
        let unrot = 0x80 | (imm12 & 0x7F);
        let v = unrot.rotate_right(bits(imm12, 11, 7));
        Some((v, if v >> 31 != 0 { ImmCarry::Set } else { ImmCarry::Clear }))
    }
}

fn d(op: Op, size: u8) -> Decoded {
    Decoded { op, size }
}

/// Decodes the instruction starting with `hw1`; `hw2` is ignored for 16-bit encodings.
pub fn decode(hw1: u16, hw2: u16) -> Decoded {
    if is_32bit(hw1) {
        d(decode32(hw1 as u32, hw2 as u32), 4)
    } else {
        d(decode16(hw1 as u32), 2)
    }
}

fn decode16(h: u32) -> Op {
    let un = Op::Unimplemented { raw: h };
    let r3 = |lo: u32| bits(h, lo + 2, lo) as u8;
    match bits(h, 15, 10) {
        // A5.2.1 shift (immediate), add, subtract, move, and compare.
        0b000000..=0b001111 => {
            let (rd, rn) = (r3(0), r3(3));
            match bits(h, 13, 9) {
                0b00000..=0b01011 => {
                    let ty = bits(h, 12, 11);
                    let sh = decode_imm_shift(ty, bits(h, 10, 6));
                    Op::DpReg {
                        op: DpOp::Mov,
                        s: SetFlags::OutsideIt,
                        rd,
                        rn: 0,
                        rm: rn,
                        sh,
                    }
                }
                0b01100 => Op::DpReg {
                    op: DpOp::Add,
                    s: SetFlags::OutsideIt,
                    rd,
                    rn,
                    rm: r3(6),
                    sh: Shift::NONE,
                },
                0b01101 => Op::DpReg {
                    op: DpOp::Sub,
                    s: SetFlags::OutsideIt,
                    rd,
                    rn,
                    rm: r3(6),
                    sh: Shift::NONE,
                },
                0b01110 | 0b01111 => Op::DpImm {
                    op: if bit(h, 9) { DpOp::Sub } else { DpOp::Add },
                    s: SetFlags::OutsideIt,
                    rd,
                    rn,
                    imm: bits(h, 8, 6),
                    carry: ImmCarry::Unchanged,
                },
                op => {
                    let rdn = r3(8);
                    let imm = h & 0xFF;
                    let (dp, s) = match op >> 2 {
                        0b100 => (DpOp::Mov, SetFlags::OutsideIt),
                        0b101 => (DpOp::Cmp, SetFlags::Yes),
                        0b110 => (DpOp::Add, SetFlags::OutsideIt),
                        _ => (DpOp::Sub, SetFlags::OutsideIt),
                    };
                    Op::DpImm {
                        op: dp,
                        s,
                        rd: rdn,
                        rn: rdn,
                        imm,
                        carry: ImmCarry::Unchanged,
                    }
                }
            }
        }
        // A5.2.2 data processing.
        0b010000 => {
            let (rdn, rm) = (r3(0), r3(3));
            let s = SetFlags::OutsideIt;
            let dp = |op| Op::DpReg {
                op,
                s,
                rd: rdn,
                rn: rdn,
                rm,
                sh: Shift::NONE,
            };
            let shr = |ty| Op::ShiftReg {
                ty,
                s,
                rd: rdn,
                rn: rdn,
                rm,
            };
            match bits(h, 9, 6) {
                0b0000 => dp(DpOp::And),
                0b0001 => dp(DpOp::Eor),
                0b0010 => shr(ShiftType::Lsl),
                0b0011 => shr(ShiftType::Lsr),
                0b0100 => shr(ShiftType::Asr),
                0b0101 => dp(DpOp::Adc),
                0b0110 => dp(DpOp::Sbc),
                0b0111 => shr(ShiftType::Ror),
                0b1000 => Op::DpReg {
                    op: DpOp::Tst,
                    s: SetFlags::Yes,
                    rd: rdn,
                    rn: rdn,
                    rm,
                    sh: Shift::NONE,
                },
                0b1001 => Op::DpImm {
                    op: DpOp::Rsb,
                    s,
                    rd: rdn,
                    rn: rm,
                    imm: 0,
                    carry: ImmCarry::Unchanged,
                },
                0b1010 => Op::DpReg {
                    op: DpOp::Cmp,
                    s: SetFlags::Yes,
                    rd: rdn,
                    rn: rdn,
                    rm,
                    sh: Shift::NONE,
                },
                0b1011 => Op::DpReg {
                    op: DpOp::Cmn,
                    s: SetFlags::Yes,
                    rd: rdn,
                    rn: rdn,
                    rm,
                    sh: Shift::NONE,
                },
                0b1100 => dp(DpOp::Orr),
                0b1101 => Op::Mul {
                    s,
                    rd: rdn,
                    rn: rm,
                    rm: rdn,
                },
                0b1110 => dp(DpOp::Bic),
                _ => Op::DpReg {
                    op: DpOp::Mvn,
                    s,
                    rd: rdn,
                    rn: 0,
                    rm,
                    sh: Shift::NONE,
                },
            }
        }
        // A5.2.3 special data instructions and branch and exchange.
        0b010001 => {
            let rdn = (bits(h, 7, 7) << 3 | bits(h, 2, 0)) as u8;
            let rm = bits(h, 6, 3) as u8;
            match bits(h, 9, 8) {
                0b00 => Op::DpReg {
                    op: DpOp::Add,
                    s: SetFlags::No,
                    rd: rdn,
                    rn: rdn,
                    rm,
                    sh: Shift::NONE,
                },
                0b01 => Op::DpReg {
                    op: DpOp::Cmp,
                    s: SetFlags::Yes,
                    rd: rdn,
                    rn: rdn,
                    rm,
                    sh: Shift::NONE,
                },
                0b10 => Op::DpReg {
                    op: DpOp::Mov,
                    s: SetFlags::No,
                    rd: rdn,
                    rn: 0,
                    rm,
                    sh: Shift::NONE,
                },
                _ => Op::Bx { rm, link: bit(h, 7) },
            }
        }
        0b010010 | 0b010011 => Op::Mem {
            kind: MemKind::Ldr,
            rt: r3(8),
            rn: 15,
            mode: AddrMode::Literal {
                off: ((h & 0xFF) << 2) as i32,
            },
        },
        // A5.2.4 load/store single data item.
        0b010100..=0b100111 => {
            let (rt, rn) = (r3(0), r3(3));
            let imm5 = bits(h, 10, 6);
            let imm = |scale: u32| AddrMode::Imm {
                off: (imm5 << scale) as i32,
                index: true,
                wback: false,
            };
            match bits(h, 15, 12) {
                0b0101 => {
                    let kind = match bits(h, 11, 9) {
                        0 => MemKind::Str,
                        1 => MemKind::Strh,
                        2 => MemKind::Strb,
                        3 => MemKind::Ldrsb,
                        4 => MemKind::Ldr,
                        5 => MemKind::Ldrh,
                        6 => MemKind::Ldrb,
                        _ => MemKind::Ldrsh,
                    };
                    Op::Mem {
                        kind,
                        rt,
                        rn,
                        mode: AddrMode::Reg { rm: r3(6), shift: 0 },
                    }
                }
                0b0110 => Op::Mem {
                    kind: if bit(h, 11) { MemKind::Ldr } else { MemKind::Str },
                    rt,
                    rn,
                    mode: imm(2),
                },
                0b0111 => Op::Mem {
                    kind: if bit(h, 11) { MemKind::Ldrb } else { MemKind::Strb },
                    rt,
                    rn,
                    mode: imm(0),
                },
                0b1000 => Op::Mem {
                    kind: if bit(h, 11) { MemKind::Ldrh } else { MemKind::Strh },
                    rt,
                    rn,
                    mode: imm(1),
                },
                _ => Op::Mem {
                    kind: if bit(h, 11) { MemKind::Ldr } else { MemKind::Str },
                    rt: r3(8),
                    rn: 13,
                    mode: AddrMode::Imm {
                        off: ((h & 0xFF) << 2) as i32,
                        index: true,
                        wback: false,
                    },
                },
            }
        }
        0b101000 | 0b101001 => Op::Adr {
            rd: r3(8),
            off: ((h & 0xFF) << 2) as i32,
        },
        0b101010 | 0b101011 => Op::DpImm {
            op: DpOp::Add,
            s: SetFlags::No,
            rd: r3(8),
            rn: 13,
            imm: (h & 0xFF) << 2,
            carry: ImmCarry::Unchanged,
        },
        // A5.2.5 miscellaneous 16-bit instructions.
        0b101100..=0b101111 => decode16_misc(h),
        0b110000 | 0b110001 => Op::Multi {
            load: false,
            rn: r3(8),
            list: (h & 0xFF) as u16,
            wback: true,
            db: false,
        },
        0b110010 | 0b110011 => {
            let rn = r3(8);
            let list = (h & 0xFF) as u16;
            Op::Multi {
                load: true,
                rn,
                list,
                wback: list & (1 << rn) == 0,
                db: false,
            }
        }
        // A5.2.6 conditional branch and supervisor call.
        0b110100..=0b110111 => match bits(h, 11, 8) {
            0b1110 => Op::Udf { imm: h as u8 },
            0b1111 => Op::Svc { imm: h as u8 },
            cond => Op::B {
                cond: cond as u8,
                off: sign_extend((h & 0xFF) << 1, 9),
            },
        },
        0b111000 | 0b111001 => Op::B {
            cond: 14,
            off: sign_extend((h & 0x7FF) << 1, 12),
        },
        _ => un,
    }
}

fn decode16_misc(h: u32) -> Op {
    let (r0, r3) = (bits(h, 2, 0) as u8, bits(h, 5, 3) as u8);
    if h & 0xFF00 == 0xB000 {
        let op = if bit(h, 7) { DpOp::Sub } else { DpOp::Add };
        return Op::DpImm {
            op,
            s: SetFlags::No,
            rd: 13,
            rn: 13,
            imm: (h & 0x7F) << 2,
            carry: ImmCarry::Unchanged,
        };
    }
    if h & 0xF500 == 0xB100 {
        let off = (bits(h, 9, 9) << 6 | bits(h, 7, 3) << 1) as u8;
        return Op::Cbz {
            nz: bit(h, 11),
            rn: r0,
            off,
        };
    }
    if h & 0xFF00 == 0xB200 {
        let kind = match bits(h, 7, 6) {
            0 => ExtendKind::Sxth,
            1 => ExtendKind::Sxtb,
            2 => ExtendKind::Uxth,
            _ => ExtendKind::Uxtb,
        };
        return Op::Extend {
            kind,
            rd: r0,
            rn: 15,
            rm: r3,
            rot: 0,
        };
    }
    if h & 0xFE00 == 0xB400 {
        let list = (h & 0xFF) as u16 | if bit(h, 8) { 1 << 14 } else { 0 };
        return Op::Multi {
            load: false,
            rn: 13,
            list,
            wback: true,
            db: true,
        };
    }
    if h & 0xFFE8 == 0xB660 {
        return Op::Cps {
            disable: bit(h, 4),
            primask: bit(h, 1),
            faultmask: bit(h, 0),
        };
    }
    if h & 0xFF00 == 0xBA00 {
        let kind = match bits(h, 7, 6) {
            0 => RevKind::Rev,
            1 => RevKind::Rev16,
            3 => RevKind::Revsh,
            _ => return Op::Unimplemented { raw: h },
        };
        return Op::Rev { kind, rd: r0, rm: r3 };
    }
    if h & 0xFE00 == 0xBC00 {
        let list = (h & 0xFF) as u16 | if bit(h, 8) { 1 << 15 } else { 0 };
        return Op::Multi {
            load: true,
            rn: 13,
            list,
            wback: true,
            db: false,
        };
    }
    if h & 0xFF00 == 0xBE00 {
        return Op::Bkpt { imm: h as u8 };
    }
    if h & 0xFF00 == 0xBF00 {
        if h & 0xF != 0 {
            return Op::It { state: h as u8 };
        }
        return match bits(h, 7, 4) {
            0 => Op::Hint { kind: HintKind::Nop },
            1 => Op::Hint { kind: HintKind::Yield },
            2 => Op::Hint { kind: HintKind::Wfe },
            3 => Op::Hint { kind: HintKind::Wfi },
            4 => Op::Hint { kind: HintKind::Sev },
            _ => Op::Hint { kind: HintKind::Nop },
        };
    }
    Op::Unimplemented { raw: h }
}

fn decode32(h1: u32, h2: u32) -> Op {
    let raw = h1 << 16 | h2;
    let un = Op::Unimplemented { raw };
    let op1 = bits(h1, 12, 11);
    let op2 = bits(h1, 10, 4);
    match op1 {
        0b01 => {
            if op2 & 0b1100100 == 0 {
                decode_ldst_multiple(h1, h2).unwrap_or(un)
            } else if op2 & 0b1100100 == 0b0000100 {
                decode_ldst_dual(h1, h2).unwrap_or(un)
            } else if op2 & 0b1100000 == 0b0100000 {
                decode_dp_shifted(h1, h2).unwrap_or(un)
            } else {
                decode_coproc(h1, h2).unwrap_or(un)
            }
        }
        0b10 => {
            if bit(h2, 15) {
                decode_branch_misc(h1, h2).unwrap_or(un)
            } else if !bit(h1, 9) {
                decode_dp_modified_imm(h1, h2).unwrap_or(un)
            } else {
                decode_dp_plain_imm(h1, h2).unwrap_or(un)
            }
        }
        _ => {
            if op2 & 0b1000000 != 0 {
                decode_coproc(h1, h2).unwrap_or(un)
            } else if op2 & 0b1110001 == 0 {
                decode_store_single(h1, h2).unwrap_or(un)
            } else if op2 & 0b1100111 == 0b0000001 {
                decode_load(h1, h2, MemKind::Ldrb, MemKind::Ldrsb).unwrap_or(un)
            } else if op2 & 0b1100111 == 0b0000011 {
                decode_load(h1, h2, MemKind::Ldrh, MemKind::Ldrsh).unwrap_or(un)
            } else if op2 & 0b1100111 == 0b0000101 {
                decode_load(h1, h2, MemKind::Ldr, MemKind::Ldr).unwrap_or(un)
            } else if op2 & 0b1110000 == 0b0100000 {
                decode_dp_register(h1, h2).unwrap_or(un)
            } else if op2 & 0b1111000 == 0b0110000 {
                decode_multiply(h1, h2).unwrap_or(un)
            } else if op2 & 0b1111000 == 0b0111000 {
                decode_long_multiply(h1, h2).unwrap_or(un)
            } else {
                un
            }
        }
    }
}

/// A5.3.5 load/store multiple.
fn decode_ldst_multiple(h1: u32, h2: u32) -> Option<Op> {
    let load = bit(h1, 4);
    let wback = bit(h1, 5);
    let rn = bits(h1, 3, 0) as u8;
    let list = h2 as u16;
    let db = match bits(h1, 8, 7) {
        0b01 => false,
        0b10 => true,
        _ => return None,
    };
    Some(Op::Multi {
        load,
        rn,
        list,
        wback,
        db,
    })
}

/// A5.3.6 load/store dual or exclusive, table branch.
fn decode_ldst_dual(h1: u32, h2: u32) -> Option<Op> {
    let (op1, op2, op3) = (bits(h1, 8, 7), bits(h1, 5, 4), bits(h2, 7, 4));
    let rn = bits(h1, 3, 0) as u8;
    let rt = bits(h2, 15, 12) as u8;
    let rd = bits(h2, 11, 8) as u8;
    let rd_low = bits(h2, 3, 0) as u8;
    if op1 == 0 && op2 == 0 {
        return Some(Op::Strex {
            rd,
            rt,
            rn,
            imm: ((h2 & 0xFF) << 2) as u16,
            size: 4,
        });
    }
    if op1 == 0 && op2 == 1 {
        return Some(Op::Ldrex {
            rt,
            rn,
            imm: ((h2 & 0xFF) << 2) as u16,
            size: 4,
        });
    }
    if (op1 & 2 == 0 && op2 & 2 == 2) || op1 & 2 == 2 {
        let index = bit(h1, 8);
        let up = bit(h1, 7);
        let wback = bit(h1, 5);
        let imm = ((h2 & 0xFF) << 2) as i32;
        let off = if up { imm } else { -imm };
        return Some(Op::Dual {
            load: bit(h1, 4),
            rt,
            rt2: rd,
            rn,
            off,
            index,
            wback,
        });
    }
    if op1 == 1 && op2 == 0 {
        return match op3 {
            0b0100 => Some(Op::Strex {
                rd: rd_low,
                rt,
                rn,
                imm: 0,
                size: 1,
            }),
            0b0101 => Some(Op::Strex {
                rd: rd_low,
                rt,
                rn,
                imm: 0,
                size: 2,
            }),
            _ => None,
        };
    }
    if op1 == 1 && op2 == 1 {
        return match op3 {
            0b0000 => Some(Op::Tb {
                half: false,
                rn,
                rm: rd_low,
            }),
            0b0001 => Some(Op::Tb {
                half: true,
                rn,
                rm: rd_low,
            }),
            0b0100 => Some(Op::Ldrex {
                rt,
                rn,
                imm: 0,
                size: 1,
            }),
            0b0101 => Some(Op::Ldrex {
                rt,
                rn,
                imm: 0,
                size: 2,
            }),
            _ => None,
        };
    }
    None
}

fn dp_op(op: u32, rd: u8, rn: u8, s: bool) -> Option<(DpOp, bool)> {
    // Returns the operation and whether it is a compare/test form (no Rd write).
    Some(match op {
        0b0000 if rd == 15 && s => (DpOp::Tst, true),
        0b0000 => (DpOp::And, false),
        0b0001 => (DpOp::Bic, false),
        0b0010 if rn == 15 => (DpOp::Mov, false),
        0b0010 => (DpOp::Orr, false),
        0b0011 if rn == 15 => (DpOp::Mvn, false),
        0b0011 => (DpOp::Orn, false),
        0b0100 if rd == 15 && s => (DpOp::Teq, true),
        0b0100 => (DpOp::Eor, false),
        0b1000 if rd == 15 && s => (DpOp::Cmn, true),
        0b1000 => (DpOp::Add, false),
        0b1010 => (DpOp::Adc, false),
        0b1011 => (DpOp::Sbc, false),
        0b1101 if rd == 15 && s => (DpOp::Cmp, true),
        0b1101 => (DpOp::Sub, false),
        0b1110 => (DpOp::Rsb, false),
        _ => return None,
    })
}

/// A5.3.11 data processing (shifted register).
fn decode_dp_shifted(h1: u32, h2: u32) -> Option<Op> {
    let s = bit(h1, 4);
    let rn = bits(h1, 3, 0) as u8;
    let rd = bits(h2, 11, 8) as u8;
    let rm = bits(h2, 3, 0) as u8;
    let sh = decode_imm_shift(bits(h2, 5, 4), bits(h2, 14, 12) << 2 | bits(h2, 7, 6));
    let (op, _) = dp_op(bits(h1, 8, 5), rd, rn, s)?;
    let s = if s { SetFlags::Yes } else { SetFlags::No };
    Some(Op::DpReg { op, s, rd, rn, rm, sh })
}

/// A5.3.1 data processing (modified immediate).
fn decode_dp_modified_imm(h1: u32, h2: u32) -> Option<Op> {
    let s = bit(h1, 4);
    let rn = bits(h1, 3, 0) as u8;
    let rd = bits(h2, 11, 8) as u8;
    let imm12 = bits(h1, 10, 10) << 11 | bits(h2, 14, 12) << 8 | (h2 & 0xFF);
    let (imm, carry) = thumb_expand_imm(imm12)?;
    let (op, _) = dp_op(bits(h1, 8, 5), rd, rn, s)?;
    let s = if s { SetFlags::Yes } else { SetFlags::No };
    Some(Op::DpImm {
        op,
        s,
        rd,
        rn,
        imm,
        carry,
    })
}

/// A5.3.3 data processing (plain binary immediate).
fn decode_dp_plain_imm(h1: u32, h2: u32) -> Option<Op> {
    let rn = bits(h1, 3, 0) as u8;
    let rd = bits(h2, 11, 8) as u8;
    let imm12 = bits(h1, 10, 10) << 11 | bits(h2, 14, 12) << 8 | (h2 & 0xFF);
    let lsb = (bits(h2, 14, 12) << 2 | bits(h2, 7, 6)) as u8;
    let w5 = bits(h2, 4, 0) as u8;
    Some(match bits(h1, 8, 4) {
        0b00000 if rn == 15 => Op::Adr { rd, off: imm12 as i32 },
        0b00000 => Op::DpImm {
            op: DpOp::Add,
            s: SetFlags::No,
            rd,
            rn,
            imm: imm12,
            carry: ImmCarry::Unchanged,
        },
        0b00100 => Op::Movw {
            rd,
            imm: (bits(h1, 3, 0) << 12 | imm12) as u16,
        },
        0b01010 if rn == 15 => Op::Adr {
            rd,
            off: -(imm12 as i32),
        },
        0b01010 => Op::DpImm {
            op: DpOp::Sub,
            s: SetFlags::No,
            rd,
            rn,
            imm: imm12,
            carry: ImmCarry::Unchanged,
        },
        0b01100 => Op::Movt {
            rd,
            imm: (bits(h1, 3, 0) << 12 | imm12) as u16,
        },
        0b10100 => Op::Bfx {
            signed: true,
            rd,
            rn,
            lsb,
            width: w5 + 1,
        },
        0b10110 => Op::Bfi { rd, rn, lsb, msb: w5 },
        0b11100 => Op::Bfx {
            signed: false,
            rd,
            rn,
            lsb,
            width: w5 + 1,
        },
        _ => return None, // SSAT/USAT and the 16-bit variants: M1.
    })
}

/// A5.3.4 branches and miscellaneous control.
fn decode_branch_misc(h1: u32, h2: u32) -> Option<Op> {
    let op = bits(h1, 10, 4);
    let op1 = bits(h2, 14, 12);
    let s = bits(h1, 10, 10);
    let (j1, j2) = (bits(h2, 13, 13), bits(h2, 11, 11));
    if op1 & 0b101 == 0b000 {
        if op & 0b0111000 != 0b0111000 {
            let cond = bits(h1, 9, 6) as u8;
            let imm = s << 20 | j2 << 19 | j1 << 18 | bits(h1, 5, 0) << 12 | (h2 & 0x7FF) << 1;
            return Some(Op::B {
                cond,
                off: sign_extend(imm, 21),
            });
        }
        return match op {
            0b0111000 | 0b0111001 => Some(Op::Msr {
                rn: bits(h1, 3, 0) as u8,
                sysm: (h2 & 0xFF) as u8,
                mask: bits(h2, 11, 10) as u8,
            }),
            0b0111010 => Some(match h2 & 0xFF {
                0 => Op::Hint { kind: HintKind::Nop },
                1 => Op::Hint { kind: HintKind::Yield },
                2 => Op::Hint { kind: HintKind::Wfe },
                3 => Op::Hint { kind: HintKind::Wfi },
                4 => Op::Hint { kind: HintKind::Sev },
                _ => Op::Hint { kind: HintKind::Nop },
            }),
            0b0111011 => match bits(h2, 7, 4) {
                0b0010 => Some(Op::Clrex),
                0b0100..=0b0110 => Some(Op::Hint {
                    kind: HintKind::Barrier,
                }),
                _ => None,
            },
            0b0111110 | 0b0111111 => Some(Op::Mrs {
                rd: bits(h2, 11, 8) as u8,
                sysm: (h2 & 0xFF) as u8,
            }),
            _ => None,
        };
    }
    if op1 == 0b010 && op == 0b1111111 {
        return Some(Op::Udf { imm: h2 as u8 });
    }
    let i1 = !(j1 ^ s) & 1;
    let i2 = !(j2 ^ s) & 1;
    let imm = s << 24 | i1 << 23 | i2 << 22 | bits(h1, 9, 0) << 12 | (h2 & 0x7FF) << 1;
    let off = sign_extend(imm, 25);
    match op1 & 0b101 {
        0b001 => Some(Op::B { cond: 14, off }),
        0b101 => Some(Op::Bl { off }),
        _ => None, // BLX immediate does not exist in ARMv7-M.
    }
}

/// A5.3.10 store single data item.
fn decode_store_single(h1: u32, h2: u32) -> Option<Op> {
    let kind = match bits(h1, 6, 5) {
        0 => MemKind::Strb,
        1 => MemKind::Strh,
        2 => MemKind::Str,
        _ => return None,
    };
    let rn = bits(h1, 3, 0) as u8;
    let rt = bits(h2, 15, 12) as u8;
    if rn == 15 {
        return None;
    }
    let mode = ldst_mode(h1, h2)?;
    Some(Op::Mem { kind, rt, rn, mode })
}

/// Addressing mode shared by the 32-bit load/store single encodings (A5.3.7–A5.3.10).
fn ldst_mode(h1: u32, h2: u32) -> Option<AddrMode> {
    if bit(h1, 7) {
        return Some(AddrMode::Imm {
            off: (h2 & 0xFFF) as i32,
            index: true,
            wback: false,
        });
    }
    if bit(h2, 11) {
        let (p, u, w) = (bit(h2, 10), bit(h2, 9), bit(h2, 8));
        let imm = (h2 & 0xFF) as i32;
        // P=1,U=1,W=0 is the unprivileged (LDRT/STRT) form: same access in this model.
        return Some(AddrMode::Imm {
            off: if u { imm } else { -imm },
            index: p,
            wback: w,
        });
    }
    if bits(h2, 10, 6) == 0 {
        return Some(AddrMode::Reg {
            rm: bits(h2, 3, 0) as u8,
            shift: bits(h2, 5, 4) as u8,
        });
    }
    None
}

/// A5.3.7–A5.3.9 loads (word, halfword, byte; including literal forms).
fn decode_load(h1: u32, h2: u32, unsigned: MemKind, signed: MemKind) -> Option<Op> {
    let rn = bits(h1, 3, 0) as u8;
    let rt = bits(h2, 15, 12) as u8;
    let kind = if bit(h1, 8) { signed } else { unsigned };
    if rt == 15 && kind != MemKind::Ldr {
        // PLD/PLI and unallocated memory hints: no architectural effect.
        return Some(Op::Hint { kind: HintKind::Nop });
    }
    let mode = if rn == 15 {
        let imm = (h2 & 0xFFF) as i32;
        AddrMode::Literal {
            off: if bit(h1, 7) { imm } else { -imm },
        }
    } else {
        ldst_mode(h1, h2)?
    };
    Some(Op::Mem { kind, rt, rn, mode })
}

/// A5.3.12 data processing (register).
fn decode_dp_register(h1: u32, h2: u32) -> Option<Op> {
    let op1 = bits(h1, 7, 4);
    let op2 = bits(h2, 7, 4);
    let rn = bits(h1, 3, 0) as u8;
    let rd = bits(h2, 11, 8) as u8;
    let rm = bits(h2, 3, 0) as u8;
    if op1 & 0b1110 == 0 && op2 == 0 || op1 & 0b1110 != 0 && op1 < 0b1000 && op2 == 0 {
        let ty = match bits(h1, 6, 5) {
            0 => ShiftType::Lsl,
            1 => ShiftType::Lsr,
            2 => ShiftType::Asr,
            _ => ShiftType::Ror,
        };
        let s = if bit(h1, 4) { SetFlags::Yes } else { SetFlags::No };
        return Some(Op::ShiftReg { ty, s, rd, rn, rm });
    }
    if op1 < 0b1000 && op2 & 0b1000 != 0 {
        let rot = (bits(h2, 5, 4) * 8) as u8;
        let kind = match op1 {
            0b0000 => ExtendKind::Sxth,
            0b0001 => ExtendKind::Uxth,
            0b0100 => ExtendKind::Sxtb,
            0b0101 => ExtendKind::Uxtb,
            _ => return None, // SXTB16/UXTB16: M1.
        };
        return Some(Op::Extend { kind, rd, rn, rm, rot });
    }
    if op1 & 0b1100 == 0b1000 && op2 & 0b1100 == 0b1000 {
        let kind = match (bits(h1, 5, 4), bits(h2, 5, 4)) {
            (0b01, 0b00) => RevKind::Rev,
            (0b01, 0b01) => RevKind::Rev16,
            (0b01, 0b10) => RevKind::Rbit,
            (0b01, 0b11) => RevKind::Revsh,
            (0b11, 0b00) => RevKind::Clz,
            _ => return None, // QADD family, SEL: M1.
        };
        return Some(Op::Rev { kind, rd, rm });
    }
    None // Parallel add/subtract (DSP): M1.
}

/// A5.3.16 multiply, multiply accumulate, absolute difference.
fn decode_multiply(h1: u32, h2: u32) -> Option<Op> {
    let rn = bits(h1, 3, 0) as u8;
    let ra = bits(h2, 15, 12) as u8;
    let rd = bits(h2, 11, 8) as u8;
    let rm = bits(h2, 3, 0) as u8;
    match (bits(h1, 6, 4), bits(h2, 5, 4)) {
        (0, 0) if ra == 15 => Some(Op::Mul {
            s: SetFlags::No,
            rd,
            rn,
            rm,
        }),
        (0, 0) => Some(Op::Mla {
            sub: false,
            rd,
            rn,
            rm,
            ra,
        }),
        (0, 1) => Some(Op::Mla {
            sub: true,
            rd,
            rn,
            rm,
            ra,
        }),
        _ => None, // Halfword/dual DSP multiplies: M1.
    }
}

/// A5.3.17 long multiply, long multiply accumulate, divide.
fn decode_long_multiply(h1: u32, h2: u32) -> Option<Op> {
    let rn = bits(h1, 3, 0) as u8;
    let rdlo = bits(h2, 15, 12) as u8;
    let rdhi = bits(h2, 11, 8) as u8;
    let rm = bits(h2, 3, 0) as u8;
    let long = |kind| {
        Some(Op::LongMul {
            kind,
            rdlo,
            rdhi,
            rn,
            rm,
        })
    };
    match (bits(h1, 6, 4), bits(h2, 7, 4)) {
        (0b000, 0b0000) => long(LongMulKind::Smull),
        (0b001, 0b1111) => Some(Op::Div {
            signed: true,
            rd: rdhi,
            rn,
            rm,
        }),
        (0b010, 0b0000) => long(LongMulKind::Umull),
        (0b011, 0b1111) => Some(Op::Div {
            signed: false,
            rd: rdhi,
            rn,
            rm,
        }),
        (0b100, 0b0000) => long(LongMulKind::Smlal),
        (0b110, 0b0000) => long(LongMulKind::Umlal),
        (0b110, 0b0110) => long(LongMulKind::Umaal),
        _ => None, // SMLALxy/SMLALD/SMLSLD: M1.
    }
}

/// A6 FP instructions (coprocessors 10 and 11). Other coprocessors are UNDEFINED on the M4.
fn decode_coproc(h1: u32, h2: u32) -> Option<Op> {
    let coproc = bits(h2, 11, 8);
    if coproc & 0b1110 != 0b1010 {
        return None;
    }
    let double = coproc == 0b1011;
    // Extension register load/store (A6.5).
    // 1110 110x xxxx: VLDR/VSTR/VLDM/VSTM and 64-bit transfers.
    if h1 & 0xFE00 == 0xEC00 {
        let (p, u, w, l) = (bit(h1, 8), bit(h1, 7), bit(h1, 5), bit(h1, 4));
        let rn = bits(h1, 3, 0) as u8;
        let vd = bits(h2, 15, 12);
        let dbit = bits(h1, 6, 6);
        let imm8 = h2 & 0xFF;
        if h1 & 0xFFE0 == 0xEC40 {
            // 64-bit transfers between core and extension registers (A6.7).
            let rt = bits(h2, 15, 12) as u8;
            let rt2 = bits(h1, 3, 0) as u8;
            let vm = bits(h2, 3, 0);
            let mbit = bits(h2, 5, 5);
            let sm = if double { (mbit << 4 | vm) * 2 } else { vm << 1 | mbit };
            return Some(Op::VMov2 {
                to_core: l,
                rt,
                rt2,
                sm: sm as u8,
            });
        }
        if !p && !u && !w {
            return None;
        }
        let d = if double { (dbit << 4 | vd) * 2 } else { vd << 1 | dbit } as u8;
        if p && !w {
            let off = (imm8 << 2) as i32;
            return Some(Op::VLdSt {
                load: l,
                double,
                d,
                rn,
                off: if u { off } else { -off },
            });
        }
        if p == u {
            return None;
        }
        // For doubles imm8 counts words too (FLDMX odd values are not used on the M4).
        let count = (imm8 & !1 | if double { 0 } else { imm8 & 1 }) as u8;
        return Some(Op::VLdStM {
            load: l,
            d,
            count,
            rn,
            wback: w,
            db: p,
        });
    }
    if h1 & 0xFF00 != 0xEE00 || double {
        return None;
    }
    let vd = bits(h2, 15, 12);
    let dbit = bits(h1, 6, 6);
    let vn = bits(h1, 3, 0);
    let nbit = bits(h2, 7, 7);
    let vm = bits(h2, 3, 0);
    let mbit = bits(h2, 5, 5);
    let sd = (vd << 1 | dbit) as u8;
    let sn = (vn << 1 | nbit) as u8;
    let sm = (vm << 1 | mbit) as u8;
    if bit(h2, 4) {
        // 8/16/32-bit transfers between core and extension registers (A6.6).
        let rt = bits(h2, 15, 12) as u8;
        return match bits(h1, 7, 5) {
            0b000 if bits(h2, 6, 5) == 0 => Some(Op::VMovCore {
                to_core: bit(h1, 4),
                rt,
                sn,
            }),
            0b111 if bits(h1, 4, 0) == 0b10001 && h2 & 0xFFF == 0xA10 => Some(Op::Vmrs { rt }),
            0b111 if bits(h1, 4, 0) == 0b00001 && h2 & 0xFFF == 0xA10 => Some(Op::Vmsr { rt }),
            _ => None,
        };
    }
    // FP data processing (A6.4).
    let opc1 = bits(h1, 7, 4) & 0b1011;
    let opb = bit(h2, 6);
    let three = |op| {
        Some(Op::VDp {
            op,
            d: sd,
            n: sn,
            m: sm,
        })
    };
    match opc1 {
        0b0000 => three(if opb { VOp::Mls } else { VOp::Mla }),
        0b0001 => three(if opb { VOp::Nmla } else { VOp::Nmls }),
        0b0010 => three(if opb { VOp::Nmul } else { VOp::Mul }),
        0b0011 => three(if opb { VOp::Sub } else { VOp::Add }),
        0b1000 if !opb => three(VOp::Div),
        0b1001 => three(if opb { VOp::Fnma } else { VOp::Fnms }),
        0b1010 => three(if opb { VOp::Fms } else { VOp::Fma }),
        0b1011 => {
            let opc2 = bits(h1, 3, 0);
            let opc3 = bits(h2, 7, 6);
            if opc3 & 1 == 0 {
                // VFPExpandImm (A6.4.1) for single precision.
                let imm8 = bits(h1, 3, 0) << 4 | bits(h2, 3, 0);
                let b6 = bits(imm8, 6, 6);
                let exp = (b6 ^ 1) << 7 | if b6 != 0 { 0b11111 << 2 } else { 0 } | bits(imm8, 5, 4);
                let bits32 = bits(imm8, 7, 7) << 31 | exp << 23 | bits(imm8, 3, 0) << 19;
                return Some(Op::VMovImm { d: sd, bits: bits32 });
            }
            match (opc2, opc3) {
                (0b0000, 0b01) => Some(Op::VUn {
                    op: VUnOp::Mov,
                    d: sd,
                    m: sm,
                }),
                (0b0000, 0b11) => Some(Op::VUn {
                    op: VUnOp::Abs,
                    d: sd,
                    m: sm,
                }),
                (0b0001, 0b01) => Some(Op::VUn {
                    op: VUnOp::Neg,
                    d: sd,
                    m: sm,
                }),
                (0b0001, 0b11) => Some(Op::VUn {
                    op: VUnOp::Sqrt,
                    d: sd,
                    m: sm,
                }),
                (0b0100, _) => Some(Op::VCmp {
                    e: bit(h2, 7),
                    d: sd,
                    m: Some(sm),
                }),
                (0b0101, _) => Some(Op::VCmp {
                    e: bit(h2, 7),
                    d: sd,
                    m: None,
                }),
                (0b1000, _) => {
                    let kind = if bit(h2, 7) {
                        VCvtKind::FromS32
                    } else {
                        VCvtKind::FromU32
                    };
                    Some(Op::VCvt { kind, d: sd, m: sm })
                }
                (0b1100, _) => Some(Op::VCvt {
                    kind: VCvtKind::ToU32 { rz: bit(h2, 7) },
                    d: sd,
                    m: sm,
                }),
                (0b1101, _) => Some(Op::VCvt {
                    kind: VCvtKind::ToS32 { rz: bit(h2, 7) },
                    d: sd,
                    m: sm,
                }),
                _ => None, // Half-precision and fixed-point conversions: M1.
            }
        }
        _ => None,
    }
}

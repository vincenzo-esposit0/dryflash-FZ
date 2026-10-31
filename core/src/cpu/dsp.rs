// SPDX-License-Identifier: GPL-3.0-or-later
//! DSP extension instructions (ARMv7E-M). Semantics from the per-instruction pseudocode
//! in DDI 0403E A7.7 (SignedSatQ/UnsignedSatQ in A2.2.1).

use super::Cpu;
use super::op::*;

/// SignedSatQ(i, n): returns the saturated value and whether saturation occurred.
#[inline]
pub(crate) fn signed_sat(i: i64, n: u32) -> (i32, bool) {
    let max = (1i64 << (n - 1)) - 1;
    let min = -(1i64 << (n - 1));
    if i > max {
        (max as i32, true)
    } else if i < min {
        (min as i32, true)
    } else {
        (i as i32, false)
    }
}

/// UnsignedSatQ(i, n).
#[inline]
pub(crate) fn unsigned_sat(i: i64, n: u32) -> (u32, bool) {
    let max = if n == 32 { u32::MAX as i64 } else { (1i64 << n) - 1 };
    if i > max {
        (max as u32, true)
    } else if i < 0 {
        (0, true)
    } else {
        (i as u32, false)
    }
}

#[inline]
fn half(v: u32, top: bool) -> i32 {
    (if top { v >> 16 } else { v }) as u16 as i16 as i32
}

#[inline]
fn lanes16(v: u32) -> [i64; 2] {
    [v as u16 as i64, (v >> 16) as u16 as i64]
}

#[inline]
fn lanes16s(v: u32) -> [i64; 2] {
    [v as u16 as i16 as i64, (v >> 16) as u16 as i16 as i64]
}

#[inline]
fn lanes8(v: u32, signed: bool) -> [i64; 4] {
    let mut out = [0; 4];
    for (i, o) in out.iter_mut().enumerate() {
        let b = (v >> (8 * i)) as u8;
        *o = if signed { b as i8 as i64 } else { b as i64 };
    }
    out
}

impl Cpu {
    pub(crate) fn exec_dsp(&mut self, op: Op) {
        let r = |cpu: &Cpu, n: u8| cpu.r[n as usize];
        match op {
            Op::Sat {
                unsigned,
                rd,
                rn,
                sat,
                sh,
            } => {
                // The shifted operand is computed without carry (only LSL/ASR are encodable).
                let v = r(self, rn);
                let operand = match sh.ty {
                    ShiftType::Asr => ((v as i32) >> sh.amt.min(31)) as i64,
                    _ => ((v << sh.amt) as i32) as i64,
                };
                let (res, sat_hit) = if unsigned {
                    unsigned_sat(operand, sat as u32)
                } else {
                    let (x, s) = signed_sat(operand, sat as u32);
                    (x as u32, s)
                };
                self.r[rd as usize] = res;
                self.q |= sat_hit;
            }
            Op::Sat16 { unsigned, rd, rn, sat } => {
                let v = r(self, rn);
                let mut out = 0u32;
                for (i, lane) in lanes16s(v).into_iter().enumerate() {
                    let (x, s) = if unsigned {
                        unsigned_sat(lane, sat as u32)
                    } else {
                        let (x, s) = signed_sat(lane, sat as u32);
                        (x as u32, s)
                    };
                    out |= (x & 0xFFFF) << (16 * i);
                    self.q |= s;
                }
                self.r[rd as usize] = out;
            }
            Op::Qarith { op, rd, rn, rm } => {
                let (m, n) = (r(self, rm) as i32 as i64, r(self, rn) as i32 as i64);
                let mut q = false;
                let operand = match op {
                    QOp::Qdadd | QOp::Qdsub => {
                        let (d, s) = signed_sat(2 * n, 32);
                        q |= s;
                        d as i64
                    }
                    _ => n,
                };
                let sum = match op {
                    QOp::Qadd | QOp::Qdadd => m + operand,
                    QOp::Qsub | QOp::Qdsub => m - operand,
                };
                let (res, s) = signed_sat(sum, 32);
                self.r[rd as usize] = res as u32;
                self.q |= q || s;
            }
            Op::Sel { rd, rn, rm } => {
                let (n, m) = (r(self, rn), r(self, rm));
                let mut out = 0;
                for i in 0..4 {
                    let src = if self.ge & (1 << i) != 0 { n } else { m };
                    out |= src & (0xFF << (8 * i));
                }
                self.r[rd as usize] = out;
            }
            Op::Parallel { prefix, op, rd, rn, rm } => {
                let (n, m) = (r(self, rn), r(self, rm));
                self.r[rd as usize] = self.parallel(prefix, op, n, m);
            }
            Op::SmulXY {
                n_top,
                m_top,
                rd,
                rn,
                rm,
                ra,
            } => {
                let p = half(r(self, rn), n_top) as i64 * half(r(self, rm), m_top) as i64;
                if ra == 15 {
                    self.r[rd as usize] = p as u32;
                } else {
                    let sum = p + r(self, ra) as i32 as i64;
                    self.r[rd as usize] = sum as u32;
                    if sum != sum as i32 as i64 {
                        self.q = true;
                    }
                }
            }
            Op::SmulWY { m_top, rd, rn, rm, ra } => {
                let p = (r(self, rn) as i32 as i64 * half(r(self, rm), m_top) as i64) >> 16;
                if ra == 15 {
                    self.r[rd as usize] = p as u32;
                } else {
                    let sum = p + r(self, ra) as i32 as i64;
                    self.r[rd as usize] = sum as u32;
                    if sum != sum as i32 as i64 {
                        self.q = true;
                    }
                }
            }
            Op::SmlaD { sub, x, rd, rn, rm, ra } => {
                let n = r(self, rn);
                let m = if x { r(self, rm).rotate_right(16) } else { r(self, rm) };
                let p1 = half(n, false) as i64 * half(m, false) as i64;
                let p2 = half(n, true) as i64 * half(m, true) as i64;
                let acc = if ra == 15 { 0 } else { r(self, ra) as i32 as i64 };
                let res = if sub { p1 - p2 + acc } else { p1 + p2 + acc };
                self.r[rd as usize] = res as u32;
                if res != res as i32 as i64 {
                    self.q = true;
                }
            }
            Op::Smmla {
                sub,
                round,
                rd,
                rn,
                rm,
                ra,
            } => {
                let p = r(self, rn) as i32 as i64 * r(self, rm) as i32 as i64;
                let acc = if ra == 15 { 0 } else { (r(self, ra) as i32 as i64) << 32 };
                let mut res = if sub { acc - p } else { acc + p };
                if round {
                    res = res.wrapping_add(0x8000_0000);
                }
                self.r[rd as usize] = (res >> 32) as u32;
            }
            Op::Usad8 { rd, rn, rm, ra } => {
                let (n, m) = (lanes8(r(self, rn), false), lanes8(r(self, rm), false));
                let sum: i64 = n.iter().zip(m.iter()).map(|(a, b)| (a - b).abs()).sum();
                let acc = if ra == 15 { 0 } else { r(self, ra) };
                self.r[rd as usize] = acc.wrapping_add(sum as u32);
            }
            Op::SmlalXY {
                n_top,
                m_top,
                rdlo,
                rdhi,
                rn,
                rm,
            } => {
                let p = half(r(self, rn), n_top) as i64 * half(r(self, rm), m_top) as i64;
                let acc = ((r(self, rdhi) as u64) << 32 | r(self, rdlo) as u64) as i64;
                let res = acc.wrapping_add(p);
                self.r[rdlo as usize] = res as u32;
                self.r[rdhi as usize] = (res >> 32) as u32;
            }
            Op::SmlalD {
                sub,
                x,
                rdlo,
                rdhi,
                rn,
                rm,
            } => {
                let n = r(self, rn);
                let m = if x { r(self, rm).rotate_right(16) } else { r(self, rm) };
                let p1 = half(n, false) as i64 * half(m, false) as i64;
                let p2 = half(n, true) as i64 * half(m, true) as i64;
                let acc = ((r(self, rdhi) as u64) << 32 | r(self, rdlo) as u64) as i64;
                let res = acc.wrapping_add(if sub { p1 - p2 } else { p1 + p2 });
                self.r[rdlo as usize] = res as u32;
                self.r[rdhi as usize] = (res >> 32) as u32;
            }
            Op::Extend16 {
                signed,
                rd,
                rn,
                rm,
                rot,
            } => {
                let v = r(self, rm).rotate_right(rot as u32);
                let ext = |b: u32| if signed { b as u8 as i8 as i32 as u32 } else { b & 0xFF };
                let lo = ext(v);
                let hi = ext(v >> 16);
                let (add_lo, add_hi) = if rn == 15 {
                    (0, 0)
                } else {
                    (r(self, rn), r(self, rn) >> 16)
                };
                let lo = add_lo.wrapping_add(lo) & 0xFFFF;
                let hi = add_hi.wrapping_add(hi) & 0xFFFF;
                self.r[rd as usize] = hi << 16 | lo;
            }
            Op::Pkh { tb, rd, rn, rm, sh } => {
                let (n, m) = (r(self, rn), r(self, rm));
                self.r[rd as usize] = if tb {
                    let shifted = ((m as i32) >> sh.amt.min(31)) as u32;
                    (n & 0xFFFF_0000) | (shifted & 0xFFFF)
                } else {
                    let shifted = m << sh.amt;
                    (shifted & 0xFFFF_0000) | (n & 0xFFFF)
                };
            }
            _ => unreachable!("exec_dsp called with {op:?}"),
        }
    }

    /// Parallel add/subtract (A7.7: SADD16 … UHSUB8). GE flags are set by the plain
    /// signed/unsigned forms only.
    fn parallel(&mut self, prefix: ParPrefix, op: ParOp, n: u32, m: u32) -> u32 {
        let signed = matches!(prefix, ParPrefix::S | ParPrefix::Q | ParPrefix::Sh);
        let bytes = matches!(op, ParOp::Add8 | ParOp::Sub8);
        let (lane_bits, lanes): (u32, Vec<(i64, i64)>) = if bytes {
            let (a, b) = (lanes8(n, signed), lanes8(m, signed));
            (8, a.into_iter().zip(b).collect())
        } else {
            let (a, b) = if signed {
                (lanes16s(n), lanes16s(m))
            } else {
                (lanes16(n), lanes16(m))
            };
            (16, a.into_iter().zip(b).collect())
        };
        // Per-lane results before saturation/halving.
        let results: Vec<i64> = match op {
            ParOp::Add16 | ParOp::Add8 => lanes.iter().map(|(a, b)| a + b).collect(),
            ParOp::Sub16 | ParOp::Sub8 => lanes.iter().map(|(a, b)| a - b).collect(),
            // ASX: low = n.lo − m.hi, high = n.hi + m.lo. SAX: low = n.lo + m.hi, high = n.hi − m.lo.
            ParOp::Asx => vec![lanes[0].0 - lanes[1].1, lanes[1].0 + lanes[0].1],
            ParOp::Sax => vec![lanes[0].0 + lanes[1].1, lanes[1].0 - lanes[0].1],
        };
        let is_add: Vec<bool> = match op {
            ParOp::Add16 | ParOp::Add8 => vec![true; results.len()],
            ParOp::Sub16 | ParOp::Sub8 => vec![false; results.len()],
            ParOp::Asx => vec![false, true],
            ParOp::Sax => vec![true, false],
        };
        let mut out = 0u32;
        let mut ge = 0u8;
        let ge_per_lane = if bytes { 1 } else { 2 };
        for (i, &x) in results.iter().enumerate() {
            let v: u32 = match prefix {
                ParPrefix::S | ParPrefix::U => {
                    let set = if signed {
                        x >= 0
                    } else if is_add[i] {
                        x >= 1 << lane_bits
                    } else {
                        x >= 0
                    };
                    if set {
                        ge |= ((1 << ge_per_lane) - 1) << (i * ge_per_lane);
                    }
                    x as u32
                }
                ParPrefix::Q => signed_sat(x, lane_bits).0 as u32,
                ParPrefix::Uq => unsigned_sat(x, lane_bits).0,
                ParPrefix::Sh | ParPrefix::Uh => (x >> 1) as u32,
            };
            let mask = (1u32 << lane_bits) - 1;
            out |= (v & mask) << (i as u32 * lane_bits);
        }
        if matches!(prefix, ParPrefix::S | ParPrefix::U) {
            self.ge = ge;
        }
        out
    }
}

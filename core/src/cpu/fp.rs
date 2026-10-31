// SPDX-License-Identifier: GPL-3.0-or-later
//! FPv4-SP arithmetic with ARM semantics (DDI 0403E A2.5): NaN processing (FPProcessNaNs),
//! default NaN (FPSCR.DN), flush-to-zero (FPSCR.FZ) and the cumulative exception flags.
//!
//! The host only ever computes operations whose inputs are not NaN. Results that are NaN
//! are replaced by the ARM default NaN, so results do not depend on the host's NaN
//! propagation — which WebAssembly leaves nondeterministic. Rounding is round-to-nearest-even;
//! FPSCR.RMode is honoured by VCVT/VCVTR but not by arithmetic (see BRINGUP_LOG, known gaps).

pub const DEFAULT_NAN: u32 = 0x7FC0_0000;

// FPSCR cumulative exception bits (A2.5.3).
pub const IOC: u32 = 1 << 0;
pub const DZC: u32 = 1 << 1;
pub const OFC: u32 = 1 << 2;
pub const UFC: u32 = 1 << 3;
pub const IXC: u32 = 1 << 4;
pub const IDC: u32 = 1 << 7;
const FZ: u32 = 1 << 24;
const DN: u32 = 1 << 25;

#[inline]
fn is_nan(x: u32) -> bool {
    x & 0x7F80_0000 == 0x7F80_0000 && x & 0x007F_FFFF != 0
}

#[inline]
fn is_snan(x: u32) -> bool {
    is_nan(x) && x & 0x0040_0000 == 0
}

#[inline]
fn is_denormal(x: u32) -> bool {
    x & 0x7F80_0000 == 0 && x & 0x007F_FFFF != 0
}

#[inline]
fn is_inf(x: u32) -> bool {
    x & 0x7FFF_FFFF == 0x7F80_0000
}

#[inline]
fn is_zero(x: u32) -> bool {
    x & 0x7FFF_FFFF == 0
}

/// Whether `r` equals x + y exactly (Knuth's TwoSum: s + err is the exact sum).
fn exact_sum(x: f64, y: f64, r: f32) -> bool {
    let s = x + y;
    let bb = s - x;
    let err = (x - (s - bb)) + (y - bb);
    r as f64 == s && err == 0.0
}

/// FPNeg: flips the sign bit, NaNs included.
#[inline]
pub fn neg(x: u32) -> u32 {
    x ^ 0x8000_0000
}

/// Floating-point context for one instruction: FPSCR controls in, flags out.
pub struct Fp<'a> {
    pub fpscr: &'a mut u32,
}

impl Fp<'_> {
    fn flag(&mut self, f: u32) {
        *self.fpscr |= f;
    }

    /// FPUnpack's flush of denormal inputs when FPSCR.FZ is set.
    fn input(&mut self, x: u32) -> u32 {
        if *self.fpscr & FZ != 0 && is_denormal(x) {
            self.flag(IDC);
            x & 0x8000_0000
        } else {
            x
        }
    }

    fn quiet(&self, x: u32) -> u32 {
        if *self.fpscr & DN != 0 {
            DEFAULT_NAN
        } else {
            x | 0x0040_0000
        }
    }

    /// FPProcessNaNs: signalling NaNs first (operand order), then quiet NaNs.
    fn process_nans(&mut self, ops: &[u32]) -> Option<u32> {
        if let Some(&s) = ops.iter().find(|&&x| is_snan(x)) {
            self.flag(IOC);
            return Some(self.quiet(s));
        }
        ops.iter().find(|&&x| is_nan(x)).map(|&q| self.quiet(q))
    }

    /// Rounds a result computed from non-NaN inputs: default NaN for invalid operations,
    /// flush-to-zero, and the overflow/underflow/inexact flags. `inexact` tells whether the
    /// rounded result differs from the infinitely precise one; it is only evaluated when
    /// the answer can change FPSCR (the flags are cumulative, so once IXC is set only tiny
    /// results still need it, for UFC).
    #[inline]
    fn finish(&mut self, r: f32, inexact: impl FnOnce() -> bool, inputs_finite: bool) -> u32 {
        if r.is_nan() {
            self.flag(IOC);
            return DEFAULT_NAN;
        }
        let bits = r.to_bits();
        if r.is_infinite() {
            if inputs_finite {
                self.flag(OFC | IXC);
            }
            return bits;
        }
        let tiny_candidate = bits & 0x7F80_0000 == 0;
        if !tiny_candidate && *self.fpscr & IXC != 0 {
            return bits;
        }
        let inexact = inexact();
        // Tiny: subnormal, or rounded to zero from a non-zero exact value.
        if is_denormal(bits) || (is_zero(bits) && inexact) {
            if *self.fpscr & FZ != 0 {
                // Flush-to-zero output: underflow is signalled, inexact is not (A2.5.4).
                self.flag(UFC);
                return bits & 0x8000_0000;
            }
            if inexact {
                self.flag(UFC);
            }
        }
        if inexact {
            self.flag(IXC);
        }
        bits
    }

    pub fn add(&mut self, a: u32, b: u32) -> u32 {
        let (a, b) = (self.input(a), self.input(b));
        if let Some(n) = self.process_nans(&[a, b]) {
            return n;
        }
        let (fa, fb) = (f32::from_bits(a), f32::from_bits(b));
        let r = fa + fb;
        let finite = fa.is_finite() && fb.is_finite();
        self.finish(r, || finite && !exact_sum(fa as f64, fb as f64, r), finite)
    }

    /// FPSub processes NaNs on the original operands; only then is b negated
    /// (unlike VMLS/VNMLA, whose pseudocode applies FPNeg before FPAdd).
    pub fn sub(&mut self, a: u32, b: u32) -> u32 {
        let (a, b) = (self.input(a), self.input(b));
        if let Some(n) = self.process_nans(&[a, b]) {
            return n;
        }
        self.add(a, neg(b))
    }

    pub fn mul(&mut self, a: u32, b: u32) -> u32 {
        let (a, b) = (self.input(a), self.input(b));
        if let Some(n) = self.process_nans(&[a, b]) {
            return n;
        }
        let (fa, fb) = (f32::from_bits(a), f32::from_bits(b));
        let r = fa * fb;
        let finite = fa.is_finite() && fb.is_finite();
        // The product of two f32 values is exact in f64.
        self.finish(r, || finite && r as f64 != fa as f64 * fb as f64, finite)
    }

    pub fn div(&mut self, a: u32, b: u32) -> u32 {
        let (a, b) = (self.input(a), self.input(b));
        if let Some(n) = self.process_nans(&[a, b]) {
            return n;
        }
        if is_zero(b) && !is_zero(a) && !is_inf(a) {
            self.flag(DZC);
            return f32::INFINITY.to_bits() | ((a ^ b) & 0x8000_0000);
        }
        let (fa, fb) = (f32::from_bits(a), f32::from_bits(b));
        let r = fa / fb;
        let finite = fa.is_finite() && fb.is_finite();
        // r is exact iff r * b == a; that product is exact in f64.
        self.finish(
            r,
            || finite && r.is_finite() && r as f64 * fb as f64 != fa as f64,
            finite,
        )
    }

    pub fn sqrt(&mut self, a: u32) -> u32 {
        let a = self.input(a);
        if let Some(n) = self.process_nans(&[a]) {
            return n;
        }
        let fa = f32::from_bits(a);
        let r = fa.sqrt();
        self.finish(r, || r.is_finite() && r as f64 * r as f64 != fa as f64, fa.is_finite())
    }

    /// FPMulAdd(addend, op1, op2): fused multiply-add with one rounding (A2.5.8).
    pub fn mul_add(&mut self, addend: u32, a: u32, b: u32) -> u32 {
        let (c, a, b) = (self.input(addend), self.input(a), self.input(b));
        // A quiet-NaN addend with an Inf*0 product is still an invalid operation.
        let inf_times_zero = (is_inf(a) && is_zero(b)) || (is_zero(a) && is_inf(b));
        if is_nan(c) && !is_snan(c) && inf_times_zero && !is_nan(a) && !is_nan(b) {
            self.flag(IOC);
            return DEFAULT_NAN;
        }
        if let Some(n) = self.process_nans(&[c, a, b]) {
            return n;
        }
        let (fc, fa, fb) = (f32::from_bits(c), f32::from_bits(a), f32::from_bits(b));
        let r = fa.mul_add(fb, fc);
        let finite = fa.is_finite() && fb.is_finite() && fc.is_finite();
        // a*b is exact in f64; TwoSum then gives the exact sum.
        self.finish(r, || finite && !exact_sum(fa as f64 * fb as f64, fc as f64, r), finite)
    }

    /// VCMP/VCMPE: returns NZCV. Signalling NaNs (or any NaN for VCMPE) raise IOC.
    pub fn compare(&mut self, a: u32, b: u32, e: bool) -> u32 {
        let (a, b) = (self.input(a), self.input(b));
        if is_nan(a) || is_nan(b) {
            if e || is_snan(a) || is_snan(b) {
                self.flag(IOC);
            }
            return 0b0011;
        }
        let (fa, fb) = (f32::from_bits(a), f32::from_bits(b));
        if fa == fb {
            0b0110
        } else if fa < fb {
            0b1000
        } else {
            0b0010
        }
    }

    /// FPToFixed for VCVT to 32-bit integer (A2.5.9). `round` is applied first.
    pub fn cvt_to_int(&mut self, a: u32, unsigned: bool, round: impl Fn(f64) -> f64) -> u32 {
        let a = self.input(a);
        if is_nan(a) {
            self.flag(IOC);
            return 0;
        }
        let x = f32::from_bits(a) as f64;
        let r = round(x);
        let (lo, hi) = if unsigned {
            (0.0, u32::MAX as f64)
        } else {
            (i32::MIN as f64, i32::MAX as f64)
        };
        if r < lo || r > hi {
            self.flag(IOC);
            let sat = r.clamp(lo, hi);
            return if unsigned { sat as u32 } else { sat as i32 as u32 };
        }
        if r != x {
            self.flag(IXC);
        }
        if unsigned { r as u32 } else { r as i32 as u32 }
    }

    /// FixedToFP for VCVT from 32-bit integer.
    pub fn cvt_from_int(&mut self, x: u32, unsigned: bool) -> u32 {
        let v = if unsigned { x as f64 } else { x as i32 as f64 };
        let r = v as f32;
        if r as f64 != v {
            self.flag(IXC);
        }
        r.to_bits()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with<R>(fpscr: u32, f: impl FnOnce(&mut Fp) -> R) -> (R, u32) {
        let mut s = fpscr;
        let r = f(&mut Fp { fpscr: &mut s });
        (r, s)
    }

    #[test]
    fn nan_propagation_follows_operand_order_and_sign() {
        let qnan = 0x7FC0_0001;
        // FPNeg applies to NaNs too: VNMLA's -prod keeps the flipped sign.
        let (r, _) = with(0, |fp| fp.add(0x3F80_0000, neg(qnan)));
        assert_eq!(r, 0xFFC0_0001);
        // Signalling NaN wins over an earlier quiet NaN and is quietened.
        let (r, f) = with(0, |fp| fp.add(qnan, 0x7F80_0001));
        assert_eq!((r, f & IOC), (0x7FC0_0001, IOC));
        // Default NaN mode.
        let (r, _) = with(DN, |fp| fp.add(qnan, 1.0f32.to_bits()));
        assert_eq!(r, DEFAULT_NAN);
    }

    #[test]
    fn fpsub_keeps_nan_sign() {
        // VSUB s0, s1, NaN returns the NaN operand unchanged (FPSub, A2.5.6).
        let (r, _) = with(0, |fp| fp.sub(1.0f32.to_bits(), 0x7FC0_0000));
        assert_eq!(r, 0x7FC0_0000);
    }

    #[test]
    fn invalid_operations_give_default_nan() {
        let inf = f32::INFINITY.to_bits();
        let (r, f) = with(0, |fp| fp.add(inf, neg(inf)));
        assert_eq!((r, f), (DEFAULT_NAN, IOC));
        let (r, f) = with(0, |fp| fp.mul(inf, 0));
        assert_eq!((r, f), (DEFAULT_NAN, IOC));
    }

    #[test]
    fn exception_flags() {
        let (_, f) = with(0, |fp| fp.div(1.0f32.to_bits(), 3.0f32.to_bits()));
        assert_eq!(f, IXC);
        let (_, f) = with(0, |fp| fp.div(1.0f32.to_bits(), 0));
        assert_eq!(f, DZC);
        let (_, f) = with(0, |fp| fp.mul(f32::MAX.to_bits(), 2.0f32.to_bits()));
        assert_eq!(f, OFC | IXC);
        let (r, f) = with(FZ, |fp| fp.mul(1e-30f32.to_bits(), 1e-10f32.to_bits()));
        assert_eq!((r, f & 0x9F), (0, UFC));
    }
}

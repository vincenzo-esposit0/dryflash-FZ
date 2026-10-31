// SPDX-License-Identifier: GPL-3.0-or-later
//! Bare-metal Thumb-2 workload for the ADR-0001 CPU engine spike.
//!
//! Runs a mix of integer, 64-bit, memory-heavy and FPv4-SP workloads, writes a
//! result block at the start of SRAM1 and stops with `BKPT #0`.
//! The number of rounds is read from `RESULT.rounds` so the host can scale the run
//! without rebuilding.
#![no_std]
#![no_main]

use core::hint::black_box;
use core::ptr::{addr_of_mut, read_volatile, write_volatile};

#[repr(C)]
pub struct Results {
    pub magic: u32,
    pub rounds: u32,
    pub crc: u32,
    pub matrix: u32,
    pub sort: u32,
    pub hash64_lo: u32,
    pub hash64_hi: u32,
    pub float_bits: u32,
    pub done: u32,
}

// Placed at the start of .bss → 0x20000000; the host pokes `rounds` before reset.
#[unsafe(no_mangle)]
#[unsafe(link_section = ".bss.results")]
static mut RESULT: Results = Results {
    magic: 0,
    rounds: 0,
    crc: 0,
    matrix: 0,
    sort: 0,
    hash64_lo: 0,
    hash64_hi: 0,
    float_bits: 0,
    done: 0,
};

static mut BUF: [u8; 4096] = [0; 4096];
static mut SORT: [u16; 256] = [0; 256];
static mut MA: [[i32; 16]; 16] = [[0; 16]; 16];
static mut MB: [[i32; 16]; 16] = [[0; 16]; 16];
static mut MC: [[i32; 16]; 16] = [[0; 16]; 16];

#[inline(never)]
fn xorshift32(s: &mut u32) -> u32 {
    let mut x = *s;
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    *s = x;
    x
}

#[inline(never)]
fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

#[inline(never)]
fn matmul(a: &[[i32; 16]; 16], b: &[[i32; 16]; 16], c: &mut [[i32; 16]; 16]) -> u32 {
    let mut acc = 0u32;
    for i in 0..16 {
        for j in 0..16 {
            let mut s = 0i32;
            for k in 0..16 {
                s = s.wrapping_add(a[i][k].wrapping_mul(b[k][j]));
            }
            c[i][j] = s;
            acc = acc.rotate_left(3) ^ s as u32;
        }
    }
    acc
}

#[inline(never)]
fn insertion_sort(v: &mut [u16]) -> u32 {
    for i in 1..v.len() {
        let key = v[i];
        let mut j = i;
        while j > 0 && v[j - 1] > key {
            v[j] = v[j - 1];
            j -= 1;
        }
        v[j] = key;
    }
    v.iter()
        .enumerate()
        .fold(0u32, |a, (i, &x)| a.wrapping_mul(31).wrapping_add(x as u32 ^ i as u32))
}

#[inline(never)]
fn hash64(seed: u64, n: u32) -> u64 {
    // splitmix64: 64-bit multiplies, adds with carry, 64-bit shifts.
    let mut z = seed;
    let mut acc = 0u64;
    for _ in 0..n {
        z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut x = z;
        x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        acc ^= x ^ (x >> 31);
    }
    acc
}

#[inline(never)]
fn float_work(n: u32) -> f32 {
    // Damped oscillator + Newton iterations: VMUL/VADD/VFMA/VDIV/VSQRT/VCVT/VCMP (VSQRT left to M1 tests).
    let (mut x, mut v) = (1.0f32, 0.0f32);
    let dt = black_box(0.01f32);
    let mut acc = 0.0f32;
    for i in 0..n {
        let a = -4.0 * x - 0.1 * v;
        v += a * dt;
        x += v * dt;
        let q = x * x + v * v + 1.0;
        // Two Newton steps for sqrt (core has no f32::sqrt in no_std).
        let mut r = 0.5 * (q + 1.0);
        r = 0.5 * (r + q / r);
        r = 0.5 * (r + q / r);
        acc += r / (1.0 + (i % 7) as f32);
        if acc > 1.0e6 {
            acc -= 1.0e6;
        }
    }
    acc + (x as i32) as f32
}

#[unsafe(no_mangle)]
pub extern "C" fn reset() -> ! {
    unsafe {
        // The FPU is disabled at reset: grant CP10/CP11 full access in SCB->CPACR
        // (0xE000ED88, PM0214 §4.6.1), as the firmware's SystemInit does.
        let cpacr = 0xE000_ED88 as *mut u32;
        write_volatile(cpacr, read_volatile(cpacr) | (0xF << 20));
        core::arch::asm!("dsb", "isb");
        let r = &mut *addr_of_mut!(RESULT);
        let rounds = match read_volatile(&r.rounds) {
            0 => 1,
            n => n,
        };
        let mut seed = 0x1234_5678u32;
        let (mut crc, mut mat, mut srt, mut h, mut f) = (0u32, 0u32, 0u32, 0u64, 0u32);
        for round in 0..rounds {
            let buf = &mut *addr_of_mut!(BUF);
            for b in buf.iter_mut() {
                *b = xorshift32(&mut seed) as u8;
            }
            crc ^= crc32(buf);

            let (ma, mb, mc) = (&mut *addr_of_mut!(MA), &mut *addr_of_mut!(MB), &mut *addr_of_mut!(MC));
            for i in 0..16 {
                for j in 0..16 {
                    ma[i][j] = (xorshift32(&mut seed) % 2001) as i32 - 1000;
                    mb[i][j] = (xorshift32(&mut seed) % 2001) as i32 - 1000;
                }
            }
            mat = mat.rotate_left(1) ^ matmul(ma, mb, mc);

            let s = &mut *addr_of_mut!(SORT);
            for x in s.iter_mut() {
                *x = xorshift32(&mut seed) as u16;
            }
            srt ^= insertion_sort(s);

            h ^= hash64(black_box(round as u64 * 0x1_0000_0001), 512);
            f ^= float_work(black_box(512)).to_bits();
        }
        write_volatile(&mut r.crc, crc);
        write_volatile(&mut r.matrix, mat);
        write_volatile(&mut r.sort, srt);
        write_volatile(&mut r.hash64_lo, h as u32);
        write_volatile(&mut r.hash64_hi, (h >> 32) as u32);
        write_volatile(&mut r.float_bits, f);
        write_volatile(&mut r.magic, 0xD0F1_A5E0);
        write_volatile(&mut r.done, 1);
        core::arch::asm!("bkpt #0");
    }
    loop {}
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    unsafe { core::arch::asm!("bkpt #1") };
    loop {}
}

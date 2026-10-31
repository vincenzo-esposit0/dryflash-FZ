// SPDX-License-Identifier: GPL-3.0-or-later
//! Small per-instruction tests. Encodings are taken from `llvm-objdump --mcpu=cortex-m4`
//! output or hand-assembled from DDI 0403E; each test notes which.
//! Broad coverage comes from the differential check against Unicorn (ADR-0001) and,
//! from M1, from per-encoding test tables.

use super::decode::{decode, is_32bit};
use super::op::Op;
use super::{Bus, BusError, Cpu, DecodeCache, Size, Stop};

/// 64 KiB of RAM at address 0, code and data alike.
struct FlatBus(Vec<u8>);

impl Bus for FlatBus {
    fn read(&mut self, addr: u32, size: Size) -> Result<u32, BusError> {
        let a = addr as usize;
        let b = self
            .0
            .get(a..a + size as usize)
            .ok_or(BusError { addr, write: false })?;
        let mut w = [0u8; 4];
        w[..b.len()].copy_from_slice(b);
        Ok(u32::from_le_bytes(w))
    }
    fn write(&mut self, addr: u32, value: u32, size: Size) -> Result<(), BusError> {
        let a = addr as usize;
        let b = self
            .0
            .get_mut(a..a + size as usize)
            .ok_or(BusError { addr, write: true })?;
        b.copy_from_slice(&value.to_le_bytes()[..size as usize]);
        Ok(())
    }
    fn fetch16(&mut self, addr: u32) -> Result<u16, BusError> {
        self.read(addr, Size::Half).map(|v| v as u16)
    }
}

/// Runs `code` (halfwords, terminated by the caller with BKPT) with `setup` applied first.
fn run(code: &[u16], setup: impl FnOnce(&mut Cpu)) -> Cpu {
    let mut bus = FlatBus(vec![0; 0x10000]);
    for (i, hw) in code.iter().enumerate() {
        bus.write(0x100 + 2 * i as u32, *hw as u32, Size::Half).unwrap();
    }
    let mut cpu = Cpu::new();
    cpu.halt_on_bkpt = true;
    cpu.r[13] = 0x8000;
    cpu.r[15] = 0x100;
    setup(&mut cpu);
    let mut cache = DecodeCache::new(0, 0x10000);
    let stop = cpu.run(&mut bus, &mut cache, 1000);
    assert!(matches!(stop, Stop::Breakpoint { .. }), "unexpected stop {stop:x?}");
    cpu
}

const BKPT: u16 = 0xBE00;

#[test]
fn width_detection() {
    assert!(!is_32bit(0x4051)); // eors r1, r2
    assert!(is_32bit(0xEA83)); // eor.w ...
    assert!(is_32bit(0xF04F)); // mov.w ...
    assert_eq!(decode(0xBF08, 0).op, Op::It { state: 0x08 }); // it eq
}

#[test]
fn adds_sets_overflow_and_negative() {
    // adds r0, r1, r2 — hand-assembled, A7.7.4 T1: 0001100 Rm Rn Rd.
    let c = run(&[0x1888, BKPT], |c| {
        c.r[1] = 0x7FFF_FFFF;
        c.r[2] = 1;
    });
    assert_eq!(c.r[0], 0x8000_0000);
    assert!(c.n && c.v && !c.z && !c.c);
}

#[test]
fn it_block_executes_or_skips() {
    // lsls r4, r0, #31 ; it eq ; lsreq r1, r0, #1 — from the benchmark's crc32 loop.
    let code = [0x07C4, 0xBF08, 0x0841, BKPT];
    let taken = run(&code, |c| {
        c.r[0] = 2;
        c.r[1] = 0x55;
    });
    assert_eq!(taken.r[1], 1);
    let skipped = run(&code, |c| {
        c.r[0] = 3;
        c.r[1] = 0x55;
    });
    assert_eq!(skipped.r[1], 0x55);
    assert_eq!(skipped.it, 0);
}

#[test]
fn shifted_register_operand() {
    // eor.w r1, r3, r1, lsr #1 — from the benchmark's crc32 loop.
    let c = run(&[0xEA83, 0x0151, BKPT], |c| {
        c.r[1] = 0b1010;
        c.r[3] = 0xEDB8_8320;
    });
    assert_eq!(c.r[1], 0xEDB8_8320 ^ 0b101);
}

#[test]
fn umull_produces_64_bit_product() {
    // umull r0, r1, r2, r3 — hand-assembled, A7.7.189 T1: FBA2 0103.
    let c = run(&[0xFBA2, 0x0103, BKPT], |c| {
        c.r[2] = 0xFFFF_FFFF;
        c.r[3] = 2;
    });
    assert_eq!((c.r[0], c.r[1]), (0xFFFF_FFFE, 1));
}

#[test]
fn fp_requires_cpacr_and_converts() {
    // vmov s0, r0 ; vcvt.f32.s32 s0, s0 ; vadd.f32 s0, s0, s0 ; vcvt.s32.f32 s1, s0 ; vmov r1, s1
    // Hand-assembled from A7.7.243, A7.7.226, A7.7.222.
    let code = [
        0xEE00, 0x0A10, 0xEEB8, 0x0AC0, 0xEE30, 0x0A00, 0xEEFD, 0x0AC0, 0xEE10, 0x1A90, BKPT,
    ];
    let c = run(&code, |c| {
        c.sys.cpacr = 0xF << 20;
        c.r[0] = (-7i32) as u32;
    });
    assert_eq!(f32::from_bits(c.s[0]), -14.0);
    assert_eq!(c.r[1] as i32, -14);

    // Without CPACR access the FP instruction raises UsageFault NOCP. UsageFault is not
    // enabled in SHCSR, so it escalates to HardFault; the HardFault vector (0) has bit 0
    // clear, so the handler's first instruction raises INVSTATE at priority -1: lockup.
    let mut bus = FlatBus(vec![0; 0x10000]);
    bus.write(0x100, 0xEE00, Size::Half).unwrap();
    bus.write(0x102, 0x0A10, Size::Half).unwrap();
    let mut cpu = Cpu::new();
    cpu.r[13] = 0x8000;
    cpu.r[15] = 0x100;
    let stop = cpu.run(&mut bus, &mut DecodeCache::new(0, 0x10000), 10);
    assert!(matches!(stop, Stop::Lockup { .. }), "{stop:x?}");
    assert_ne!(cpu.sys.cfsr & crate::sysctl::CFSR_NOCP, 0);
    assert_ne!(cpu.sys.hfsr & crate::sysctl::HFSR_FORCED, 0);
}

#[test]
fn unimplemented_encoding_stops_instead_of_skipping() {
    // F870 0000: op1 = 11, op2 = 0000111 is not an allocated encoding (A5.3, Table A5-14
    // marks it UNDEFINED); dryflash reports it as an emulator gap instead of guessing.
    let mut bus = FlatBus(vec![0; 0x10000]);
    bus.write(0x100, 0xF870, Size::Half).unwrap();
    bus.write(0x102, 0x0000, Size::Half).unwrap();
    let mut cpu = Cpu::new();
    cpu.r[15] = 0x100;
    let stop = cpu.run(&mut bus, &mut DecodeCache::new(0, 0x10000), 10);
    assert_eq!(
        stop,
        Stop::Unimplemented {
            raw: 0xF870_0000,
            pc: 0x100
        }
    );
}

// SPDX-License-Identifier: GPL-3.0-or-later
//! Self-checking firmware for the dryflash M-profile exception model.
//!
//! Each test sets bit n of `RESULT.pass` or `RESULT.fail`; the run ends with `BKPT #0`.
//! Expectations follow the ARMv7-M Architecture Reference Manual (DDI 0403E) B1.5
//! (exception model), B3.2 (SCB), B3.4 (NVIC) and B3.5 (MPU).
#![no_std]
#![no_main]

use core::arch::{asm, global_asm};
use core::ptr::{addr_of_mut, read_volatile as rd, write_volatile as wr};

#[repr(C)]
pub struct Results {
    pub magic: u32,
    pub pass: u32,
    pub fail: u32,
    pub detail: [u32; 16],
    pub done: u32,
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".bss.results")]
static mut RESULT: Results = Results {
    magic: 0,
    pass: 0,
    fail: 0,
    detail: [0; 16],
    done: 0,
};

// Scratch shared between thread code and handlers.
static mut SEEN_EXC_RETURN: u32 = 0;
static mut SEEN_FRAME: u32 = 0;
static mut SEEN_XPSR: u32 = 0;
static mut SEEN_IPSR: u32 = 0;
static mut TICKS: u32 = 0;
static mut IRQ0_COUNT: u32 = 0;
static mut ORDER: u32 = 0;
static mut FAULTS: u32 = 0;
static mut PSTACK: [u64; 64] = [0; 64];

// System registers (DDI 0403E B3.2, B3.3, B3.4, B3.5).
const ICSR: *mut u32 = 0xE000_ED04usize as _;
const SHCSR: *mut u32 = 0xE000_ED24usize as _;
const CFSR: *mut u32 = 0xE000_ED28usize as _;
const HFSR: *mut u32 = 0xE000_ED2Cusize as _;
const MMFAR: *mut u32 = 0xE000_ED34usize as _;
const CPACR: *mut u32 = 0xE000_ED88usize as _;
const SYST_CSR: *mut u32 = 0xE000_E010usize as _;
const SYST_RVR: *mut u32 = 0xE000_E014usize as _;
const SYST_CVR: *mut u32 = 0xE000_E018usize as _;
const NVIC_ISER0: *mut u32 = 0xE000_E100usize as _;
const NVIC_ISPR0: *mut u32 = 0xE000_E200usize as _;
const NVIC_IPR0: *mut u8 = 0xE000_E400usize as _;
const MPU_CTRL: *mut u32 = 0xE000_ED94usize as _;
const MPU_RNR: *mut u32 = 0xE000_ED98usize as _;
const MPU_RBAR: *mut u32 = 0xE000_ED9Cusize as _;
const MPU_RASR: *mut u32 = 0xE000_EDA0usize as _;

fn check(n: u32, ok: bool, detail: u32) {
    unsafe {
        let r = &mut *addr_of_mut!(RESULT);
        if ok {
            r.pass |= 1 << n;
        } else {
            r.fail |= 1 << n;
            r.detail[n as usize] = detail;
        }
    }
}

fn isb() {
    unsafe { asm!("dsb", "isb") };
}

// One entry point for every exception: passes the stacked frame (MSP or PSP according to
// EXC_RETURN bit 2), EXC_RETURN and IPSR to `dispatch`. The tail call keeps LR intact, so
// `dispatch` returns through EXC_RETURN.
global_asm!(
    ".section .text.entry,\"ax\"",
    ".global exc_entry",
    ".thumb_func",
    "exc_entry:",
    "tst lr, #4",
    "ite eq",
    "mrseq r0, msp",
    "mrsne r0, psp",
    "mov r1, lr",
    "mrs r2, ipsr",
    "b dispatch",
);

// Vector table: initial SP (top of SRAM1, as `_stack_end` in stm32wb55xx_flash.ld), reset,
// then 14 system vectors and IRQ0/IRQ1, all routed to `exc_entry`. Thumb function symbols
// get bit 0 set by the linker.
global_asm!(
    ".section .vectors,\"a\"",
    ".word 0x20030000",
    ".word reset",
    ".rept 16",
    ".word exc_entry",
    ".endr",
);

#[unsafe(no_mangle)]
extern "C" fn dispatch(frame: *mut u32, exc_return: u32, ipsr: u32) {
    unsafe {
        wr(addr_of_mut!(SEEN_EXC_RETURN), exc_return);
        wr(addr_of_mut!(SEEN_FRAME), frame as u32);
        wr(addr_of_mut!(SEEN_XPSR), rd(frame.add(7)));
        wr(addr_of_mut!(SEEN_IPSR), ipsr);
        let stacked_pc = rd(frame.add(6));
        match ipsr {
            11 => {
                // SVC: the immediate is in the instruction before the stacked PC.
                let imm = rd((stacked_pc - 2) as *const u16) & 0xFF;
                match imm {
                    7 => {
                        let ok = rd(frame) == 0x1234 && rd(frame.add(1)) == 0x5678;
                        check(0, ok, rd(frame));
                        wr(frame, 0xABCD); // Return value through the stacked R0.
                    }
                    2 => {
                        // Clobber S0; the extended frame must restore it on return.
                        asm!("vmov s0, {0}", in(reg) 0x4000_0000u32);
                    }
                    _ => {}
                }
            }
            14 => {} // PendSV: everything is checked from the recorded values.
            15 => wr(addr_of_mut!(TICKS), rd(addr_of_mut!(TICKS)) + 1),
            16 => {
                wr(addr_of_mut!(IRQ0_COUNT), rd(addr_of_mut!(IRQ0_COUNT)) + 1);
                let order = rd(addr_of_mut!(ORDER));
                if order == 0xA {
                    // Pend the higher-priority IRQ1: it must preempt right here.
                    wr(addr_of_mut!(ORDER), 0xA1);
                    wr(NVIC_ISPR0, 1 << 1);
                    isb();
                    wr(addr_of_mut!(ORDER), rd(addr_of_mut!(ORDER)) << 4 | 0xC);
                }
            }
            17 => {
                // Nested inside IRQ0: EXC_RETURN must return to Handler mode.
                wr(addr_of_mut!(ORDER), rd(addr_of_mut!(ORDER)) << 4 | 0xB);
                check(8, exc_return == 0xFFFF_FFF1, exc_return);
            }
            3 | 4 | 6 => {
                // HardFault / MemManage / UsageFault: skip the 16-bit faulting instruction.
                wr(addr_of_mut!(FAULTS), rd(addr_of_mut!(FAULTS)) + 1);
                wr(frame.add(6), stacked_pc + 2);
            }
            _ => check(15, false, ipsr),
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn reset() -> ! {
    unsafe {
        let r = &mut *addr_of_mut!(RESULT);

        // T0: SVC from Thread mode on MSP, basic frame, return value via stacked R0.
        let ret: u32;
        asm!("svc #7", inout("r0") 0x1234u32 => ret, in("r1") 0x5678u32, out("r2") _, out("r3") _, out("r12") _);
        let exc = rd(addr_of_mut!(SEEN_EXC_RETURN));
        check(
            0,
            ret == 0xABCD && exc == 0xFFFF_FFF9 && rd(addr_of_mut!(SEEN_IPSR)) == 11,
            exc,
        );
        // Stacked xPSR: Thumb bit set, IPSR 0 (Thread mode).
        let xpsr = rd(addr_of_mut!(SEEN_XPSR));
        check(1, xpsr & (1 << 24) != 0 && xpsr & 0x1FF == 0, xpsr);

        // T2: PendSV taken on the process stack.
        // One asm block: while CONTROL.SPSEL = 1, Rust code must not touch the stack.
        let pstack_top = (addr_of_mut!(PSTACK) as u32) + 64 * 8;
        asm!(
            "msr psp, {top}",
            "movs {t}, #2",
            "msr control, {t}",
            "isb",
            "str {pend}, [{icsr}]",
            "dsb",
            "isb",
            "movs {t}, #0",
            "msr control, {t}",
            "isb",
            top = in(reg) pstack_top,
            t = out(reg) _,
            pend = in(reg) 1u32 << 28,
            icsr = in(reg) ICSR,
        );
        let exc = rd(addr_of_mut!(SEEN_EXC_RETURN));
        let frame = rd(addr_of_mut!(SEEN_FRAME));
        check(2, exc == 0xFFFF_FFFD && frame == pstack_top - 0x20, exc);

        // T3: FP context: enable CP10/CP11, use S0, then SVC: extended frame (EXC_RETURN
        // bit 4 clear), stacked S0 at frame + 0x20, restored on return.
        wr(CPACR, rd(CPACR) | (0xF << 20));
        isb();
        let s0_after: u32;
        let sp_before: u32;
        let sp_after: u32;
        asm!(
            "vmov s0, {v}",
            "mov {b}, sp",
            "svc #2",
            "mov {a}, sp",
            "vmov {o}, s0",
            v = in(reg) 0x3F80_0000u32,
            b = out(reg) sp_before,
            a = out(reg) sp_after,
            o = out(reg) s0_after,
            out("r0") _, out("r1") _, out("r2") _, out("r3") _, out("r12") _,
        );
        let exc = rd(addr_of_mut!(SEEN_EXC_RETURN));
        let frame = rd(addr_of_mut!(SEEN_FRAME));
        let stacked_s0 = rd((frame + 0x20) as *const u32);
        check(
            3,
            exc == 0xFFFF_FFE9 && stacked_s0 == 0x3F80_0000 && s0_after == 0x3F80_0000 && sp_before == sp_after,
            exc,
        );

        // T4: BASEPRI masks IRQ0 at priority 0x80; lowering BASEPRI lets it in.
        wr(NVIC_IPR0, 0x80);
        wr(NVIC_ISER0, 1);
        asm!("movs {0}, #0x40", "msr basepri, {0}", out(reg) _);
        wr(NVIC_ISPR0, 1);
        isb();
        let masked = rd(addr_of_mut!(IRQ0_COUNT));
        asm!("movs {0}, #0", "msr basepri, {0}", "isb", out(reg) _);
        check(4, masked == 0 && rd(addr_of_mut!(IRQ0_COUNT)) == 1, masked);

        // T5: SysTick interrupts wake WFI.
        wr(SYST_RVR, 999);
        wr(SYST_CVR, 0);
        wr(SYST_CSR, 0b111);
        while rd(addr_of_mut!(TICKS)) < 3 {
            asm!("wfi");
        }
        wr(SYST_CSR, 0);
        check(5, rd(addr_of_mut!(TICKS)) == 3, rd(addr_of_mut!(TICKS)));

        // T6: UsageFault UNDEFINSTR with the handler enabled.
        wr(SHCSR, rd(SHCSR) | (1 << 18));
        asm!("udf #0");
        let cfsr = rd(CFSR);
        check(6, rd(addr_of_mut!(SEEN_IPSR)) == 6 && cfsr & (1 << 16) != 0, cfsr);
        wr(CFSR, cfsr);

        // T7: with UsageFault disabled it escalates to HardFault (HFSR.FORCED).
        wr(SHCSR, rd(SHCSR) & !(1 << 18));
        asm!("udf #1");
        let hfsr = rd(HFSR);
        check(7, rd(addr_of_mut!(SEEN_IPSR)) == 3 && hfsr & (1 << 30) != 0, hfsr);
        wr(HFSR, hfsr);
        wr(CFSR, rd(CFSR));

        // T8 is checked in the IRQ1 handler; T9: nesting order IRQ0 → IRQ1 → IRQ0.
        wr(NVIC_IPR0.add(1), 0x40);
        wr(NVIC_ISER0, 0b11);
        wr(addr_of_mut!(ORDER), 0xA);
        wr(NVIC_ISPR0, 1);
        isb();
        let order = rd(addr_of_mut!(ORDER));
        check(9, order == 0xA1BC, order);

        // T10: the Furi NULL guard. Region 0, base 0, 1 MiB, no access, PRIVDEFENA;
        // a read of address 4 raises MemManage DACCVIOL with MMFAR = 4.
        wr(SHCSR, rd(SHCSR) | (1 << 16));
        wr(MPU_RNR, 0);
        wr(MPU_RBAR, 0);
        wr(MPU_RASR, (19 << 1) | 1);
        wr(MPU_CTRL, 0b101);
        isb();
        // Low registers so the load is the 16-bit encoding the handler skips.
        asm!("ldr r0, [r1]", out("r0") _, in("r1") 4u32);
        let cfsr = rd(CFSR);
        let mmfar = rd(MMFAR);
        wr(MPU_CTRL, 0);
        isb();
        check(
            10,
            rd(addr_of_mut!(SEEN_IPSR)) == 4 && cfsr & 0x82 == 0x82 && mmfar == 4,
            cfsr,
        );
        wr(CFSR, cfsr);

        // T11: 8-byte stack alignment on entry (CCR.STKALIGN): with SP ≡ 4 (mod 8) the
        // frame is aligned down, xPSR bit 9 records it, and SP is restored on return.
        let sp_before: u32;
        let sp_after: u32;
        asm!(
            "sub sp, #4",
            "mov {b}, sp",
            "svc #9",
            "mov {a}, sp",
            "add sp, #4",
            b = out(reg) sp_before,
            a = out(reg) sp_after,
            out("r0") _, out("r1") _, out("r2") _, out("r3") _, out("r12") _,
        );
        let frame = rd(addr_of_mut!(SEEN_FRAME));
        let xpsr = rd(addr_of_mut!(SEEN_XPSR));
        check(
            11,
            sp_before & 4 == 4 && frame & 7 == 0 && xpsr & (1 << 9) != 0 && sp_after == sp_before,
            frame,
        );

        check(12, rd(addr_of_mut!(FAULTS)) == 3, rd(addr_of_mut!(FAULTS)));

        wr(&mut r.magic, 0xE7C0_0001);
        wr(&mut r.done, 1);
        asm!("bkpt #0");
    }
    loop {}
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    unsafe { asm!("bkpt #1") };
    loop {}
}

// SPDX-License-Identifier: GPL-3.0-or-later
//! M-profile exception model (DDI 0403E B1.5): priorities, entry (PushStack,
//! ExceptionTaken), return (ExceptionReturn, PopStack), fault escalation and lockup.
//!
//! FP context stacking is done eagerly: when CONTROL.FPCA is set the extended frame is
//! reserved *and* written at entry, instead of deferring the S0–S15/FPSCR stores until the
//! handler's first FP instruction (lazy stacking, FPCCR.LSPEN). Memory contents seen by
//! software are the same; only FPCCR.LSPACT/FPCAR differ. See ADR-0007.

use super::{Bus, CONTROL_FPCA, CONTROL_SPSEL, Cpu, CpuEvent, Fault, Size, Stop};
use crate::sysctl::*;

/// Base priority of Thread mode with no boosting: one below the lowest configurable level.
const THREAD_PRIORITY: i16 = 256;

impl Cpu {
    /// ExecutionPriority (B1.5.4): the highest (numerically lowest) of the active
    /// exceptions' group priorities and the PRIMASK/FAULTMASK/BASEPRI boosts.
    pub fn execution_priority(&self) -> i16 {
        let mut p = THREAD_PRIORITY;
        for n in 1..NUM_EXC {
            if self.sys.active[n] {
                p = p.min(self.sys.group_priority(n));
            }
        }
        p.min(self.boost_priority(true))
    }

    fn boost_priority(&self, with_primask: bool) -> i16 {
        let mut p = THREAD_PRIORITY;
        if self.basepri != 0 {
            p = p.min(self.sys.group_of(self.basepri) as i16);
        }
        if with_primask && self.primask {
            p = p.min(0);
        }
        if self.faultmask {
            p = p.min(-1);
        }
        p
    }

    /// The pending, enabled exception that should be taken now, if any. Among candidates
    /// the lowest full priority wins, then the lowest exception number (B1.5.4).
    pub(crate) fn preempting_exception(&self) -> Option<usize> {
        let current = self.execution_priority();
        let mut best: Option<(u8, usize)> = None;
        for n in 2..NUM_EXC {
            if !self.sys.pending[n] || !self.sys.exception_enabled(n) {
                continue;
            }
            if self.sys.group_priority(n) >= current {
                continue;
            }
            let full = match n {
                EXC_NMI | EXC_HARDFAULT => 0,
                _ => self.sys.prio[n],
            };
            let key = (full, n);
            let fixed_first = n == EXC_NMI || n == EXC_HARDFAULT;
            if fixed_first || best.is_none_or(|b| key < b) {
                if fixed_first {
                    return Some(n);
                }
                best = Some(key);
            }
        }
        best.map(|b| b.1)
    }

    /// WFI wake-up condition: an exception pending that would preempt if PRIMASK were 0
    /// (B1.5.19), or an event for WFE.
    pub(crate) fn wakeup_pending(&self) -> bool {
        let mut current = THREAD_PRIORITY;
        for n in 1..NUM_EXC {
            if self.sys.active[n] {
                current = current.min(self.sys.group_priority(n));
            }
        }
        current = current.min(self.boost_priority(false));
        (2..NUM_EXC)
            .any(|n| self.sys.pending[n] && self.sys.exception_enabled(n) && self.sys.group_priority(n) < current)
    }

    /// Stack write used by exception entry; MPU-checked with the stacking privilege.
    fn stack_write<B: Bus>(&mut self, bus: &mut B, addr: u32, v: u32) -> Result<(), ()> {
        // Stacking uses the privilege of the interrupted context (B1.5.6 PushStack).
        let privileged = self.privileged();
        if self.sys.mpu.enabled()
            && !self
                .sys
                .mpu
                .allowed(addr, MpuAccess::Write, privileged, self.mpu_default_map())
        {
            self.sys.cfsr |= CFSR_MSTKERR;
            return Err(());
        }
        bus.write(addr, v, Size::Word).map_err(|_| {
            self.sys.cfsr |= CFSR_STKERR;
        })
    }

    fn using_psp_for_stacking(&self) -> bool {
        !self.handler_mode() && self.control & CONTROL_SPSEL != 0
    }

    /// PushStack + ExceptionTaken (B1.5.6, B1.5.7). `return_address` is stacked as PC.
    pub(crate) fn take_exception<B: Bus>(&mut self, bus: &mut B, n: usize, return_address: u32) -> Result<(), Stop> {
        let fp_frame = self.control & CONTROL_FPCA != 0;
        let frame_size: u32 = if fp_frame { 0x68 } else { 0x20 };
        let force_align = self.sys.ccr & (1 << 9) != 0;
        let sp = self.r[13];
        let align_pad = force_align && sp & 4 != 0;
        let frame = sp.wrapping_sub(frame_size) & if force_align { !4 } else { !0 };
        let xpsr = self.xpsr() & !(1 << 9) | (align_pad as u32) << 9;
        let mut words = vec![
            self.r[0],
            self.r[1],
            self.r[2],
            self.r[3],
            self.r[12],
            self.r[14],
            return_address,
            xpsr,
        ];
        if fp_frame {
            words.extend_from_slice(&self.s[..16]);
            words.push(self.fpscr);
            words.push(0); // Reserved word, keeps the frame 8-byte aligned.
        }
        let mut stacking_failed = false;
        for (i, w) in words.iter().enumerate() {
            if self.stack_write(bus, frame.wrapping_add(4 * i as u32), *w).is_err() {
                stacking_failed = true;
                break;
            }
        }
        let was_psp = self.using_psp_for_stacking();
        self.r[13] = frame;
        // EXC_RETURN (B1.5.6): bit 4 = no FP frame, bits [3:0] = return mode and stack.
        let mode_bits = if self.handler_mode() {
            0x1
        } else if was_psp {
            0xD
        } else {
            0x9
        };
        self.r[14] = 0xFFFF_FFE0 | ((!fp_frame) as u32) << 4 | mode_bits;

        // ExceptionTaken.
        self.ipsr = n as u16;
        self.reselect_sp(was_psp);
        self.control &= !(CONTROL_SPSEL | CONTROL_FPCA);
        self.sys.pending[n] = false;
        self.sys.active[n] = true;
        self.it = 0;
        self.exclusive = None;
        self.event = true;
        self.exc_check = true;
        self.mpu_changed();

        if stacking_failed {
            // A derived fault during stacking: the exception being taken stays active and
            // the stacking error is handled as a BusFault/MemManage (B1.5.14). Escalate.
            return self.derived_fault(bus, n);
        }
        // Vector reads always use the default memory map (B3.5.1).
        match bus.read(self.sys.vtor.wrapping_add(4 * n as u32), Size::Word) {
            Ok(vector) => {
                self.t = vector & 1 != 0;
                self.r[15] = vector & !1;
                Ok(())
            }
            Err(_) => {
                self.sys.hfsr |= HFSR_VECTTBL;
                if n == EXC_HARDFAULT || n == EXC_NMI {
                    return Err(Stop::Lockup { pc: return_address });
                }
                self.sys.active[n] = false;
                self.sys.pending[EXC_HARDFAULT] = true;
                self.exc_check = true;
                Ok(())
            }
        }
    }

    fn derived_fault<B: Bus>(&mut self, bus: &mut B, n: usize) -> Result<(), Stop> {
        if n == EXC_HARDFAULT || n == EXC_NMI {
            return Err(Stop::Lockup { pc: self.r[15] });
        }
        self.sys.active[n] = false;
        self.sys.hfsr |= HFSR_FORCED;
        let ret = self.r[15];
        self.take_exception(bus, EXC_HARDFAULT, ret)
    }

    /// Raises a synchronous fault for the instruction at `pc` (which does not retire).
    pub(crate) fn fault_at<B: Bus>(&mut self, bus: &mut B, fault: Fault, pc: u32) -> Option<Stop> {
        self.r[15] = pc;
        let (exc, bits) = match fault {
            Fault::BusData { addr } => {
                self.sys.bfar = addr;
                (EXC_BUSFAULT, CFSR_PRECISERR | CFSR_BFARVALID)
            }
            Fault::BusFetch => (EXC_BUSFAULT, CFSR_IBUSERR),
            Fault::MemData { addr } => {
                self.sys.mmfar = addr;
                (EXC_MEMMANAGE, CFSR_DACCVIOL | CFSR_MMARVALID)
            }
            Fault::MemFetch => (EXC_MEMMANAGE, CFSR_IACCVIOL),
            Fault::Undefined => (EXC_USAGEFAULT, CFSR_UNDEFINSTR),
            Fault::InvalidState => (EXC_USAGEFAULT, CFSR_INVSTATE),
            Fault::InvalidPc => (EXC_USAGEFAULT, CFSR_INVPC),
            Fault::NoCoprocessor => (EXC_USAGEFAULT, CFSR_NOCP),
            Fault::Unaligned { .. } => (EXC_USAGEFAULT, CFSR_UNALIGNED),
            Fault::DivideByZero => (EXC_USAGEFAULT, CFSR_DIVBYZERO),
        };
        self.sys.cfsr |= bits;
        if self.first_fault.is_none() {
            self.first_fault = Some(self.snapshot(bus, pc, Some(fault)));
        }
        self.events.push(CpuEvent::Fault {
            cycles: self.cycles,
            fault,
            exception: exc as u8,
            pc,
        });
        let current = self.execution_priority();
        if self.sys.exception_enabled(exc) && self.sys.group_priority(exc) < current {
            return self.take_exception(bus, exc, pc).err();
        }
        self.events.push(CpuEvent::Escalated {
            cycles: self.cycles,
            from: exc as u8,
            pc,
        });
        self.sys.hfsr |= HFSR_FORCED;
        self.take_hardfault(bus, pc)
    }

    /// Takes HardFault for the instruction at `pc`, or locks up if that is impossible.
    pub(crate) fn take_hardfault<B: Bus>(&mut self, bus: &mut B, pc: u32) -> Option<Stop> {
        if self.execution_priority() <= -1 {
            return Some(Stop::Lockup { pc });
        }
        self.take_exception(bus, EXC_HARDFAULT, pc).err()
    }

    /// SVC: synchronous; escalates to HardFault when it cannot be taken (B1.5.14).
    pub(crate) fn svc<B: Bus>(&mut self, bus: &mut B, next: u32) -> Option<Stop> {
        if self.sys.group_priority(EXC_SVCALL) < self.execution_priority() {
            return self.take_exception(bus, EXC_SVCALL, next).err();
        }
        self.events.push(CpuEvent::Escalated {
            cycles: self.cycles,
            from: EXC_SVCALL as u8,
            pc: next,
        });
        self.sys.hfsr |= HFSR_FORCED;
        self.take_hardfault(bus, next)
    }

    /// ExceptionReturn + PopStack (B1.5.8).
    pub(crate) fn exception_return<B: Bus>(&mut self, bus: &mut B, exc_return: u32) -> Option<Stop> {
        let pc = self.cur_pc;
        let returning = self.ipsr as usize;
        let kind = exc_return & 0xF;
        if exc_return & 0x0FFF_FFE0 != 0x0FFF_FFE0 || !matches!(kind, 0x1 | 0x9 | 0xD) {
            return self.fault_at(bus, Fault::InvalidPc, pc);
        }
        let others_active = (1..NUM_EXC).any(|n| n != returning && self.sys.active[n]);
        let to_thread = kind != 0x1;
        if to_thread && others_active && self.sys.ccr & 1 == 0 {
            // Return to Thread with other exceptions active and NONBASETHRDENA = 0.
            return self.fault_at(bus, Fault::InvalidPc, pc);
        }
        if !to_thread && !others_active {
            return self.fault_at(bus, Fault::InvalidPc, pc);
        }
        // DeActivate (B1.5.8): FAULTMASK clears on return from anything but NMI.
        self.sys.active[returning] = false;
        if returning != EXC_NMI {
            self.faultmask = false;
        }
        // Handler mode always runs on MSP, so the live MSP is r[13] and PSP is banked.
        let msp_now = self.r[13];
        let use_psp = kind == 0xD;
        let frame = if use_psp { self.psp } else { msp_now };
        let fp_frame = exc_return & 0x10 == 0;
        let words = if fp_frame { 26 } else { 8 };
        let mut w = [0u32; 26];
        for (i, slot) in w.iter_mut().enumerate().take(words) {
            match bus.read(frame.wrapping_add(4 * i as u32), Size::Word) {
                Ok(v) => *slot = v,
                Err(_) => {
                    // Unstacking error: BusFault UNSTKERR (B1.5.8).
                    self.sys.cfsr |= 1 << 11;
                    return self.fault_at(
                        bus,
                        Fault::BusData {
                            addr: frame.wrapping_add(4 * i as u32),
                        },
                        pc,
                    );
                }
            }
        }
        let xpsr = w[7];
        let pad = if xpsr & (1 << 9) != 0 && self.sys.ccr & (1 << 9) != 0 {
            4
        } else {
            0
        };
        let new_sp = frame.wrapping_add(words as u32 * 4 + pad);

        // Switch mode and stack.
        self.ipsr = if to_thread { 0 } else { (xpsr & 0x1FF) as u16 };
        if use_psp {
            self.control |= CONTROL_SPSEL;
            self.msp = msp_now;
            self.r[13] = new_sp;
        } else {
            if to_thread {
                self.control &= !CONTROL_SPSEL;
            }
            self.r[13] = new_sp;
        }

        self.r[..4].copy_from_slice(&w[..4]);
        self.r[12] = w[4];
        self.r[14] = w[5];
        if fp_frame {
            self.s[..16].copy_from_slice(&w[8..24]);
            self.fpscr = w[24];
            self.control |= CONTROL_FPCA;
        } else {
            self.control &= !CONTROL_FPCA;
        }
        self.set_apsr_flags(xpsr);
        self.ge = ((xpsr >> 16) & 0xF) as u8;
        self.t = xpsr & (1 << 24) != 0;
        self.it = (((xpsr >> 10) & 0x3F) << 2 | ((xpsr >> 25) & 3)) as u8;
        self.r[15] = w[6] & !1;
        self.branched = true;
        self.exclusive = None;
        self.event = true;
        self.exc_check = true;
        self.mpu_changed();

        let stacked_ipsr = xpsr & 0x1FF;
        if to_thread && stacked_ipsr != 0 || !to_thread && stacked_ipsr == 0 {
            let ret = self.r[15];
            return self.fault_at(bus, Fault::InvalidPc, ret);
        }
        None
    }

    /// Records an exception taken outside `fault_at` (diagnostics only).
    #[allow(dead_code)]
    fn note(&mut self, ev: CpuEvent) {
        self.events.push(ev);
    }
}

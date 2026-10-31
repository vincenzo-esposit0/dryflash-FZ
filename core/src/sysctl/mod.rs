// SPDX-License-Identifier: GPL-3.0-or-later
//! Private Peripheral Bus of the Cortex-M4: System Control Space (SCB, NVIC, SysTick,
//! MPU, FPU control, CoreDebug), DWT and the remaining PPB blocks.
//!
//! References: ARMv7-M ARM (DDI 0403E) B3.2–B3.5 and C1; Cortex-M4 TRM (DDI 0439B) for
//! implementation-defined values; `lib/cmsis_core/core_cm4.h` and
//! `lib/stm32wb_cmsis/Include/stm32wb55xx.h` (firmware 1.5.1) for STM32WB55 parameters
//! (`__NVIC_PRIO_BITS = 4`, 63 external interrupts, `__CM4_REV = r0p1`).

mod mpu;

pub use mpu::{Mpu, MpuAccess, default_map_xn};

/// Number of external interrupts on the STM32WB55 (IRQn 0..=62, `DMAMUX1_OVR_IRQn = 62`).
pub const NUM_IRQ: usize = 63;
/// Exception numbers 0..16 are system exceptions; IRQ n is exception 16 + n.
pub const NUM_EXC: usize = 16 + NUM_IRQ;

pub const EXC_NMI: usize = 2;
pub const EXC_HARDFAULT: usize = 3;
pub const EXC_MEMMANAGE: usize = 4;
pub const EXC_BUSFAULT: usize = 5;
pub const EXC_USAGEFAULT: usize = 6;
pub const EXC_SVCALL: usize = 11;
pub const EXC_DEBUGMON: usize = 12;
pub const EXC_PENDSV: usize = 14;
pub const EXC_SYSTICK: usize = 15;

/// `__NVIC_PRIO_BITS`: implemented priority bits are [7:4].
const PRIO_MASK: u8 = 0xF0;

// CFSR bits (DDI 0403E B3.2.15).
pub const CFSR_IACCVIOL: u32 = 1 << 0;
pub const CFSR_DACCVIOL: u32 = 1 << 1;
pub const CFSR_MSTKERR: u32 = 1 << 4;
pub const CFSR_MMARVALID: u32 = 1 << 7;
pub const CFSR_IBUSERR: u32 = 1 << 8;
pub const CFSR_PRECISERR: u32 = 1 << 9;
pub const CFSR_STKERR: u32 = 1 << 12;
pub const CFSR_BFARVALID: u32 = 1 << 15;
pub const CFSR_UNDEFINSTR: u32 = 1 << 16;
pub const CFSR_INVSTATE: u32 = 1 << 17;
pub const CFSR_INVPC: u32 = 1 << 18;
pub const CFSR_NOCP: u32 = 1 << 19;
pub const CFSR_UNALIGNED: u32 = 1 << 24;
pub const CFSR_DIVBYZERO: u32 = 1 << 25;
// HFSR bits (B3.2.16).
pub const HFSR_VECTTBL: u32 = 1 << 1;
pub const HFSR_FORCED: u32 = 1 << 30;
pub const HFSR_DEBUGEVT: u32 = 1 << 31;

/// Result of a PPB access that the caller must turn into an architectural effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PpbEffect {
    None,
    /// AIRCR.SYSRESETREQ written with a valid key.
    ResetRequest,
    /// Pending/priority/enable state changed: re-evaluate exceptions.
    ExceptionStateChanged,
    /// MPU configuration changed: refresh cached permissions.
    MpuChanged,
}

/// Unprivileged or otherwise faulting PPB access.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PpbFault;

/// A PPB register the model does not implement (reported in the bring-up digest).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PpbUnimplemented {
    pub addr: u32,
    pub write: bool,
}

#[derive(Clone, Debug)]
pub struct SysTick {
    pub ctrl: u32,
    pub reload: u32,
    /// Cycle at which the counter was last (re)loaded with `reload`, i.e. read as `reload`.
    loaded_at: u64,
    /// Cycle of the next wrap to 0 → reload, if the counter is enabled.
    pub next_wrap: Option<u64>,
    countflag: bool,
}

impl SysTick {
    fn new() -> Self {
        SysTick {
            ctrl: 0,
            reload: 0,
            loaded_at: 0,
            next_wrap: None,
            countflag: false,
        }
    }

    fn enabled(&self) -> bool {
        self.ctrl & 1 != 0
    }

    /// After a system reset the timer is disabled; keeps the time base monotonic.
    pub fn reset_time(&mut self, now: u64) {
        *self = SysTick::new();
        self.loaded_at = now;
    }

    /// Cycles per SysTick decrement. CLKSOURCE=1: processor clock; 0: the external
    /// reference, which is HCLK/8 on the STM32WB (RM0434 §6.2, "Cortex system timer").
    fn divider(&self) -> u64 {
        if self.ctrl & 4 != 0 { 1 } else { 8 }
    }

    fn period(&self) -> u64 {
        (self.reload as u64 + 1) * self.divider()
    }

    /// Current counter value at cycle `now`.
    fn current(&self, now: u64) -> u32 {
        if !self.enabled() || self.reload == 0 || now < self.loaded_at {
            // Disabled, or the tick between reaching 0 and reloading.
            return 0;
        }
        let elapsed = (now - self.loaded_at) / self.divider();
        let span = self.reload as u64 + 1;
        (self.reload as u64 - (elapsed % span)) as u32
    }

    fn reschedule(&mut self) {
        self.next_wrap = if self.enabled() && self.reload != 0 {
            // The counter reads `reload` at `loaded_at` and reaches 0 after reload ticks.
            Some(self.loaded_at + self.reload as u64 * self.divider())
        } else {
            None
        };
    }
}

/// The System Control Space and friends.
#[derive(Clone, Debug)]
pub struct Sysctl {
    pub vtor: u32,
    pub aircr_prigroup: u8,
    pub scr: u32,
    pub ccr: u32,
    /// SHCSR enable bits (MEMFAULTENA, BUSFAULTENA, USGFAULTENA).
    pub shcsr_enables: u32,
    pub cfsr: u32,
    pub hfsr: u32,
    pub mmfar: u32,
    pub bfar: u32,
    pub cpacr: u32,
    pub fpccr: u32,
    pub fpcar: u32,
    pub fpdscr: u32,
    pub demcr: u32,
    pub dwt_ctrl: u32,
    /// CYCCNT = (cycles - dwt_cyc_base) while counting.
    dwt_cyc_base: u64,
    dwt_cyc_frozen: u32,
    pub actlr: u32,

    /// Per-exception priority (implemented bits only). Index = exception number.
    pub prio: [u8; NUM_EXC],
    pub pending: [bool; NUM_EXC],
    pub active: [bool; NUM_EXC],
    /// External interrupt enables (ISER), index = IRQ number.
    pub irq_enabled: [bool; NUM_IRQ],

    pub systick: SysTick,
    pub mpu: Mpu,
    /// DHCSR.C_DEBUGEN as seen by software. dryflash does not present itself as an
    /// attached debugger, so firmware takes its non-debug paths (e.g. `furi_crash`).
    pub debugger_attached: bool,
}

impl Default for Sysctl {
    fn default() -> Self {
        Self::new()
    }
}

impl Sysctl {
    pub fn new() -> Self {
        let mut prio = [0u8; NUM_EXC];
        // Fixed priorities are handled in `group_priority`; configurable ones reset to 0.
        prio[0] = 0;
        Sysctl {
            vtor: 0,
            aircr_prigroup: 0,
            scr: 0,
            // STKALIGN resets to 1 on the Cortex-M4 (DDI 0439B §4.3.8).
            ccr: 0x0000_0200,
            shcsr_enables: 0,
            cfsr: 0,
            hfsr: 0,
            mmfar: 0,
            bfar: 0,
            cpacr: 0,
            // ASPEN and LSPEN reset to 1 (DDI 0403E B3.2.21).
            fpccr: 0xC000_0000,
            fpcar: 0,
            fpdscr: 0,
            demcr: 0,
            // NUMCOMP = 4 comparators (DDI 0439B §8.2).
            dwt_ctrl: 0x4000_0000,
            dwt_cyc_base: 0,
            dwt_cyc_frozen: 0,
            actlr: 0,
            prio,
            pending: [false; NUM_EXC],
            active: [false; NUM_EXC],
            irq_enabled: [false; NUM_IRQ],
            systick: SysTick::new(),
            mpu: Mpu::new(),
            debugger_attached: false,
        }
    }

    pub fn fp_enabled(&self) -> bool {
        // CP10 and CP11 must both be "full access" (0b11); privileged-only (0b01) is
        // treated as full access for privileged code by the caller.
        (self.cpacr >> 20) & 0xF != 0
    }

    pub fn fp_access(&self, privileged: bool) -> bool {
        match (self.cpacr >> 20) & 0x3 {
            0b11 => true,
            0b01 => privileged,
            _ => false,
        }
    }

    /// Whether exception `n` is enabled (can become active instead of escalating/ignoring).
    pub fn exception_enabled(&self, n: usize) -> bool {
        match n {
            EXC_MEMMANAGE => self.shcsr_enables & (1 << 16) != 0,
            EXC_BUSFAULT => self.shcsr_enables & (1 << 17) != 0,
            EXC_USAGEFAULT => self.shcsr_enables & (1 << 18) != 0,
            // DebugMonitor requires DEMCR.MON_EN.
            EXC_DEBUGMON => self.demcr & (1 << 16) != 0,
            n if n >= 16 => self.irq_enabled[n - 16],
            _ => true,
        }
    }

    /// Group priority of exception `n` (B1.5.4): fixed negative for Reset/NMI/HardFault.
    pub fn group_priority(&self, n: usize) -> i16 {
        match n {
            1 => -3,
            EXC_NMI => -2,
            EXC_HARDFAULT => -1,
            _ => self.group_of(self.prio[n]) as i16,
        }
    }

    /// Masks off subpriority bits according to AIRCR.PRIGROUP.
    pub fn group_of(&self, p: u8) -> u8 {
        let sub_bits = self.aircr_prigroup as u32 + 1;
        if sub_bits >= 8 { 0 } else { p & (0xFFu8 << sub_bits) }
    }

    pub fn cyccnt(&self, now: u64) -> u32 {
        if self.dwt_ctrl & 1 != 0 && self.demcr & (1 << 24) != 0 {
            (now.wrapping_sub(self.dwt_cyc_base)) as u32
        } else {
            self.dwt_cyc_frozen
        }
    }

    /// Handles a SysTick wrap at `now` (= `systick.next_wrap`); returns true if the
    /// SysTick exception must be pended.
    pub fn systick_wrap(&mut self, now: u64) -> bool {
        let st = &mut self.systick;
        st.countflag = true;
        // The counter reloads one tick after reaching 0.
        st.loaded_at = now + st.divider();
        st.next_wrap = Some(now + st.period());
        st.ctrl & 2 != 0
    }

    /// ICSR.VECTPENDING / RETTOBASE helpers need a view of pending state.
    fn highest_pending(&self) -> usize {
        let mut best = 0;
        let mut best_p = i16::MAX;
        for n in 2..NUM_EXC {
            if self.pending[n] && self.exception_enabled(n) {
                let p = self.group_priority(n);
                if p < best_p {
                    best_p = p;
                    best = n;
                }
            }
        }
        best
    }

    fn active_count(&self) -> usize {
        self.active.iter().filter(|a| **a).count()
    }

    /// Read from the PPB. `now` is the current cycle count, `ipsr` the current exception.
    pub fn read(&mut self, addr: u32, now: u64, ipsr: u16) -> Result<u32, PpbUnimplemented> {
        let unimpl = Err(PpbUnimplemented { addr, write: false });
        Ok(match addr {
            // ITM/DWT/FPB/TPIU/ETM/ROM table: only what software actually uses.
            0xE000_1000 => self.dwt_ctrl,
            0xE000_1004 => self.cyccnt(now),
            0xE000_E004 => 1, // ICTR: INTLINESNUM = 1 → 33..64 interrupts.
            0xE000_E008 => self.actlr,
            0xE000_E010 => {
                let flag = self.systick.countflag;
                self.systick.countflag = false;
                self.systick.ctrl | (flag as u32) << 16
            }
            0xE000_E014 => self.systick.reload,
            0xE000_E018 => self.systick.current(now),
            0xE000_E01C => 0, // CALIB: no calibration value (NOREF=0, SKEW=0, TENMS=0).
            0xE000_E100..=0xE000_E13C => self.irq_bits(addr - 0xE000_E100, |s, i| s.irq_enabled[i]),
            0xE000_E180..=0xE000_E1BC => self.irq_bits(addr - 0xE000_E180, |s, i| s.irq_enabled[i]),
            0xE000_E200..=0xE000_E23C => self.irq_bits(addr - 0xE000_E200, |s, i| s.pending[16 + i]),
            0xE000_E280..=0xE000_E2BC => self.irq_bits(addr - 0xE000_E280, |s, i| s.pending[16 + i]),
            0xE000_E300..=0xE000_E33C => self.irq_bits(addr - 0xE000_E300, |s, i| s.active[16 + i]),
            0xE000_E400..=0xE000_E5EF => {
                let base = (addr - 0xE000_E400) as usize & !3;
                let mut v = 0;
                for b in 0..4 {
                    if let Some(p) = self.prio.get(16 + base + b) {
                        v |= (*p as u32) << (8 * b);
                    }
                }
                v
            }
            0xE000_ED00 => 0x410F_C241, // CPUID: Cortex-M4 r0p1 (`__CM4_REV`).
            0xE000_ED04 => self.icsr(ipsr),
            0xE000_ED08 => self.vtor,
            0xE000_ED0C => 0xFA05_0000 | (self.aircr_prigroup as u32) << 8,
            0xE000_ED10 => self.scr,
            0xE000_ED14 => self.ccr,
            0xE000_ED18..=0xE000_ED23 => {
                let base = 4 + ((addr - 0xE000_ED18) as usize & !3);
                let mut v = 0;
                for b in 0..4 {
                    v |= (self.prio[base + b] as u32) << (8 * b);
                }
                v
            }
            0xE000_ED24 => self.shcsr(),
            0xE000_ED28 => self.cfsr,
            0xE000_ED2C => self.hfsr,
            0xE000_ED30 => 0, // DFSR
            0xE000_ED34 => self.mmfar,
            0xE000_ED38 => self.bfar,
            0xE000_ED3C => 0, // AFSR
            // Feature registers, values from DDI 0439B §4.3 (Cortex-M4 r0p1).
            0xE000_ED40 => 0x0000_0030,
            0xE000_ED44 => 0x0000_0200,
            0xE000_ED48 => 0x0010_0000,
            0xE000_ED4C => 0,
            0xE000_ED50 => 0x0010_0030,
            0xE000_ED54 => 0,
            0xE000_ED58 => 0x0100_0000,
            0xE000_ED5C => 0,
            0xE000_ED60 => 0x0110_1110,
            0xE000_ED64 => 0x0211_1000,
            0xE000_ED68 => 0x2111_2231,
            0xE000_ED6C => 0x0111_1110,
            0xE000_ED70 => 0x0131_0132,
            0xE000_ED88 => self.cpacr,
            0xE000_ED90..=0xE000_EDB8 => self.mpu.read(addr),
            0xE000_EDF0 => self.debugger_attached as u32, // DHCSR: C_DEBUGEN only.
            0xE000_EDFC => self.demcr,
            0xE000_EF34 => self.fpccr,
            0xE000_EF38 => self.fpcar,
            0xE000_EF3C => self.fpdscr,
            0xE000_EF40 => 0x1011_0021, // MVFR0: single precision, square root, divide.
            0xE000_EF44 => 0x1100_0011, // MVFR1: FTZ, D_NaN, FP fused MAC, half precision.
            _ => return unimpl,
        })
    }

    /// Write to the PPB.
    pub fn write(&mut self, addr: u32, v: u32, now: u64) -> Result<PpbEffect, PpbUnimplemented> {
        let mut effect = PpbEffect::ExceptionStateChanged;
        match addr {
            0xE000_1000 => {
                let was = self.dwt_ctrl & 1 != 0;
                let cur = self.cyccnt(now);
                self.dwt_ctrl = (self.dwt_ctrl & 0xF000_0000) | (v & 0x0FFF_FFFF);
                if was && self.dwt_ctrl & 1 == 0 {
                    self.dwt_cyc_frozen = cur;
                } else if !was && self.dwt_ctrl & 1 != 0 {
                    self.dwt_cyc_base = now.wrapping_sub(self.dwt_cyc_frozen as u64);
                }
                effect = PpbEffect::None;
            }
            0xE000_1004 => {
                self.dwt_cyc_frozen = v;
                self.dwt_cyc_base = now.wrapping_sub(v as u64);
                effect = PpbEffect::None;
            }
            0xE000_E008 => self.actlr = v & 0x307,
            0xE000_E010 => {
                self.systick.ctrl = v & 0x7;
                if v & 1 != 0 && self.systick.next_wrap.is_none() {
                    self.systick.loaded_at = now;
                }
                self.systick.reschedule();
            }
            0xE000_E014 => self.systick.reload = v & 0x00FF_FFFF,
            0xE000_E018 => {
                // Any write clears the counter and COUNTFLAG; the next tick reloads.
                self.systick.countflag = false;
                self.systick.loaded_at = now + self.systick.divider();
                self.systick.reschedule();
            }
            0xE000_E100..=0xE000_E13C => self.set_irq_bits(addr - 0xE000_E100, v, |s, i| s.irq_enabled[i] = true),
            0xE000_E180..=0xE000_E1BC => self.set_irq_bits(addr - 0xE000_E180, v, |s, i| s.irq_enabled[i] = false),
            0xE000_E200..=0xE000_E23C => self.set_irq_bits(addr - 0xE000_E200, v, |s, i| s.pending[16 + i] = true),
            0xE000_E280..=0xE000_E2BC => self.set_irq_bits(addr - 0xE000_E280, v, |s, i| s.pending[16 + i] = false),
            0xE000_E300..=0xE000_E33C => effect = PpbEffect::None, // IABR is read-only.
            0xE000_E400..=0xE000_E5EF => {
                // Word writes only here; byte writes go through `write_byte`.
                let base = (addr - 0xE000_E400) as usize & !3;
                for b in 0..4 {
                    if let Some(p) = self.prio.get_mut(16 + base + b) {
                        *p = (v >> (8 * b)) as u8 & PRIO_MASK;
                    }
                }
            }
            0xE000_ED04 => self.write_icsr(v),
            0xE000_ED08 => {
                // TBLOFF[29:7] (DDI 0439B §4.3.4: VTOR bits [6:0] are reserved).
                self.vtor = v & 0x3FFF_FF80;
                effect = PpbEffect::None;
            }
            0xE000_ED0C => {
                if v >> 16 != 0x05FA {
                    return Ok(PpbEffect::None);
                }
                self.aircr_prigroup = ((v >> 8) & 7) as u8;
                if v & (1 << 2) != 0 {
                    return Ok(PpbEffect::ResetRequest);
                }
                if v & 1 != 0 {
                    // VECTRESET is for debug use only and UNPREDICTABLE with SYSRESETREQ.
                    return Ok(PpbEffect::ResetRequest);
                }
            }
            0xE000_ED10 => {
                self.scr = v & 0x16;
                effect = PpbEffect::None;
            }
            0xE000_ED14 => {
                // STKALIGN is configurable on the M4; NONBASETHRDENA, USERSETMPEND,
                // UNALIGN_TRP, DIV_0_TRP, BFHFNMIGN, STKALIGN.
                self.ccr = v & 0x31B;
                effect = PpbEffect::None;
            }
            0xE000_ED18..=0xE000_ED23 => {
                let base = 4 + ((addr - 0xE000_ED18) as usize & !3);
                for b in 0..4 {
                    let n = base + b;
                    // Only MemManage, BusFault, UsageFault, SVCall, DebugMon, PendSV, SysTick.
                    if matches!(n, 4..=6 | 11 | 12 | 14 | 15) {
                        self.prio[n] = (v >> (8 * b)) as u8 & PRIO_MASK;
                    }
                }
            }
            0xE000_ED24 => self.write_shcsr(v),
            0xE000_ED28 => {
                self.cfsr &= !v;
                effect = PpbEffect::None;
            }
            0xE000_ED2C => {
                self.hfsr &= !v;
                effect = PpbEffect::None;
            }
            0xE000_ED30 | 0xE000_ED3C => effect = PpbEffect::None,
            0xE000_ED34 => {
                self.mmfar = v;
                effect = PpbEffect::None;
            }
            0xE000_ED38 => {
                self.bfar = v;
                effect = PpbEffect::None;
            }
            0xE000_ED88 => {
                // Only CP10/CP11 are implemented.
                self.cpacr = v & 0x00F0_0000;
                effect = PpbEffect::None;
            }
            0xE000_ED90..=0xE000_EDB8 => {
                self.mpu.write(addr, v);
                effect = PpbEffect::MpuChanged;
            }
            0xE000_EDF0 | 0xE000_EDF4 | 0xE000_EDF8 => effect = PpbEffect::None, // Debug halting: no debugger.
            0xE000_EDFC => {
                self.demcr = v & 0x010F_07F1;
                effect = PpbEffect::None;
            }
            0xE000_EF00 => {
                // STIR: software trigger of an external interrupt.
                let irq = (v & 0x1FF) as usize;
                if irq < NUM_IRQ {
                    self.pending[16 + irq] = true;
                }
            }
            0xE000_EF34 => {
                // ASPEN, LSPEN, MONRDY, BFRDY, MMRDY, HFRDY, THREAD, USER, LSPACT (B3.2.21).
                self.fpccr = v & 0xC000_017B;
                effect = PpbEffect::None;
            }
            0xE000_EF38 => {
                self.fpcar = v & !7;
                effect = PpbEffect::None;
            }
            0xE000_EF3C => {
                self.fpdscr = v & 0x07C0_0000;
                effect = PpbEffect::None;
            }
            _ => return Err(PpbUnimplemented { addr, write: true }),
        }
        Ok(effect)
    }

    /// Byte/halfword writes: the NVIC priority registers and SHPR are byte-accessible.
    pub fn write_sized(&mut self, addr: u32, v: u32, bytes: u32, now: u64) -> Result<PpbEffect, PpbUnimplemented> {
        if bytes == 4 {
            return self.write(addr, v, now);
        }
        let byte_prio = |a: u32| -> Option<usize> {
            match a {
                0xE000_E400..=0xE000_E5EF => Some(16 + (a - 0xE000_E400) as usize),
                0xE000_ED18..=0xE000_ED23 => Some(4 + (a - 0xE000_ED18) as usize),
                _ => None,
            }
        };
        if byte_prio(addr).is_some() {
            for i in 0..bytes {
                if let Some(n) = byte_prio(addr + i) {
                    let p = (v >> (8 * i)) as u8 & PRIO_MASK;
                    if (16..NUM_EXC).contains(&n) || matches!(n, 4..=6 | 11 | 12 | 14 | 15) {
                        self.prio[n] = p;
                    }
                }
            }
            return Ok(PpbEffect::ExceptionStateChanged);
        }
        // Other registers: merge into the containing word (read-modify-write).
        let word = addr & !3;
        let shift = (addr & 3) * 8;
        let mask = (if bytes == 1 { 0xFFu32 } else { 0xFFFF }) << shift;
        let old = self
            .read(word, now, 0)
            .map_err(|_| PpbUnimplemented { addr, write: true })?;
        self.write(word, (old & !mask) | ((v << shift) & mask), now)
    }

    pub fn read_sized(&mut self, addr: u32, bytes: u32, now: u64, ipsr: u16) -> Result<u32, PpbUnimplemented> {
        let w = self.read(addr & !3, now, ipsr)?;
        let v = w >> ((addr & 3) * 8);
        Ok(match bytes {
            1 => v & 0xFF,
            2 => v & 0xFFFF,
            _ => w,
        })
    }

    fn irq_bits(&self, off: u32, f: impl Fn(&Self, usize) -> bool) -> u32 {
        let base = (off as usize / 4) * 32;
        let mut v = 0;
        for b in 0..32 {
            let i = base + b;
            if i < NUM_IRQ && f(self, i) {
                v |= 1 << b;
            }
        }
        v
    }

    fn set_irq_bits(&mut self, off: u32, v: u32, f: impl Fn(&mut Self, usize)) {
        let base = (off as usize / 4) * 32;
        for b in 0..32 {
            let i = base + b;
            if i < NUM_IRQ && v & (1 << b) != 0 {
                f(self, i);
            }
        }
    }

    fn icsr(&self, ipsr: u16) -> u32 {
        let pending = self.highest_pending();
        let mut v = ipsr as u32 & 0x1FF;
        // RETTOBASE: no other active exception besides the current one.
        if ipsr != 0 && self.active_count() <= 1 {
            v |= 1 << 11;
        }
        v |= (pending as u32 & 0x1FF) << 12;
        if (16..NUM_EXC).any(|n| self.pending[n]) {
            v |= 1 << 22; // ISRPENDING
        }
        if self.pending[EXC_SYSTICK] {
            v |= 1 << 26;
        }
        if self.pending[EXC_PENDSV] {
            v |= 1 << 28;
        }
        if self.pending[EXC_NMI] {
            v |= 1 << 31;
        }
        v
    }

    fn write_icsr(&mut self, v: u32) {
        if v & (1 << 31) != 0 {
            self.pending[EXC_NMI] = true;
        }
        if v & (1 << 28) != 0 {
            self.pending[EXC_PENDSV] = true;
        } else if v & (1 << 27) != 0 {
            self.pending[EXC_PENDSV] = false;
        }
        if v & (1 << 26) != 0 {
            self.pending[EXC_SYSTICK] = true;
        } else if v & (1 << 25) != 0 {
            self.pending[EXC_SYSTICK] = false;
        }
    }

    fn shcsr(&self) -> u32 {
        let mut v = self.shcsr_enables;
        let bits = [
            (EXC_MEMMANAGE, 0, false),
            (EXC_BUSFAULT, 1, false),
            (EXC_USAGEFAULT, 3, false),
            (EXC_SVCALL, 7, false),
            (EXC_DEBUGMON, 8, false),
            (EXC_PENDSV, 10, false),
            (EXC_SYSTICK, 11, false),
            (EXC_USAGEFAULT, 12, true),
            (EXC_MEMMANAGE, 13, true),
            (EXC_BUSFAULT, 14, true),
            (EXC_SVCALL, 15, true),
        ];
        for (n, bit, pending) in bits {
            let set = if pending { self.pending[n] } else { self.active[n] };
            if set {
                v |= 1 << bit;
            }
        }
        v
    }

    fn write_shcsr(&mut self, v: u32) {
        self.shcsr_enables = v & 0x0007_0000;
        // Active/pending bits are writable (DDI 0403E B3.2.13); software rarely uses this.
        self.active[EXC_MEMMANAGE] = v & (1 << 0) != 0;
        self.active[EXC_BUSFAULT] = v & (1 << 1) != 0;
        self.active[EXC_USAGEFAULT] = v & (1 << 3) != 0;
        self.active[EXC_SVCALL] = v & (1 << 7) != 0;
        self.active[EXC_DEBUGMON] = v & (1 << 8) != 0;
        self.active[EXC_PENDSV] = v & (1 << 10) != 0;
        self.active[EXC_SYSTICK] = v & (1 << 11) != 0;
        self.pending[EXC_USAGEFAULT] = v & (1 << 12) != 0;
        self.pending[EXC_MEMMANAGE] = v & (1 << 13) != 0;
        self.pending[EXC_BUSFAULT] = v & (1 << 14) != 0;
        self.pending[EXC_SVCALL] = v & (1 << 15) != 0;
    }
}

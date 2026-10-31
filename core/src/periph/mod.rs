// SPDX-License-Identifier: GPL-3.0-or-later
//! STM32WB55 peripherals as seen by CPU1.
//!
//! Every peripheral in the SVD gets a register file with real reset values and write masks
//! (ADR-0010). Peripherals with a behavioural model (RCC, …) add side effects on top; for
//! all others, accesses are latched and reported in the digest as "register file only".
//! Offsets with no register at all keep read-as-zero / write-ignored semantics (ADR-0009).

pub mod exti;
pub mod gpio;
pub mod i2c;
pub mod irq;
pub mod rcc;
pub mod regfile;
pub mod rtc;
pub mod spi;
#[rustfmt::skip]
pub mod svd;
pub mod usart;

use crate::cpu::Size;
use crate::trace::digest::Digest;
use regfile::{PeriphDesc, RegFile};

/// Why the system is being reset (selects which state survives).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResetKind {
    /// Power-on/brown-out: everything resets, including the backup domain.
    PowerOn,
    /// SYSRESETREQ from software.
    Software,
    /// Independent watchdog timeout.
    Iwdg,
}

/// Clock frequencies derived from the RCC configuration, in Hz.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Clocks {
    pub sysclk: u32,
    /// HCLK1: CPU1, AHB1/2/3, SysTick (processor clock).
    pub hclk1: u32,
    pub pclk1: u32,
    pub pclk2: u32,
}

/// Per-access context handed to peripheral models.
pub struct Ctx<'a> {
    pub now: u64,
    pub digest: &'a mut Digest,
    pub pc: u32,
}

/// Core peripherals in the SVD that live in the CPU's PPB model, not on the bus.
const CORE_BLOCKS: &[&str] = &[
    "SCB_ACTRL",
    "STK",
    "NVIC",
    "SCB",
    "FPU_CPACR",
    "MPU",
    "NVIC_STIR",
    "FPU",
];

/// Which model handles a block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Slot {
    Rcc,
    Gpio(usize),
    Exti,
    I2c(usize),
    Spi(usize),
    Usart(usize),
    Rtc,
    Generic(usize),
}

pub struct Periphs {
    pub rcc: rcc::Rcc,
    /// GPIOA, GPIOB, GPIOC, GPIOD, GPIOE, GPIOH.
    pub gpio: Vec<gpio::Gpio>,
    pub exti: exti::Exti,
    /// I2C1 (power bus: fuel gauge, charger, LED driver), I2C3 (external).
    pub i2c: Vec<i2c::I2c>,
    /// SPI1 (radio, NFC, external), SPI2 (display, SD card).
    pub spi: Vec<spi::Spi>,
    /// USART1 (Furi log / CLI), LPUART1.
    pub usart: Vec<usart::Usart>,
    pub rtc: rtc::Rtc,
    /// Register-file-only peripherals.
    pub generic: Vec<RegFile>,
    /// (base, end, slot), sorted by base.
    map: Vec<(u32, u32, Slot)>,
    pub clocks: Clocks,
    /// NVIC input levels, bit n = IRQ n.
    pub irq_lines: u64,
    pub irq_dirty: bool,
    /// Earliest cycle at which some peripheral has an event (timers, RTC, …).
    pub next_event: Option<u64>,
    /// Virtual time base: `tb_ns` nanoseconds at cycle `tb_cycles`, HCLK1 = `tb_hz` since.
    tb_cycles: u64,
    tb_ns: u128,
}

impl Default for Periphs {
    fn default() -> Self {
        Self::new()
    }
}

impl Periphs {
    pub fn new() -> Self {
        let mut descs: Vec<&'static PeriphDesc> = svd::ALL.iter().filter(|d| !CORE_BLOCKS.contains(&d.name)).collect();
        descs.sort_by_key(|d| d.base);
        let (mut gpio, mut i2c, mut spi, mut usart, mut generic) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let mut map = Vec::new();
        for (i, d) in descs.iter().enumerate() {
            // A block extends to the next block or its last register, within the 1 KiB stride.
            let last = d.regs.iter().map(|r| r.offset as u32 + 4).max().unwrap_or(4);
            let next = descs.get(i + 1).map(|n| n.base).unwrap_or(u32::MAX);
            let end = (d.base + last.max(0x400)).min(next);
            let slot = match d.name {
                "RCC" => Slot::Rcc,
                "EXTI" => Slot::Exti,
                n if n.starts_with("GPIO") => {
                    gpio.push(gpio::Gpio::new(d));
                    Slot::Gpio(gpio.len() - 1)
                }
                "I2C1" | "I2C3" => {
                    i2c.push(i2c::I2c::new(d));
                    Slot::I2c(i2c.len() - 1)
                }
                "SPI1" | "SPI2" => {
                    spi.push(spi::Spi::new(d));
                    Slot::Spi(spi.len() - 1)
                }
                "USART1" | "LPUART1" => {
                    usart.push(usart::Usart::new(d));
                    Slot::Usart(usart.len() - 1)
                }
                "RTC" => Slot::Rtc,
                _ => {
                    generic.push(RegFile::new(d));
                    Slot::Generic(generic.len() - 1)
                }
            };
            map.push((d.base, end, slot));
        }
        let rcc = rcc::Rcc::new();
        let clocks = rcc.clocks();
        Periphs {
            rcc,
            gpio,
            exti: exti::Exti::new(),
            i2c,
            spi,
            usart,
            rtc: rtc::Rtc::new(),
            generic,
            map,
            clocks,
            irq_lines: 0,
            irq_dirty: false,
            next_event: None,
            tb_cycles: 0,
            tb_ns: 0,
        }
    }

    /// Virtual time in nanoseconds at CPU cycle `cycles` (cycles are HCLK1 cycles).
    pub fn now_ns(&self, cycles: u64) -> u128 {
        let hz = self.clocks.hclk1.max(1) as u128;
        self.tb_ns + (cycles.saturating_sub(self.tb_cycles) as u128) * 1_000_000_000 / hz
    }

    /// Re-anchors the time base before HCLK1 changes.
    fn retime(&mut self, now: u64, new: Clocks) {
        if new.hclk1 != self.clocks.hclk1 {
            self.tb_ns = self.now_ns(now);
            self.tb_cycles = now;
        }
        self.clocks = new;
    }

    pub fn reset(&mut self, kind: ResetKind, now: u64) {
        let ns = self.now_ns(now);
        self.tb_ns = ns;
        self.tb_cycles = now;
        self.rcc.reset(kind);
        self.usart.iter_mut().for_each(|u| u.reset());
        if kind == ResetKind::PowerOn {
            // The RTC is in the backup domain: only power-on (or BDRST) resets it.
            self.rtc.reset(ns);
        }
        self.gpio.iter_mut().for_each(|g| g.reset());
        self.exti.reset();
        self.i2c.iter_mut().for_each(|i| i.reset());
        self.spi.iter_mut().for_each(|s| s.reset());
        self.generic.iter_mut().for_each(|g| g.reset());
        self.clocks = self.rcc.clocks();
        self.update_irqs();
    }

    /// Runs peripheral events that are due and recomputes `next_event`.
    pub fn advance(&mut self, _ctx: &mut Ctx) {
        self.next_event = None;
        self.update_irqs();
    }

    fn update_irqs(&mut self) {
        let mut lines = self.exti.irq_lines();
        if self.usart[0].irq() {
            lines |= 1 << irq::USART1;
        }
        if self.usart[1].irq() {
            lines |= 1 << irq::LPUART1;
        }
        if lines != self.irq_lines {
            self.irq_lines = lines;
        }
        self.irq_dirty = true;
    }

    #[inline]
    fn slot(&self, addr: u32) -> Option<(u32, Slot)> {
        let i = self.map.partition_point(|m| m.0 <= addr);
        let &(base, end, slot) = self.map.get(i.checked_sub(1)?)?;
        (addr < end).then_some((base, slot))
    }

    fn regfile(&self, slot: Slot) -> &RegFile {
        match slot {
            Slot::Rcc => &self.rcc.regs,
            Slot::Gpio(i) => &self.gpio[i].regs,
            Slot::Exti => &self.exti.regs,
            Slot::I2c(i) => &self.i2c[i].regs,
            Slot::Spi(i) => &self.spi[i].regs,
            Slot::Usart(i) => &self.usart[i].regs,
            Slot::Rtc => &self.rtc.regs,
            Slot::Generic(i) => &self.generic[i],
        }
    }

    /// Reads a peripheral register; `None` if `addr` is not in a peripheral block.
    pub fn read(&mut self, addr: u32, size: Size, ctx: &mut Ctx) -> Option<u32> {
        let (base, slot) = self.slot(addr)?;
        let off = addr - base;
        let word_off = off & !3;
        let word = match slot {
            Slot::Rcc => self.rcc.read(word_off, ctx, addr),
            Slot::Gpio(i) => self.gpio[i].read(word_off),
            Slot::Exti => self.exti.regs.read(word_off),
            Slot::I2c(i) => {
                let v = self.i2c[i].read(word_off);
                self.irq_dirty = true;
                v
            }
            Slot::Usart(i) => {
                let v = self.usart[i].read(word_off);
                self.update_irqs();
                v
            }
            Slot::Rtc => {
                let ns = self.now_ns(ctx.now);
                self.rtc.read(word_off, ns)
            }
            // SPI DR reads pop frames from the FIFO: the access size matters.
            Slot::Spi(i) => return Some(self.spi[i].read(off, size).unwrap_or(0)),
            Slot::Generic(i) => {
                let v = self.generic[i].read(word_off);
                if v.is_some() {
                    ctx.digest.latched(addr, false, None, ctx.pc);
                }
                v
            }
        };
        let Some(word) = word else {
            ctx.digest.unimplemented(addr, false, None, ctx.pc);
            return Some(0);
        };
        let shift = (off & 3) * 8;
        Some(match size {
            Size::Byte => (word >> shift) & 0xFF,
            Size::Half => (word >> shift) & 0xFFFF,
            Size::Word => word,
        })
    }

    /// Writes a peripheral register; `false` if `addr` is not in a peripheral block.
    pub fn write(&mut self, addr: u32, value: u32, size: Size, ctx: &mut Ctx) -> bool {
        let Some((base, slot)) = self.slot(addr) else {
            return false;
        };
        let off = addr - base;
        let word_off = off & !3;
        if let Slot::Spi(i) = slot {
            if !self.spi[i].write(off, value, size) {
                ctx.digest.unimplemented(addr, true, Some(value), ctx.pc);
            }
            return true;
        }
        // Sub-word writes update their byte lanes of the stored word.
        let value = match size {
            Size::Word => value,
            _ => {
                let shift = (off & 3) * 8;
                let mask = if size == Size::Byte { 0xFFu32 } else { 0xFFFF } << shift;
                let old = self.regfile(slot).get(word_off);
                (old & !mask) | ((value << shift) & mask)
            }
        };
        let handled = match slot {
            Slot::Rcc => {
                let h = self.rcc.write(word_off, value, ctx, addr);
                let clocks = self.rcc.clocks();
                self.retime(ctx.now, clocks);
                if self.rcc.backup_domain_reset_requested() {
                    let ns = self.now_ns(ctx.now);
                    self.rtc.reset(ns);
                }
                h
            }
            Slot::Usart(i) => {
                let h = self.usart[i].write(word_off, value);
                self.update_irqs();
                h
            }
            Slot::Rtc => {
                let ns = self.now_ns(ctx.now);
                self.rtc.write(word_off, value, ns)
            }
            Slot::Gpio(i) => self.gpio[i].write(word_off, value),
            Slot::Exti => {
                let h = self.exti.write(word_off, value);
                self.update_irqs();
                h
            }
            Slot::I2c(i) => {
                let h = self.i2c[i].write(word_off, value);
                self.irq_dirty = true;
                h
            }
            Slot::Spi(_) => unreachable!(),
            Slot::Generic(i) => {
                let ok = self.generic[i].write(word_off, value);
                if ok {
                    ctx.digest.latched(addr, true, Some(value), ctx.pc);
                }
                ok
            }
        };
        if !handled {
            ctx.digest.unimplemented(addr, true, Some(value), ctx.pc);
        }
        true
    }

    /// Hardware fidelity table (BRIEF §5.4 "status registry").
    pub fn fidelity(&self) -> Vec<(&'static str, crate::bus::Fidelity, &'static str)> {
        use crate::bus::Fidelity::*;
        let mut v = vec![
            ("RCC", rcc::FIDELITY.0, rcc::FIDELITY.1),
            ("GPIO", Partial, "pin levels from ODR/pulls; board inputs from M3"),
            ("EXTI", Partial, "pending/mask/software trigger; GPIO edges from M3"),
            ("I2C1/I2C3", Partial, "master transfers, instant; absent devices NACK"),
            (
                "SPI1/SPI2",
                Partial,
                "master transfers with FIFO, instant; MISO 0xFF without device",
            ),
            (
                "USART1/LPUART1",
                Partial,
                "instant TX captured as log, RX from host; no DMA",
            ),
            (
                "RTC",
                Partial,
                "calendar, init mode, write protection, backup registers; no alarms/wakeup",
            ),
        ];
        for g in &self.generic {
            v.push((
                g.desc.name,
                Stubbed,
                "register file only (reset values, latched writes)",
            ));
        }
        v
    }
}

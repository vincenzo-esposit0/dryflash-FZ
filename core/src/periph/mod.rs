// SPDX-License-Identifier: GPL-3.0-or-later
//! STM32WB55 peripherals as seen by CPU1.
//!
//! Every peripheral in the SVD gets a register file with real reset values and write masks
//! (ADR-0010). Peripherals with a behavioural model (RCC, …) add side effects on top; for
//! all others, accesses are latched and reported in the digest as "register file only".
//! Offsets with no register at all keep read-as-zero / write-ignored semantics (ADR-0009).

pub mod dma;
pub mod exti;
pub mod flash;
pub mod gpio;
pub mod hsem;
pub mod i2c;
pub mod ipcc;
pub mod irq;
pub mod iwdg;
pub mod lptim;
pub mod rcc;
pub mod regfile;
pub mod rng;
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
    Hsem,
    Ipcc,
    Dma(usize),
    Flash,
    Rng,
    Iwdg,
    Lptim(usize),
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
    pub flash: flash::Flash,
    /// DMA1, DMA2.
    pub dma: Vec<dma::Dma>,
    /// USART1 (Furi log / CLI), LPUART1.
    pub usart: Vec<usart::Usart>,
    pub rtc: rtc::Rtc,
    pub hsem: hsem::Hsem,
    pub ipcc: ipcc::Ipcc,
    pub rng: rng::Rng,
    pub iwdg: iwdg::Iwdg,
    /// LPTIM1 (Furi's tickless idle timer), LPTIM2.
    pub lptim: Vec<lptim::Lptim>,
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
    /// LCD controller on SPI2 (board wiring in `crate::board`).
    pub display: crate::board::st7567::St7567,
    /// GPIO levels after the last update, per port (edge detection for EXTI and board).
    last_levels: [u16; 6],
    /// Index of SYSCFG in `generic` (EXTICRx select the GPIO port of each EXTI line).
    syscfg: Option<usize>,
    /// microSD card, if inserted.
    pub sd: Option<crate::board::sdcard::SdCard>,
    /// Buttons currently held by the user.
    pub buttons_down: Vec<crate::board::Button>,
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
        let (mut gpio, mut i2c, mut generic) = (Vec::new(), Vec::new(), Vec::new());
        let (mut spi, mut usart, mut dma) = ([None, None], [None, None], [None, None]);
        let mut lptim = Vec::new();
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
                // Fixed indices (SPI1 = 0, SPI2 = 1; USART1 = 0, LPUART1 = 1): blocks are
                // visited by address, which would put SPI2 and LPUART1 first.
                "SPI1" | "SPI2" => {
                    let i = (d.name == "SPI2") as usize;
                    spi[i] = Some(spi::Spi::new(d));
                    Slot::Spi(i)
                }
                "USART1" | "LPUART1" => {
                    let i = (d.name == "LPUART1") as usize;
                    usart[i] = Some(usart::Usart::new(d));
                    Slot::Usart(i)
                }
                "RTC" => Slot::Rtc,
                "HSEM" => Slot::Hsem,
                "IPCC" => Slot::Ipcc,
                "Flash" => Slot::Flash,
                "DMA1" | "DMA2" => {
                    let i = (d.name == "DMA2") as usize;
                    dma[i] = Some(dma::Dma::new(d));
                    Slot::Dma(i)
                }
                "RNG" => Slot::Rng,
                "IWDG" => Slot::Iwdg,
                "LPTIM1" | "LPTIM2" => {
                    lptim.push(lptim::Lptim::new(d));
                    Slot::Lptim(lptim.len() - 1)
                }
                _ => {
                    generic.push(RegFile::new(d));
                    Slot::Generic(generic.len() - 1)
                }
            };
            map.push((d.base, end, slot));
        }
        // Board wiring (targets/f7/furi_hal/furi_hal_i2c_config.c): I2C1 is the power bus.
        i2c[0].devices.push(Box::new(crate::board::bq27220::Bq27220::new()));
        i2c[0].devices.push(Box::new(crate::board::bq25896::Bq25896::new()));
        i2c[0].devices.push(Box::new(crate::board::lp5562::Lp5562::new()));
        let rcc = rcc::Rcc::new();
        let clocks = rcc.clocks();
        let syscfg = generic.iter().position(|g: &RegFile| g.desc.name == "SYSCFG_VREFBUF");
        Periphs {
            rcc,
            gpio,
            exti: exti::Exti::new(),
            i2c,
            spi: spi.into_iter().map(|s| s.expect("SPI1 and SPI2 in the SVD")).collect(),
            flash: flash::Flash::new(),
            dma: dma.into_iter().map(|d| d.expect("DMA1 and DMA2 in the SVD")).collect(),
            usart: usart
                .into_iter()
                .map(|u| u.expect("USART1 and LPUART1 in the SVD"))
                .collect(),
            rtc: rtc::Rtc::new(),
            hsem: hsem::Hsem::new(),
            ipcc: ipcc::Ipcc::new(),
            rng: rng::Rng::new(rng::DEFAULT_SEED),
            iwdg: iwdg::Iwdg::new(),
            lptim,
            generic,
            map,
            clocks,
            irq_lines: 0,
            irq_dirty: false,
            next_event: None,
            tb_cycles: 0,
            tb_ns: 0,
            syscfg,
            display: crate::board::st7567::St7567::new(),
            last_levels: [0; 6],
            sd: None,
            buttons_down: Vec::new(),
        }
    }

    /// SPI register write, with the board wiring behind the data register. SPI2 carries the
    /// display and the SD card, selected by their chip selects; SPI1 devices (radio, NFC) are not
    /// attached yet, so MISO reads 0xFF (pulled up).
    pub fn spi_write(&mut self, i: usize, off: u32, value: u32, size: Size) -> bool {
        let Periphs {
            spi, gpio, display, sd, ..
        } = self;
        let mut route = |mosi: u8| -> u8 {
            if i != 1 {
                return 0xFF;
            }
            let low = |(port, pin): (usize, u8)| gpio[port].driven_low(pin);
            if low(crate::board::DISPLAY_CS) {
                let (port, pin) = crate::board::DISPLAY_A0;
                display.write(mosi, gpio[port].levels() & (1 << pin) != 0);
            }
            match sd {
                Some(card) if low(crate::board::SD_CS) => card.transfer(mosi),
                _ => 0xFF,
            }
        };
        spi[i].write(off, value, size, &mut route)
    }

    /// Inserts a card: the card-detect switch pulls SD_CD low.
    pub fn insert_sd(&mut self, card: crate::board::sdcard::SdCard) {
        self.sd = Some(card);
        self.drive_sd_detect();
        self.gpio_changed();
    }

    /// Removes the card and returns it.
    pub fn remove_sd(&mut self) -> Option<crate::board::sdcard::SdCard> {
        let card = self.sd.take();
        self.drive_sd_detect();
        self.gpio_changed();
        card
    }

    fn drive_sd_detect(&mut self) {
        let (port, pin) = crate::board::SD_CD;
        self.gpio[port].drive(pin, self.sd.is_some().then_some(false));
    }

    fn levels(&self) -> [u16; 6] {
        let mut l = [0u16; 6];
        for (i, g) in self.gpio.iter().enumerate() {
            l[i] = g.levels();
        }
        l
    }

    /// Reacts to GPIO level changes: EXTI edges (port selected by SYSCFG_EXTICRx) and board
    /// signals (display reset on a falling RST edge).
    fn gpio_changed(&mut self) {
        let now = self.levels();
        if now == self.last_levels {
            return;
        }
        let syscfg = self.syscfg.map(|i| {
            let g = &self.generic[i];
            [g.get(0x08), g.get(0x0C), g.get(0x10), g.get(0x14)]
        });
        for port in 0..6 {
            let changed = now[port] ^ self.last_levels[port];
            if changed == 0 {
                continue;
            }
            // EXTICR port codes: 0 PA, 1 PB, 2 PC, 3 PD, 4 PE, 7 PH.
            let code = [0u32, 1, 2, 3, 4, 7][port];
            for line in 0..16u32 {
                if changed & (1 << line) == 0 {
                    continue;
                }
                let sel = syscfg
                    .map(|r| (r[(line / 4) as usize] >> ((line % 4) * 4)) & 7)
                    .unwrap_or(0);
                if sel == code {
                    self.exti.edge(line, now[port] & (1 << line) != 0);
                }
            }
        }
        let (cp, cpin) = crate::board::SD_CS;
        if now[cp] & !self.last_levels[cp] & (1 << cpin) != 0
            && let Some(card) = &mut self.sd
        {
            card.deselect();
        }
        let (rp, rpin) = crate::board::DISPLAY_RST;
        if self.last_levels[rp] & (1 << rpin) != 0 && now[rp] & (1 << rpin) == 0 {
            self.display.reset();
        }
        self.last_levels = now;
        self.update_irqs();
    }

    fn drive_button(&mut self, b: crate::board::Button, pressed: bool) {
        let (port, pin, active_high) = b.pin();
        // Pressed: the button drives its active level; released: only the pull remains.
        self.gpio[port].drive(pin, pressed.then_some(active_high));
    }

    /// Presses or releases a button (user input).
    pub fn set_button(&mut self, b: crate::board::Button, pressed: bool) {
        self.buttons_down.retain(|x| *x != b);
        if pressed {
            self.buttons_down.push(b);
        }
        self.drive_button(b, pressed);
        self.gpio_changed();
    }

    /// LED and backlight levels at virtual time `now` (cycles).
    pub fn lights(&mut self, now: u64) -> crate::board::lp5562::Lights {
        let ns = self.now_ns(now);
        match self.i2c[0].device_mut::<crate::board::lp5562::Lp5562>() {
            Some(l) => {
                l.now_ns = ns;
                l.lights()
            }
            None => Default::default(),
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
        self.hsem.reset();
        self.ipcc.reset();
        self.dma.iter_mut().for_each(|d| d.reset());
        self.flash.reset();
        self.rng.reset();
        self.iwdg.reset();
        self.lptim.iter_mut().for_each(|l| l.reset());
        if kind == ResetKind::PowerOn {
            // The RTC is in the backup domain: only power-on (or BDRST) resets it.
            self.rtc.reset(ns);
        }
        self.gpio.iter_mut().for_each(|g| g.reset());
        // Held buttons keep driving their pins across resets.
        for b in self.buttons_down.clone() {
            self.drive_button(b, true);
        }
        self.drive_sd_detect();
        self.last_levels = self.levels();
        self.exti.reset();
        self.i2c.iter_mut().for_each(|i| i.reset());
        self.spi.iter_mut().for_each(|s| s.reset());
        self.generic.iter_mut().for_each(|g| g.reset());
        self.clocks = self.rcc.clocks();
        self.update_irqs();
    }

    /// Runs peripheral events that are due and recomputes `next_event`.
    pub fn advance(&mut self, ctx: &mut Ctx) {
        let ns = self.now_ns(ctx.now);
        for l in &mut self.lptim {
            l.advance(ns);
        }
        self.update_irqs();
        self.schedule();
    }

    /// CPU cycle at which virtual time reaches `ns` (rounded up).
    fn cycles_at(&self, ns: u128) -> u64 {
        let hz = self.clocks.hclk1.max(1) as u128;
        let d = ns.saturating_sub(self.tb_ns);
        self.tb_cycles + (d * hz).div_ceil(1_000_000_000) as u64
    }

    /// Recomputes the earliest peripheral event.
    fn schedule(&mut self) {
        let ns = self.lptim.iter().filter_map(|l| l.next_event_ns()).min();
        self.next_event = ns.map(|n| self.cycles_at(n));
    }

    /// LPTIM kernel clocks from RCC_CCIPR.LPTIMxSEL: PCLK1, LSI, HSI16, LSE (RM0434 RCC).
    fn lptim_clocks(&mut self) {
        let ccipr = self.rcc.regs.get(rcc::CCIPR);
        for (i, shift) in [(0usize, 18u32), (1, 20)] {
            self.lptim[i].clock_hz = match (ccipr >> shift) & 3 {
                0 => self.clocks.pclk1,
                1 => 32_000,
                2 => 16_000_000,
                _ => 32_768,
            };
        }
    }

    fn update_irqs(&mut self) {
        let mut lines = self.exti.irq_lines();
        if self.usart[0].irq() {
            lines |= 1 << irq::USART1;
        }
        if self.usart[1].irq() {
            lines |= 1 << irq::LPUART1;
        }
        lines |= (self.dma[0].irq_channels() as u64) << irq::DMA1_CH1;
        lines |= (self.dma[1].irq_channels() as u64) << irq::DMA2_CH1;
        if self.hsem.irq() {
            lines |= 1 << irq::HSEM;
        }
        if self.ipcc.irq_rx() {
            lines |= 1 << irq::IPCC_C1_RX;
        }
        if self.ipcc.irq_tx() {
            lines |= 1 << irq::IPCC_C1_TX;
        }
        if self.lptim[0].irq() {
            lines |= 1 << irq::LPTIM1;
        }
        if self.lptim[1].irq() {
            lines |= 1 << irq::LPTIM2;
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
            Slot::Hsem => &self.hsem.regs,
            Slot::Ipcc => &self.ipcc.regs,
            Slot::Dma(i) => &self.dma[i].regs,
            Slot::Flash => &self.flash.regs,
            Slot::Rng => &self.rng.regs,
            Slot::Iwdg => &self.iwdg.regs,
            Slot::Lptim(i) => &self.lptim[i].regs,
            Slot::Generic(i) => &self.generic[i],
        }
    }

    /// Register-file content at `addr`, without side effects (for board models).
    pub fn peek(&self, addr: u32) -> Option<u32> {
        let (base, slot) = self.slot(addr)?;
        Some(self.regfile(slot).get((addr - base) & !3))
    }

    /// Recomputes the interrupt lines after a board model changed peripheral state.
    pub fn refresh_irqs(&mut self) {
        self.update_irqs();
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
            Slot::Hsem => self.hsem.read(word_off),
            Slot::Ipcc => self.ipcc.regs.read(word_off),
            Slot::Dma(i) => self.dma[i].regs.read(word_off),
            Slot::Flash => self.flash.regs.read(word_off),
            Slot::Rng => self.rng.read(word_off),
            Slot::Iwdg => self.iwdg.read(word_off),
            Slot::Lptim(i) => {
                let ns = self.now_ns(ctx.now);
                let v = self.lptim[i].read(word_off, ns);
                self.update_irqs();
                v
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
            if !self.spi_write(i, off, value, size) {
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
                // Peripheral resets held through RCC_APBxRSTRx (furi_hal_bus_reset).
                if self.rcc.regs.get(rcc::APB1RSTR1) & (1 << 31) != 0 {
                    self.lptim[0].reset();
                }
                if self.rcc.regs.get(rcc::APB1RSTR2) & (1 << 5) != 0 {
                    self.lptim[1].reset();
                }
                self.lptim_clocks();
                self.update_irqs();
                self.schedule();
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
            Slot::Hsem => {
                let h = self.hsem.write(word_off, value);
                self.update_irqs();
                h
            }
            Slot::Flash => self.flash.write(word_off, value),
            Slot::Dma(i) => {
                let h = self.dma[i].write(word_off, value);
                self.update_irqs();
                h
            }
            Slot::Ipcc => {
                let h = self.ipcc.write(word_off, value);
                self.update_irqs();
                h
            }
            Slot::Rng => self.rng.write(word_off, value),
            Slot::Iwdg => {
                let ns = self.now_ns(ctx.now);
                self.iwdg.write(word_off, value, ns)
            }
            Slot::Lptim(i) => {
                let ns = self.now_ns(ctx.now);
                self.lptim_clocks();
                let h = self.lptim[i].write(word_off, value, ns);
                self.update_irqs();
                self.schedule();
                h
            }
            Slot::Gpio(i) => {
                let h = self.gpio[i].write(word_off, value);
                self.gpio_changed();
                h
            }
            Slot::Exti => {
                let h = self.exti.write(word_off, value);
                self.update_irqs();
                h
            }
            Slot::I2c(i) => {
                let ns = self.now_ns(ctx.now);
                self.i2c[i].set_time(ns);
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
            ("GPIO", Partial, "pin levels from ODR/pulls and board inputs (buttons)"),
            (
                "EXTI",
                Partial,
                "pending/mask/software trigger, GPIO edges via SYSCFG_EXTICRx",
            ),
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
            (
                "IWDG",
                Partial,
                "resets at zero, with its reset flag; window not enforced",
            ),
            (
                "RNG",
                Partial,
                "deterministic xorshift64* with a configurable seed, never errors",
            ),
            (
                "LPTIM1/LPTIM2",
                Partial,
                "internal clock, one-shot/continuous, CMP/ARR events; no triggers/outputs",
            ),
            ("HSEM", Emulated, "1-step/2-step locks, free interrupts (CPU1 side)"),
            (
                "IPCC",
                Partial,
                "CPU1 side of the mailboxes; CPU2 side driven by the FUS model (ADR-0012)",
            ),
            (
                "Display (ST7567 on SPI2)",
                Emulated,
                "RAM, addressing, start line, orientation, inverse, all-on, reset; contrast not rendered",
            ),
            (
                "LEDs and backlight (LP5562)",
                Partial,
                "registers, LED map, program engines (PWM, ramp, wait, branch); timings approximate",
            ),
            (
                "microSD card (SPI mode)",
                Emulated,
                "SDHC commands 0/8/9/10/12/13/16/17/18/24/25/55/41/58, FAT32 prepared; instant, CRC not checked",
            ),
            (
                "DMA1/DMA2 + DMAMUX",
                Partial,
                "SPI requests only, transfers instantaneous; other requests stay idle",
            ),
            (
                "FLASH controller",
                Partial,
                "unlock, page erase, 64-bit and fast programming, option bytes; instant; no sector protection",
            ),
            ("Buttons", Emulated, "six keys with the board's polarity and pulls"),
            (
                "Battery (BQ27220, BQ25896)",
                Simulated,
                "fixed virtual battery state; data memory provisioning",
            ),
            (
                "CPU2 (radio core)",
                Stubbed,
                "not executed: behavioural FUS without wireless stack; no Bluetooth, no factory keys (ADR-0012)",
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

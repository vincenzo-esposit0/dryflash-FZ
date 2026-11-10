// SPDX-License-Identifier: GPL-3.0-or-later
//! Runs DMA channels whose request source has a model:
//!
//! - SPI1/SPI2 TX and RX, the only streams the SD driver uses (`furi_hal_spi_bus_trx_dma`,
//!   targets/f7/furi_hal/furi_hal_spi.c: DMA2 channels 6 and 7), all at once;
//! - TIM1's update request, one item per update event on every channel that listens to it (the IR
//!   driver's DMA2 channels 1 and 2, furi_hal_infrared.c), memory to peripheral.
//!
//! Requests come through DMAMUX1 (`LL_DMAMUX_REQ_*`, stm32wbxx_ll_dmamux.h). Other requests stay
//! idle with the channel enabled.

use super::SystemBus;
use crate::cpu::Size;
use crate::periph::dma::{CCR_DIR, CCR_EN, CCR_MINC, CHANNELS};
use crate::periph::{Ctx, svd};

const DMAMUX_BASE: u32 = 0x4002_0800;
const REQ_SPI1_RX: u32 = 6;
const REQ_SPI1_TX: u32 = 7;
const REQ_SPI2_RX: u32 = 8;
const REQ_SPI2_TX: u32 = 9;
const REQ_TIM1_UP: u32 = 0x19;
const SPI_CR2: u32 = svd::SPI1.offset_of("CR2");
const SPI_DR: u32 = svd::SPI1.offset_of("DR");
const CR2_RXDMAEN: u32 = 1 << 0;
const CR2_TXDMAEN: u32 = 1 << 1;

/// True if a write at `addr` can start a transfer.
pub(super) fn concerns_dma(addr: u32) -> bool {
    let block = |base: u32| addr.wrapping_sub(base) < 0x400;
    block(svd::DMA1.base)
        || block(svd::DMA2.base)
        || block(svd::SPI1.base)
        || block(svd::SPI2.base)
        || block(svd::TIM1.base)
}

impl SystemBus {
    /// DMAMUX1 channel request for (controller, channel): DMA1 maps to 0–6, DMA2 to 7–13.
    fn dma_request(&self, ctrl: usize, ch: usize) -> u32 {
        let mux = (ctrl * CHANNELS + ch) as u32;
        self.periphs.peek(DMAMUX_BASE + 4 * mux).unwrap_or(0) & 0x3F
    }

    /// Enabled channel that serves `request`, if any.
    fn dma_channel_for(&self, request: u32) -> Option<(usize, usize)> {
        for ctrl in 0..2 {
            for ch in 0..CHANNELS {
                let d = &self.periphs.dma[ctrl];
                if d.ccr(ch) & CCR_EN != 0 && d.cndtr(ch) > 0 && self.dma_request(ctrl, ch) == request {
                    return Some((ctrl, ch));
                }
            }
        }
        None
    }

    pub(super) fn dma_service(&mut self) {
        if self.periphs.tim1.dma_requests > 0 {
            let ns = self.periphs.now_ns(self.now);
            self.tim1_dma(ns);
            self.periphs.tim1_updated(ns);
        }
        for (spi, tx_req, rx_req) in [(0usize, REQ_SPI1_TX, REQ_SPI1_RX), (1, REQ_SPI2_TX, REQ_SPI2_RX)] {
            let cr2 = self.periphs.spi[spi].regs.get(SPI_CR2);
            if cr2 & CR2_TXDMAEN == 0 {
                continue;
            }
            // Started by the TX channel: SPI clocks only when it has data to send.
            let Some((tc, tn)) = self.dma_channel_for(tx_req) else {
                continue;
            };
            let rx = if cr2 & CR2_RXDMAEN != 0 {
                self.dma_channel_for(rx_req)
            } else {
                None
            };
            let tx_ccr = self.periphs.dma[tc].ccr(tn);
            if tx_ccr & CCR_DIR == 0 {
                continue;
            }
            let (tx_mem, count) = (self.periphs.dma[tc].cmar(tn), self.periphs.dma[tc].cndtr(tn));
            let tx_inc = tx_ccr & CCR_MINC != 0;
            let rx_mem = rx.map(|(c, n)| self.periphs.dma[c].cmar(n));
            for i in 0..count {
                let src = tx_mem + if tx_inc { i } else { 0 };
                let byte = self.dma_read(src);
                self.periphs.spi_write(spi, SPI_DR, byte as u32, Size::Byte);
                if let Some(dst) = rx_mem {
                    let miso = self.periphs.spi[spi].take_rx().unwrap_or(0xFF);
                    self.dma_write(dst + i, miso);
                }
            }
            self.periphs.dma[tc].complete(tn);
            if let Some((c, n)) = rx {
                self.periphs.dma[c].complete(n);
            }
            self.periphs.refresh_irqs();
        }
    }

    /// Serves TIM1's pending update requests at `ns`: one item on each enabled channel requested by
    /// TIM1_UP, from memory to the peripheral (RM0434 DMA: a source narrower than the destination is
    /// zero-extended).
    pub(super) fn tim1_dma(&mut self, ns: u128) {
        let requests = std::mem::take(&mut self.periphs.tim1.dma_requests);
        for _ in 0..requests {
            for ctrl in 0..2 {
                for ch in 0..CHANNELS {
                    let d = &self.periphs.dma[ctrl];
                    let ccr = d.ccr(ch);
                    if ccr & CCR_EN == 0 || d.cndtr(ch) == 0 || self.dma_request(ctrl, ch) != REQ_TIM1_UP {
                        continue;
                    }
                    if ccr & CCR_DIR == 0 {
                        self.digest
                            .event(self.now, "TIM1_UP DMA from a peripheral: not modelled".to_string());
                        continue;
                    }
                    let msize = 1u32 << ((ccr >> 10) & 3);
                    let psize = match (ccr >> 8) & 3 {
                        0 => Size::Byte,
                        1 => Size::Half,
                        _ => Size::Word,
                    };
                    let step = if ccr & CCR_MINC != 0 { msize } else { 0 };
                    let src = d.cmar(ch) + d.transferred(ch) * step;
                    let dst = d.cpar(ch);
                    let msz = match msize {
                        1 => Size::Byte,
                        2 => Size::Half,
                        _ => Size::Word,
                    };
                    let v = self.read_mem(src, msz).unwrap_or_else(|| {
                        let what = format!("DMA read outside memory at 0x{src:08X}");
                        self.digest.event(self.now, what);
                        0
                    });
                    self.dma_to_peripheral(dst, v, psize, ns);
                    self.periphs.dma[ctrl].item_done(ch);
                }
            }
        }
        self.periphs.refresh_irqs();
    }

    /// A DMA write to a peripheral register. TIM1 gets the time of its update event, which may be
    /// before the bus's current time.
    fn dma_to_peripheral(&mut self, addr: u32, v: u32, size: Size, ns: u128) {
        let off = addr.wrapping_sub(svd::TIM1.base);
        if off < 0x400 && off & 3 == 0 {
            self.periphs.tim1.write(off, v, ns);
            return;
        }
        let mut ctx = Ctx {
            now: self.now,
            digest: &mut self.digest,
            pc: self.access_pc,
        };
        if !self.periphs.write(addr, v, size, &mut ctx) {
            let what = format!("DMA write outside peripherals at 0x{addr:08X}");
            self.digest.event(self.now, what);
        }
    }

    fn dma_read(&mut self, addr: u32) -> u8 {
        self.read_mem(addr, Size::Byte).unwrap_or_else(|| {
            let what = format!("DMA read outside memory at 0x{addr:08X}");
            self.digest.event(self.now, what);
            0
        }) as u8
    }

    fn dma_write(&mut self, addr: u32, v: u8) {
        if !self.ram_write(addr, &[v]) {
            let what = format!("DMA write outside RAM at 0x{addr:08X}");
            self.digest.event(self.now, what);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::bus::SystemBus;
    use crate::cpu::{Bus, Size};
    use crate::periph::{irq, svd};

    const DMA2: u32 = 0x4002_0400;
    const DMAMUX: u32 = 0x4002_0800;
    const SPI2: u32 = 0x4000_3800;
    const GPIOC: u32 = 0x4800_0800;
    const CH6: u32 = DMA2 + 8 + 20 * 5;
    const CH7: u32 = DMA2 + 8 + 20 * 6;
    const RX_BUF: u32 = 0x2000_0100;
    const DUMMY: u32 = 0x2000_0080;

    fn w(bus: &mut SystemBus, a: u32, v: u32) {
        bus.write(a, v, Size::Word).unwrap();
    }

    #[test]
    fn spi_trx_through_dma2_6_and_7_raises_the_rx_interrupt() {
        let mut bus = SystemBus::new();
        // SD chip select (PC12) as a driven-low output, with the card behind it.
        bus.periphs
            .insert_sd(crate::board::sdcard::SdCard::new(2048, Default::default()));
        w(&mut bus, GPIOC, 1 << 24);
        w(&mut bus, GPIOC + 0x18, 1 << 28); // BSRR: PC12 low
        w(&mut bus, SPI2, 1 << 6); // SPE
        w(&mut bus, SPI2 + svd::SPI1.offset_of("CR2"), 0b11); // RX and TX DMA requests
        w(&mut bus, DUMMY, 0xFFFF_FFFF);
        // Channel 6: RX, request 8. Channel 7: TX, request 9. DMAMUX channels 12 and 13.
        w(&mut bus, DMAMUX + 4 * 12, 8);
        w(&mut bus, DMAMUX + 4 * 13, 9);
        w(&mut bus, CH6 + 4, 4);
        w(&mut bus, CH6 + 12, RX_BUF);
        w(&mut bus, CH6, (1 << 7) | (1 << 1) | 1); // MINC, TCIE, EN
        assert_eq!(
            bus.irq_lines() & (1 << irq::DMA2_CH6),
            0,
            "RX alone does not clock the bus"
        );
        w(&mut bus, CH7 + 4, 4);
        w(&mut bus, CH7 + 12, DUMMY);
        w(&mut bus, CH7, (1 << 4) | 1); // DIR, EN
        assert_ne!(bus.irq_lines() & (1 << irq::DMA2_CH6), 0, "transfer complete");
        assert_eq!(bus.read(CH6 + 4, Size::Word).unwrap(), 0);
        // 0xFF clocked in, an idle card answers 0xFF.
        assert_eq!(bus.read(RX_BUF, Size::Word).unwrap(), 0xFFFF_FFFF);
        // IFCR clears the flags and the line.
        w(&mut bus, DMA2 + 4, 0xF << 20);
        assert_eq!(bus.irq_lines() & (1 << irq::DMA2_CH6), 0);
    }
}

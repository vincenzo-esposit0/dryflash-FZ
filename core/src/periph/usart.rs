// SPDX-License-Identifier: GPL-3.0-or-later
//! USART1 / LPUART1 (RM0434 "USART" and "LPUART"; registers and bits from the SVD).
//!
//! Transmission is instant: TDR writes complete at once (TXE/TXFNF and TC stay set) and
//! the bytes are captured in `tx` (Furi's log on USART1). Reception comes from `rx`
//! (fed by the host, e.g. the CLI in M7). TEACK/REACK follow TE/RE while UE is set.

use super::regfile::{PeriphDesc, RegFile};
use super::svd;
use std::collections::VecDeque;

const CR1: u32 = svd::USART1.offset_of("CR1");
const ISR: u32 = svd::USART1.offset_of("ISR");
const ICR: u32 = svd::USART1.offset_of("ICR");
const RDR: u32 = svd::USART1.offset_of("RDR");
const TDR: u32 = svd::USART1.offset_of("TDR");

const RXNE: u32 = 1 << 5;
const TC: u32 = 1 << 6;
const TXE: u32 = 1 << 7;
const TEACK: u32 = 1 << 21;
const REACK: u32 = 1 << 22;
const TXFE: u32 = 1 << 23;

/// Bytes kept from the transmit stream (older bytes are dropped).
const TX_KEEP: usize = 64 * 1024;

#[derive(Clone, Debug)]
pub struct Usart {
    pub regs: RegFile,
    pub tx: Vec<u8>,
    pub rx: VecDeque<u8>,
}

impl Usart {
    pub fn new(desc: &'static PeriphDesc) -> Self {
        Usart {
            regs: RegFile::new(desc),
            tx: Vec::new(),
            rx: VecDeque::new(),
        }
    }

    pub fn reset(&mut self) {
        self.regs.reset();
        self.rx.clear();
    }

    fn isr(&self) -> u32 {
        let cr1 = self.regs.get(CR1);
        let ue = cr1 & 1 != 0;
        let mut v = (self.regs.get(ISR) & !(TEACK | REACK | RXNE)) | TXE | TC | TXFE;
        if ue && cr1 & (1 << 3) != 0 {
            v |= TEACK;
        }
        if ue && cr1 & (1 << 2) != 0 {
            v |= REACK;
        }
        if !self.rx.is_empty() {
            v |= RXNE;
        }
        v
    }

    /// Interrupt request: TXE/TXFNF (TXEIE), TC (TCIE), RXNE (RXNEIE).
    pub fn irq(&self) -> bool {
        let cr1 = self.regs.get(CR1);
        let isr = self.isr();
        cr1 & 1 != 0
            && ((cr1 & (1 << 7) != 0 && isr & TXE != 0)
                || (cr1 & (1 << 6) != 0 && isr & TC != 0)
                || (cr1 & (1 << 5) != 0 && isr & RXNE != 0))
    }

    pub fn read(&mut self, off: u32) -> Option<u32> {
        match off {
            ISR => Some(self.isr()),
            RDR => Some(self.rx.pop_front().unwrap_or(0) as u32),
            _ => self.regs.read(off),
        }
    }

    pub fn write(&mut self, off: u32, v: u32) -> bool {
        match off {
            TDR => {
                let cr1 = self.regs.get(CR1);
                if cr1 & 1 != 0 && cr1 & (1 << 3) != 0 {
                    if self.tx.len() >= TX_KEEP {
                        self.tx.drain(..TX_KEEP / 2);
                    }
                    self.tx.push(v as u8);
                }
                true
            }
            ICR => {
                let isr = self.regs.get(ISR);
                self.regs.set(ISR, isr & !v);
                true
            }
            _ => self.regs.write(off, v),
        }
    }
}

// SPDX-License-Identifier: GPL-3.0-or-later
//! Inter-processor communication controller (RM0434 "IPCC"; offsets from the SVD).
//!
//! Six simplex channels per direction. CPU1 sets a CPU1→CPU2 channel (C1SCR.CHnS), which
//! stays occupied (C1TOC2SR) until CPU2 clears it; the TX-free interrupt (IPCC_C1_TX) then
//! fires if C1CR.TXFIE and the channel is unmasked (C1MR.CHnFM = 0). CPU2→CPU1 channels set
//! by CPU2 (C2TOC1SR) raise IPCC_C1_RX when C1CR.RXOIE and C1MR.CHnOM = 0; CPU1 clears them
//! with C1SCR.CHnC. The CPU2 side is driven through `cpu2_*` by a CPU2 model; while CPU2 is
//! not emulated nobody calls them, so CPU1→CPU2 channels stay occupied (ADR-0011).

use super::regfile::RegFile;
use super::svd;

const C1CR: u32 = svd::IPCC.offset_of("C1CR");
const C1MR: u32 = svd::IPCC.offset_of("C1MR");
const C1SCR: u32 = svd::IPCC.offset_of("C1SCR");
const C1TOC2SR: u32 = svd::IPCC.offset_of("C1TO2SR");
const C2TOC1SR: u32 = svd::IPCC.offset_of("C2TOC1SR");

#[derive(Clone, Debug)]
pub struct Ipcc {
    pub regs: RegFile,
}

impl Default for Ipcc {
    fn default() -> Self {
        Self::new()
    }
}

impl Ipcc {
    pub fn new() -> Self {
        Ipcc {
            regs: RegFile::new(&svd::IPCC),
        }
    }

    pub fn reset(&mut self) {
        self.regs.reset();
    }

    pub fn write(&mut self, off: u32, v: u32) -> bool {
        if off == C1SCR {
            let mut sr = self.regs.get(C1TOC2SR);
            sr |= (v >> 16) & 0x3F; // CHnS: occupy CPU1→CPU2 channels
            self.regs.set(C1TOC2SR, sr);
            let rx = self.regs.get(C2TOC1SR);
            self.regs.set(C2TOC1SR, rx & !(v & 0x3F)); // CHnC: release CPU2→CPU1 channels
            return true;
        }
        self.regs.write(off, v)
    }

    /// IPCC_C1_RX: a CPU2→CPU1 channel is occupied and unmasked, with RXOIE.
    pub fn irq_rx(&self) -> bool {
        self.regs.get(C1CR) & 1 != 0 && self.regs.get(C2TOC1SR) & !self.regs.get(C1MR) & 0x3F != 0
    }

    /// IPCC_C1_TX: a CPU1→CPU2 channel is free and its free-interrupt unmasked, with TXFIE.
    pub fn irq_tx(&self) -> bool {
        let free = !self.regs.get(C1TOC2SR) & 0x3F;
        self.regs.get(C1CR) & (1 << 16) != 0 && free & !(self.regs.get(C1MR) >> 16) & 0x3F != 0
    }

    /// CPU2 took the message on CPU1→CPU2 channel `ch`.
    pub fn cpu2_clear(&mut self, ch: u32) {
        self.regs.set_bits(C1TOC2SR, 1 << ch, false);
    }

    /// CPU2 posts a message on CPU2→CPU1 channel `ch`.
    pub fn cpu2_set(&mut self, ch: u32) {
        self.regs.set_bits(C2TOC1SR, 1 << ch, true);
    }

    pub fn cpu1_to_cpu2_occupied(&self) -> u32 {
        self.regs.get(C1TOC2SR) & 0x3F
    }
}

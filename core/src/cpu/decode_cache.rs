// SPDX-License-Identifier: GPL-3.0-or-later
//! Decoded-instruction cache.
//!
//! One slot per halfword of a cached region (typically flash), decoded lazily on first
//! execution. Code outside the region is decoded on every fetch. Callers must call
//! [`DecodeCache::invalidate`] when cached memory changes (flash program/erase).

use super::decode::{decode, is_32bit};
use super::op::Decoded;
use super::{Bus, BusError};

pub struct DecodeCache {
    base: u32,
    slots: Vec<Decoded>,
}

impl DecodeCache {
    /// Caches `[base, base + len)`; `len` must be a multiple of 2.
    pub fn new(base: u32, len: u32) -> Self {
        DecodeCache {
            base,
            slots: vec![Decoded::EMPTY; (len / 2) as usize],
        }
    }

    #[inline]
    pub fn lookup<B: Bus>(&mut self, bus: &mut B, pc: u32) -> Result<Decoded, BusError> {
        let idx = (pc.wrapping_sub(self.base) >> 1) as usize;
        if let Some(slot) = self.slots.get(idx) {
            if slot.size != 0 {
                return Ok(*slot);
            }
            let dec = fetch_decode(bus, pc)?;
            self.slots[idx] = dec;
            return Ok(dec);
        }
        fetch_decode(bus, pc)
    }

    /// Drops cached decodes overlapping `[addr, addr + len)`, including a 32-bit
    /// instruction that starts just before `addr`.
    pub fn invalidate(&mut self, addr: u32, len: u32) {
        let start = addr.saturating_sub(2).max(self.base);
        let end = addr.saturating_add(len);
        let mut a = start;
        while a < end {
            if let Some(slot) = self.slots.get_mut((a.wrapping_sub(self.base) >> 1) as usize) {
                *slot = Decoded::EMPTY;
            }
            a += 2;
        }
    }
}

fn fetch_decode<B: Bus>(bus: &mut B, pc: u32) -> Result<Decoded, BusError> {
    let hw1 = bus.fetch16(pc)?;
    let hw2 = if is_32bit(hw1) {
        bus.fetch16(pc.wrapping_add(2))?
    } else {
        0
    };
    Ok(decode(hw1, hw2))
}

// SPDX-License-Identifier: GPL-3.0-or-later
//! True random number generator (RM0434 "RNG"; registers from the SVD), replaced by a
//! deterministic generator: same firmware + same inputs + same seed ⇒ same execution
//! (BRIEF §6). DRDY is set while RNGEN is set; clock and seed errors never occur.

use super::regfile::RegFile;
use super::svd;

const CR: u32 = svd::RNG.offset_of("CR");
const SR: u32 = svd::RNG.offset_of("SR");
const DR: u32 = svd::RNG.offset_of("DR");

/// Default seed when the virtual device does not set one.
pub const DEFAULT_SEED: u64 = 0x6472_7966_6C61_7368; // "dryflash"

#[derive(Clone, Debug)]
pub struct Rng {
    pub regs: RegFile,
    state: u64,
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng {
            regs: RegFile::new(&svd::RNG),
            state: seed | 1,
        }
    }

    pub fn reset(&mut self) {
        self.regs.reset();
    }

    /// xorshift64*: deterministic, not cryptographic (the emulator is not a security device).
    fn next(&mut self) -> u32 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 32) as u32
    }

    pub fn read(&mut self, off: u32) -> Option<u32> {
        let enabled = self.regs.get(CR) & (1 << 2) != 0;
        match off {
            SR => Some((self.regs.get(SR) & 0x60) | enabled as u32),
            DR => Some(if enabled { self.next() } else { 0 }),
            _ => self.regs.read(off),
        }
    }

    pub fn write(&mut self, off: u32, v: u32) -> bool {
        self.regs.write(off, v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_and_ready_when_enabled() {
        let mut a = Rng::new(42);
        let mut b = Rng::new(42);
        assert_eq!(a.read(SR), Some(0), "not ready while disabled");
        a.write(CR, 1 << 2);
        b.write(CR, 1 << 2);
        assert_eq!(a.read(SR), Some(1));
        let xs: Vec<_> = (0..4).map(|_| a.read(DR).unwrap()).collect();
        let ys: Vec<_> = (0..4).map(|_| b.read(DR).unwrap()).collect();
        assert_eq!(xs, ys);
        assert_ne!(xs[0], xs[1]);
    }
}

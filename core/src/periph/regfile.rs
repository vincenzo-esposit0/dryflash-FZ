// SPDX-License-Identifier: GPL-3.0-or-later
//! Register file generated from the SVD: reset values and readable/writable bit masks.
//!
//! A peripheral model keeps its registers in a [`RegFile`] and adds behaviour on top
//! (ready flags, status bits, side effects). Registers a model gives no behaviour to are
//! still latched with the right reset value and masks, and accesses to them are reported in
//! the bring-up digest as "register file only" (ADR-0010).

#[derive(Clone, Copy, Debug)]
pub struct RegDesc {
    pub name: &'static str,
    pub offset: u16,
    pub reset: u32,
    /// Bits that read back (read-only and read-write fields).
    pub read: u32,
    /// Bits that software can write (write-only and read-write fields).
    pub write: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct PeriphDesc {
    pub name: &'static str,
    pub base: u32,
    pub regs: &'static [RegDesc],
}

impl PeriphDesc {
    pub fn reg(&self, offset: u32) -> Option<usize> {
        self.regs.iter().position(|r| r.offset as u32 == offset)
    }

    /// Offset of the register called `name` (panics on typos: tables are generated).
    pub const fn offset_of(&self, name: &str) -> u32 {
        let mut i = 0;
        while i < self.regs.len() {
            if str_eq(self.regs[i].name, name) {
                return self.regs[i].offset as u32;
            }
            i += 1;
        }
        panic!("register not in SVD table")
    }
}

const fn str_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

#[derive(Clone, Debug)]
pub struct RegFile {
    pub desc: &'static PeriphDesc,
    pub vals: Vec<u32>,
    /// offset/4 → register index (u16::MAX = no register).
    index: Vec<u16>,
}

impl RegFile {
    pub fn new(desc: &'static PeriphDesc) -> Self {
        let max = desc.regs.iter().map(|r| r.offset as usize / 4).max().unwrap_or(0);
        let mut index = vec![u16::MAX; max + 1];
        for (i, r) in desc.regs.iter().enumerate() {
            index[r.offset as usize / 4] = i as u16;
        }
        RegFile {
            desc,
            vals: desc.regs.iter().map(|r| r.reset).collect(),
            index,
        }
    }

    pub fn reset(&mut self) {
        for (v, r) in self.vals.iter_mut().zip(self.desc.regs) {
            *v = r.reset;
        }
    }

    #[inline]
    pub fn idx(&self, offset: u32) -> Option<usize> {
        match self.index.get(offset as usize / 4) {
            Some(&i) if i != u16::MAX && offset & 3 == 0 => Some(i as usize),
            _ => None,
        }
    }

    /// Raw stored value (no masking).
    #[inline]
    pub fn get(&self, offset: u32) -> u32 {
        self.idx(offset).map(|i| self.vals[i]).unwrap_or(0)
    }

    #[inline]
    pub fn set(&mut self, offset: u32, v: u32) {
        if let Some(i) = self.idx(offset) {
            self.vals[i] = v;
        }
    }

    /// Software read: only readable bits.
    pub fn read(&self, offset: u32) -> Option<u32> {
        self.idx(offset).map(|i| self.vals[i] & self.desc.regs[i].read)
    }

    /// Software write: only writable bits change.
    pub fn write(&mut self, offset: u32, v: u32) -> bool {
        match self.idx(offset) {
            Some(i) => {
                let m = self.desc.regs[i].write;
                self.vals[i] = (self.vals[i] & !m) | (v & m);
                true
            }
            None => false,
        }
    }

    /// Sets or clears hardware-owned bits.
    pub fn set_bits(&mut self, offset: u32, mask: u32, on: bool) {
        if let Some(i) = self.idx(offset) {
            if on {
                self.vals[i] |= mask;
            } else {
                self.vals[i] &= !mask;
            }
        }
    }
}

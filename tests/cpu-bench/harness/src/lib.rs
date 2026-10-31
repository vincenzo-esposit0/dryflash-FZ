// SPDX-License-Identifier: GPL-3.0-or-later
//! ADR-0001 spike harness: a minimal flash + SRAM1 system around `dryflash_core::cpu`.
//!
//! The memory map is reduced to what the benchmark firmware touches; the real bus
//! (ARCHITECTURE.md §3) replaces it in M1. Exported as plain `extern "C"` functions so the
//! same build runs in browsers and Node without wasm-bindgen.

use dryflash_core::cpu::{Bus, BusError, Cpu, DecodeCache, Size, Stop};

pub const FLASH_BASE: u32 = 0x0800_0000;
pub const FLASH_LEN: u32 = 1024 * 1024;
pub const RAM_BASE: u32 = 0x2000_0000;
/// SRAM1 is 192 KiB on the STM32WB55RG (RM0434 §2.2.2).
pub const RAM_LEN: u32 = 192 * 1024;
/// `Results` block of the benchmark firmware (start of .bss).
pub const RESULTS_ADDR: u32 = RAM_BASE;
pub const RESULT_WORDS: usize = 9;

pub struct SpikeBus {
    pub flash: Vec<u8>,
    pub ram: Vec<u8>,
}

impl SpikeBus {
    pub fn new(image: &[u8]) -> Self {
        let mut flash = vec![0xFF; FLASH_LEN as usize];
        flash[..image.len()].copy_from_slice(image);
        SpikeBus {
            flash,
            ram: vec![0; RAM_LEN as usize],
        }
    }

    #[inline]
    fn slice(&self, addr: u32, len: u32) -> Option<&[u8]> {
        let r = addr.wrapping_sub(RAM_BASE);
        if r < RAM_LEN && r + len <= RAM_LEN {
            return Some(&self.ram[r as usize..(r + len) as usize]);
        }
        let f = addr.wrapping_sub(FLASH_BASE);
        if f < FLASH_LEN && f + len <= FLASH_LEN {
            return Some(&self.flash[f as usize..(f + len) as usize]);
        }
        None
    }
}

impl Bus for SpikeBus {
    #[inline]
    fn read(&mut self, addr: u32, size: Size) -> Result<u32, BusError> {
        let b = self.slice(addr, size as u32).ok_or(BusError { addr, write: false })?;
        Ok(match size {
            Size::Byte => b[0] as u32,
            Size::Half => u16::from_le_bytes([b[0], b[1]]) as u32,
            Size::Word => u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
        })
    }

    #[inline]
    fn write(&mut self, addr: u32, value: u32, size: Size) -> Result<(), BusError> {
        let r = addr.wrapping_sub(RAM_BASE);
        let n = size as u32;
        if r >= RAM_LEN || r + n > RAM_LEN {
            return Err(BusError { addr, write: true });
        }
        let r = r as usize;
        self.ram[r..r + n as usize].copy_from_slice(&value.to_le_bytes()[..n as usize]);
        Ok(())
    }

    #[inline]
    fn fetch16(&mut self, addr: u32) -> Result<u16, BusError> {
        self.read(addr, Size::Half).map(|v| v as u16)
    }
}

pub struct Bench {
    pub cpu: Cpu,
    pub bus: SpikeBus,
    pub cache: DecodeCache,
}

impl Bench {
    pub fn new(image: &[u8], rounds: u32) -> Self {
        let mut bus = SpikeBus::new(image);
        // `Results.rounds` is read by the firmware at start-up.
        bus.write(RESULTS_ADDR + 4, rounds, Size::Word).unwrap();
        let mut cpu = Cpu::new();
        cpu.reset(&mut bus, FLASH_BASE).expect("vector table in flash");
        Bench {
            cpu,
            bus,
            cache: DecodeCache::new(FLASH_BASE, FLASH_LEN),
        }
    }

    /// Runs until the firmware's final `BKPT` (or any other stop).
    pub fn run(&mut self) -> Stop {
        loop {
            match self.cpu.run(&mut self.bus, &mut self.cache, 50_000_000) {
                Stop::Budget => continue,
                other => return other,
            }
        }
    }

    pub fn results(&mut self) -> [u32; RESULT_WORDS] {
        let mut out = [0; RESULT_WORDS];
        for (i, w) in out.iter_mut().enumerate() {
            *w = self.bus.read(RESULTS_ADDR + 4 * i as u32, Size::Word).unwrap();
        }
        out
    }
}

// ---- WebAssembly / FFI surface -------------------------------------------------

static mut LAST: Option<(u64, [u32; RESULT_WORDS], u32)> = None;

/// Allocates `len` bytes for the firmware image; the host copies the image there.
#[unsafe(no_mangle)]
pub extern "C" fn bench_alloc(len: u32) -> *mut u8 {
    let mut v = vec![0u8; len as usize];
    let p = v.as_mut_ptr();
    std::mem::forget(v);
    p
}

/// Runs the benchmark image at `ptr[..len]` for `rounds`; returns retired instructions
/// (as f64 so JS gets a Number), or -1 if the firmware did not stop at `BKPT #0`.
///
/// # Safety
/// `ptr` must come from `bench_alloc(len)`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bench_run(ptr: *const u8, len: u32, rounds: u32) -> f64 {
    let image = unsafe { std::slice::from_raw_parts(ptr, len as usize) };
    let mut b = Bench::new(image, rounds);
    let stop = b.run();
    let ok = matches!(stop, Stop::Breakpoint { imm: 0, .. });
    let res = b.results();
    unsafe { LAST = Some((b.cpu.instret, res, b.cpu.pc())) };
    if ok { b.cpu.instret as f64 } else { -1.0 }
}

/// Word `i` of the last run's result block (see the benchmark firmware's `Results`).
#[unsafe(no_mangle)]
pub extern "C" fn bench_result(i: u32) -> u32 {
    #[allow(static_mut_refs)]
    unsafe {
        LAST.map(|l| l.1[i as usize]).unwrap_or(0)
    }
}

/// PC at which the last run stopped.
#[unsafe(no_mangle)]
pub extern "C" fn bench_stop_pc() -> u32 {
    #[allow(static_mut_refs)]
    unsafe {
        LAST.map(|l| l.2).unwrap_or(0)
    }
}

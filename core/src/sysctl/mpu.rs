// SPDX-License-Identifier: GPL-3.0-or-later
//! Memory Protection Unit, PMSAv7 (DDI 0403E B3.5), 8 regions on the Cortex-M4
//! (MPU_TYPE.DREGION = 8, DDI 0439B §4.5).
//!
//! The f7 firmware relies on it: `furi_hal_mpu_init` (targets/f7/furi_hal/furi_hal_mpu.c,
//! firmware 1.5.1) makes 0x00000000–0x000FFFFF no-access to trap NULL dereferences and
//! puts 32-byte no-access guards at the bottom of thread stacks.

const REGIONS: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MpuAccess {
    Read,
    Write,
    Exec,
}

#[derive(Clone, Debug, Default)]
pub struct Mpu {
    pub ctrl: u32,
    pub rnr: u32,
    pub rbar: [u32; REGIONS],
    pub rasr: [u32; REGIONS],
}

/// XN regions of the default memory map (DDI 0403E B3.1, Table B3-1): Peripheral,
/// Device and System space are never executable.
pub fn default_map_xn(addr: u32) -> bool {
    matches!(addr, 0x4000_0000..=0x5FFF_FFFF | 0xA000_0000..=0xFFFF_FFFF)
}

impl Mpu {
    pub fn new() -> Self {
        Mpu::default()
    }

    pub fn enabled(&self) -> bool {
        self.ctrl & 1 != 0
    }

    pub fn read(&self, addr: u32) -> u32 {
        match addr {
            0xE000_ED90 => (REGIONS as u32) << 8, // TYPE: DREGION = 8, unified.
            0xE000_ED94 => self.ctrl,
            0xE000_ED98 => self.rnr,
            0xE000_ED9C | 0xE000_EDA4 | 0xE000_EDAC | 0xE000_EDB4 => {
                let r = (self.rnr & 7) as usize;
                self.rbar[r] | r as u32
            }
            0xE000_EDA0 | 0xE000_EDA8 | 0xE000_EDB0 | 0xE000_EDB8 => self.rasr[(self.rnr & 7) as usize],
            _ => 0,
        }
    }

    pub fn write(&mut self, addr: u32, v: u32) {
        match addr {
            0xE000_ED94 => self.ctrl = v & 7,
            0xE000_ED98 => self.rnr = v & 0xFF,
            0xE000_ED9C | 0xE000_EDA4 | 0xE000_EDAC | 0xE000_EDB4 => {
                if v & (1 << 4) != 0 {
                    // VALID: the REGION field selects the region and updates RNR.
                    self.rnr = v & 0xF;
                }
                let r = (self.rnr & 7) as usize;
                self.rbar[r] = v & !0x1F;
            }
            0xE000_EDA0 | 0xE000_EDA8 | 0xE000_EDB0 | 0xE000_EDB8 => {
                let r = (self.rnr & 7) as usize;
                self.rasr[r] = v & 0x173F_FF3F;
            }
            _ => {}
        }
    }

    /// Returns the highest-numbered enabled region that contains `addr`, honouring
    /// subregion-disable bits (B3.5.3).
    fn region_for(&self, addr: u32) -> Option<usize> {
        (0..REGIONS).rev().find(|&r| {
            let rasr = self.rasr[r];
            if rasr & 1 == 0 {
                return false;
            }
            let size_field = (rasr >> 1) & 0x1F;
            if size_field < 4 {
                return false; // UNPREDICTABLE sizes below 32 bytes.
            }
            let size = 1u64 << (size_field + 1);
            let base = (self.rbar[r] as u64) & !(size - 1);
            let a = addr as u64;
            if a < base || a >= base + size {
                return false;
            }
            // Subregions exist for regions of 256 bytes or more.
            if size >= 256 {
                let sub = ((a - base) / (size / 8)) as u32;
                if (rasr >> 8) & (1 << sub) != 0 {
                    return false;
                }
            }
            true
        })
    }

    /// Permission check (B3.5.5 ValidateAddress). `use_default` forces the default map
    /// (HardFault/NMI/FAULTMASK with HFNMIENA = 0, PPB and vector table accesses).
    pub fn allowed(&self, addr: u32, access: MpuAccess, privileged: bool, use_default: bool) -> bool {
        let default_ok = !(access == MpuAccess::Exec && default_map_xn(addr));
        if !self.enabled() || use_default {
            return default_ok;
        }
        let Some(r) = self.region_for(addr) else {
            // Background region: privileged accesses only, with PRIVDEFENA.
            return privileged && self.ctrl & (1 << 2) != 0 && default_ok;
        };
        let rasr = self.rasr[r];
        if access == MpuAccess::Exec && rasr & (1 << 28) != 0 {
            return false;
        }
        let (read, write) = match (rasr >> 24) & 7 {
            0b000 => (false, false),
            0b001 => (privileged, privileged),
            0b010 => (true, privileged),
            0b011 => (true, true),
            0b101 => (privileged, false),
            0b110 | 0b111 => (true, false),
            _ => (false, false), // 0b100 is reserved.
        };
        match access {
            MpuAccess::Write => write,
            _ => read,
        }
    }

    /// Address boundaries where the permission can change, sorted. Used by the CPU to
    /// cache instruction-fetch permission for a whole address window.
    pub fn boundaries(&self) -> Vec<u32> {
        let mut b = vec![0x4000_0000, 0x6000_0000, 0xA000_0000];
        if self.enabled() {
            for r in 0..REGIONS {
                let rasr = self.rasr[r];
                let size_field = (rasr >> 1) & 0x1F;
                if rasr & 1 == 0 || size_field < 4 {
                    continue;
                }
                let size = 1u64 << (size_field + 1);
                let base = (self.rbar[r] as u64) & !(size - 1);
                let step = if size >= 256 { size / 8 } else { size };
                let mut a = base;
                while a <= base + size {
                    if a <= u32::MAX as u64 {
                        b.push(a as u32);
                    }
                    a += step;
                }
            }
        }
        b.sort_unstable();
        b.dedup();
        b
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The NULL-guard region exactly as `furi_hal_mpu_init` programs it:
    /// region 0, base 0, 1 MiB (SIZE = 19), AP = no access, PRIVDEFENA.
    fn furi_null_guard() -> Mpu {
        let mut m = Mpu::new();
        m.write(0xE000_ED98, 0);
        m.write(0xE000_ED9C, 0);
        m.write(0xE000_EDA0, (19 << 1) | 1);
        m.write(0xE000_ED94, 0b101);
        m
    }

    #[test]
    fn null_guard_blocks_low_memory_only() {
        let m = furi_null_guard();
        assert!(!m.allowed(0x0000_0000, MpuAccess::Read, true, false));
        assert!(!m.allowed(0x000F_FFFC, MpuAccess::Write, true, false));
        assert!(m.allowed(0x0800_0000, MpuAccess::Exec, true, false));
        assert!(m.allowed(0x2000_0000, MpuAccess::Write, true, false));
        // Unprivileged code has no background region.
        assert!(!m.allowed(0x2000_0000, MpuAccess::Read, false, false));
        // HardFault/NMI with HFNMIENA = 0 use the default map.
        assert!(m.allowed(0x0000_0000, MpuAccess::Read, true, true));
    }

    #[test]
    fn default_map_is_never_executable_in_device_space() {
        let m = Mpu::new();
        assert!(!m.allowed(0x4000_0000, MpuAccess::Exec, true, false));
        assert!(m.allowed(0x4000_0000, MpuAccess::Read, true, false));
        assert!(!m.allowed(0xE000_E000, MpuAccess::Exec, true, false));
    }

    #[test]
    fn subregion_disable_falls_through() {
        let mut m = Mpu::new();
        // 2 KiB region at 0x2000_0000, subregion 0 disabled, read-only.
        m.write(0xE000_ED9C, 0x2000_0000 | (1 << 4));
        m.write(0xE000_EDA0, (0b110 << 24) | (1 << 8) | (10 << 1) | 1);
        m.write(0xE000_ED94, 0b101);
        assert!(m.allowed(0x2000_0000, MpuAccess::Write, true, false)); // background map
        assert!(!m.allowed(0x2000_0100, MpuAccess::Write, true, false));
    }
}

// SPDX-License-Identifier: GPL-3.0-or-later
//! STM32WB55 memory map.
//!
//! Base addresses from `lib/stm32wb_cmsis/Include/stm32wb55xx.h` (CMSIS device v1.12.0,
//! pinned by firmware 1.5.1): FLASH_BASE, SRAM1/2A/2B_BASE and *_SIZE, SYSTEM_MEMORY_BASE,
//! OTP_AREA_BASE, ENGI_BYTE_BASE and the peripheral `*_BASE` defines. Block sizes are
//! the distance to the next block, capped at the 1 KiB peripheral stride.

pub const FLASH_BASE: u32 = 0x0800_0000;
pub const FLASH_SIZE: u32 = 1024 * 1024;
pub const SRAM1_BASE: u32 = 0x2000_0000;
pub const SRAM1_SIZE: u32 = 0x0003_0000;
pub const SRAM2A_BASE: u32 = 0x2003_0000;
pub const SRAM2A_SIZE: u32 = 0x0000_8000;
pub const SRAM2B_BASE: u32 = 0x2003_8000;
pub const SRAM2B_SIZE: u32 = 0x0000_8000;
pub const SYSTEM_MEMORY_BASE: u32 = 0x1FFF_0000;
pub const SYSTEM_MEMORY_SIZE: u32 = 0x7000;
pub const OTP_BASE: u32 = 0x1FFF_7000;
pub const OTP_SIZE: u32 = 0x400;
pub const ENGI_BASE: u32 = 0x1FFF_7400;
pub const ENGI_SIZE: u32 = 0xC00;
/// `FLASHSIZE_BASE`: flash size in KiB (u16).
pub const FLASHSIZE_ADDR: u32 = 0x1FFF_75E0;
/// `UID64_BASE`: 64-bit unique device identifier.
pub const UID64_ADDR: u32 = 0x1FFF_7580;
/// Main flash is aliased at address 0 when booting from flash (SYSCFG_MEMRMP = 0);
/// `SystemInit` relies on it by setting VTOR = 0 (targets/f7/src/stm32wb55_startup.c).
pub const BOOT_ALIAS_SIZE: u32 = FLASH_SIZE;

/// A named register block (for the bring-up digest and, from M2, device dispatch).
#[derive(Clone, Copy, Debug)]
pub struct Block {
    pub name: &'static str,
    pub base: u32,
    pub size: u32,
}

const fn b(name: &'static str, base: u32, size: u32) -> Block {
    Block { name, base, size }
}

/// Peripheral blocks, sorted by base address.
pub const BLOCKS: &[Block] = &[
    b("TIM2", 0x4000_0000, 0x400),
    b("LCD", 0x4000_2400, 0x400),
    b("RTC", 0x4000_2800, 0x400),
    b("WWDG", 0x4000_2C00, 0x400),
    b("IWDG", 0x4000_3000, 0x400),
    b("SPI2", 0x4000_3800, 0x400),
    b("I2C1", 0x4000_5400, 0x400),
    b("I2C3", 0x4000_5C00, 0x400),
    b("CRS", 0x4000_6000, 0x400),
    b("USB", 0x4000_6800, 0x400),
    b("USB_PMA", 0x4000_6C00, 0x400),
    b("LPTIM1", 0x4000_7C00, 0x400),
    b("LPUART1", 0x4000_8000, 0x400),
    b("LPTIM2", 0x4000_9400, 0x400),
    b("SYSCFG", 0x4001_0000, 0x30),
    b("VREFBUF", 0x4001_0030, 0x1D0),
    b("COMP", 0x4001_0200, 0x200),
    b("TIM1", 0x4001_2C00, 0x400),
    b("SPI1", 0x4001_3000, 0x400),
    b("USART1", 0x4001_3800, 0x400),
    b("TIM16", 0x4001_4400, 0x400),
    b("TIM17", 0x4001_4800, 0x400),
    b("SAI1", 0x4001_5400, 0x400),
    b("DMA1", 0x4002_0000, 0x400),
    b("DMA2", 0x4002_0400, 0x400),
    b("DMAMUX1", 0x4002_0800, 0x400),
    b("CRC", 0x4002_3000, 0x400),
    b("TSC", 0x4002_4000, 0x400),
    b("GPIOA", 0x4800_0000, 0x400),
    b("GPIOB", 0x4800_0400, 0x400),
    b("GPIOC", 0x4800_0800, 0x400),
    b("GPIOD", 0x4800_0C00, 0x400),
    b("GPIOE", 0x4800_1000, 0x400),
    b("GPIOH", 0x4800_1C00, 0x400),
    b("ADC1", 0x5004_0000, 0x400),
    b("AES1", 0x5006_0000, 0x400),
    b("RCC", 0x5800_0000, 0x400),
    b("PWR", 0x5800_0400, 0x400),
    b("EXTI", 0x5800_0800, 0x400),
    b("IPCC", 0x5800_0C00, 0x400),
    b("RNG", 0x5800_1000, 0x400),
    b("HSEM", 0x5800_1400, 0x400),
    b("AES2", 0x5800_1800, 0x400),
    b("PKA", 0x5800_2000, 0x2000),
    b("FLASH", 0x5800_4000, 0x400),
    b("QUADSPI_MEM", 0x9000_0000, 0x1000_0000),
    b("QUADSPI", 0xA000_1000, 0x400),
    b("DBGMCU", 0xE004_2000, 0x400),
];

/// The block containing `addr`, if any.
pub fn block_of(addr: u32) -> Option<&'static Block> {
    let i = BLOCKS.partition_point(|b| b.base <= addr);
    let blk = BLOCKS.get(i.checked_sub(1)?)?;
    (addr - blk.base < blk.size).then_some(blk)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_are_sorted_and_disjoint() {
        for w in BLOCKS.windows(2) {
            assert!(
                w[0].base + w[0].size <= w[1].base,
                "{} overlaps {}",
                w[0].name,
                w[1].name
            );
        }
    }

    #[test]
    fn lookup() {
        assert_eq!(block_of(0x5800_0000).unwrap().name, "RCC");
        assert_eq!(block_of(0x5800_005C).unwrap().name, "RCC");
        assert_eq!(block_of(0x4001_0204).unwrap().name, "COMP");
        assert!(block_of(0x4000_0400).is_none());
        assert!(block_of(0x3FFF_FFFF).is_none());
    }
}

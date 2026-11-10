// SPDX-License-Identifier: GPL-3.0-or-later
//! IRQ numbers of the STM32WB55 (`IRQn_Type` in stm32wb55xx.h, CMSIS device v1.12.0).

pub const RTC_WKUP: u32 = 3;
pub const FLASH: u32 = 4;
pub const RCC: u32 = 5;
pub const EXTI0: u32 = 6;
pub const EXTI1: u32 = 7;
pub const EXTI2: u32 = 8;
pub const EXTI3: u32 = 9;
pub const EXTI4: u32 = 10;
pub const DMA1_CH1: u32 = 11;
pub const USB_LP: u32 = 20;
pub const EXTI9_5: u32 = 23;
pub const I2C1_EV: u32 = 30;
pub const I2C3_EV: u32 = 32;
pub const SPI1: u32 = 34;
pub const SPI2: u32 = 35;
pub const USART1: u32 = 36;
pub const LPUART1: u32 = 37;
pub const EXTI15_10: u32 = 40;
pub const RTC_ALARM: u32 = 41;
pub const DMA2_CH1: u32 = 55;
#[allow(dead_code)]
pub const DMA2_CH6: u32 = 60;
pub const IPCC_C1_RX: u32 = 44;
pub const IPCC_C1_TX: u32 = 45;
pub const HSEM: u32 = 46;
pub const LPTIM1: u32 = 47;
pub const LPTIM2: u32 = 48;

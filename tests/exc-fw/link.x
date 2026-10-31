/* SPDX-License-Identifier: GPL-3.0-or-later */
/* Flash and SRAM1 as on the f7 target (stm32wb55xx_flash.ld); vectors at the flash start. */
MEMORY
{
  FLASH (rx) : ORIGIN = 0x08000000, LENGTH = 1024K
  RAM (rwx)  : ORIGIN = 0x20000000, LENGTH = 192K
}
ENTRY(reset);
SECTIONS
{
  .vectors ORIGIN(FLASH) : { KEEP(*(.vectors)) } > FLASH
  .text : { *(.text .text.*) } > FLASH
  .rodata : { *(.rodata .rodata.*) } > FLASH
  .bss (NOLOAD) : { KEEP(*(.bss.results)) *(.bss .bss.*) *(COMMON) . = ALIGN(4); } > RAM
  .data : { *(.data .data.*) } > RAM AT > FLASH
  /DISCARD/ : { *(.ARM.exidx .ARM.exidx.*) *(.ARM.attributes) }
}

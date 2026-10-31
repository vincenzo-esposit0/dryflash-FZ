/* SPDX-License-Identifier: GPL-3.0-or-later */
/* Same flash/SRAM1 origins as the f7 target (RM0434 §2.2.2); sizes reduced for the benchmark. */
MEMORY
{
  FLASH (rx) : ORIGIN = 0x08000000, LENGTH = 1024K
  RAM (rwx)  : ORIGIN = 0x20000000, LENGTH = 192K
}
ENTRY(reset);
SECTIONS
{
  .vectors ORIGIN(FLASH) : { LONG(ORIGIN(RAM) + LENGTH(RAM)); LONG(reset | 1); } > FLASH
  .text : { *(.text .text.*) } > FLASH
  .rodata : { *(.rodata .rodata.*) } > FLASH
  .bss (NOLOAD) : { __sbss = .; KEEP(*(.bss.results)) *(.bss .bss.*) *(COMMON) . = ALIGN(4); __ebss = .; } > RAM
  .data : { *(.data .data.*) } > RAM AT > FLASH
  /DISCARD/ : { *(.ARM.exidx .ARM.exidx.*) *(.ARM.attributes) }
}

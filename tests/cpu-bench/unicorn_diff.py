# SPDX-License-Identifier: GPL-3.0-or-later
"""Differential check of the dryflash CPU against Unicorn 2 (ADR-0001 spike).

Runs the benchmark image in Unicorn (Cortex-M4, Thumb, M-class) and compares the
architectural state before every instruction with the trace written by
`cpu-bench <bin> <rounds> --trace <file>`. Unicorn is a dev-only tool here and is
never distributed with dryflash (see ADR-0001, licence section).

usage: python unicorn_diff.py <bench-fw.bin> <trace> <rounds> <stop_pc_hex>
"""
import sys
from array import array

from unicorn import Uc, UC_ARCH_ARM, UC_MODE_THUMB, UC_MODE_MCLASS, UC_HOOK_CODE
from unicorn.arm_const import *

FLASH, FLASH_LEN = 0x08000000, 1 << 20
RAM, RAM_LEN = 0x20000000, 192 << 10
WORDS = 1 + 15 + 1 + 32  # pc, r0-r14, nzcv, s0-s31

image = open(sys.argv[1], "rb").read()
trace = array("I")
with open(sys.argv[2], "rb") as f:
    trace.frombytes(f.read())
rounds = int(sys.argv[3])
stop_pc = int(sys.argv[4], 16)

uc = Uc(UC_ARCH_ARM, UC_MODE_THUMB | UC_MODE_MCLASS)
uc.ctl_set_cpu_model(UC_CPU_ARM_CORTEX_M4)
uc.mem_map(FLASH, FLASH_LEN)
uc.mem_map(RAM, RAM_LEN)
uc.mem_map(0xE000E000, 0x1000)  # SCB page, so the CPACR write lands somewhere
uc.mem_write(FLASH, image)
uc.mem_write(RAM + 4, rounds.to_bytes(4, "little"))
sp = int.from_bytes(image[0:4], "little")
reset = int.from_bytes(image[4:8], "little")
uc.reg_write(UC_ARM_REG_SP, sp)
# Reset state per DDI 0403E B1.5.5: LR = 0xFFFFFFFF; APSR is UNKNOWN, both sides use 0.
uc.reg_write(UC_ARM_REG_LR, 0xFFFFFFFF)
uc.reg_write(UC_ARM_REG_XPSR, uc.reg_read(UC_ARM_REG_XPSR) & 0x0FFFFFFF)

REGS = [UC_ARM_REG_R0 + i for i in range(13)] + [UC_ARM_REG_SP, UC_ARM_REG_LR]
SREGS = [UC_ARM_REG_S0 + i for i in range(32)]
names = [f"r{i}" for i in range(13)] + ["sp", "lr"]
step = 0
skipped_it = [0]


def hook(uc, addr, size, _):
    global step
    base = step * WORDS
    if base + WORDS > len(trace):
        print(f"trace ended at step {step}")
        uc.emu_stop()
        return
    # Unicorn does not fire UC_HOOK_CODE for instructions skipped inside an IT block,
    # while dryflash retires them as a step (as the hardware does). Skip up to 4 such
    # steps when the PC is the only difference.
    for _ in range(4):
        if trace[base] == addr:
            break
        step += 1
        skipped_it[0] += 1
        base = step * WORDS
    t = trace[base : base + WORDS]
    nzcv = uc.reg_read(UC_ARM_REG_XPSR) >> 28
    ours = {"pc": t[0], "nzcv": t[16], **{n: t[1 + i] for i, n in enumerate(names)}}
    ref = {"pc": addr, "nzcv": nzcv, **{n: uc.reg_read(r) for n, r in zip(names, REGS)}}
    for i, r in enumerate(SREGS):
        ours[f"s{i}"] = t[17 + i]
        ref[f"s{i}"] = uc.reg_read(r)
    diff = {k: (hex(ours[k]), hex(ref[k])) for k in ref if ours[k] != ref[k]}
    if diff:
        prev = trace[(step - 1) * WORDS] if step else 0
        print(f"MISMATCH at step {step}, previous instruction @ {prev:#x}: (ours, unicorn) {diff}")
        uc.emu_stop()
        sys.exit(1)
    step += 1


uc.hook_add(UC_HOOK_CODE, hook)
uc.emu_start(reset | 1, stop_pc)
print(f"OK: {step} trace steps identical (registers, NZCV, S0-S31); {skipped_it[0]} IT-skipped steps not reported by Unicorn")

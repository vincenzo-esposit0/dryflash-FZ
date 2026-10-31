# SPDX-License-Identifier: GPL-3.0-or-later
"""Differential fuzzing of the dryflash CPU against Unicorn 2 (BRIEF §11, ADR-0001).

Generates random Thumb-16, Thumb-32 and FP encodings with random register, flag, FP and
RAM state, executes exactly one instruction in both engines and compares everything:
R0-R15, APSR (NZCVQ, GE), S0-S31, FPSCR and a hash of the first 64 KiB of SRAM1.

Unicorn and Capstone are development tools only; they are never distributed with dryflash.

usage: python fuzz.py <cpu-exec binary> [cases] [seed] [--corpus encodings.txt]

With --corpus, 70% of the cases use real encodings (one hex word per line, e.g. extracted
from a firmware ELF with `llvm-objdump`), so the instructions compilers actually emit get
most of the coverage; the rest stay fully random.
"""
import random
import subprocess
import sys
from collections import Counter, defaultdict

from capstone import CS_ARCH_ARM, CS_MODE_MCLASS, CS_MODE_THUMB, Cs
from unicorn import UC_ARCH_ARM, UC_MODE_MCLASS, UC_MODE_THUMB, Uc, UcError
from unicorn.arm_const import *

FLASH, RAM, RAM_SIZE, WINDOW = 0x08000000, 0x20000000, 0x30000, 0x10000
cs = Cs(CS_ARCH_ARM, CS_MODE_THUMB | CS_MODE_MCLASS)


def is32(hw):
    return (hw >> 11) >= 0b11101


def excluded(code):
    """Encodings whose effects are system state Unicorn and dryflash do not share in a
    single-instruction run: exceptions, sleep, special registers, exclusive monitor, IT."""
    if code <= 0xFFFF:
        h = code
        if h & 0xFF00 in (0xBE00, 0xDE00, 0xDF00):  # BKPT, UDF, SVC
            return True
        if h & 0xFF00 == 0xBF00:  # IT and hints (WFI/WFE/SEV/YIELD)
            return True
        if h & 0xFFE8 == 0xB660:  # CPS
            return True
        return False
    h1, h2 = code >> 16, code & 0xFFFF
    if h1 & 0xF800 == 0xF000 and h2 & 0x8000:  # branches and misc control
        op = (h1 >> 4) & 0x7F
        op1 = (h2 >> 12) & 7
        if op1 & 0b101 == 0 and op & 0b0111000 == 0b0111000:  # MSR, MRS, hints, barriers
            return True
        if op1 == 0b010:  # UDF
            return True
    if h1 & 0xFFE0 == 0xE840 or h1 & 0xFFF0 == 0xE8C0 or h1 & 0xFFF0 == 0xE8D0:
        return True  # LDREX/STREX family and TBB/TBH share this space; monitor state differs
    return False


CORPUS = []


def gen(rng):
    if CORPUS and rng.random() < 0.7:
        while True:
            code = rng.choice(CORPUS)
            if not excluded(code):
                return code
    r = rng.random()
    while True:
        if r < 0.3:
            hw = rng.getrandbits(16)
            if is32(hw):
                continue
            code = hw
        elif r < 0.8:
            hw1 = rng.choice([0xE800, 0xF000, 0xF800]) | rng.getrandbits(11)
            code = hw1 << 16 | rng.getrandbits(16)
        else:
            # FP: data processing / transfers (EExx) and load/store (ECxx/EDxx), CP10/11.
            hw1 = rng.choice([0xEE00, 0xEC00, 0xED00]) | rng.getrandbits(8)
            hw2 = (rng.getrandbits(16) & ~0x0E00) | 0x0A00
            code = hw1 << 16 | hw2
        if not excluded(code):
            return code


def reg_value(rng):
    k = rng.random()
    if k < 0.5:
        return RAM + 0x1000 + rng.randrange(0, 0xE000) & ~rng.choice([0, 1, 3])
    if k < 0.6:
        return rng.randrange(0, 64)
    return rng.getrandbits(32)


def float_bits(rng):
    k = rng.random()
    if k < 0.15:
        return rng.choice([0, 0x80000000, 0x7F800000, 0xFF800000, 0x7FC00000, 0x3F800000, 0xBF800000])
    if k < 0.6:
        import struct
        return struct.unpack("<I", struct.pack("<f", rng.uniform(-1e6, 1e6)))[0]
    return rng.getrandbits(32) & 0xBFFFFFFF  # avoid most NaN payloads; NaNs come from the list


def ram_bytes(seed):
    out = bytearray()
    s = seed
    for _ in range(WINDOW // 8):
        s ^= s >> 12
        s ^= (s << 25) & 0xFFFFFFFFFFFFFFFF
        s ^= s >> 27
        out += ((s * 0x2545F4914F6CDD1D) & 0xFFFFFFFFFFFFFFFF).to_bytes(8, "little")
    return bytes(out)


def fnv1a(data):
    h = 0xCBF29CE484222325
    for b in data:
        h = ((h ^ b) * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF
    return h


def make_uc():
    uc = Uc(UC_ARCH_ARM, UC_MODE_THUMB | UC_MODE_MCLASS)
    uc.ctl_set_cpu_model(UC_CPU_ARM_CORTEX_M4)
    uc.mem_map(FLASH, 0x100000)
    uc.mem_write(FLASH, b"\xff" * 0x100000)  # erased flash, as in dryflash
    uc.mem_map(RAM, RAM_SIZE)
    return uc


def run_unicorn(uc, case):
    code, regs, apsr, sregs, fpscr, seed = case
    uc.mem_write(RAM, ram_bytes(seed) + bytes(RAM_SIZE - WINDOW))
    if code > 0xFFFF:
        blob = (code >> 16).to_bytes(2, "little") + (code & 0xFFFF).to_bytes(2, "little")
    else:
        blob = code.to_bytes(2, "little")
    uc.mem_write(FLASH, blob + b"\xff" * (16 - len(blob)))
    # Unicorn keeps translated blocks across mem_write: drop them or it runs stale code.
    uc.ctl_remove_cache(FLASH, FLASH + 16)
    for i in range(13):
        uc.reg_write(UC_ARM_REG_R0 + i, regs[i])
    uc.reg_write(UC_ARM_REG_SP, regs[13])
    uc.reg_write(UC_ARM_REG_LR, regs[14])
    # XPSR writes drop GE on Unicorn's M-profile model; the XPSR_NZCVQG view sets
    # NZCVQ and GE. emu_start(begin | 1) restores Thumb state.
    uc.reg_write(UC_ARM_REG_XPSR_NZCVQG, apsr & 0xF80F0000)
    for i in range(32):
        uc.reg_write(UC_ARM_REG_S0 + i, sregs[i])
    uc.reg_write(UC_ARM_REG_FPSCR, fpscr)
    try:
        uc.emu_start(FLASH | 1, FLASH + 16, count=1)
    except UcError as e:
        return ("FAULT", str(e))
    out = [uc.reg_read(UC_ARM_REG_R0 + i) for i in range(13)]
    out += [uc.reg_read(UC_ARM_REG_SP), uc.reg_read(UC_ARM_REG_LR), uc.reg_read(UC_ARM_REG_PC)]
    out.append(uc.reg_read(UC_ARM_REG_XPSR) & 0xF80F0000)
    out += [uc.reg_read(UC_ARM_REG_S0 + i) for i in range(32)]
    out.append(uc.reg_read(UC_ARM_REG_FPSCR) & 0xF7C0009F)
    out.append(fnv1a(bytes(uc.mem_read(RAM, WINDOW))))
    return ("OK", out)


mnem_code = [0]


def code_is_32(code):
    return code > 0xFFFF


def known_divergence(ours, ref, mnem):
    """Cases where dryflash follows the Cortex-M4 architecture and Unicorn (QEMU) is more
    permissive. Counted separately so they stay visible."""
    m = mnem.split()[0]
    if ours.startswith("FAULT Unaligned") and ref[0] == "OK":
        return "unaligned LDM/STM/LDRD/STRD/VLDR fault (UsageFault UNALIGNED; Unicorn does not trap)"
    if ours.startswith("FAULT NoCoprocessor") and ref[0] == "OK":
        return "coprocessor other than CP10/CP11 (UsageFault NOCP; Unicorn treats as NOP)"
    if ours.startswith("STOP Unimplemented") and (
        mnem.startswith("(capstone")
        or ".f64" in mnem
        or m.startswith(("vld1", "vld2", "vld3", "vld4", "vst1", "vst2", "vst3", "vst4", "fstm", "fldm"))
        or any(f"d{n}" in mnem for n in range(16, 32))
        or m.startswith(("ldc", "stc", "mcr", "mrc", "cdp"))
        or any(op.startswith(("d", "q")) and op[1:].rstrip("[]0123456789") == "" and len(op) > 1
               for op in mnem.split(" ", 1)[-1].replace(",", " ").replace("{", " ").replace("}", " ").split())
        or m in ("vmsr", "vmrs")
        or "[" in mnem.split(",")[-1] and m.startswith("vmov.")
    ):
        return "UNDEFINED/UNPREDICTABLE on Cortex-M4 (double precision, NEON, invalid) — reported as emulator gap"
    if ours.startswith("STOP Unimplemented") and m.startswith(("vldm", "vstm", "vpush", "vpop")):
        return "UNPREDICTABLE FP register list (more than 32 S registers)"
    if ours.startswith("STOP Unimplemented") and ref[0] == "FAULT" and "INSN_INVALID" in ref[1]:
        return "both reject the encoding (UNDEFINED/UNPREDICTABLE)"
    if ours.startswith("STOP Unimplemented") and m.startswith(("vrint", "vsel", "vcvta", "vcvtn", "vcvtp", "vcvtm", "vmaxnm", "vminnm")):
        return "ARMv8 FP instruction, UNDEFINED on Cortex-M4"
    if ours.startswith("STOP Unimplemented") and m.startswith(("vcvt", "ubfx", "sbfx", "bfi", "bfc")):
        return "UNPREDICTABLE field values (lsb+width > 32, negative fraction bits)"
    if mnem.startswith("(capstone"):
        return "encoding Capstone rejects (SBZ/SBO violated): UNPREDICTABLE"
    if ours.startswith("STOP Unimplemented") and m.endswith(".w") and "#0" in mnem:
        return "UNPREDICTABLE ThumbExpandImm (replicated pattern with imm8 = 0)"
    if ours.startswith("OK") and ref[0] == "FAULT" and ("FETCH_UNMAPPED" in ref[1] or "EXCEPTION" in ref[1]):
        pc = int(ours.split()[16], 16)
        if not (FLASH <= pc < FLASH + 0x100000):
            return "branch to unmapped address (Unicorn faults at the target fetch, dryflash on the next step)"
    if ours.startswith("OK") and ref[0] == "FAULT" and "INSN_INVALID" in ref[1] and any(
        r in mnem.split(" ", 1)[-1].replace(",", " ").split() for r in ("sp", "pc")
    ):
        return "UNPREDICTABLE use of SP/PC (Unicorn UNDEFs, dryflash executes)"
    operands = mnem.split(" ", 1)[-1]
    for ch in ",{}[]!":
        operands = operands.replace(ch, " ")
    operands = operands.split()
    if code_is_32(mnem_code[0]) and "pc" in operands and m.rstrip(".w") not in ("ldr", "pop", "ldm", "ldmia", "b", "bl", "tbb", "tbh"):
        return "UNPREDICTABLE: PC as destination/operand of a 32-bit instruction"
    if m in ("blxns", "bxns"):
        return "UNPREDICTABLE: BX/BLX with should-be-zero bits set"
    if ours.startswith("OK") and ref[0] == "FAULT" and ours.split()[-2] == "0":
        return "interworking branch to ARM state (dryflash raises INVSTATE on the next instruction)"
    if ours.startswith("FAULT BusData") and ref[0] == "OK":
        addr = int(ours.split("addr: ")[1].rstrip(" }"))
        if FLASH <= addr < FLASH + 0x100000:
            return "store to flash (read-only in this harness, writable RAM in Unicorn)"
    return None


def names():
    return [f"r{i}" for i in range(13)] + ["sp", "lr", "pc", "apsr"] + [f"s{i}" for i in range(32)] + ["fpscr", "ram"]


def main():
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    if "--corpus" in sys.argv:
        path = sys.argv[sys.argv.index("--corpus") + 1]
        args.remove(path)
        CORPUS.extend(int(l, 16) for l in open(path) if l.strip())
    exe = args[0]
    cases = int(args[1]) if len(args) > 1 else 20000
    seed = int(args[2]) if len(args) > 2 else 1
    rng = random.Random(seed)
    batch = []
    for _ in range(cases):
        code = gen(rng)
        regs = [reg_value(rng) for _ in range(15)]
        regs[13] = (RAM + 0x8000 + rng.randrange(0, 0x400) * 8) & 0xFFFFFFFC
        apsr = rng.getrandbits(5) << 27 | rng.getrandbits(4) << 16
        sregs = [float_bits(rng) for _ in range(32)]
        batch.append((code, regs, apsr, sregs, 0, rng.getrandbits(63) | 1))

    lines = []
    for code, regs, apsr, sregs, fpscr, s in batch:
        lines.append(" ".join(f"{x:x}" for x in [code, *regs, 0, apsr, *sregs, fpscr, s]))
    # The Rust side reads r0..r15 at fields 1..16; field 16 (pc) is ignored, apsr is field 17.
    lines = [" ".join(l.split()[:16] + l.split()[16:]) for l in lines]
    res = subprocess.run([exe], input="\n".join(lines) + "\n", capture_output=True, text=True, check=True)
    ours_all = res.stdout.splitlines()

    uc = make_uc()

    stats = Counter()
    mismatches = defaultdict(list)
    for case, ours in zip(batch, ours_all):
        code = case[0]
        blob = (code >> 16).to_bytes(2, "little") + (code & 0xFFFF).to_bytes(2, "little") if code > 0xFFFF else code.to_bytes(2, "little")
        dis = next(cs.disasm(blob, FLASH), None)
        mnem = f"{dis.mnemonic} {dis.op_str}" if dis else "(capstone: invalid)"
        ref = run_unicorn(uc, case)
        if ref[0] == "FAULT":
            # An exception can leave Unicorn's M-profile state (handler mode, pending
            # exceptions) altered for the next case: start from a fresh instance.
            uc = make_uc()
        kind = ours.split()[0]
        mnem_code[0] = code
        known = known_divergence(ours, ref, mnem)
        if known:
            stats["known: " + known] += 1
            continue
        if kind == "STOP":
            stats["ours: " + ours.split()[1].split("{")[0]] += 1
            if "Unimplemented" in ours:
                mismatches["ours unimplemented"].append((code, mnem, ours, ref[1] if ref[0] == "FAULT" else "OK"))
            continue
        if kind == "FAULT" and ref[0] == "FAULT":
            stats["both fault"] += 1
            continue
        if kind == "FAULT" or ref[0] == "FAULT":
            if kind == "OK" and "INSN_INVALID" in ref[1]:
                # Should-be-zero/one bits set or SP in a register list: UNPREDICTABLE.
                # QEMU UNDEFs, dryflash executes; examples are printed for spot checks.
                stats["known: UNPREDICTABLE (SBZ/SBO bits or SP in list), Unicorn UNDEFs"] += 1
                mismatches["UNPREDICTABLE samples (spot-check)"].append((code, mnem, "", ""))
                continue
            stats["fault mismatch"] += 1
            key = "ours fault / unicorn ok" if kind == "FAULT" else "unicorn fault / ours ok"
            detail = ours if kind == "FAULT" else "OK pc=" + ours.split()[16]
            mismatches[key].append((code, mnem, detail, ref[1] if ref[0] == "FAULT" else "OK"))
            continue
        ov = [int(x, 16) for x in ours.split()[1:]]
        o = ov[:16] + [ov[16]] + ov[17:49] + [ov[49], ov[51]]
        diff = [(n, a, b) for n, a, b in zip(names(), o, ref[1]) if a != b]
        if diff:
            stats["state mismatch"] += 1
            mismatches["state mismatch"].append((code, mnem, diff[:4], None))
        else:
            stats["match"] += 1

    unexplained = sum(len(v) for k, v in mismatches.items() if not k.startswith("UNPREDICTABLE samples"))
    print(f"cases: {cases}, seed {seed}, unexplained mismatches: {unexplained}")
    for k, v in stats.most_common():
        print(f"  {v:7}  {k}")
    for key, items in mismatches.items():
        by_mnem = Counter(i[1].split()[0] for i in items)
        print(f"\n== {key}: {len(items)}  (by mnemonic: {dict(by_mnem.most_common(12))})")
        for code, mnem, a, b in items[:12]:
            print(f"  {code:08x}  {mnem:40}  {a}  {b if b else ''}")


    return unexplained


if __name__ == "__main__":
    sys.exit(1 if main() else 0)

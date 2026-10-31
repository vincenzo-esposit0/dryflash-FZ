// SPDX-License-Identifier: GPL-3.0-or-later
//! Runs the self-checking exception-model firmware (tests/exc-fw) on the full machine.
//! Build it first with `tests/build-fw.sh`.

use dryflash_core::cpu::Stop;
use dryflash_core::loader::dfu::Element;
use dryflash_core::machine::{Machine, RunExit};

const TESTS: &[&str] = &[
    "T0  SVC: basic frame, EXC_RETURN 0xFFFFFFF9, return value via stacked R0",
    "T1  stacked xPSR: Thumb bit, IPSR 0",
    "T2  PendSV on the process stack (EXC_RETURN 0xFFFFFFFD, frame on PSP)",
    "T3  FP context: extended frame (EXC_RETURN 0xFFFFFFE9), S0 stacked and restored",
    "T4  BASEPRI masks IRQ0, lowering it lets the IRQ in",
    "T5  SysTick wakes WFI three times",
    "T6  UsageFault UNDEFINSTR",
    "T7  UsageFault disabled: escalation to HardFault (HFSR.FORCED)",
    "T8  nested IRQ returns to Handler mode (EXC_RETURN 0xFFFFFFF1)",
    "T9  preemption order IRQ0 → IRQ1 → IRQ0",
    "T10 MPU NULL guard: MemManage DACCVIOL, MMFAR = 4",
    "T11 8-byte stack alignment on entry (xPSR bit 9)",
    "T12 three faults handled and resumed",
];

#[test]
fn exception_model_conformance() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/exc-fw.bin");
    let image = std::fs::read(path).unwrap_or_else(|e| panic!("{path}: {e} — run tests/build-fw.sh first"));
    let mut m = Machine::new();
    m.load_elements(
        &[Element {
            address: 0x0800_0000,
            data: image,
        }],
        None,
    )
    .unwrap();
    m.cpu.halt_on_bkpt = true;
    m.power_on();
    let exit = m.run(10_000_000);
    assert_eq!(
        exit,
        RunExit::Stopped(Stop::Breakpoint { imm: 0, pc: m.cpu.pc() }),
        "{}",
        m.digest().render(&|_| None, 8)
    );

    let word = |i: usize| u32::from_le_bytes(m.bus.sram1[4 * i..4 * i + 4].try_into().unwrap());
    assert_eq!(word(0), 0xE7C0_0001, "results magic");
    let (pass, fail) = (word(1), word(2));
    let mut report = String::new();
    for (n, name) in TESTS.iter().enumerate() {
        let ok = pass & (1 << n) != 0 && fail & (1 << n) == 0;
        report += &format!(
            "{} {name}{}\n",
            if ok { "PASS" } else { "FAIL" },
            if ok {
                String::new()
            } else {
                format!(" (detail 0x{:08X})", word(3 + n))
            }
        );
    }
    println!("{report}");
    assert_eq!((pass, fail), ((1 << TESTS.len()) - 1, 0), "\n{report}");
}

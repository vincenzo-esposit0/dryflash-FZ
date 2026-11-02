// SPDX-License-Identifier: GPL-3.0-or-later
//! Crash and hang detection (BRIEF §7) on small hand-assembled firmwares, and on the real one with a
//! fault or a hang patched into it, so the firmware's own crash path runs.
//!
//! The tests on the real firmware need it: `node scripts/fetch-firmware.mjs official --elf`.

use dryflash_core::board::Button;
use dryflash_core::cpu::Fault;
use dryflash_core::crash::{CrashKind, FirmwareInfo, Report, Verdict};
use dryflash_core::loader::dfu::{self, Element};
use dryflash_core::loader::elf::Symbols;
use dryflash_core::machine::{Machine, RunExit};

const DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../firmware/cache/official");
const MS: u64 = 64_000;
/// HCLK at reset: MSI at 4 MHz (RM0434, RCC).
const RESET_HZ: u64 = 4_000_000;

/// A firmware with `code` at 0x100 and a HardFault handler that spins at 0x180.
fn machine(code: &[u16], pool: &[u32]) -> Machine {
    let mut img = vec![0u8; 0x400];
    img[0..4].copy_from_slice(&0x2003_0000u32.to_le_bytes());
    img[4..8].copy_from_slice(&0x0800_0101u32.to_le_bytes());
    img[12..16].copy_from_slice(&0x0800_0181u32.to_le_bytes());
    for (i, hw) in code.iter().enumerate() {
        img[0x100 + 2 * i..0x102 + 2 * i].copy_from_slice(&hw.to_le_bytes());
    }
    for (i, w) in pool.iter().enumerate() {
        img[0x140 + 4 * i..0x144 + 4 * i].copy_from_slice(&w.to_le_bytes());
    }
    img[0x180..0x182].copy_from_slice(&0xE7FEu16.to_le_bytes()); // b .
    let mut m = Machine::new();
    m.load_elements(
        &[Element {
            address: 0x0800_0000,
            data: img,
        }],
        None,
    )
    .unwrap();
    m
}

fn report(m: &Machine) -> Report {
    Report::new(
        m.captures.last().expect("a report"),
        &m.bus,
        None,
        &FirmwareInfo::default(),
    )
}

#[test]
fn a_tight_loop_is_a_hang_and_its_pcs_are_kept() {
    let mut m = machine(&[0xBF00, 0xBF00, 0xE7FC], &[]); // nop; nop; b back to the first nop
    m.power_on();
    assert_eq!(m.run(RESET_HZ * 4), RunExit::Budget, "not after 4 s");
    assert_eq!(m.run(RESET_HZ * 2), RunExit::Hung, "after 5 s");
    let r = report(&m);
    assert_eq!(r.kind, "hang");
    assert!(matches!(
        m.captures[0].kind,
        CrashKind::TightLoop { lo: 0x0800_0100.., .. }
    ));
    assert_eq!(r.recent_pcs.len(), 64);
    assert!(r.recent_pcs.iter().all(|(p, _)| (0x0800_0100..0x0800_0106).contains(p)));
    assert_eq!(r.verdict, Verdict::FirmwareBug, "{:?}", r.basis);
    // Carrying on is allowed, and takes the same time again to decide.
    assert_eq!(m.run(RESET_HZ * 4), RunExit::Budget);
}

#[test]
fn a_button_or_a_sleep_restarts_the_hang_timer() {
    let mut m = machine(&[0xE7FE], &[]);
    m.power_on();
    assert_eq!(m.run(RESET_HZ * 4), RunExit::Budget);
    m.set_button(Button::Ok, true);
    assert_eq!(m.run(RESET_HZ * 4), RunExit::Budget, "the press counts as activity");
    assert_eq!(m.run(RESET_HZ * 2), RunExit::Hung);

    // wfi; b back. Nothing can wake it, which is a report of its own.
    let mut m = machine(&[0xBF30, 0xE7FD], &[]);
    m.power_on();
    assert!(matches!(m.run(RESET_HZ * 10), RunExit::IdleForever { .. }));
    assert_eq!(report(&m).verdict, Verdict::Unclear);
}

#[test]
fn hang_detection_is_off_with_the_sleep_method_that_never_sleeps() {
    let mut m = machine(&[0xE7FE], &[]);
    m.bus.legacy_sleep = false;
    m.power_on();
    assert_eq!(m.run(RESET_HZ * 60), RunExit::Budget);
}

#[test]
fn a_fault_is_reported_as_it_was_before_the_exception() {
    // ldr r0, =0x30000000 ; str r0, [r0]: nothing exists there.
    let mut m = machine(&[0x4800, 0x6000], &[]);
    m.load_elements(
        &[Element {
            address: 0x0800_0104,
            data: 0x3000_0000u32.to_le_bytes().to_vec(),
        }],
        None,
    )
    .unwrap();
    m.power_on();
    assert_eq!(m.run(RESET_HZ * 6), RunExit::Hung, "the handler spins");
    let r = report(&m);
    assert_eq!(r.state.fault, Some(Fault::BusData { addr: 0x3000_0000 }));
    assert_eq!(r.state.r[15], 0x0800_0102, "the store, not the handler");
    assert_eq!(r.state.r[0], 0x3000_0000);
    assert_eq!(r.state.bfar, 0x3000_0000);
    assert!(r.fault_causes.iter().any(|c| c.starts_with("PRECISERR")));
    assert_eq!(r.state.recent_pcs.last(), Some(&0x0800_0102));
    assert_eq!(r.verdict, Verdict::FirmwareBug, "{:?}", r.basis);
    assert!(r.basis[0].contains("0x30000000"), "{:?}", r.basis);
}

#[test]
fn an_undefined_instruction_could_be_either() {
    let mut m = machine(&[0xDE00], &[]); // udf #0
    m.power_on();
    assert_eq!(m.run(RESET_HZ * 6), RunExit::Hung);
    assert_eq!(report(&m).verdict, Verdict::Unclear);
}

#[test]
fn the_watchdog_resets_the_system_and_is_reported() {
    // ldr r0, =IWDG ; ldr r1, =start key ; str r1, [r0] ; b .   Reset values: 4096 counts at 8 kHz, 512 ms.
    let mut m = machine(&[0x4805, 0x4906, 0x6001, 0xE7FE], &[]);
    m.load_elements(
        &[Element {
            address: 0x0800_0118,
            data: [0x4000_3000u32, 0xCCCC].iter().flat_map(|w| w.to_le_bytes()).collect(),
        }],
        None,
    )
    .unwrap();
    m.power_on();
    assert_eq!(m.run(RESET_HZ / 4), RunExit::Budget);
    assert!(m.bus.periphs.iwdg.deadline_ns.is_some(), "started");
    assert_eq!(m.run(RESET_HZ), RunExit::Crashed);
    assert_eq!(report(&m).kind, "watchdog");
    assert_eq!(m.resets, 1);
    assert_eq!(m.bus.periphs.iwdg.deadline_ns, None, "a reset stops it");
}

#[test]
fn a_loop_right_after_using_a_stubbed_chip_looks_like_the_emulator() {
    let mut m = machine(&[0xE7FE], &[]);
    m.power_on();
    // At this clock the machine looks every 160 ms: this is the last look before it decides.
    assert_eq!(m.run(RESET_HZ * 51 / 10), RunExit::Budget);
    m.bus.periphs.cc1101.transfer(0x3D);
    assert_eq!(m.run(RESET_HZ), RunExit::Hung);
    let r = report(&m);
    assert_eq!(r.verdict, Verdict::EmulatorLimitation, "{:?}", r.basis);
    assert!(r.gaps.iter().any(|g| g.block == "CC1101 (stub)"), "{:?}", r.gaps);
}

#[test]
fn reports_are_json_and_markdown() {
    let mut m = machine(&[0xDE00], &[]);
    m.power_on();
    m.run(RESET_HZ * 6);
    let info = FirmwareInfo {
        name: "a \"quoted\" name".to_string(),
        version: Some("1.2.3".to_string()),
        sha256: None,
    };
    let r = Report::new(&m.captures[0], &m.bus, None, &info);
    let json = r.to_json();
    assert!(json.contains(r#""name":"a \"quoted\" name""#), "{json}");
    assert!(json.contains(r#""verdict":"unclear""#), "{json}");
    assert!(json.contains(r#""sha256":null"#), "{json}");
    let md = r.to_markdown();
    assert!(md.contains("## Could be either"), "{md}");
    assert!(md.contains("UNDEFINSTR"), "{md}");
}

fn official() -> (Machine, Symbols) {
    let dfu_file = std::fs::read(format!("{DIR}/f7-update-1.5.1/firmware.dfu")).expect("fetch the firmware first");
    let elf = std::fs::read(format!("{DIR}/flipper-z-f7-firmware-1.5.1.elf")).expect("fetch the ELF first");
    let mut m = Machine::new();
    m.load_dfu(&dfu::parse(&dfu_file).unwrap(), Some(0x080D_7000)).unwrap();
    (m, Symbols::from_elf(&elf).unwrap())
}

fn patch(m: &mut Machine, at: u32, code: &[u16]) {
    let data = code.iter().flat_map(|h| h.to_le_bytes()).collect();
    m.load_elements(&[Element { address: at, data }], Some(0x080D_7000))
        .unwrap();
}

/// To the Desktop, then OK, which calls `loader_show_menu`.
fn open_menu(m: &mut Machine) -> RunExit {
    assert_eq!(m.run(5_000 * MS), RunExit::Budget);
    for b in [Button::Back, Button::Ok] {
        m.set_button(b, true);
        assert_eq!(m.run(100 * MS), RunExit::Budget, "while pressing {b:?}");
        m.set_button(b, false);
        let exit = m.run(900 * MS);
        if exit != RunExit::Budget {
            return exit;
        }
    }
    RunExit::Budget
}

#[test]
#[ignore = "needs firmware/cache/official (scripts/fetch-firmware.mjs official --elf)"]
fn a_fault_in_the_real_firmware_runs_its_own_crash_path() {
    let (mut m, syms) = official();
    let target = syms.address_of("loader_show_menu").unwrap();
    patch(&mut m, target, &[0xDE00]); // udf #0
    m.power_on();
    assert_eq!(open_menu(&mut m), RunExit::Crashed);

    let r = Report::new(&m.captures[0], &m.bus, Some(&syms), &FirmwareInfo::default());
    assert_eq!(r.kind, "crash");
    assert_eq!(r.state.fault, Some(Fault::Undefined));
    assert_eq!(r.state.r[15], target, "the registers are from the fault");
    let furi = r.furi.as_ref().expect("Furi printed its record");
    assert_eq!(furi.message, "UsageFault");
    assert!(furi.task.as_deref().is_some_and(|t| t.starts_with("ISR")), "{furi:?}");
    assert_eq!(furi.registers.len(), 13, "{furi:?}");
    assert_eq!(r.backtrace[0].symbol.as_deref(), Some("loader_show_menu"));
    assert!(
        r.backtrace
            .iter()
            .any(|f| f.symbol.as_deref().is_some_and(|s| s.starts_with("desktop_"))),
        "{:?}",
        r.backtrace
    );
    assert_eq!(r.recent_pcs.last().map(|p| p.0), Some(target));
    assert_eq!(r.inputs.len(), 4, "Back and OK, pressed and released: {:?}", r.inputs);
    assert_eq!(m.crashes[0].message.as_deref(), Some("UsageFault"));

    // On a real device it would have rebooted into its crash scene: carrying on does that.
    assert_eq!(m.run(3_000 * MS), RunExit::Budget);
    assert_eq!(m.resets, 1);
}

#[test]
#[ignore = "needs firmware/cache/official (scripts/fetch-firmware.mjs official --elf)"]
fn with_an_elf_a_furi_check_has_the_pcs_from_before_the_crash() {
    let (mut m, syms) = official();
    let crash = syms.address_of("__furi_crash_implementation").unwrap();
    let halt = syms.address_of("__furi_halt_implementation").unwrap();
    m.set_crash_entries(&[crash, halt]);
    let target = syms.address_of("loader_show_menu").unwrap();
    // Into Furi's crash code, as furi_crash(NULL) gets there.
    let branch = thumb_b_w(target, crash);
    patch(&mut m, target, &branch);
    m.power_on();
    assert_eq!(open_menu(&mut m), RunExit::Crashed);

    let r = Report::new(&m.captures[0], &m.bus, Some(&syms), &FirmwareInfo::default());
    assert_eq!(r.state.fault, None);
    assert_eq!(r.state.r[15], crash, "from the entry");
    assert!(r.pcs_note.contains("leading into"));
    assert_eq!(
        r.recent_pcs.last().map(|p| p.0),
        Some(target),
        "the patched branch came last"
    );
    assert_eq!(r.verdict, Verdict::FirmwareBug, "{:?}", r.basis);
    let furi = r.furi.as_ref().unwrap();
    assert!(!furi.task.as_deref().unwrap_or("ISR").starts_with("ISR"), "{furi:?}");
}

/// `b.w` from `from` to `to` (DDI 0403E A7.7.12, encoding T4).
fn thumb_b_w(from: u32, to: u32) -> [u16; 2] {
    let off = to.wrapping_sub(from + 4) as i32;
    let s = ((off >> 24) & 1) as u16;
    let i1 = ((off >> 23) & 1) as u16;
    let i2 = ((off >> 22) & 1) as u16;
    let imm10 = ((off >> 12) & 0x3FF) as u16;
    let imm11 = ((off >> 1) & 0x7FF) as u16;
    let j1 = (!(i1 ^ s)) & 1;
    let j2 = (!(i2 ^ s)) & 1;
    [0xF000 | (s << 10) | imm10, 0x9000 | (j1 << 13) | (j2 << 11) | imm11]
}

#[test]
#[ignore = "needs firmware/cache/official (scripts/fetch-firmware.mjs official --elf)"]
fn the_radio_chips_start_and_their_apps_close() {
    // Before the stubs, closing Sub-GHz crashed: furi_check in furi_hal_subghz_sleep (BRIEF §5.2).
    let (mut m, _) = official();
    m.power_on();
    assert_eq!(open_menu(&mut m), RunExit::Budget);
    let log = String::from_utf8_lossy(&m.bus.periphs.usart[0].tx).into_owned();
    assert!(log.contains("[FuriHalSubGhz]") && log.contains("Init OK"), "{log}");
    assert!(!log.contains("Wrong chip id"), "{log}");
    assert!(!log.contains("Init Fail"), "{log}");
}

#[test]
#[ignore = "needs firmware/cache/official (scripts/fetch-firmware.mjs official --elf)"]
fn a_hang_in_the_real_firmware_is_found() {
    let (mut m, syms) = official();
    let target = syms.address_of("loader_show_menu").unwrap();
    patch(&mut m, target, &[0xB672, 0xE7FE]); // cpsid i; b .
    m.power_on();
    assert_eq!(open_menu(&mut m), RunExit::Budget);
    let mut exit = RunExit::Budget;
    for _ in 0..7 {
        exit = m.run(1_000 * MS);
        if exit != RunExit::Budget {
            break;
        }
    }
    assert_eq!(exit, RunExit::Hung);
    let r = Report::new(&m.captures[0], &m.bus, Some(&syms), &FirmwareInfo::default());
    assert_eq!(r.kind, "hang");
    assert!(
        matches!(m.captures[0].kind, CrashKind::TightLoop { .. }),
        "{:?}",
        m.captures[0].kind
    );
    assert_eq!(
        r.backtrace[0].symbol.as_deref().map(|s| s.split('+').next().unwrap()),
        Some("loader_show_menu")
    );
}

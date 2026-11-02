// SPDX-License-Identifier: GPL-3.0-or-later
//! Crash records and reports (BRIEF §7).
//!
//! In release builds, `__furi_crash_implementation` (furi/core/check.c, firmware 1.5.1)
//! stores a pointer to the crash message in RTC backup register `FuriHalRtcRegisterFaultData`
//! (index 4 of `FuriHalRtcRegister`, targets/f7/furi_hal/furi_hal_rtc.h) and then resets.
//! The message is a NUL-terminated string in flash.

use crate::bus::SystemBus;
use crate::bus::map::{FLASH_BASE, FLASH_SIZE};
use crate::cpu::{Fault, Snapshot};
use crate::loader::elf::Symbols;
use crate::trace::digest::GapTouch;
use std::fmt::Write;

/// RTC backup register holding Furi's fault message pointer.
pub const FURI_FAULT_DATA_BKP: usize = 4;

/// Reads the message a pointer in flash points to (at most 128 bytes, printable ASCII).
pub fn flash_string(bus: &SystemBus, ptr: u32) -> Option<String> {
    let off = ptr.checked_sub(FLASH_BASE)? as usize;
    if off >= FLASH_SIZE as usize {
        return None;
    }
    let bytes: Vec<u8> = bus.flash[off..]
        .iter()
        .take(128)
        .take_while(|&&b| b != 0)
        .copied()
        .collect();
    if bytes.is_empty() || !bytes.iter().all(|b| b.is_ascii_graphic() || *b == b' ') {
        return None;
    }
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// The fault pointer Furi left in the RTC backup register, and its message if it decodes.
pub fn furi_fault(bus: &SystemBus) -> Option<(u32, Option<String>)> {
    let ptr = bus.periphs.rtc.backup(FURI_FAULT_DATA_BKP);
    (ptr != 0).then(|| (ptr, flash_string(bus, ptr)))
}

/// Furi's record of a crash, from what `__furi_crash_implementation` prints on USART1
/// (furi/core/check.c, firmware 1.5.1): `[CRASH][task] message`, then r0–r11 and lr in hex.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FuriRecord {
    /// Task name, or `ISR name` when it crashed in an interrupt.
    pub task: Option<String>,
    pub message: String,
    /// r0–r11 as the firmware saved them, then lr.
    pub registers: Vec<(String, u32)>,
}

const CRASH_MARK: &[u8] = b"[CRASH]";
const HALT_MARK: &[u8] = b"[HALT]";
/// Printed by check.c when the firmware waits for a debugger instead of resetting.
const HALTED: &[&[u8]] = &[b"System halted. Connect debugger", b"System halted. Bye-bye!"];

fn rfind(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).rposition(|w| w == needle)
}

/// Has this part of the log got to the point where Furi halts for good?
pub fn furi_log_halted(log: &[u8]) -> bool {
    HALTED.iter().any(|m| rfind(log, m).is_some())
}

/// The last crash record in this part of the log.
pub fn furi_log_record(log: &[u8]) -> Option<FuriRecord> {
    let start = rfind(log, CRASH_MARK)
        .map(|i| i + CRASH_MARK.len())
        .or_else(|| rfind(log, HALT_MARK).map(|i| i + HALT_MARK.len()))?;
    let text = String::from_utf8_lossy(&log[start..]);
    let mut lines = text.split("\r\n");
    let first = lines.next().unwrap_or("");
    // __furi_print_name: "[name] " or "[ISR name] ".
    let (task, message) = match first.strip_prefix('[').and_then(|r| r.split_once("] ")) {
        Some((name, rest)) => (Some(name.to_string()), rest.to_string()),
        None => (None, first.to_string()),
    };
    let registers = lines
        .filter_map(|l| {
            let (name, value) = l.trim().split_once(" : ")?;
            let is_register = name == "lr" || name.strip_prefix('r').is_some_and(|n| n.parse::<u8>().is_ok());
            if !is_register {
                return None;
            }
            // __furi_put_uint32_as_hex prints itoa(data, 16): no prefix.
            Some((name.to_string(), u32::from_str_radix(value.trim(), 16).ok()?))
        })
        .collect();
    Some(FuriRecord {
        task,
        message,
        registers,
    })
}

/// What happened, as the detectors saw it (BRIEF §7).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CrashKind {
    /// Furi's own crash path ran.
    FirmwareCrash {
        message: String,
    },
    /// Furi's halt, or a build that halts instead of resetting.
    Halted,
    /// A fault, then a reset, with no word from Furi.
    Fault {
        reset_pc: u32,
    },
    BootLoop {
        resets: u32,
        window_s: u32,
    },
    /// The code stayed in `lo..=hi` with no sleep, display change or input.
    TightLoop {
        lo: u32,
        hi: u32,
        seconds: u32,
    },
    /// No sleep, display change or input, wherever the code was.
    Busy {
        seconds: u32,
    },
    /// Asleep with nothing scheduled that could wake it.
    IdleForever {
        pc: u32,
    },
    /// An encoding the emulator does not implement.
    Unimplemented {
        raw: u32,
        pc: u32,
    },
    /// A fault while handling HardFault or NMI.
    Lockup {
        pc: u32,
    },
    /// The independent watchdog ran out.
    Watchdog,
}

/// A button change, for the report's input history.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InputEvent {
    pub at_ns: u128,
    pub button: crate::board::Button,
    pub pressed: bool,
}

/// The raw material of a report, taken when it happened.
#[derive(Clone, Debug)]
pub struct Capture {
    pub at_ns: u128,
    pub kind: CrashKind,
    pub furi: Option<FuriRecord>,
    /// The first recent fault, before its exception was taken.
    pub fault: Option<Snapshot>,
    /// The state on entering Furi's crash code. Needs an ELF to know where that is.
    pub entry: Option<Snapshot>,
    /// The state when this was detected.
    pub live: Snapshot,
    pub hfsr: u32,
    pub lcd: Vec<u8>,
    pub inputs: Vec<InputEvent>,
    pub gaps: Vec<GapTouch>,
    pub unmapped: Vec<u32>,
    pub log_tail: String,
}

/// Whose problem this looks like. Never a certainty: `basis` says why.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    FirmwareBug,
    EmulatorLimitation,
    Unclear,
}

impl Verdict {
    pub fn id(self) -> &'static str {
        match self {
            Verdict::FirmwareBug => "firmware",
            Verdict::EmulatorLimitation => "emulator",
            Verdict::Unclear => "unclear",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Verdict::FirmwareBug => "Looks like a firmware bug",
            Verdict::EmulatorLimitation => "Looks like an emulator limitation",
            Verdict::Unclear => "Could be either",
        }
    }
}

/// What a report says about the firmware; the host knows it, the core does not.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FirmwareInfo {
    pub name: String,
    pub version: Option<String>,
    pub sha256: Option<String>,
}

/// One line of a backtrace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub address: u32,
    pub symbol: Option<String>,
    /// Where it was found: `pc`, `lr`, `furi lr` or `stack`.
    pub source: &'static str,
}

/// A readable report on a [`Capture`] (BRIEF §7).
#[derive(Clone, Debug)]
pub struct Report {
    pub firmware: FirmwareInfo,
    pub at_ms: u64,
    pub kind: &'static str,
    pub summary: String,
    pub verdict: Verdict,
    pub basis: Vec<String>,
    /// Which moment the registers are from.
    pub state_from: &'static str,
    pub state: Snapshot,
    pub fault_causes: Vec<&'static str>,
    pub hfsr: u32,
    pub furi: Option<FuriRecord>,
    pub backtrace: Vec<Frame>,
    /// Which moment the PCs are from, and how far to trust them.
    pub pcs_note: &'static str,
    pub recent_pcs: Vec<(u32, Option<String>)>,
    pub inputs: Vec<InputEvent>,
    pub gaps: Vec<GapTouch>,
    pub log_tail: String,
}

/// CFSR's bits (DDI 0403E B3.2.15).
fn fault_causes(cfsr: u32) -> Vec<&'static str> {
    const BITS: &[(u32, &str)] = &[
        (0, "IACCVIOL: instruction access violation"),
        (1, "DACCVIOL: data access violation"),
        (3, "MUNSTKERR: MemManage fault on unstacking"),
        (4, "MSTKERR: MemManage fault on stacking"),
        (8, "IBUSERR: instruction bus error"),
        (9, "PRECISERR: precise data bus error"),
        (10, "IMPRECISERR: imprecise data bus error"),
        (11, "UNSTKERR: bus fault on unstacking"),
        (12, "STKERR: bus fault on stacking"),
        (16, "UNDEFINSTR: undefined instruction"),
        (17, "INVSTATE: invalid EPSR state"),
        (18, "INVPC: invalid EXC_RETURN"),
        (19, "NOCP: no coprocessor"),
        (24, "UNALIGNED: unaligned access"),
        (25, "DIVBYZERO: division by zero"),
    ];
    BITS.iter()
        .filter(|(b, _)| cfsr >> b & 1 != 0)
        .map(|(_, s)| *s)
        .collect()
}

/// Could this stack word be a return address? It must point after a BL or BLX in flash
/// (DDI 0403E A7.7.18, A7.7.19).
fn return_address(bus: &SystemBus, word: u32) -> Option<u32> {
    if word & 1 == 0 {
        return None;
    }
    let ret = word & !1;
    let off = ret.checked_sub(FLASH_BASE)? as usize;
    if off < 4 || off >= FLASH_SIZE as usize {
        return None;
    }
    let half = |o: usize| u16::from_le_bytes([bus.flash[o], bus.flash[o + 1]]);
    let (hi, lo) = (half(off - 4), half(off - 2));
    let bl = hi & 0xF800 == 0xF000 && lo & 0xD000 == 0xD000;
    let blx = lo & 0xFF87 == 0x4780;
    (bl || blx).then_some(ret)
}

const MAX_FRAMES: usize = 16;

fn backtrace(bus: &SystemBus, state: &Snapshot, furi: Option<&FuriRecord>, symbols: Option<&Symbols>) -> Vec<Frame> {
    let name = |a: u32| symbols.and_then(|s| s.lookup(a));
    let mut frames = vec![Frame {
        address: state.r[15],
        symbol: name(state.r[15]),
        source: "pc",
    }];
    let mut push = |address: u32, source| {
        if frames.len() < MAX_FRAMES && frames.iter().all(|f| f.address != address) {
            frames.push(Frame {
                address,
                symbol: name(address),
                source,
            });
        }
    };
    if let Some(lr) = return_address(bus, state.r[14]) {
        push(lr, "lr");
    }
    if let Some(lr) = furi.and_then(|f| f.registers.iter().find(|(n, _)| n == "lr")) {
        push(lr.1 & !1, "furi lr");
    }
    for &w in &state.stack {
        if let Some(a) = return_address(bus, w) {
            push(a, "stack");
        }
    }
    frames
}

/// Whose problem it looks like, and why. The basis is part of the answer: this is a heuristic.
fn judge(c: &Capture, state: &Snapshot, flash_raw: Option<u32>) -> (Verdict, Vec<String>) {
    match &c.kind {
        CrashKind::Unimplemented { raw, pc } => {
            return (
                Verdict::EmulatorLimitation,
                vec![format!(
                    "The emulator does not implement the encoding 0x{raw:08X} at 0x{pc:08X}."
                )],
            );
        }
        CrashKind::IdleForever { .. } => {
            return (
                Verdict::Unclear,
                vec!["The CPU sleeps and nothing scheduled can wake it. A peripheral the emulator lacks could be what was meant to.".to_string()],
            );
        }
        _ => {}
    }
    let mut basis = Vec::new();
    match state.fault {
        Some(Fault::Undefined) => {
            let raw = flash_raw.map_or_else(|| "not in flash".to_string(), |r| format!("0x{r:08X}"));
            basis.push(format!(
                "The fault is an undefined instruction ({raw}). Either the firmware ran into data, or the emulator's decoder lacks a valid encoding."
            ));
            return (Verdict::Unclear, basis);
        }
        Some(Fault::BusData { addr }) if c.unmapped.contains(&addr) => {
            basis.push(format!(
                "Nothing exists at 0x{addr:08X} in this chip's memory map (RM0434), so hardware would fault too."
            ));
        }
        Some(f) => basis.push(format!(
            "The CPU raised {f:?}, which comes from the instruction, not from a device."
        )),
        None => {}
    }
    let names = |only_new: bool| {
        c.gaps
            .iter()
            .filter(|g| g.new || !only_new)
            .map(|g| g.block.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    // A loop that stays put and keeps touching a register with no behaviour is probably waiting on it.
    if matches!(c.kind, CrashKind::TightLoop { .. }) && state.fault.is_none() && !c.gaps.is_empty() {
        basis.push(format!(
            "While stuck it touched registers the emulator has no behaviour for: {}. It may be waiting for something that never comes.",
            names(false)
        ));
        return (Verdict::EmulatorLimitation, basis);
    }
    if c.gaps.iter().any(|g| g.new) {
        basis.push(format!(
            "Just before this it touched, for the first time in 1 s, registers the emulator has no behaviour for: {}.",
            names(true)
        ));
        return (Verdict::Unclear, basis);
    }
    basis.push(if c.gaps.is_empty() {
        "It touched no register without a behavioural model in the last 100 ms.".to_string()
    } else {
        format!(
            "The registers with no behaviour it touched in the last 100 ms ({}) had been in use for longer than 1 s without trouble.",
            names(false)
        )
    });
    (Verdict::FirmwareBug, basis)
}

impl Report {
    /// Builds the report. The bus must still hold the same firmware in flash.
    pub fn new(c: &Capture, bus: &SystemBus, symbols: Option<&Symbols>, firmware: &FirmwareInfo) -> Report {
        let (state_from, state) = match (&c.fault, &c.entry) {
            (Some(f), _) => ("the fault, before its exception was taken", f),
            (None, Some(e)) => ("the entry of Furi's crash code", e),
            (None, None) => ("when this was detected", &c.live),
        };
        let pcs_note = match (&c.kind, &c.fault, &c.entry) {
            (_, Some(_), _) => "The instructions leading to the fault.",
            (_, None, Some(_)) => "The instructions leading into Furi's crash code.",
            (CrashKind::FirmwareCrash { .. } | CrashKind::Halted, None, None) => {
                "These are Furi printing its crash record, not what led to it. Load the firmware's ELF to get those."
            }
            _ => "The last instructions before this was detected.",
        };
        let name = |a: u32| symbols.and_then(|s| s.lookup(a));
        let flash_raw = state.r[15].checked_sub(FLASH_BASE).and_then(|o| {
            let o = o as usize;
            let b = bus.flash.get(o..o + 4)?;
            Some(u32::from_le_bytes([b[2], b[3], b[0], b[1]]))
        });
        let (verdict, basis) = judge(c, state, flash_raw);
        let (kind, summary) = match &c.kind {
            CrashKind::FirmwareCrash { message } => ("crash", format!("The firmware crashed: {message}")),
            CrashKind::Halted => ("crash", "The firmware halted itself.".to_string()),
            CrashKind::Fault { .. } => ("fault", "A CPU fault, followed by a reset.".to_string()),
            CrashKind::BootLoop { resets, window_s } => {
                ("boot-loop", format!("Boot loop: {resets} resets within {window_s} s."))
            }
            CrashKind::TightLoop { lo, hi, seconds } => (
                "hang",
                format!(
                    "Hang: {seconds} s in the loop at 0x{lo:08X}–0x{hi:08X}, with no sleep, display change or input."
                ),
            ),
            CrashKind::Busy { seconds } => (
                "hang",
                format!("Hang: {seconds} s with no sleep, display change or input."),
            ),
            CrashKind::IdleForever { pc } => ("hang", format!("The CPU sleeps at 0x{pc:08X} and nothing can wake it.")),
            CrashKind::Unimplemented { raw, pc } => (
                "emulator-gap",
                format!("Instruction 0x{raw:08X} at 0x{pc:08X} is not implemented by the emulator."),
            ),
            CrashKind::Watchdog => (
                "watchdog",
                "The independent watchdog ran out: the firmware stopped refreshing it.".to_string(),
            ),
            CrashKind::Lockup { pc } => (
                "lockup",
                format!("The CPU locked up at 0x{pc:08X}: a fault while handling a fault."),
            ),
        };
        Report {
            firmware: firmware.clone(),
            at_ms: (c.at_ns / 1_000_000) as u64,
            kind,
            summary,
            verdict,
            basis,
            state_from,
            state: state.clone(),
            fault_causes: fault_causes(state.cfsr),
            hfsr: c.hfsr,
            furi: c.furi.clone(),
            backtrace: backtrace(bus, state, c.furi.as_ref(), symbols),
            pcs_note,
            recent_pcs: state.recent_pcs.iter().map(|&p| (p, name(p))).collect(),
            inputs: c.inputs.clone(),
            gaps: c.gaps.clone(),
            log_tail: c.log_tail.clone(),
        }
    }

    pub fn to_json(&self) -> String {
        let hex = |v: u32| json_str(&format!("0x{v:08X}"));
        let opt = |s: &Option<String>| s.as_deref().map_or("null".to_string(), json_str);
        let list = |rows: Vec<String>| format!("[{}]", rows.join(","));
        let s = &self.state;
        let mut regs: Vec<String> = (0..13).map(|i| format!("\"r{i}\":{}", hex(s.r[i]))).collect();
        for (n, v) in [
            ("sp", s.r[13]),
            ("lr", s.r[14]),
            ("pc", s.r[15]),
            ("xpsr", s.xpsr),
            ("msp", s.msp),
            ("psp", s.psp),
            ("control", s.control),
            ("cfsr", s.cfsr),
            ("hfsr", self.hfsr),
            ("mmfar", s.mmfar),
            ("bfar", s.bfar),
        ] {
            regs.push(format!("\"{n}\":{}", hex(v)));
        }
        let furi = self.furi.as_ref().map_or("null".to_string(), |f| {
            format!(
                "{{\"task\":{},\"message\":{},\"registers\":{{{}}}}}",
                opt(&f.task),
                json_str(&f.message),
                f.registers
                    .iter()
                    .map(|(n, v)| format!("{}:{}", json_str(n), hex(*v)))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        });
        format!(
            "{{\"firmware\":{{\"name\":{},\"version\":{},\"sha256\":{}}},\"atMs\":{},\"kind\":{},\"summary\":{},\"verdict\":{},\"verdictLabel\":{},\"basis\":{},\"stateFrom\":{},\"registers\":{{{}}},\"faultCauses\":{},\"furi\":{},\"backtrace\":{},\"recentPcsNote\":{},\"recentPcs\":{},\"inputs\":{},\"gaps\":{},\"logTail\":{}}}",
            json_str(&self.firmware.name),
            opt(&self.firmware.version),
            opt(&self.firmware.sha256),
            self.at_ms,
            json_str(self.kind),
            json_str(&self.summary),
            json_str(self.verdict.id()),
            json_str(self.verdict.label()),
            list(self.basis.iter().map(|b| json_str(b)).collect()),
            json_str(self.state_from),
            regs.join(","),
            list(self.fault_causes.iter().map(|c| json_str(c)).collect()),
            furi,
            list(
                self.backtrace
                    .iter()
                    .map(|f| format!(
                        "{{\"address\":{},\"symbol\":{},\"source\":{}}}",
                        hex(f.address),
                        opt(&f.symbol),
                        json_str(f.source)
                    ))
                    .collect()
            ),
            json_str(self.pcs_note),
            list(
                self.recent_pcs
                    .iter()
                    .map(|(p, n)| format!("{{\"pc\":{},\"symbol\":{}}}", hex(*p), opt(n)))
                    .collect()
            ),
            list(
                self.inputs
                    .iter()
                    .map(|i| format!(
                        "{{\"atMs\":{},\"button\":\"{:?}\",\"pressed\":{}}}",
                        i.at_ns / 1_000_000,
                        i.button,
                        i.pressed
                    ))
                    .collect()
            ),
            list(
                self.gaps
                    .iter()
                    .map(|g| format!(
                        "{{\"block\":{},\"registers\":{}}}",
                        json_str(&g.block),
                        list(g.registers.iter().map(|r| json_str(r)).collect())
                    ))
                    .collect()
            ),
            json_str(&self.log_tail),
        )
    }

    pub fn to_markdown(&self) -> String {
        let mut o = String::new();
        let at = self.at_ms as f64 / 1000.0;
        let _ = writeln!(o, "# dryflash crash report\n\n**{}**\n", self.summary);
        let _ = writeln!(o, "- Firmware: {}", self.firmware.name);
        if let Some(v) = &self.firmware.version {
            let _ = writeln!(o, "- Version: {v}");
        }
        if let Some(h) = &self.firmware.sha256 {
            let _ = writeln!(o, "- SHA-256: `{h}`");
        }
        let _ = writeln!(o, "- At: {at:.3} s of virtual time\n");
        let _ = writeln!(o, "## {}\n", self.verdict.label());
        for b in &self.basis {
            let _ = writeln!(o, "- {b}");
        }
        let _ = writeln!(o, "\nThis is a judgement from what the emulator saw, not a proof.");
        if let Some(f) = &self.furi {
            let _ = writeln!(
                o,
                "\n## Furi's record\n\n- Task: {}",
                f.task.as_deref().unwrap_or("unknown")
            );
            let _ = writeln!(o, "- Message: {}", f.message);
            for (n, v) in &f.registers {
                let _ = writeln!(o, "- {n}: `0x{v:08X}`");
            }
        }
        let _ = writeln!(o, "\n## Registers\n\nAt {}.\n\n```", self.state_from);
        let s = &self.state;
        for i in 0..13 {
            let _ = writeln!(o, "r{i:<3} 0x{:08X}", s.r[i]);
        }
        let _ = writeln!(
            o,
            "sp   0x{:08X}\nlr   0x{:08X}\npc   0x{:08X}\nxpsr 0x{:08X}",
            s.r[13], s.r[14], s.r[15], s.xpsr
        );
        let _ = writeln!(
            o,
            "msp  0x{:08X}\npsp  0x{:08X}\ncontrol 0x{:08X}",
            s.msp, s.psp, s.control
        );
        let _ = writeln!(
            o,
            "cfsr 0x{:08X}\nhfsr 0x{:08X}\nmmfar 0x{:08X}\nbfar 0x{:08X}\n```",
            s.cfsr, self.hfsr, s.mmfar, s.bfar
        );
        for c in &self.fault_causes {
            let _ = writeln!(o, "\n- {c}");
        }
        let _ = writeln!(
            o,
            "\n## Backtrace\n\nFound by scanning the stack for return addresses, so it can include stale frames.\n"
        );
        for f in &self.backtrace {
            let _ = writeln!(
                o,
                "- `0x{:08X}` {} ({})",
                f.address,
                f.symbol.as_deref().unwrap_or("?"),
                f.source
            );
        }
        let _ = writeln!(o, "\n## Last executed instructions\n\n{}\n\n```", self.pcs_note);
        for (p, n) in &self.recent_pcs {
            let _ = writeln!(o, "0x{p:08X} {}", n.as_deref().unwrap_or(""));
        }
        let _ = writeln!(o, "```\n\n## Inputs\n");
        if self.inputs.is_empty() {
            let _ = writeln!(o, "None.");
        }
        for i in &self.inputs {
            let what = if i.pressed { "pressed" } else { "released" };
            let _ = writeln!(o, "- {:.3} s: {:?} {what}", i.at_ns as f64 / 1e9, i.button);
        }
        let _ = writeln!(
            o,
            "\n## Registers with no behavioural model, touched in the last 100 ms\n"
        );
        if self.gaps.is_empty() {
            let _ = writeln!(o, "None.");
        }
        for g in &self.gaps {
            let _ = writeln!(o, "- {}: {}", g.block, g.registers.join(", "));
        }
        let _ = writeln!(o, "\n## End of the firmware's log\n\n```\n{}\n```", self.log_tail);
        o
    }
}

/// `s` as a JSON string.
pub fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn furi_prints_this_and_it_is_read_back() {
        // As __furi_crash_implementation prints it, firmware 1.5.1 (furi/core/check.c).
        let log = b"noise\r\n\x1b[0;31m[CRASH][desktop] furi_check failed\r\n\tr0 : 20001f00\r\n\tr1 : 0\r\n\tlr : 8012345\r\n\tstack watermark: 512\r\n\t     heap total: 186432\r\n\tcore2: not faulted\r\nRebooting system.\r\n\x1b[0m\r\n";
        let r = furi_log_record(log).unwrap();
        assert_eq!(r.task.as_deref(), Some("desktop"));
        assert_eq!(r.message, "furi_check failed");
        assert_eq!(
            r.registers,
            vec![
                ("r0".to_string(), 0x2000_1F00),
                ("r1".to_string(), 0),
                ("lr".to_string(), 0x0801_2345)
            ]
        );
        assert!(!furi_log_halted(log));
    }

    #[test]
    fn an_interrupt_and_a_halt_are_read_too() {
        let log = b"\r\n\x1b[0;31m[CRASH][ISR HardFault] HardFault\r\n\tr0 : 1";
        assert_eq!(furi_log_record(log).unwrap().task.as_deref(), Some("ISR HardFault"));
        let log = b"\r\n\x1b[0;31m[HALT][main] System halt requested.\r\nSystem halted. Bye-bye!\r\n";
        let r = furi_log_record(log).unwrap();
        assert_eq!(r.message, "System halt requested.");
        assert!(furi_log_halted(log));
        assert!(furi_log_record(b"a log with no crash").is_none());
    }
}

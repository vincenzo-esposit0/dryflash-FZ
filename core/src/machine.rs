// SPDX-License-Identifier: GPL-3.0-or-later
//! The emulated system: CPU1 + system bus + decoded-instruction cache, plus the run loop
//! that advances virtual time (WFI fast-forward, ARCHITECTURE §3.3) and handles resets.

use crate::board::Button;
use crate::bus::SystemBus;
use crate::bus::map::{FLASH_BASE, FLASH_SIZE};
use crate::cpu::{Bus, Cpu, CpuEvent, DecodeCache, Snapshot, Stop};
use crate::crash::{Capture, CrashKind, FuriRecord, InputEvent};
use crate::loader::dfu::{Dfu, Element};
use crate::periph::ResetKind;
use crate::trace::digest::Digest;
use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadError {
    /// Element outside main flash.
    OutsideFlash {
        address: u32,
        len: usize,
    },
    /// Element overlapping the region reserved for CPU2's wireless stack.
    OverlapsCpu2 {
        address: u32,
        len: usize,
        cpu2_start: u32,
    },
    NoElements,
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::OutsideFlash { address, len } => {
                write!(f, "element at 0x{address:08X} ({len} bytes) is outside main flash")
            }
            LoadError::OverlapsCpu2 {
                address,
                len,
                cpu2_start,
            } => write!(
                f,
                "element at 0x{address:08X} ({len} bytes) overlaps the CPU2 region from 0x{cpu2_start:08X}"
            ),
            LoadError::NoElements => write!(f, "image has no elements"),
        }
    }
}

impl std::error::Error for LoadError {}

/// Why [`Machine::run`] returned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunExit {
    /// The requested number of cycles elapsed.
    Budget,
    /// The CPU sleeps and nothing is scheduled that could wake it up.
    IdleForever { pc: u32 },
    /// The CPU stopped for another reason (watchpoint, lockup, emulator gap, …).
    Stopped(Stop),
    /// Boot loop: `resets` system resets within the configured window (BRIEF §7).
    BootLoop { resets: u32 },
    /// The firmware crashed and reset itself. Running again continues from that reset.
    Crashed,
    /// No sleep, no display change and no input for too long (BRIEF §7).
    Hung,
}

/// When a firmware that never sleeps counts as hung, in virtual time. Everything restarts on a WFI,
/// a display change or a button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HangLimit {
    /// The code has stayed inside [`TIGHT_LOOP_BYTES`], interrupts aside.
    pub tight_ns: u128,
    /// Anything else.
    pub busy_ns: u128,
}

impl Default for HangLimit {
    fn default() -> Self {
        HangLimit {
            tight_ns: 5_000_000_000,
            busy_ns: 30_000_000_000,
        }
    }
}

/// Code span that counts as one tight loop.
const TIGHT_LOOP_BYTES: u32 = 256;
/// The run loop looks at the machine this often, in cycles of HCLK1.
const SAMPLE_CYCLES: u64 = 640_000;
/// A fault older than this has nothing to do with a crash now, in ns of virtual time.
const FAULT_RELEVANT_NS: u128 = 2_000_000_000;
/// Inputs kept for the crash report.
const INPUT_HISTORY: usize = 64;
/// Emulator gaps touched this long before a crash are named in its report, in ns of virtual time.
const GAP_WINDOW_NS: u128 = 100_000_000;
/// A gap first touched this long before a crash is a suspect, in ns of virtual time. Registers the
/// firmware has been using all along are not: every run touches some (PWR and SYSCFG in the idle path).
const GAP_NEW_NS: u128 = 1_000_000_000;
/// Crash reports kept since power-on.
const MAX_CAPTURES: usize = 8;

/// What the hang detector has seen since it last restarted.
#[derive(Clone, Copy, Debug, Default)]
struct HangWatch {
    /// Virtual time of the last sleep, display change or input.
    active_ns: u128,
    generation: u64,
    /// Code span sampled, and since when.
    tight: Option<(u32, u32, u128)>,
}

/// Boot-loop threshold: `resets` system resets within `window_ns` of virtual time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BootLoopLimit {
    pub resets: u32,
    pub window_ns: u128,
}

impl Default for BootLoopLimit {
    fn default() -> Self {
        BootLoopLimit {
            resets: 5,
            window_ns: 30_000_000_000,
        }
    }
}

/// A firmware crash decoded from Furi's own record (crate::crash).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FirmwareCrash {
    /// Virtual time of the reset that followed the crash, in ns.
    pub at_ns: u128,
    pub message_ptr: u32,
    pub message: Option<String>,
}

pub struct Machine {
    pub cpu: Cpu,
    pub bus: SystemBus,
    cache: DecodeCache,
    /// Number of system resets since power-on.
    pub resets: u32,
    /// Virtual times (ns) of the system resets inside the boot-loop window.
    reset_times: Vec<u128>,
    pub boot_loop: BootLoopLimit,
    /// Firmware crashes reported by Furi before resetting, oldest first.
    pub crashes: Vec<FirmwareCrash>,
    /// Fault pointer already reported (Furi leaves it in the backup register).
    last_fault: u32,
    /// Hang detection; `None` turns it off.
    pub hang: Option<HangLimit>,
    hang_watch: HangWatch,
    /// Crash reports' raw material, oldest first.
    pub captures: Vec<Capture>,
    /// Pressed and released buttons, oldest first.
    inputs: std::collections::VecDeque<InputEvent>,
    /// `__furi_crash_implementation` and `__furi_halt_implementation`, when an ELF says where.
    crash_entries: Vec<u32>,
    /// The state at the entry of one of those.
    entry_snapshot: Option<Snapshot>,
    /// How much of the log has been searched for Furi's crash records.
    log_seen: usize,
}

impl Default for Machine {
    fn default() -> Self {
        Self::new()
    }
}

fn earliest(a: Option<u64>, b: Option<u64>) -> Option<u64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.min(y)),
        (x, None) => x,
        (None, y) => y,
    }
}

impl Machine {
    pub fn new() -> Self {
        Machine {
            cpu: Cpu::new(),
            bus: SystemBus::new(),
            cache: DecodeCache::new(FLASH_BASE, FLASH_SIZE),
            resets: 0,
            reset_times: Vec::new(),
            boot_loop: BootLoopLimit::default(),
            crashes: Vec::new(),
            last_fault: 0,
            hang: Some(HangLimit::default()),
            hang_watch: HangWatch::default(),
            captures: Vec::new(),
            inputs: std::collections::VecDeque::new(),
            crash_entries: Vec::new(),
            entry_snapshot: None,
            log_seen: 0,
        }
    }

    /// Writes image elements into main flash ("Quick install", BRIEF §4). `cpu2_start`
    /// is the first address reserved for CPU2 (from the package's `Radio address` or the
    /// option bytes); nothing may be written there.
    pub fn load_elements(&mut self, elements: &[Element], cpu2_start: Option<u32>) -> Result<(), LoadError> {
        if elements.is_empty() {
            return Err(LoadError::NoElements);
        }
        for e in elements {
            let (address, len) = (e.address, e.data.len());
            let end = address as u64 + len as u64;
            if address < FLASH_BASE || end > (FLASH_BASE + FLASH_SIZE) as u64 {
                return Err(LoadError::OutsideFlash { address, len });
            }
            if let Some(c2) = cpu2_start
                && end > c2 as u64
            {
                return Err(LoadError::OverlapsCpu2 {
                    address,
                    len,
                    cpu2_start: c2,
                });
            }
        }
        for e in elements {
            let off = (e.address - FLASH_BASE) as usize;
            self.bus.flash[off..off + e.data.len()].copy_from_slice(&e.data);
            self.cache.invalidate(e.address, e.data.len() as u32);
        }
        Ok(())
    }

    /// Loads every element of every target of a DfuSe image.
    pub fn load_dfu(&mut self, dfu: &Dfu, cpu2_start: Option<u32>) -> Result<(), LoadError> {
        let elements: Vec<Element> = dfu.targets.iter().flat_map(|t| t.elements.iter().cloned()).collect();
        self.load_elements(&elements, cpu2_start)
    }

    /// Power-on reset: RAM cleared, CPU reset through the vector table at address 0
    /// (VTOR resets to 0; main flash is aliased there).
    pub fn power_on(&mut self) {
        self.bus.power_on_clear();
        self.bus.now = self.cpu.cycles;
        self.bus.reset_peripherals(ResetKind::PowerOn);
        self.resets = 0;
        self.reset_times.clear();
        self.last_fault = 0;
        self.hang_watch = HangWatch::default();
        self.captures.clear();
        self.inputs.clear();
        self.entry_snapshot = None;
        self.log_seen = self.bus.periphs.usart[0].tx.len();
        self.reset_cpu("power-on reset");
    }

    fn reset_cpu(&mut self, why: &str) {
        let cycles = self.cpu.cycles;
        self.bus.digest.event(cycles, why.to_string());
        if let Err(e) = self.cpu.reset(&mut self.bus, 0) {
            self.bus
                .digest
                .event(cycles, format!("reset vector fetch failed at 0x{:08X}", e.addr));
        }
    }

    /// Starts the firmware's own update of a package staged on the card
    /// (`package::stage_ota`): what `update_operation_prepare` leaves behind after its checks, the
    /// boot mode for the updater, followed by a system reset.
    pub fn start_ota(&mut self) {
        const FURI_HAL_RTC_BOOT_MODE_PRE_UPDATE: u32 = 2;
        self.bus.set_boot_mode(FURI_HAL_RTC_BOOT_MODE_PRE_UPDATE);
        self.bus.now = self.cpu.cycles;
        self.bus.reset_peripherals(ResetKind::Software);
        self.reset_cpu("update armed");
    }

    /// Presses or releases a button; the firmware sees it through GPIO and EXTI.
    pub fn set_button(&mut self, b: Button, pressed: bool) {
        self.bus.periphs.set_button(b, pressed);
        let at_ns = self.bus.periphs.now_ns(self.cpu.cycles);
        self.hang_watch.active_ns = at_ns;
        self.hang_watch.tight = None;
        if self.inputs.len() == INPUT_HISTORY {
            self.inputs.pop_front();
        }
        self.inputs.push_back(InputEvent {
            at_ns,
            button: b,
            pressed,
        });
    }

    /// Stops briefly at these addresses to record the state on the way into Furi's crash code, so a
    /// report has the PCs from before the crash rather than those of the crash's own printing.
    pub fn set_crash_entries(&mut self, addresses: &[u32]) {
        self.cpu.watch.retain(|a| !self.crash_entries.contains(a));
        self.crash_entries = addresses.to_vec();
        self.cpu.watch.extend_from_slice(addresses);
        self.cpu.watch.sort_unstable();
    }

    /// The visible 128×64 LCD image (1 byte per pixel, 1 = dark).
    pub fn display_frame(&self) -> Vec<u8> {
        self.bus.periphs.display.frame()
    }

    /// Changes whenever the LCD content changes.
    pub fn display_generation(&self) -> u64 {
        self.bus.periphs.display.generation
    }

    /// LED and backlight levels now.
    pub fn lights(&mut self) -> crate::board::lp5562::Lights {
        self.bus.periphs.lights(self.cpu.cycles)
    }

    pub fn digest(&self) -> &Digest {
        &self.bus.digest
    }

    /// Runs for up to `cycles` of virtual time.
    pub fn run(&mut self, cycles: u64) -> RunExit {
        let end = self.cpu.cycles + cycles;
        let exit = loop {
            let budget = end.saturating_sub(self.cpu.cycles);
            if budget == 0 {
                break RunExit::Budget;
            }
            self.bus.now = self.cpu.cycles;
            self.bus.digest.now = self.cpu.cycles;
            let stop = self.cpu.run(&mut self.bus, &mut self.cache, budget.min(SAMPLE_CYCLES));
            if let Some((addr, len)) = self.bus.take_flash_dirty() {
                self.cache.invalidate(addr, len);
            }
            if self.bus.reset_requested {
                self.bus.now = self.cpu.cycles;
                self.bus.reset_peripherals(ResetKind::Software);
                self.reset_cpu("option bytes reload (OBL_LAUNCH)");
                continue;
            }
            // Without a reset for it to end in, a firmware built to halt stops here.
            if let Some(record) = self.halted_record() {
                self.capture(CrashKind::Halted, Some(record));
                break RunExit::Crashed;
            }
            if self
                .bus
                .periphs
                .iwdg
                .deadline_ns
                .is_some_and(|t| self.bus.periphs.now_ns(self.cpu.cycles) >= t)
            {
                self.watchdog_reset();
                break RunExit::Crashed;
            }
            match stop {
                Stop::Budget => {
                    if self.check_hang() {
                        break RunExit::Hung;
                    }
                }
                Stop::Sleep { pc } => {
                    self.hang_watch.active_ns = self.bus.periphs.now_ns(self.cpu.cycles);
                    self.hang_watch.tight = None;
                    match earliest(self.cpu.next_event(), self.bus.next_event()) {
                        // WFI fast-forward: jump to the next scheduled event.
                        Some(t) if t <= end => self.cpu.cycles = self.cpu.cycles.max(t),
                        Some(_) => {
                            self.cpu.cycles = end;
                            break RunExit::Budget;
                        }
                        // Nothing but the watchdog, which LSI keeps counting.
                        None if self.bus.periphs.iwdg.deadline_ns.is_some() => {
                            let now = self.bus.periphs.now_ns(self.cpu.cycles);
                            let hz = self.bus.periphs.clocks.hclk1.max(1) as u128;
                            let left = self.bus.periphs.iwdg.deadline_ns.unwrap_or(now).saturating_sub(now);
                            let t = self.cpu.cycles + (left * hz).div_ceil(1_000_000_000) as u64;
                            if t > end {
                                self.cpu.cycles = end;
                                break RunExit::Budget;
                            }
                            self.cpu.cycles = t;
                        }
                        None => {
                            self.capture(CrashKind::IdleForever { pc }, None);
                            break RunExit::IdleForever { pc };
                        }
                    }
                }
                Stop::ResetRequest { pc } => {
                    let crashed = self.note_reset(pc);
                    self.resets += 1;
                    let now = self.bus.periphs.now_ns(self.cpu.cycles);
                    let window = self.boot_loop.window_ns;
                    self.reset_times.push(now);
                    self.reset_times.retain(|&t| now - t <= window);
                    let boot_loop = self.reset_times.len() as u32 >= self.boot_loop.resets;
                    if boot_loop {
                        self.capture(
                            CrashKind::BootLoop {
                                resets: self.reset_times.len() as u32,
                                window_s: (window / 1_000_000_000) as u32,
                            },
                            None,
                        );
                    }
                    self.bus.now = self.cpu.cycles;
                    self.bus.reset_peripherals(ResetKind::Software);
                    self.reset_cpu(&format!("SYSRESETREQ from pc 0x{pc:08X}"));
                    self.hang_watch.active_ns = self.bus.periphs.now_ns(self.cpu.cycles);
                    self.hang_watch.tight = None;
                    if boot_loop {
                        self.bus.digest.event(
                            self.cpu.cycles,
                            format!(
                                "boot loop: {} resets within {} s",
                                self.reset_times.len(),
                                window / 1_000_000_000
                            ),
                        );
                        break RunExit::BootLoop {
                            resets: self.reset_times.len() as u32,
                        };
                    }
                    if crashed {
                        break RunExit::Crashed;
                    }
                }
                Stop::Watch { pc } if self.crash_entries.contains(&pc) => {
                    self.entry_snapshot = Some(self.cpu.snapshot(&mut self.bus, pc, None));
                }
                other => {
                    let kind = match other {
                        Stop::Unimplemented { raw, pc } => Some(CrashKind::Unimplemented { raw, pc }),
                        Stop::Lockup { pc } => Some(CrashKind::Lockup { pc }),
                        _ => None,
                    };
                    if let Some(kind) = kind {
                        self.capture(kind, None);
                    }
                    break RunExit::Stopped(other);
                }
            }
        };
        self.drain_cpu_events();
        exit
    }

    /// The independent watchdog ran out: a system reset, with its own flag in RCC_CSR.
    fn watchdog_reset(&mut self) {
        self.capture(CrashKind::Watchdog, None);
        self.resets += 1;
        self.bus.now = self.cpu.cycles;
        self.bus.reset_peripherals(ResetKind::Iwdg);
        self.reset_cpu("independent watchdog reset");
        self.hang_watch.active_ns = self.bus.periphs.now_ns(self.cpu.cycles);
        self.hang_watch.tight = None;
    }

    /// A reset: records a crash if the firmware left word of one. Whether it did.
    fn note_reset(&mut self, pc: u32) -> bool {
        let tail = &self.bus.periphs.usart[0].tx[self.log_seen..];
        let record = crate::crash::furi_log_record(tail);
        self.log_seen = self.bus.periphs.usart[0].tx.len();
        let noted = self.note_firmware_crash();
        let fault = self.recent_fault();
        if record.is_some() || noted {
            let message = record
                .as_ref()
                .map(|r| r.message.clone())
                .or_else(|| self.crashes.last().and_then(|c| c.message.clone()))
                .unwrap_or_else(|| "no message".to_string());
            self.capture(CrashKind::FirmwareCrash { message }, record);
            true
        } else if fault {
            self.capture(CrashKind::Fault { reset_pc: pc }, None);
            true
        } else {
            false
        }
    }

    /// Furi's halt, which prints a record and then waits for ever instead of resetting.
    fn halted_record(&mut self) -> Option<FuriRecord> {
        let tx = &self.bus.periphs.usart[0].tx;
        if tx.len() == self.log_seen || !crate::crash::furi_log_halted(&tx[self.log_seen..]) {
            return None;
        }
        let record = crate::crash::furi_log_record(&tx[self.log_seen..]);
        self.log_seen = tx.len();
        record
    }

    /// Is the fault on record recent enough to explain what is happening now? Forgets it if not.
    fn recent_fault(&mut self) -> bool {
        let Some(f) = &self.cpu.first_fault else {
            return false;
        };
        let hz = self.bus.periphs.clocks.hclk1.max(1) as u128;
        let age = (self.cpu.cycles.saturating_sub(f.cycles) as u128) * 1_000_000_000 / hz;
        // Still in a fault handler (exceptions 2–6), however long ago it came in.
        if age > FAULT_RELEVANT_NS && !(2..=6).contains(&self.cpu.ipsr) {
            self.cpu.first_fault = None;
            return false;
        }
        true
    }

    /// One sample of the hang detector. True when it has decided the firmware is hung.
    fn check_hang(&mut self) -> bool {
        let Some(limit) = self.hang else {
            return false;
        };
        let now = self.bus.periphs.now_ns(self.cpu.cycles);
        let generation = self.display_generation();
        // Off with the default sleep method: without a radio stack its idle loop spins (ADR-0012).
        if generation != self.hang_watch.generation || !self.bus.legacy_sleep_on() {
            self.hang_watch.generation = generation;
            self.hang_watch.active_ns = now;
            self.hang_watch.tight = None;
            return false;
        }
        self.recent_fault();
        // Interrupts come and go through a spinning thread. A fault handler does not: it is where
        // bare-metal firmware spins for good (exceptions 2–6: DDI 0403E B1.5.2).
        if self.cpu.ipsr <= 6 {
            let pc = self.cpu.pc();
            self.hang_watch.tight = match self.hang_watch.tight {
                Some((lo, hi, since)) if pc.max(hi) - pc.min(lo) <= TIGHT_LOOP_BYTES => {
                    Some((pc.min(lo), pc.max(hi), since))
                }
                _ => Some((pc, pc, now)),
            };
        }
        let idle_for = now.saturating_sub(self.hang_watch.active_ns);
        let tight = self
            .hang_watch
            .tight
            .filter(|&(_, _, since)| idle_for >= limit.tight_ns && now - since >= limit.tight_ns);
        if tight.is_none() && idle_for < limit.busy_ns {
            return false;
        }
        let seconds = (idle_for / 1_000_000_000) as u32;
        let kind = match tight {
            Some((lo, hi, _)) => CrashKind::TightLoop { lo, hi, seconds },
            None => CrashKind::Busy { seconds },
        };
        self.capture(kind, None);
        self.bus
            .digest
            .event(self.cpu.cycles, format!("hang: nothing happened for {seconds} s"));
        self.hang_watch.active_ns = now;
        self.hang_watch.tight = None;
        true
    }

    /// Records everything a report needs about this moment.
    fn capture(&mut self, kind: CrashKind, furi: Option<FuriRecord>) {
        let pc = self.cpu.pc();
        let fault = if self.recent_fault() {
            self.cpu.first_fault.take()
        } else {
            None
        };
        let entry = self.entry_snapshot.take();
        let live = self.cpu.snapshot(&mut self.bus, pc, None);
        let hz = self.bus.periphs.clocks.hclk1.max(1) as u128;
        let window = (GAP_WINDOW_NS * hz / 1_000_000_000) as u64;
        if self.captures.len() == MAX_CAPTURES {
            self.captures.remove(0);
        }
        self.captures.push(Capture {
            at_ns: self.bus.periphs.now_ns(self.cpu.cycles),
            kind,
            furi,
            fault,
            entry,
            live,
            hfsr: self.cpu.sys.hfsr,
            lcd: self.display_frame(),
            inputs: self.inputs.iter().copied().collect(),
            gaps: self.bus.digest.gaps_since(
                self.cpu.cycles.saturating_sub(window),
                self.cpu.cycles.saturating_sub((GAP_NEW_NS * hz / 1_000_000_000) as u64),
            ),
            unmapped: self.bus.digest.unmapped.keys().copied().collect(),
            log_tail: {
                let tx = &self.bus.periphs.usart[0].tx;
                String::from_utf8_lossy(&tx[tx.len().saturating_sub(2048)..]).into_owned()
            },
        });
    }

    /// Records a crash if Furi left a new fault pointer in the RTC backup register. Whether it did.
    fn note_firmware_crash(&mut self) -> bool {
        let Some((ptr, message)) = crate::crash::furi_fault(&self.bus) else {
            // Desktop clears it when the user dismisses the crash scene, so the same crash can come back.
            self.last_fault = 0;
            return false;
        };
        if ptr == self.last_fault {
            return false;
        }
        self.last_fault = ptr;
        let at_ns = self.bus.periphs.now_ns(self.cpu.cycles);
        let text = message.clone().unwrap_or_else(|| format!("message at 0x{ptr:08X}"));
        self.bus
            .digest
            .event(self.cpu.cycles, format!("firmware crash: \"{text}\""));
        self.crashes.push(FirmwareCrash {
            at_ns,
            message_ptr: ptr,
            message,
        });
        true
    }

    fn drain_cpu_events(&mut self) {
        for ev in self.cpu.events.drain(..) {
            match ev {
                CpuEvent::Fault {
                    cycles,
                    fault,
                    exception,
                    pc,
                } => self.bus.digest.event(
                    cycles,
                    format!("fault {fault:?} → exception {exception} at pc 0x{pc:08X}"),
                ),
                CpuEvent::Escalated { cycles, from, pc } => self.bus.digest.event(
                    cycles,
                    format!("exception {from} escalated to HardFault at pc 0x{pc:08X}"),
                ),
            }
        }
    }
}

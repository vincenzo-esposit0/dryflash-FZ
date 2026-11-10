// SPDX-License-Identifier: GPL-3.0-or-later
//! The computer at the other end of the USB cable: a host that enumerates the device and, while
//! the page's terminal is open, talks to its CDC ACM serial port like a terminal program does.
//!
//! Sequence (USB 2.0 §9.1.2 and §7.1.7.3; CDC PSTN 1.2 §6.3): when the cable is in and the device
//! pulls D+ up, the host waits 100 ms, resets the bus for 10 ms, then sends a start of frame every
//! millisecond and runs control transfers: GET_DESCRIPTOR(device) at address 0, SET_ADDRESS(1),
//! GET_DESCRIPTOR(device, configuration, product and serial strings), SET_CONFIGURATION(1). Opening
//! the port sends SET_LINE_CODING (115200 8N1) and SET_CONTROL_LINE_STATE with DTR and RTS, which
//! is what makes the firmware start its CLI (applications/services/cli/cli_vcp.c); closing it
//! drops DTR.
//!
//! Transactions are not bit-timed: the host runs at each frame and shortly after the firmware
//! touches the USB registers, and each endpoint carries one packet per turn until it NAKs.

use crate::periph::usb::{Handshake, Usb};
use std::collections::VecDeque;

const MS: u128 = 1_000_000;
/// Wait after the pull-up appears before resetting (USB 2.0 §7.1.7.3, TATTDB).
const ATTACH_DEBOUNCE: u128 = 100 * MS;
/// Reset signalling (USB 2.0 §7.1.7.5, TDRST minimum).
const RESET_TIME: u128 = 10 * MS;
/// Bus idle before a device suspends (USB 2.0 §7.1.7.6).
const IDLE_SUSPEND: u128 = 3 * MS;
/// How soon the host notices that the firmware changed an endpoint.
const KICK: u128 = 10_000;
/// Address given to the device.
const ADDRESS: u8 = 1;
/// A transaction with no answer is tried this many times before the transfer fails.
const TIMEOUT_RETRIES: u32 = 3;
const MAX_PACKET: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Bus {
    /// No device pull-up, or no cable: nothing on the bus.
    Idle {
        since: u128,
        suspended: bool,
    },
    /// Pull-up seen at `since`; the host resets the bus at `since + ATTACH_DEBOUNCE`.
    Attached {
        since: u128,
    },
    Resetting {
        until: u128,
    },
    /// Start of frame every millisecond.
    Active,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Request {
    DeviceDescriptor,
    SetAddress,
    ConfigHeader,
    Config(u16),
    Product,
    Serial,
    SetConfiguration,
    LineCoding,
    LineState(bool),
}

#[derive(Clone, Debug)]
enum Stage {
    Setup,
    /// Collecting `want` bytes in packets of the control endpoint's size.
    DataIn,
    DataOut(usize),
    /// Status: OUT after a read, IN after a write or no data.
    Status,
}

#[derive(Clone, Debug)]
struct Transfer {
    req: Request,
    setup: [u8; 8],
    out: Vec<u8>,
    data: Vec<u8>,
    stage: Stage,
    timeouts: u32,
}

/// Something the page can show about the connection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostEvent {
    /// Enumerated and configured: product and serial strings as the device reported them.
    Configured {
        product: String,
        serial: String,
    },
    Disconnected,
    /// A transfer the device did not complete.
    Failed(String),
}

#[derive(Clone, Debug)]
pub struct UsbHost {
    /// The cable is plugged into this computer.
    pub cable: bool,
    /// The page's terminal has the port open (DTR).
    pub port_open: bool,
    bus: Bus,
    next_sof: u128,
    kick: Option<u128>,
    addr: u8,
    ep0_size: usize,
    queue: VecDeque<Request>,
    current: Option<Transfer>,
    device: Vec<u8>,
    config: Vec<u8>,
    product: String,
    serial: String,
    configured: bool,
    /// CDC: communication interface, data OUT and IN endpoint addresses.
    cdc: Option<(u8, u8, u8)>,
    /// The DTR state the device was last told.
    dtr_sent: Option<bool>,
    tx: VecDeque<u8>,
    rx: Vec<u8>,
    events: Vec<HostEvent>,
}

impl Default for UsbHost {
    fn default() -> Self {
        Self::new()
    }
}

fn setup(request_type: u8, request: u8, value: u16, index: u16, length: u16) -> [u8; 8] {
    let [v0, v1] = value.to_le_bytes();
    let [i0, i1] = index.to_le_bytes();
    let [l0, l1] = length.to_le_bytes();
    [request_type, request, v0, v1, i0, i1, l0, l1]
}

/// UTF-16LE string descriptor to text.
fn string(d: &[u8]) -> String {
    let n = (*d.first().unwrap_or(&0) as usize).min(d.len());
    let units: Vec<u16> = d[2.min(n)..n]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&c| u16::from_le_bytes(c))
        .collect();
    String::from_utf16_lossy(&units)
}

/// The CDC ACM function in a configuration descriptor: the communication interface (class 2,
/// subclass 2) and the bulk endpoints of the data interface (class 10) after it.
fn find_cdc(config: &[u8]) -> Option<(u8, u8, u8)> {
    let (mut comm, mut data_if, mut out, mut inp) = (None, false, None, None);
    let mut i = 0;
    while i + 2 <= config.len() {
        let len = config[i] as usize;
        if len < 2 || i + len > config.len() {
            break;
        }
        let d = &config[i..i + len];
        match d[1] {
            4 if len >= 9 => {
                if d[5] == 2 && d[6] == 2 && comm.is_none() {
                    comm = Some(d[2]);
                }
                data_if = d[5] == 10 && comm.is_some() && out.is_none();
            }
            5 if len >= 7 && data_if && d[3] & 3 == 2 => {
                if d[2] & 0x80 != 0 {
                    inp.get_or_insert(d[2] & 0x7F);
                } else {
                    out.get_or_insert(d[2]);
                }
            }
            _ => {}
        }
        i += len;
    }
    Some((comm?, out?, inp?))
}

impl UsbHost {
    pub fn new() -> Self {
        UsbHost {
            cable: false,
            port_open: false,
            bus: Bus::Idle {
                since: 0,
                suspended: false,
            },
            next_sof: 0,
            kick: None,
            addr: 0,
            ep0_size: 8,
            queue: VecDeque::new(),
            current: None,
            device: Vec::new(),
            config: Vec::new(),
            product: String::new(),
            serial: String::new(),
            configured: false,
            cdc: None,
            dtr_sent: None,
            tx: VecDeque::new(),
            rx: Vec::new(),
            events: Vec::new(),
        }
    }

    /// Enumerated, configured, with a CDC port found.
    pub fn configured(&self) -> bool {
        self.configured && self.cdc.is_some()
    }

    /// Bytes for the device's serial port, sent once it is configured and the port is open.
    pub fn send(&mut self, data: &[u8], now: u128) {
        self.tx.extend(data);
        self.kick(now);
    }

    /// What the device sent on its serial port.
    pub fn take_rx(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.rx)
    }

    pub fn take_events(&mut self) -> Vec<HostEvent> {
        std::mem::take(&mut self.events)
    }

    /// The firmware changed something the host may be waiting for.
    pub fn kick(&mut self, now: u128) {
        if self.bus == Bus::Active {
            let at = now + KICK;
            self.kick = Some(self.kick.map_or(at, |k| k.min(at)));
        }
    }

    /// When the host has something to do next, given the device's state.
    pub fn next_event_ns(&self, usb: &Usb) -> Option<u128> {
        let present = self.cable && usb.pulled_up();
        match self.bus {
            // The pull-up appeared or went away: the host reacts at once.
            Bus::Idle { since, .. } if present => Some(since),
            Bus::Attached { since } if !present => Some(since),
            Bus::Resetting { .. } | Bus::Active if !present => Some(0),
            Bus::Idle {
                since,
                suspended: false,
            } if usb.running() => Some(since + IDLE_SUSPEND),
            Bus::Idle { .. } => None,
            Bus::Attached { since } => Some(since + ATTACH_DEBOUNCE),
            Bus::Resetting { until } => Some(until),
            Bus::Active => Some(self.kick.map_or(self.next_sof, |k| k.min(self.next_sof))),
        }
    }

    fn disconnect(&mut self, now: u128) {
        if self.bus == Bus::Active && self.configured {
            self.events.push(HostEvent::Disconnected);
        }
        self.bus = Bus::Idle {
            since: now,
            suspended: false,
        };
        self.kick = None;
        self.queue.clear();
        self.current = None;
        self.configured = false;
        self.cdc = None;
        self.dtr_sent = None;
        self.addr = 0;
        self.ep0_size = 8;
        self.tx.clear();
    }

    /// Runs what is due by `now`: attach and reset, frames, transfers.
    pub fn step(&mut self, usb: &mut Usb, now: u128) {
        let present = self.cable && usb.pulled_up();
        loop {
            match self.bus {
                Bus::Idle { since, suspended } => {
                    if present {
                        self.bus = Bus::Attached { since: now };
                        continue;
                    }
                    if !suspended && usb.running() && now >= since + IDLE_SUSPEND {
                        usb.idle();
                        self.bus = Bus::Idle { since, suspended: true };
                    }
                    return;
                }
                Bus::Attached { since } => {
                    if !present {
                        self.disconnect(now);
                        continue;
                    }
                    if now < since + ATTACH_DEBOUNCE {
                        return;
                    }
                    usb.bus_reset();
                    self.bus = Bus::Resetting {
                        until: since + ATTACH_DEBOUNCE + RESET_TIME,
                    };
                }
                Bus::Resetting { until } => {
                    if !present {
                        self.disconnect(now);
                        continue;
                    }
                    if now < until {
                        return;
                    }
                    self.bus = Bus::Active;
                    self.next_sof = until;
                    self.queue.extend([
                        Request::DeviceDescriptor,
                        Request::SetAddress,
                        Request::DeviceDescriptor,
                        Request::ConfigHeader,
                    ]);
                }
                Bus::Active => {
                    if !present {
                        self.disconnect(now);
                        continue;
                    }
                    break;
                }
            }
        }
        if now >= self.next_sof {
            usb.sof();
            // Frames keep their 1 ms grid even if the step came late.
            self.next_sof += MS * ((now - self.next_sof) / MS + 1);
        }
        self.kick = None;
        self.serial_port();
        self.control(usb);
        self.bulk(usb);
    }

    /// Port open or closed: tell the device through DTR once it is configured.
    fn serial_port(&mut self) {
        if !self.configured() || self.dtr_sent == Some(self.port_open) {
            return;
        }
        let pending = |r: &Request| matches!(r, Request::LineCoding | Request::LineState(_));
        if self.queue.iter().any(pending) || self.current.as_ref().is_some_and(|t| pending(&t.req)) {
            return;
        }
        if self.port_open {
            self.queue.push_back(Request::LineCoding);
        }
        self.queue.push_back(Request::LineState(self.port_open));
    }

    fn start(&mut self, req: Request) -> Transfer {
        let comm = self.cdc.map_or(0, |c| c.0 as u16);
        let (setup, out) = match &req {
            Request::DeviceDescriptor => (setup(0x80, 6, 0x0100, 0, 18), vec![]),
            Request::SetAddress => (setup(0x00, 5, ADDRESS as u16, 0, 0), vec![]),
            Request::ConfigHeader => (setup(0x80, 6, 0x0200, 0, 9), vec![]),
            Request::Config(len) => (setup(0x80, 6, 0x0200, 0, *len), vec![]),
            // iProduct and iSerialNumber of the device descriptor; US English.
            Request::Product | Request::Serial => {
                let at = if req == Request::Product { 15 } else { 16 };
                let index = *self.device.get(at).unwrap_or(&0) as u16;
                (setup(0x80, 6, 0x0300 | index, 0x0409, 255), vec![])
            }
            Request::SetConfiguration => (setup(0x00, 9, 1, 0, 0), vec![]),
            // 115200 baud, 1 stop bit, no parity, 8 data bits (CDC PSTN 1.2 Table 17).
            Request::LineCoding => (setup(0x21, 0x20, 0, comm, 7), vec![0x00, 0xC2, 0x01, 0x00, 0, 0, 8]),
            Request::LineState(dtr) => (setup(0x21, 0x22, if *dtr { 3 } else { 0 }, comm, 0), vec![]),
        };
        Transfer {
            req,
            setup,
            out,
            data: Vec::new(),
            stage: Stage::Setup,
            timeouts: 0,
        }
    }

    /// Advances the current control transfer as far as the device lets it.
    fn control(&mut self, usb: &mut Usb) {
        loop {
            if self.current.is_none() {
                let Some(req) = self.queue.pop_front() else { return };
                self.current = Some(self.start(req));
            }
            let addr = self.addr;
            let ep0 = self.ep0_size;
            let t = self.current.as_mut().unwrap();
            let want = u16::from_le_bytes([t.setup[6], t.setup[7]]) as usize;
            let reads = t.setup[0] & 0x80 != 0;
            let hs = match t.stage {
                Stage::Setup => {
                    let hs = usb.setup(addr, 0, &t.setup);
                    if hs == Handshake::Ack {
                        t.stage = match (want, reads) {
                            (0, _) => Stage::Status,
                            (_, true) => Stage::DataIn,
                            (_, false) => Stage::DataOut(0),
                        };
                    }
                    hs
                }
                Stage::DataIn => match usb.input(addr, 0) {
                    Ok(p) => {
                        let short = p.len() < ep0;
                        t.data.extend_from_slice(&p);
                        if short || t.data.len() >= want {
                            t.data.truncate(want);
                            t.stage = Stage::Status;
                        }
                        Handshake::Ack
                    }
                    Err(hs) => hs,
                },
                Stage::DataOut(sent) => {
                    let end = (sent + ep0).min(t.out.len());
                    let hs = usb.out(addr, 0, &t.out[sent..end]);
                    if hs == Handshake::Ack {
                        t.stage = if end >= t.out.len() {
                            Stage::Status
                        } else {
                            Stage::DataOut(end)
                        };
                    }
                    hs
                }
                Stage::Status => {
                    let hs = if reads {
                        usb.out(addr, 0, &[])
                    } else {
                        usb.input(addr, 0).map(|_| Handshake::Ack).unwrap_or_else(|h| h)
                    };
                    if hs == Handshake::Ack {
                        let t = self.current.take().unwrap();
                        self.finished(t);
                        continue;
                    }
                    hs
                }
            };
            match hs {
                Handshake::Ack => continue,
                Handshake::Nak => return,
                Handshake::Timeout if t.timeouts + 1 < TIMEOUT_RETRIES => {
                    t.timeouts += 1;
                    return;
                }
                Handshake::Stall | Handshake::Timeout => {
                    let t = self.current.take().unwrap();
                    self.failed(t, hs);
                }
            }
        }
    }

    fn failed(&mut self, t: Transfer, hs: Handshake) {
        match t.req {
            // Optional strings: a device may have none.
            Request::Product | Request::Serial => self.finished(Transfer { data: vec![], ..t }),
            _ => {
                self.events.push(HostEvent::Failed(format!(
                    "{:?}: {}",
                    t.req,
                    if hs == Handshake::Stall { "stalled" } else { "no answer" }
                )));
                // Like a host giving up on a device: nothing more until it is plugged in again.
                self.queue.clear();
            }
        }
    }

    fn finished(&mut self, t: Transfer) {
        match t.req {
            Request::DeviceDescriptor => {
                if let Some(&size) = t.data.get(7) {
                    self.ep0_size = (size as usize).clamp(8, 64);
                }
                self.device = t.data;
            }
            Request::SetAddress => self.addr = ADDRESS,
            Request::ConfigHeader => {
                if t.data.len() >= 4 {
                    let len = u16::from_le_bytes([t.data[2], t.data[3]]);
                    self.queue.push_front(Request::Config(len));
                }
            }
            Request::Config(_) => {
                self.config = t.data;
                self.cdc = find_cdc(&self.config);
                for (req, at) in [(Request::Product, 15), (Request::Serial, 16)] {
                    if self.device.get(at).is_some_and(|&i| i != 0) {
                        self.queue.push_back(req);
                    }
                }
                self.queue.push_back(Request::SetConfiguration);
            }
            Request::Product => self.product = string(&t.data),
            Request::Serial => self.serial = string(&t.data),
            Request::SetConfiguration => {
                self.configured = true;
                self.events.push(HostEvent::Configured {
                    product: self.product.clone(),
                    serial: self.serial.clone(),
                });
            }
            Request::LineCoding => {}
            Request::LineState(dtr) => self.dtr_sent = Some(dtr),
        }
    }

    /// Serial data: OUT packets from `tx` while the port is open, and IN polling.
    fn bulk(&mut self, usb: &mut Usb) {
        let Some((_, out, inp)) = self.cdc else { return };
        if !self.configured || !self.port_open || self.dtr_sent != Some(true) {
            return;
        }
        while !self.tx.is_empty() {
            let n = self.tx.len().min(MAX_PACKET);
            let packet: Vec<u8> = self.tx.iter().take(n).copied().collect();
            if usb.out(self.addr, out, &packet) != Handshake::Ack {
                break;
            }
            self.tx.drain(..n);
        }
        while let Ok(p) = usb.input(self.addr, inp) {
            self.rx.extend_from_slice(&p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// furi_hal_usb_cdc.c `cdc_cfg_desc_single`, as the firmware sends it.
    #[test]
    fn finds_the_cdc_function() {
        let mut c = vec![9, 2, 75, 0, 2, 1, 0, 0xC0, 250];
        c.extend([8, 11, 0, 2, 2, 2, 0, 0]); // IAD
        c.extend([9, 4, 0, 0, 1, 2, 2, 0, 0]); // communication interface
        c.extend([
            5, 0x24, 0, 0x10, 0x01, 5, 0x24, 1, 0, 1, 4, 0x24, 2, 0, 5, 0x24, 6, 0, 1,
        ]);
        c.extend([7, 5, 0x83, 3, 8, 0, 0xFF]); // notification
        c.extend([9, 4, 1, 0, 2, 10, 0, 0, 0]); // data interface
        c.extend([7, 5, 0x01, 2, 64, 0, 1, 7, 5, 0x82, 2, 64, 0, 1]);
        assert_eq!(find_cdc(&c), Some((0, 1, 2)));
        assert_eq!(string(&[8, 3, b'f', 0, b'l', 0, b'i', 0]), "fli");
    }
}

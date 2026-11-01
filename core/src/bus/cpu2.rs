// SPDX-License-Identifier: GPL-3.0-or-later
//! CPU2 behavioural model: ST's Firmware Upgrade Service (FUS) running with **no wireless stack
//! installed** (ADR-0012). CPU2 itself (Cortex-M0+, encrypted FUS and stack) is not executed.
//!
//! What CPU1 sees, from the transport layer in the pinned `lib/stm32wb_copro`
//! (wpan/interface/patterns/ble_thread/tl/{tl_mbox.c, shci_tl.c, mbox_def.h, tl.h}, shci/shci.{c,h})
//! and Flipper's targets/f7/ble_glue/{hw_ipcc.c, ble_glue.c}:
//!
//! - **Boot.** `HW_IPCC_Enable` sets `PWR_CR4.C2BOOT` after `TL_Init`/`shci_init`/`TL_MM_Init`
//!   filled the reference table at `SRAM2A_BASE + FLASH_IPCCBR.IPCCDBA * 4`. The FUS writes its
//!   device-info table (`MB_FUS_DeviceInfoTable_t`, validity keyword `0xA94656B9`) where the
//!   table's first pointer points, and posts the `SHCI_SUB_EVT_CODE_READY` system event with
//!   `FUS_FW_RUNNING` on the system queue (IPCC channel 2, CPU2→CPU1).
//! - **System commands.** CPU1 writes a `TL_CmdPacket_t` into the system command buffer, sets
//!   CPU1→CPU2 channel 2 and spins until CPU2 clears it (`HW_IPCC_SYS_SendCmd`). The answer is a
//!   command-complete event written at the start of the same buffer, without the packet header
//!   (shci_tl.c `shci_send`). The model answers synchronously, at the write that sets the flag.
//! - **Buffer release.** CPU1 returns event buffers on `pevt_free_buffer_queue` and sets channel 4
//!   (`TL_MM_EvtDone`); CPU2 takes them and clears the channel.
//!
//! FUS commands: `FUS_GET_STATE` reports idle. Every other command (key storage, starting the
//! absent stack, wireless-stack commands) fails with status `0x01`: the device has no factory
//! keys and no stack, and the firmware's own error paths handle that ("No Factory Keys Found",
//! "Stack isn't installed!"). Each opcode is reported once in the bring-up digest.

use super::SystemBus;
use super::map::{SRAM1_BASE, SRAM1_SIZE, SRAM2A_BASE, SRAM2A_SIZE, SRAM2B_BASE, SRAM2B_SIZE};

/// PWR_CR4.C2BOOT (SVD: PWR.CR4 bit 15).
pub const PWR_CR4: u32 = 0x5800_040C;
const C2BOOT: u32 = 1 << 15;
/// FLASH_IPCCBR: IPCC data buffer base, in words from SRAM2A_BASE (SVD: Flash.IPCCBR).
const FLASH_IPCCBR: u32 = 0x5800_403C;

/// IPCC channel indices (mbox_def.h, LL_IPCC_CHANNEL_n = bit n-1).
const CH_BLE_CMD: u32 = 0;
const CH_SYSTEM: u32 = 1;
const CH_MM_RELEASE: u32 = 3;

const FUS_DEVICE_INFO_TABLE_VALIDITY_KEYWORD: u32 = 0xA946_56B9;
const TL_SYSRSP_PKT_TYPE: u8 = 0x11;
const TL_SYSEVT_PKT_TYPE: u8 = 0x12;
const TL_BLEEVT_CC_OPCODE: u8 = 0x0E;
const SHCI_EVTCODE: u8 = 0xFF;
const SHCI_SUB_EVT_CODE_READY: u16 = 0x9200;
const FUS_FW_RUNNING: u8 = 0x01;
const SHCI_OPCODE_C2_FUS_GET_STATE: u16 = (0x3F << 10) + 0x52;
const FUS_STATE_VALUE_IDLE: u8 = 0x00;
/// Command status for a failed FUS operation.
const STATUS_FAILED: u8 = 0x01;

/// FUS v1.2.0 (major.minor.sub in bits 31..8, `INFO_VERSION_*` in shci.h); a virtual-device
/// setting like the OTP data. The wireless-stack fields stay 0: no stack is installed.
const FUS_VERSION: u32 = 0x0102_0000;

#[derive(Clone, Debug, Default)]
pub struct Cpu2 {
    /// C2BOOT seen and the ready event posted.
    booted: bool,
    /// The system spare event buffer is with CPU1.
    spare_in_use: bool,
    /// SHCI opcodes already reported in the digest.
    seen: Vec<u16>,
}

impl Cpu2 {
    pub fn reset(&mut self) {
        *self = Cpu2::default();
    }
}

impl SystemBus {
    fn ram_slice(&mut self, addr: u32, len: u32) -> Option<&mut [u8]> {
        let regions: [(u32, u32, &mut Vec<u8>); 3] = [
            (SRAM1_BASE, SRAM1_SIZE, &mut self.sram1),
            (SRAM2A_BASE, SRAM2A_SIZE, &mut self.sram2a),
            (SRAM2B_BASE, SRAM2B_SIZE, &mut self.sram2b),
        ];
        for (base, size, mem) in regions {
            let off = addr.wrapping_sub(base);
            if off < size && off + len <= size {
                return Some(&mut mem[off as usize..(off + len) as usize]);
            }
        }
        None
    }

    fn ram_r32(&mut self, addr: u32) -> Option<u32> {
        self.ram_slice(addr, 4)
            .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }

    pub(super) fn ram_write(&mut self, addr: u32, bytes: &[u8]) -> bool {
        match self.ram_slice(addr, bytes.len() as u32) {
            Some(s) => {
                s.copy_from_slice(bytes);
                true
            }
            None => false,
        }
    }

    fn cpu2_event(&mut self, what: String) {
        let now = self.now;
        self.digest.event(now, format!("CPU2 (FUS model): {what}"));
    }

    /// Reference table address (`MB_RefTable_t`).
    fn ref_table(&mut self) -> u32 {
        let ipccdba = self.periphs.peek(FLASH_IPCCBR).unwrap_or(0) & 0x3FFF;
        SRAM2A_BASE + (ipccdba << 2)
    }

    /// Runs the CPU2 model after a CPU1 write that may concern it (PWR or IPCC).
    pub(super) fn cpu2_service(&mut self) {
        if !self.cpu2.booted {
            if self.periphs.peek(PWR_CR4).unwrap_or(0) & C2BOOT != 0 {
                self.cpu2_boot();
            }
            return;
        }
        let occupied = self.periphs.ipcc.cpu1_to_cpu2_occupied();
        if occupied & (1 << CH_SYSTEM) != 0 {
            self.cpu2_system_command();
        }
        if occupied & (1 << CH_MM_RELEASE) != 0 {
            self.cpu2_release_buffers();
        }
        if occupied & (1 << CH_BLE_CMD) != 0 && !self.cpu2.seen.contains(&0) {
            // No BLE stack to take it: the channel stays occupied, as with a stuck CPU2.
            self.cpu2.seen.push(0);
            self.cpu2_event("BLE command with no wireless stack, left pending".into());
        }
        self.periphs.refresh_irqs();
    }

    fn cpu2_boot(&mut self) {
        self.cpu2.booted = true;
        let rt = self.ref_table();
        let (Some(dev_info), Some(sys_table), Some(mm_table)) =
            (self.ram_r32(rt), self.ram_r32(rt + 12), self.ram_r32(rt + 16))
        else {
            self.cpu2_event(format!("C2BOOT with reference table at 0x{rt:08X} outside RAM"));
            return;
        };
        // MB_FUS_DeviceInfoTable_t (mbox_def.h): 50 bytes, packed.
        let mut t = Vec::with_capacity(50);
        t.extend_from_slice(&FUS_DEVICE_INFO_TABLE_VALIDITY_KEYWORD.to_le_bytes());
        t.extend_from_slice(&[0, 0, 0, 0]); // Reserved1, LastFusActiveState, LastWirelessStackState, CurrentWirelessStackType
        t.extend_from_slice(&0u32.to_le_bytes()); // SafeBootVersion
        t.extend_from_slice(&FUS_VERSION.to_le_bytes());
        t.extend_from_slice(&0u32.to_le_bytes()); // FusMemorySize
        t.extend_from_slice(&[0; 20]); // WirelessStack{Version,MemorySize}, Ble/ThreadInfo, Reserved2
        let uid64 = self.uid64();
        t.extend_from_slice(&uid64.to_le_bytes());
        t.extend_from_slice(&0x6495u16.to_le_bytes()); // DeviceId: STM32WB55 (DBGMCU_IDCODE.DEV_ID)
        if !self.ram_write(dev_info, &t) {
            self.cpu2_event(format!("device info table at 0x{dev_info:08X} outside RAM"));
        }
        // Ready event in the system spare buffer (MB_MemManagerTable_t.spare_sys_buffer).
        let (Some(spare), Some(queue)) = (self.ram_r32(mm_table + 4), self.ram_r32(sys_table + 4)) else {
            return;
        };
        let sub = SHCI_SUB_EVT_CODE_READY.to_le_bytes();
        let evt = [TL_SYSEVT_PKT_TYPE, SHCI_EVTCODE, 3, sub[0], sub[1], FUS_FW_RUNNING];
        self.ram_write(spare + 8, &evt);
        self.list_insert_tail(queue, spare);
        self.cpu2.spare_in_use = true;
        self.periphs.ipcc.cpu2_set(CH_SYSTEM);
        self.cpu2_event("booted, FUS running, no wireless stack; ready event posted".into());
        self.periphs.refresh_irqs();
    }

    fn uid64(&self) -> u64 {
        let off = (super::map::UID64_ADDR - super::map::ENGI_BASE) as usize;
        u64::from_le_bytes(self.engi[off..off + 8].try_into().unwrap())
    }

    /// `LST_insert_tail` (utilities/stm_list.c) on a circular list with a head node.
    fn list_insert_tail(&mut self, head: u32, node: u32) {
        let Some(last) = self.ram_r32(head + 4) else { return };
        self.ram_write(node, &head.to_le_bytes());
        self.ram_write(node + 4, &last.to_le_bytes());
        self.ram_write(head + 4, &node.to_le_bytes());
        self.ram_write(last, &node.to_le_bytes());
    }

    fn cpu2_system_command(&mut self) {
        let rt = self.ref_table();
        let Some(buf) = self.ram_r32(rt + 12).and_then(|st| self.ram_r32(st)) else {
            return;
        };
        // TL_CmdPacket_t: header (8), type (1), cmdcode (2), plen (1), payload.
        let Some(hdr) = self.ram_slice(buf + 8, 4).map(|s| s.to_vec()) else {
            return;
        };
        let opcode = u16::from_le_bytes([hdr[1], hdr[2]]);
        let ret: &[u8] = if opcode == SHCI_OPCODE_C2_FUS_GET_STATE {
            &[FUS_STATE_VALUE_IDLE, 0]
        } else {
            &[STATUS_FAILED]
        };
        if !self.cpu2.seen.contains(&opcode) {
            self.cpu2.seen.push(opcode);
            self.cpu2_event(format!(
                "SHCI 0x{opcode:04X} (OCF 0x{:02X}) → status 0x{:02X}",
                opcode & 0x3FF,
                ret[0]
            ));
        }
        // Command complete: TL_EvtSerial_t at the start of the buffer (shci_tl.c `shci_send`).
        let op = opcode.to_le_bytes();
        let mut rsp = vec![
            TL_SYSRSP_PKT_TYPE,
            TL_BLEEVT_CC_OPCODE,
            3 + ret.len() as u8,
            1,
            op[0],
            op[1],
        ];
        rsp.extend_from_slice(ret);
        self.ram_write(buf, &rsp);
        self.periphs.ipcc.cpu2_clear(CH_SYSTEM);
    }

    fn cpu2_release_buffers(&mut self) {
        let rt = self.ref_table();
        // MB_MemManagerTable_t.pevt_free_buffer_queue: CPU2 takes every buffer on it.
        if let Some(q) = self.ram_r32(rt + 16).and_then(|mm| self.ram_r32(mm + 16)) {
            self.ram_write(q, &q.to_le_bytes());
            self.ram_write(q + 4, &q.to_le_bytes());
        }
        self.cpu2.spare_in_use = false;
        self.periphs.ipcc.cpu2_clear(CH_MM_RELEASE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::{Bus, Size};

    const IPCC: u32 = 0x5800_0C00;
    const C1SCR: u32 = IPCC + crate::periph::svd::IPCC.offset_of("C1SCR");
    const C1TOC2SR: u32 = IPCC + crate::periph::svd::IPCC.offset_of("C1TO2SR");
    const C2TOC1SR: u32 = IPCC + crate::periph::svd::IPCC.offset_of("C2TOC1SR");

    /// Lays out the transport-layer tables the way `TL_Init`/`shci_init`/`TL_MM_Init` do.
    fn tables(bus: &mut SystemBus) -> (u32, u32, u32, u32, u32) {
        let w = |bus: &mut SystemBus, a: u32, v: u32| bus.write(a, v, Size::Word).unwrap();
        let (rt, dev, sys, mm) = (0x2003_0000, 0x2003_01C4, 0x2003_018C, 0x2003_0170);
        let (queue, free_q, spare, cmd) = (0x2003_0134, 0x2003_014C, 0x2003_02F0, 0x2003_03FC);
        w(bus, rt, dev);
        w(bus, rt + 12, sys);
        w(bus, rt + 16, mm);
        w(bus, sys, cmd);
        w(bus, sys + 4, queue);
        w(bus, mm + 4, spare);
        w(bus, mm + 16, free_q);
        for head in [queue, free_q] {
            w(bus, head, head);
            w(bus, head + 4, head);
        }
        (dev, queue, free_q, spare, cmd)
    }

    #[test]
    fn boots_as_fus_and_answers_system_commands() {
        let mut bus = SystemBus::new();
        let (dev, queue, free_q, spare, cmd) = tables(&mut bus);
        let r = |bus: &mut SystemBus, a: u32| bus.read(a, Size::Word).unwrap();
        bus.write(PWR_CR4, C2BOOT, Size::Word).unwrap();

        // Device info table and ready event in the system queue, channel 2 raised to CPU1.
        assert_eq!(r(&mut bus, dev), FUS_DEVICE_INFO_TABLE_VALIDITY_KEYWORD);
        assert_eq!(r(&mut bus, dev + 12), FUS_VERSION);
        assert_eq!(r(&mut bus, dev + 20), 0, "no wireless stack");
        assert_eq!((r(&mut bus, queue), r(&mut bus, queue + 4)), (spare, spare));
        assert_eq!((r(&mut bus, spare), r(&mut bus, spare + 4)), (queue, queue));
        assert_eq!(r(&mut bus, spare + 8) & 0xFFFFFF, 0x03FF12);
        assert_eq!(r(&mut bus, spare + 11) & 0xFFFFFF, 0x019200);
        assert_eq!(r(&mut bus, C2TOC1SR) & 2, 2);

        // FUS_GET_STATE: synchronous command complete at the start of the buffer.
        bus.write(cmd + 8, 0x00FC_5210, Size::Word).unwrap(); // type, opcode 0xFC52, plen 0
        bus.write(C1SCR, 1 << 17, Size::Word).unwrap();
        assert_eq!(r(&mut bus, C1TOC2SR) & 2, 0, "CPU2 took the command");
        let rsp: Vec<u8> = (0..8).map(|i| bus.read(cmd + i, Size::Byte).unwrap() as u8).collect();
        assert_eq!(rsp, [0x11, 0x0E, 5, 1, 0x52, 0xFC, 0x00, 0x00]);

        // LOAD_USR_KEY fails: there are no factory keys.
        bus.write(cmd + 8, 0x01FC_5910, Size::Word).unwrap();
        bus.write(C1SCR, 1 << 17, Size::Word).unwrap();
        assert_eq!(bus.read(cmd + 6, Size::Byte).unwrap(), 0x01);

        // Buffer release on channel 4 empties the free queue.
        bus.write(free_q, spare, Size::Word).unwrap();
        bus.write(C1SCR, 1 << 19, Size::Word).unwrap();
        assert_eq!(r(&mut bus, free_q), free_q);
        assert_eq!(r(&mut bus, C1TOC2SR) & 8, 0);
    }

    #[test]
    fn without_c2boot_cpu2_stays_silent() {
        let mut bus = SystemBus::new();
        let (dev, ..) = tables(&mut bus);
        bus.write(C1SCR, 1 << 17, Size::Word).unwrap();
        assert_eq!(
            bus.read(C1TOC2SR, Size::Word).unwrap() & 2,
            2,
            "nobody took the command"
        );
        assert_eq!(bus.read(dev, Size::Word).unwrap(), 0);
    }

    #[test]
    fn power_on_presets_legacy_sleep() {
        let mut bus = SystemBus::new();
        bus.reset_peripherals(crate::periph::ResetKind::PowerOn);
        assert_eq!(bus.periphs.rtc.backup(0), 0x10F1);
        assert_eq!(bus.periphs.rtc.backup(1), 0x2000);
    }
}

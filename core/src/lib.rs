// SPDX-License-Identifier: GPL-3.0-or-later
//! dryflash core: instruction-level emulation of the STM32WB55RG (Flipper Zero target `f7`).
//!
//! The core is deterministic and free of host I/O: no clocks, no files, no threads.
//! See `docs/ARCHITECTURE.md` §3.

pub mod bus;
pub mod cpu;
pub mod loader;
pub mod machine;
pub mod sysctl;
pub mod trace;

/// Crate version, exposed to the hosts for crash reports and the UI.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

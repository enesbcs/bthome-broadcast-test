//! BTHome Temperature Broadcaster.
//!
//! The crate contains the BTHome encoder, the advertisement name builder, CLI
//! parsing, logging, the single-instance lock, shutdown handling, diagnostics,
//! the fallback ladder and the platform backends. On Linux the providers read
//! sysfs and advertising goes through BlueZ. On Windows the providers use NVML,
//! ADLX, IGCL, the storage API and PDH, advertising goes through WinRT, and a
//! native Win32 window is the default front end.

pub mod app;
pub mod ble;
pub mod bthome;
pub mod cli;
pub mod diagnostics;
#[cfg(target_os = "windows")]
pub mod gui;
pub mod hostname;
pub mod instance;
pub mod logging;
pub mod name;
pub mod provider;
pub mod shutdown;
pub mod source;

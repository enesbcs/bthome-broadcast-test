//! BLE advertising backends.
//!
//! The application pushes a name and a temperature. BlueZ advertises both; the
//! Windows WinRT publisher can only carry the BTHome service data, so the name
//! is used there for display and diagnostics only. Each backend decides how to
//! keep the advertisement alive and how to update it.

use std::fmt;

use crate::bthome;

/// A BLE backend error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BleError {
    /// No usable Bluetooth adapter / BlueZ service.
    Unavailable(String),
    /// The adapter exists but the operation failed.
    Failed(String),
}

impl fmt::Display for BleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BleError::Unavailable(reason) | BleError::Failed(reason) => f.write_str(reason),
        }
    }
}

impl std::error::Error for BleError {}

/// A running BLE advertiser.
pub trait Broadcaster {
    /// Starts or updates the advertisement with the given name and value.
    fn update(&mut self, name: &str, celsius: f64) -> Result<(), BleError>;

    /// Stops advertising but keeps the backend open.
    fn suspend(&mut self) -> Result<(), BleError>;

    /// Stops advertising and releases the backend.
    fn shutdown(&mut self) -> Result<(), BleError>;
}

#[cfg(target_os = "linux")]
mod bluez;

#[cfg(target_os = "windows")]
mod winrt;

#[cfg(target_os = "linux")]
pub use bluez::{probe, BleInfo};

#[cfg(target_os = "windows")]
pub use winrt::{probe, BleInfo};

/// Opens the platform BLE backend without starting an advertisement yet.
#[cfg(target_os = "linux")]
pub fn start() -> Result<Box<dyn Broadcaster>, BleError> {
    bluez::BluezBroadcaster::connect()
        .map(|broadcaster| Box::new(broadcaster) as Box<dyn Broadcaster>)
}

/// Opens the platform BLE backend without starting an advertisement yet.
#[cfg(target_os = "windows")]
pub fn start() -> Result<Box<dyn Broadcaster>, BleError> {
    winrt::WinRtBroadcaster::connect()
        .map(|broadcaster| Box::new(broadcaster) as Box<dyn Broadcaster>)
}

/// Opens the platform BLE backend without starting an advertisement yet.
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub fn start() -> Result<Box<dyn Broadcaster>, BleError> {
    Err(BleError::Unavailable(
        "BLE advertising is not implemented on this platform".to_string(),
    ))
}

/// BTHome bytes the platform backend actually advertises for a temperature.
///
/// On Linux BlueZ receives the complete advertisement, including the Flags AD
/// structure. The Windows WinRT publisher only accepts the BTHome service data;
/// the Bluetooth stack adds the rest of the advertisement itself.
pub fn payload(celsius: f64) -> Option<Vec<u8>> {
    #[cfg(target_os = "windows")]
    {
        bthome::service_data_ad(celsius).map(|bytes| bytes.to_vec())
    }
    #[cfg(not(target_os = "windows"))]
    {
        bthome::advertisement(celsius).map(|bytes| bytes.to_vec())
    }
}

/// One-line description of the advertised AD structures.
pub fn platform_note() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "the WinRT publisher sends the BTHome service data only; Windows reserves the Flags (0x01) and Local Name (0x08/0x09) AD structures"
    }
    #[cfg(not(target_os = "windows"))]
    {
        "the advertisement contains the Flags (0x01) and BTHome service data (0x16) structures"
    }
}

/// Probes the platform BLE backend.
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub fn probe() -> Result<BleInfo, BleError> {
    Err(BleError::Unavailable(
        "BLE advertising is not implemented on this platform".to_string(),
    ))
}

/// Human-readable summary of the Bluetooth adapter used for `--diagnose`.
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BleInfo {
    pub adapter: String,
    pub address: String,
    pub powered: bool,
    pub advertising: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake {
        updates: usize,
    }

    impl Broadcaster for Fake {
        fn update(&mut self, _name: &str, celsius: f64) -> Result<(), BleError> {
            if celsius > 150.0 {
                return Err(BleError::Failed("implausible".to_string()));
            }
            self.updates += 1;
            Ok(())
        }

        fn suspend(&mut self) -> Result<(), BleError> {
            Ok(())
        }

        fn shutdown(&mut self) -> Result<(), BleError> {
            Ok(())
        }
    }

    #[test]
    fn trait_object_can_be_updated() {
        let mut broadcaster: Box<dyn Broadcaster> = Box::new(Fake { updates: 0 });
        broadcaster.update("host-C", 42.0).unwrap();
        assert!(broadcaster.update("host-C", 200.0).is_err());
    }

    #[test]
    fn payload_matches_the_platform_structures() {
        let payload = payload(48.5).unwrap();
        #[cfg(target_os = "windows")]
        assert_eq!(bthome::hex(&payload), "07 16 D2 FC 40 02 F2 12");
        #[cfg(not(target_os = "windows"))]
        assert_eq!(bthome::hex(&payload), "02 01 06 07 16 D2 FC 40 02 F2 12");
    }
}

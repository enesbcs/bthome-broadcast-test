//! Windows BLE advertising through the WinRT advertisement publisher.
//!
//! The publisher emits legacy advertisements on the default Bluetooth LE
//! adapter. Only the BTHome service data is pushed as a raw data section: the
//! Windows Bluetooth stack owns the Flags, Local Name and other AD structures,
//! and rejects attempts to set them through a data section. As a result the
//! hostname-derived name is shown by the application but is not part of the
//! Windows advertisement.
//!
//! WinRT reads the advertisement when `Start` is called, so an update stops
//! the publisher, rewrites the data sections and starts it again. This keeps
//! the value current without a GATT server or a connection.

use std::future::{Future, IntoFuture};
use std::pin::pin;
use std::sync::{Arc, OnceLock};
use std::task::{Context, Poll, Wake, Waker};
use std::time::Duration;

use windows::Devices::Bluetooth::Advertisement::{
    BluetoothLEAdvertisementDataSection, BluetoothLEAdvertisementPublisher,
    BluetoothLEAdvertisementPublisherStatus,
};
use windows::Devices::Bluetooth::BluetoothAdapter;
use windows::Storage::Streams::DataWriter;
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};

use crate::ble::{BleError, Broadcaster};
use crate::bthome;

/// Summary of the adapter used for `--diagnose`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BleInfo {
    pub adapter: String,
    pub address: String,
    pub powered: bool,
    pub advertising: bool,
}

/// A WinRT advertisement publisher.
pub struct WinRtBroadcaster {
    publisher: BluetoothLEAdvertisementPublisher,
    running: bool,
    last: Option<[u8; 4]>,
}

impl WinRtBroadcaster {
    /// Opens the default adapter and creates a publisher without starting it.
    pub fn connect() -> Result<Self, BleError> {
        let adapter = default_adapter()?;
        if !adapter.IsLowEnergySupported().unwrap_or(false) {
            return Err(BleError::Unavailable(
                "the default Windows Bluetooth adapter does not support Bluetooth LE".to_string(),
            ));
        }

        let publisher = BluetoothLEAdvertisementPublisher::new().map_err(|error| {
            BleError::Unavailable(format!(
                "could not create the Windows BLE advertisement publisher: {error}"
            ))
        })?;

        Ok(Self {
            publisher,
            running: false,
            last: None,
        })
    }

    /// Stops the publisher and waits until the stack has released it.
    fn stop(&mut self) -> Result<(), BleError> {
        if !self.running {
            return Ok(());
        }
        self.publisher.Stop().map_err(|error| {
            BleError::Failed(format!("could not stop BLE advertising: {error}"))
        })?;
        self.running = false;

        // Start rejects the request while the previous advertisement is still
        // being torn down, so wait a short, bounded time for `Stopping`.
        for _ in 0..100 {
            match self.publisher.Status() {
                Ok(status) if status != BluetoothLEAdvertisementPublisherStatus::Stopping => break,
                Ok(_) => std::thread::sleep(Duration::from_millis(5)),
                Err(_) => break,
            }
        }
        Ok(())
    }

    /// Waits until the stack reports that advertising actually started.
    ///
    /// `Start` can succeed while the radio is off or the advertising slot is
    /// unavailable; in that case the publisher moves to `Aborted` instead of
    /// `Started`. Reporting success only after `Started` keeps the log honest.
    fn wait_for_started(&self) -> Result<(), BleError> {
        let mut last = BluetoothLEAdvertisementPublisherStatus::Created;
        for _ in 0..200 {
            match self.publisher.Status() {
                Ok(status) => {
                    last = status;
                    if status == BluetoothLEAdvertisementPublisherStatus::Started {
                        return Ok(());
                    }
                    if status == BluetoothLEAdvertisementPublisherStatus::Aborted {
                        return Err(BleError::Failed(
                            "Windows aborted the BLE advertisement".to_string(),
                        ));
                    }
                }
                Err(error) => {
                    return Err(BleError::Failed(format!(
                        "could not query the BLE advertisement status: {error}"
                    )));
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        Err(BleError::Failed(format!(
            "the BLE advertisement did not start (status {last:?})"
        )))
    }
}

impl Broadcaster for WinRtBroadcaster {
    fn update(&mut self, _name: &str, celsius: f64) -> Result<(), BleError> {
        let payload = bthome::service_data(celsius).ok_or_else(|| {
            BleError::Failed("temperature is outside the BTHome range".to_string())
        })?;
        if self.running && self.last == Some(payload) {
            return Ok(());
        }
        let service_data = [
            bthome::SERVICE_UUID as u8,
            (bthome::SERVICE_UUID >> 8) as u8,
            payload[0],
            payload[1],
            payload[2],
            payload[3],
        ];

        self.stop()?;

        let advertisement = self.publisher.Advertisement().map_err(|error| {
            BleError::Failed(format!("could not access the BLE advertisement: {error}"))
        })?;

        let sections = advertisement.DataSections().map_err(|error| {
            BleError::Failed(format!("could not access the BLE data sections: {error}"))
        })?;
        sections
            .Clear()
            .map_err(|error| BleError::Failed(format!("could not clear the BLE data: {error}")))?;
        sections
            .Append(&data_section(bthome::AD_TYPE_SERVICE_DATA, &service_data)?)
            .map_err(|error| {
                BleError::Failed(format!("could not add the BTHome service data: {error}"))
            })?;

        self.publisher.Start().map_err(|error| {
            BleError::Failed(format!("could not start BLE advertising: {error}"))
        })?;
        self.wait_for_started()?;
        self.running = true;
        self.last = Some(payload);
        Ok(())
    }

    fn suspend(&mut self) -> Result<(), BleError> {
        self.stop()
    }

    fn shutdown(&mut self) -> Result<(), BleError> {
        self.stop()
    }
}

/// Builds one raw AD structure from its type and value.
fn data_section(
    data_type: u8,
    value: &[u8],
) -> Result<BluetoothLEAdvertisementDataSection, BleError> {
    let writer = DataWriter::new().map_err(|error| {
        BleError::Failed(format!("could not allocate the BLE data buffer: {error}"))
    })?;
    writer.WriteBytes(value).map_err(|error| {
        BleError::Failed(format!("could not write the BLE data buffer: {error}"))
    })?;
    let buffer = writer.DetachBuffer().map_err(|error| {
        BleError::Failed(format!("could not detach the BLE data buffer: {error}"))
    })?;
    let section = BluetoothLEAdvertisementDataSection::new().map_err(|error| {
        BleError::Failed(format!("could not create a BLE data section: {error}"))
    })?;
    section
        .SetDataType(data_type)
        .map_err(|error| BleError::Failed(format!("could not set the BLE data type: {error}")))?;
    section
        .SetData(&buffer)
        .map_err(|error| BleError::Failed(format!("could not set the BLE data value: {error}")))?;
    Ok(section)
}

/// Probes the default adapter for diagnostics.
pub fn probe() -> Result<BleInfo, BleError> {
    let adapter = default_adapter()?;
    let powered = adapter.IsLowEnergySupported().unwrap_or(false);
    // The peripheral-role flag is not required by the WinRT publisher; the
    // useful capability check is whether a publisher can be created at all.
    let advertising = BluetoothLEAdvertisementPublisher::new().is_ok();
    let address = adapter.BluetoothAddress().unwrap_or(0);

    Ok(BleInfo {
        adapter: "Windows default Bluetooth LE adapter".to_string(),
        address: format_address(address),
        powered,
        advertising,
    })
}

/// Initializes the Windows Runtime once per process.
fn initialize() -> Result<(), BleError> {
    static INIT: OnceLock<Result<(), String>> = OnceLock::new();
    INIT.get_or_init(|| {
        // A process that already initialized the runtime in another apartment
        // mode is still usable; only a real failure blocks WinRT calls.
        unsafe { RoInitialize(RO_INIT_MULTITHREADED) }
            .map_err(|error| format!("Windows Runtime initialization failed: {error}"))
    })
    .clone()
    .map_err(BleError::Unavailable)
}

/// Opens the default Bluetooth LE adapter.
fn default_adapter() -> Result<BluetoothAdapter, BleError> {
    initialize()?;
    let operation = BluetoothAdapter::GetDefaultAsync().map_err(|error| {
        BleError::Unavailable(format!(
            "could not query the Windows Bluetooth adapter: {error}"
        ))
    })?;
    block_on(operation.into_future()).map_err(|error| {
        BleError::Unavailable(format!("no usable Windows Bluetooth adapter: {error}"))
    })
}

/// Formats a 48-bit Bluetooth address as `AA:BB:CC:DD:EE:FF`.
fn format_address(address: u64) -> String {
    let digits = format!("{address:012X}");
    let mut out = String::with_capacity(17);
    for (index, chunk) in digits.as_bytes().chunks(2).enumerate() {
        if index > 0 {
            out.push(':');
        }
        out.push_str(std::str::from_utf8(chunk).unwrap_or("00"));
    }
    out
}

/// Blocks the calling thread until a WinRT asynchronous operation completes.
fn block_on<F: IntoFuture>(future: F) -> F::Output {
    let mut future = pin!(future.into_future());
    let waker = Waker::from(Arc::new(ThreadWaker(std::thread::current())));
    let mut context = Context::from_waker(&waker);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(output) => return output,
            Poll::Pending => std::thread::park(),
        }
    }
}

/// Wakes the blocking thread when the WinRT operation completes.
struct ThreadWaker(std::thread::Thread);

impl Wake for ThreadWaker {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.unpark();
    }
}

//! System disk temperature through the Windows storage API.
//!
//! The volume of `%SystemDrive%` is mapped to a physical disk number, then the
//! disk is queried for `StorageDeviceTemperatureProperty`. This is a direct
//! device IOCTL: no SMART tool, WMI provider or driver is involved.

use std::ffi::c_void;
use std::mem::size_of;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, GENERIC_READ, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_READ_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE,
    OPEN_EXISTING,
};
use windows::Win32::System::Ioctl::{
    PropertyStandardQuery, StorageDeviceTemperatureProperty, IOCTL_STORAGE_GET_DEVICE_NUMBER,
    IOCTL_STORAGE_QUERY_PROPERTY, STORAGE_DEVICE_NUMBER, STORAGE_PROPERTY_QUERY,
    STORAGE_TEMPERATURE_DATA_DESCRIPTOR,
};
use windows::Win32::System::IO::DeviceIoControl;

use crate::provider::{ProviderError, MAX_CELSIUS, MIN_CELSIUS};

/// Reads the temperature of the disk that hosts the system volume.
pub fn read() -> Result<f64, ProviderError> {
    let device_number = system_device_number().map_err(ProviderError::Unavailable)?;
    let path = format!(r"\\.\PhysicalDrive{device_number}");
    let disk = open(&path).map_err(ProviderError::Unavailable)?;
    let _disk = HandleGuard(disk);

    let query = STORAGE_PROPERTY_QUERY {
        PropertyId: StorageDeviceTemperatureProperty,
        QueryType: PropertyStandardQuery,
        AdditionalParameters: [0],
    };
    let mut buffer = vec![0u8; 4096];
    let mut returned = 0u32;
    unsafe {
        DeviceIoControl(
            disk,
            IOCTL_STORAGE_QUERY_PROPERTY,
            Some(&query as *const _ as *const c_void),
            size_of::<STORAGE_PROPERTY_QUERY>() as u32,
            Some(buffer.as_mut_ptr() as *mut c_void),
            buffer.len() as u32,
            Some(&mut returned),
            None,
        )
    }
    .map_err(|error| {
        ProviderError::Unavailable(format!(
            "the system disk does not expose a temperature: {error}"
        ))
    })?;

    temperature_from(&buffer, returned)
}

/// Maps the system volume to its physical disk number.
fn system_device_number() -> Result<u32, String> {
    let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".to_string());
    let volume_path = format!(r"\\.\{drive}");
    let volume = open(&volume_path)?;
    let _volume = HandleGuard(volume);

    let mut number = STORAGE_DEVICE_NUMBER::default();
    let mut returned = 0u32;
    unsafe {
        DeviceIoControl(
            volume,
            IOCTL_STORAGE_GET_DEVICE_NUMBER,
            None,
            0,
            Some(&mut number as *mut _ as *mut c_void),
            size_of::<STORAGE_DEVICE_NUMBER>() as u32,
            Some(&mut returned),
            None,
        )
    }
    .map_err(|error| format!("cannot query the system volume device number: {error}"))?;

    Ok(number.DeviceNumber)
}

/// Extracts the hottest plausible sensor from a temperature descriptor.
fn temperature_from(buffer: &[u8], returned: u32) -> Result<f64, ProviderError> {
    let header = std::mem::offset_of!(STORAGE_TEMPERATURE_DATA_DESCRIPTOR, TemperatureInfo);
    let item = size_of::<windows::Win32::System::Ioctl::STORAGE_TEMPERATURE_INFO>();
    if (returned as usize) < header {
        return Err(ProviderError::Unavailable(
            "the system disk returned a short temperature response".to_string(),
        ));
    }

    // The descriptor contains a variable-length array after its fixed header.
    let descriptor = unsafe { &*(buffer.as_ptr() as *const STORAGE_TEMPERATURE_DATA_DESCRIPTOR) };
    let count = descriptor.InfoCount as usize;
    let available = (returned as usize - header) / item;
    let count = count.min(available);
    if count == 0 {
        return Err(ProviderError::Unavailable(
            "the system disk reported no temperature sensor".to_string(),
        ));
    }

    let infos = unsafe { std::slice::from_raw_parts(descriptor.TemperatureInfo.as_ptr(), count) };
    let mut hottest: Option<f64> = None;
    for info in infos {
        let celsius = info.Temperature as f64;
        if !(MIN_CELSIUS..=MAX_CELSIUS).contains(&celsius) {
            continue;
        }
        hottest = Some(hottest.map_or(celsius, |current| current.max(celsius)));
    }

    hottest.ok_or_else(|| {
        ProviderError::Unavailable("the system disk temperature is not reported".to_string())
    })
}

/// Opens a device or volume, trying read access first and then metadata-only
/// access, which standard users are allowed to request.
fn open(path: &str) -> Result<HANDLE, String> {
    let wide = wide(path);
    let mut last_error = String::new();
    for access in [GENERIC_READ.0, FILE_READ_ATTRIBUTES.0, 0] {
        match unsafe {
            CreateFileW(
                PCWSTR(wide.as_ptr()),
                access,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                None,
            )
        } {
            Ok(handle) => return Ok(handle),
            Err(error) => last_error = format!("cannot open {path}: {error}"),
        }
    }
    Err(last_error)
}

/// Closes a device handle on every path.
struct HandleGuard(HANDLE);

impl Drop for HandleGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// Converts a Rust string to a null-terminated UTF-16 buffer.
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

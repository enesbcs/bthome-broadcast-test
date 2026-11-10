//! Intel GPU temperature through IGCL.
//!
//! `ControlLib.dll` is installed by the Intel graphics driver and is loaded at
//! runtime only when the rung is probed; no Intel DLL is packaged with the
//! program and no Intel SDK is needed at build time.
//!
//! The rung initializes the control API, enumerates the Intel display adapters,
//! enumerates their temperature sensors, and reads the GPU sensor when one is
//! present. If a driver exposes only the global sensor, that value is used
//! instead.

use std::ffi::c_void;
use std::mem::size_of;
use std::ptr;
use std::sync::OnceLock;

use super::dll::Dll;
use crate::provider::ProviderError;

type CtlResult = u32;
type ApiHandle = *mut c_void;
type DeviceHandle = *mut c_void;
type TempHandle = *mut c_void;

/// Every result at or below this value is a success code.
const CTL_SUCCESS_END: CtlResult = 0x0000_FFFF;

/// IGCL API version this binding is written against (1.1).
const CTL_IMPL_MAJOR_VERSION: u32 = 1;
const CTL_IMPL_MINOR_VERSION: u32 = 1;

/// Sensor type: maximum across all GPU sensors.
const CTL_TEMP_SENSORS_GLOBAL: u32 = 0;
/// Sensor type: maximum across all sensors in the GPU.
const CTL_TEMP_SENSORS_GPU: u32 = 1;

fn succeeded(code: CtlResult) -> bool {
    code <= CTL_SUCCESS_END
}

fn make_version(major: u32, minor: u32) -> u32 {
    (major << 16) | minor
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CtlApplicationId {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}

#[repr(C)]
struct CtlInitArgs {
    size: u32,
    version: u8,
    _pad: [u8; 3],
    app_version: u32,
    flags: u32,
    supported_version: u32,
    application_uid: CtlApplicationId,
}

#[repr(C)]
struct CtlTempProperties {
    size: u32,
    version: u8,
    _pad: [u8; 3],
    sensor_type: u32,
    max_temperature: f64,
}

type CtlInitFn = unsafe extern "system" fn(*mut CtlInitArgs, *mut ApiHandle) -> CtlResult;
type CtlCloseFn = unsafe extern "system" fn(ApiHandle) -> CtlResult;
type CtlEnumerateDevicesFn =
    unsafe extern "system" fn(ApiHandle, *mut u32, *mut DeviceHandle) -> CtlResult;
type CtlEnumTemperatureSensorsFn =
    unsafe extern "system" fn(DeviceHandle, *mut u32, *mut TempHandle) -> CtlResult;
type CtlTemperatureGetPropertiesFn =
    unsafe extern "system" fn(TempHandle, *mut CtlTempProperties) -> CtlResult;
type CtlTemperatureGetStateFn = unsafe extern "system" fn(TempHandle, *mut f64) -> CtlResult;

struct Igcl {
    _dll: Dll,
    api: ApiHandle,
    devices: Vec<DeviceHandle>,
    close: CtlCloseFn,
    enum_temperature_sensors: CtlEnumTemperatureSensorsFn,
    temperature_get_properties: CtlTemperatureGetPropertiesFn,
    temperature_get_state: CtlTemperatureGetStateFn,
}

// IGCL documents its functions as callable from simultaneous threads. The
// process-wide instance is initialized once and only read afterwards.
unsafe impl Send for Igcl {}
unsafe impl Sync for Igcl {}

impl Igcl {
    fn read_temperature(&self) -> Result<f64, String> {
        if self.devices.is_empty() {
            return Err("IGCL reports no Intel GPU".to_string());
        }

        let mut last_error = "no Intel GPU temperature".to_string();
        for (index, device) in self.devices.iter().enumerate() {
            let mut count = 0u32;
            let code =
                unsafe { (self.enum_temperature_sensors)(*device, &mut count, ptr::null_mut()) };
            if !succeeded(code) {
                last_error = format!(
                    "IGCL device {index} temperature enumeration failed with code 0x{code:08X}"
                );
                continue;
            }
            if count == 0 {
                last_error = format!("IGCL device {index} exposes no temperature sensor");
                continue;
            }

            let mut handles = vec![ptr::null_mut(); count as usize];
            let code = unsafe {
                (self.enum_temperature_sensors)(*device, &mut count, handles.as_mut_ptr())
            };
            if !succeeded(code) {
                last_error = format!(
                    "IGCL device {index} temperature handles failed with code 0x{code:08X}"
                );
                continue;
            }
            handles.truncate(count as usize);

            let mut gpu_value = None;
            let mut global_value = None;
            for handle in handles {
                if handle.is_null() {
                    continue;
                }

                let mut properties = CtlTempProperties {
                    size: size_of::<CtlTempProperties>() as u32,
                    version: 0,
                    _pad: [0; 3],
                    sensor_type: u32::MAX,
                    max_temperature: 0.0,
                };
                let code = unsafe { (self.temperature_get_properties)(handle, &mut properties) };
                if !succeeded(code) {
                    continue;
                }

                let mut celsius = 0.0f64;
                let code = unsafe { (self.temperature_get_state)(handle, &mut celsius) };
                if !succeeded(code) || !celsius.is_finite() {
                    continue;
                }

                match properties.sensor_type {
                    CTL_TEMP_SENSORS_GPU => gpu_value = Some(celsius),
                    CTL_TEMP_SENSORS_GLOBAL => global_value = Some(celsius),
                    _ => {}
                }
                if gpu_value.is_some() {
                    break;
                }
            }

            if let Some(celsius) = gpu_value.or(global_value) {
                return Ok(celsius);
            }
            last_error = format!("IGCL device {index} has no readable GPU temperature sensor");
        }

        Err(last_error)
    }
}

impl Drop for Igcl {
    fn drop(&mut self) {
        unsafe {
            let _ = (self.close)(self.api);
        }
    }
}

static IGCL: OnceLock<Result<Igcl, String>> = OnceLock::new();

fn init() -> Result<Igcl, String> {
    let dll = Dll::load("ControlLib.dll")?;
    let ctl_init: CtlInitFn = unsafe { dll.symbol("ctlInit")? };
    let close: CtlCloseFn = unsafe { dll.symbol("ctlClose")? };
    let enumerate_devices: CtlEnumerateDevicesFn = unsafe { dll.symbol("ctlEnumerateDevices")? };
    let enum_temperature_sensors: CtlEnumTemperatureSensorsFn =
        unsafe { dll.symbol("ctlEnumTemperatureSensors")? };
    let temperature_get_properties: CtlTemperatureGetPropertiesFn =
        unsafe { dll.symbol("ctlTemperatureGetProperties")? };
    let temperature_get_state: CtlTemperatureGetStateFn =
        unsafe { dll.symbol("ctlTemperatureGetState")? };

    let mut args = CtlInitArgs {
        size: size_of::<CtlInitArgs>() as u32,
        version: 0,
        _pad: [0; 3],
        app_version: make_version(CTL_IMPL_MAJOR_VERSION, CTL_IMPL_MINOR_VERSION),
        flags: 0,
        supported_version: 0,
        application_uid: CtlApplicationId {
            data1: 0,
            data2: 0,
            data3: 0,
            data4: [0; 8],
        },
    };

    let mut api = ptr::null_mut();
    let code = unsafe { ctl_init(&mut args, &mut api) };
    if !succeeded(code) || api.is_null() {
        return Err(format!("ctlInit failed with code 0x{code:08X}"));
    }

    let mut count = 0u32;
    let code = unsafe { enumerate_devices(api, &mut count, ptr::null_mut()) };
    if !succeeded(code) {
        unsafe {
            let _ = close(api);
        }
        return Err(format!("ctlEnumerateDevices failed with code 0x{code:08X}"));
    }

    let mut devices = Vec::new();
    if count > 0 {
        devices = vec![ptr::null_mut(); count as usize];
        let code = unsafe { enumerate_devices(api, &mut count, devices.as_mut_ptr()) };
        if !succeeded(code) {
            unsafe {
                let _ = close(api);
            }
            return Err(format!(
                "ctlEnumerateDevices (handles) failed with code 0x{code:08X}"
            ));
        }
        devices.truncate(count as usize);
        devices.retain(|handle| !handle.is_null());
    }

    Ok(Igcl {
        _dll: dll,
        api,
        devices,
        close,
        enum_temperature_sensors,
        temperature_get_properties,
        temperature_get_state,
    })
}

/// Reads the first working Intel GPU temperature.
pub fn read() -> Result<f64, ProviderError> {
    let igcl = IGCL
        .get_or_init(init)
        .as_ref()
        .map_err(|error| ProviderError::Unavailable(error.clone()))?;
    igcl.read_temperature().map_err(ProviderError::Io)
}

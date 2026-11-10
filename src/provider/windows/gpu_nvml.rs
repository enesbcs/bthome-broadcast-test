//! NVIDIA GPU temperature through NVML.
//!
//! `nvml-wrapper` loads `nvml.dll` at runtime, so no NVIDIA SDK is needed at
//! build time and the DLL is not packaged with the program.

use std::path::Path;

use nvml_wrapper::enum_wrappers::device::TemperatureSensor;
use nvml_wrapper::Nvml;

use crate::provider::ProviderError;

/// Reads the first working NVIDIA GPU temperature.
pub fn read() -> Result<f64, ProviderError> {
    let nvml = init()
        .map_err(|error| ProviderError::Unavailable(format!("NVML is not available: {error}")))?;

    let count = nvml.device_count().map_err(|error| {
        ProviderError::Unavailable(format!("NVML device count failed: {error}"))
    })?;
    if count == 0 {
        return Err(ProviderError::Unavailable(
            "NVML reports no NVIDIA GPU".to_string(),
        ));
    }

    let mut last_error = None;
    for index in 0..count {
        let device = match nvml.device_by_index(index) {
            Ok(device) => device,
            Err(error) => {
                last_error = Some(format!("device {index}: {error}"));
                continue;
            }
        };
        match device.temperature(TemperatureSensor::Gpu) {
            Ok(celsius) => return Ok(celsius as f64),
            Err(error) => last_error = Some(format!("device {index}: {error}")),
        }
    }

    Err(ProviderError::Unavailable(format!(
        "no NVIDIA GPU temperature ({})",
        last_error.unwrap_or_else(|| "unknown error".to_string())
    )))
}

fn init() -> Result<Nvml, nvml_wrapper::error::NvmlError> {
    match Nvml::init() {
        Ok(nvml) => Ok(nvml),
        Err(error) => {
            // Some driver installations do not put nvml.dll on the default
            // search path, but keep it in the NVSMI directory.
            let path = Path::new(r"C:\Program Files\NVIDIA Corporation\NVSMI\nvml.dll");
            Nvml::builder()
                .lib_path(path.as_os_str())
                .init()
                .map_err(|_| error)
        }
    }
}

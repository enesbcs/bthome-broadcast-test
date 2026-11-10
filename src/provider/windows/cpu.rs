//! Windows CPU temperature.
//!
//! A real package temperature comes from an MSR, which user mode can only read
//! with a kernel driver. The project deliberately does not install a driver and
//! does not estimate a value, so this rung is unavailable. ACPI is a separate,
//! last-resort rung and is never reported as CPU temperature.

use crate::provider::ProviderError;

/// Always reports that no driverless CPU sensor is available.
pub fn read() -> Result<f64, ProviderError> {
    Err(ProviderError::Unavailable(
        "Windows user mode cannot read the CPU package temperature without a kernel driver"
            .to_string(),
    ))
}

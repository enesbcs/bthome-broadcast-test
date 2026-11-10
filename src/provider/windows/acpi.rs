//! ACPI thermal zone temperature through PDH.
//!
//! This is the last rung of the ladder. The ACPI thermal zone is not a CPU
//! sensor, so its value is reported with the `-A` suffix and never as a CPU
//! temperature. The Windows performance counters expose every thermal zone;
//! the hottest plausible zone wins.

use std::mem::size_of;

use windows::core::PCWSTR;
use windows::Win32::System::Performance::{
    PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW,
    PdhOpenQueryW, PDH_CSTATUS_NEW_DATA, PDH_CSTATUS_VALID_DATA, PDH_FMT_COUNTERVALUE_ITEM_W,
    PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY, PDH_MORE_DATA,
};

use crate::provider::{ProviderError, MAX_CELSIUS, MIN_CELSIUS};

/// A counter path and its conversion to degrees Celsius.
struct Counter {
    path: &'static str,
    scale: f64,
    offset: f64,
}

/// `High Precision Temperature` is in kelvin; the older `Temperature` counter
/// is in tenths of kelvin.
const COUNTERS: [Counter; 2] = [
    Counter {
        path: r"\Thermal Zone Information(*)\High Precision Temperature",
        scale: 1.0,
        offset: -273.15,
    },
    Counter {
        path: r"\Thermal Zone Information(*)\Temperature",
        scale: 0.1,
        offset: -273.15,
    },
];

/// Reads the hottest ACPI thermal zone.
pub fn read() -> Result<f64, ProviderError> {
    let mut first_error = None;
    for counter in &COUNTERS {
        match read_counter(counter) {
            Ok(celsius) => return Ok(celsius),
            Err(error) => {
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }
    }

    Err(ProviderError::Unavailable(format!(
        "no ACPI thermal zone temperature ({})",
        first_error.unwrap_or_else(|| "no counter".to_string())
    )))
}

/// Reads every instance of one counter and returns the hottest plausible one.
fn read_counter(counter: &Counter) -> Result<f64, String> {
    let mut query = PDH_HQUERY::default();
    let status = unsafe { PdhOpenQueryW(PCWSTR::null(), 0, &mut query) };
    if status != 0 {
        return Err(pdh_error("PdhOpenQueryW", status));
    }
    let _query = QueryGuard(query);

    let path = wide(counter.path);
    let mut handle = PDH_HCOUNTER::default();
    let status = unsafe { PdhAddEnglishCounterW(query, PCWSTR(path.as_ptr()), 0, &mut handle) };
    if status != 0 {
        return Err(pdh_error("PdhAddEnglishCounterW", status));
    }

    // Temperature counters are instantaneous, but a second sample covers
    // drivers that only publish a formatted value after the first collection.
    let mut last_error = String::from("no thermal zone instance reported a value");
    for attempt in 0..2 {
        if attempt > 0 {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let status = unsafe { PdhCollectQueryData(query) };
        if status != 0 {
            last_error = pdh_error("PdhCollectQueryData", status);
            continue;
        }
        match read_values(handle, counter) {
            Ok(Some(celsius)) => return Ok(celsius),
            Ok(None) => {}
            Err(error) => last_error = error,
        }
    }
    Err(last_error)
}

/// Reads the formatted counter array and picks the hottest plausible value.
fn read_values(handle: PDH_HCOUNTER, counter: &Counter) -> Result<Option<f64>, String> {
    let mut size = 0u32;
    let mut count = 0u32;
    let status = unsafe {
        PdhGetFormattedCounterArrayW(handle, PDH_FMT_DOUBLE, &mut size, &mut count, None)
    };
    if status != PDH_MORE_DATA && status != 0 {
        return Err(pdh_error("PdhGetFormattedCounterArrayW", status));
    }
    if size == 0 || count == 0 {
        return Ok(None);
    }

    let item_size = size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>();
    let capacity = (size as usize).div_ceil(item_size);
    let mut buffer: Vec<PDH_FMT_COUNTERVALUE_ITEM_W> =
        vec![PDH_FMT_COUNTERVALUE_ITEM_W::default(); capacity];

    let status = unsafe {
        PdhGetFormattedCounterArrayW(
            handle,
            PDH_FMT_DOUBLE,
            &mut size,
            &mut count,
            Some(buffer.as_mut_ptr()),
        )
    };
    if status != 0 {
        return Err(pdh_error("PdhGetFormattedCounterArrayW", status));
    }
    let count = (count as usize).min(buffer.len());
    let items = &buffer[..count];

    let mut hottest: Option<f64> = None;
    for item in items {
        let status = item.FmtValue.CStatus;
        if status != PDH_CSTATUS_VALID_DATA && status != PDH_CSTATUS_NEW_DATA {
            continue;
        }
        let value = unsafe { item.FmtValue.Anonymous.doubleValue };
        if !value.is_finite() {
            continue;
        }
        // `High Precision Temperature` is in kelvin, while some driver
        // versions publish tenths of a kelvin through the same counter. Both
        // readings are tried before the value is discarded.
        for scale in [counter.scale, counter.scale / 10.0] {
            let celsius = value * scale + counter.offset;
            if !(MIN_CELSIUS..=MAX_CELSIUS).contains(&celsius) {
                continue;
            }
            hottest = Some(hottest.map_or(celsius, |current| current.max(celsius)));
            break;
        }
    }
    Ok(hottest)
}

/// Closes the PDH query on every path.
struct QueryGuard(PDH_HQUERY);

impl Drop for QueryGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = PdhCloseQuery(self.0);
        }
    }
}

/// Converts a Rust string to a null-terminated UTF-16 buffer.
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Human-readable PDH status.
fn pdh_error(call: &str, status: u32) -> String {
    format!("{call} failed with PDH status 0x{status:08X}")
}

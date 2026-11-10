//! CPU temperature from hwmon or the package thermal zone.
//!
//! Only sensors that are known to measure the CPU package/die are accepted.
//! Chipset sensors (`pch_*`), board sensors (`gigabyte_wmi`, ...) and ACPI
//! zones (`acpitz`) are deliberately ignored: ACPI is a separate, last-resort
//! rung, and chipset/board readings are not CPU temperature.

use std::path::Path;

use crate::provider::ProviderError;

use super::sysfs::{hwmon_channels, read_millidegrees, thermal_zones, HwmonChannel};

/// hwmon drivers that expose a CPU package/die sensor.
const HWMON_NAMES: &[&str] = &[
    "coretemp",
    "k10temp",
    "zenpower",
    "cpu_thermal",
    "soc_thermal",
    "cpu",
];

/// Thermal zone types used by CPU drivers on architectures without hwmon.
const THERMAL_TYPES: &[&str] = &["x86_pkg_temp", "cpu-thermal", "cpu_thermal", "soc_thermal"];

/// Reads the CPU temperature in degrees Celsius.
pub fn read(root: &Path) -> Result<f64, ProviderError> {
    let channels = hwmon_channels(root);
    let mut last_error = None;

    for name in HWMON_NAMES {
        let mut matching: Vec<&HwmonChannel> = channels
            .iter()
            .filter(|channel| channel.device == *name)
            .collect();
        if matching.is_empty() {
            continue;
        }

        // Prefer the package sensor, then temp1, then any other channel.
        if let Some(index) = matching
            .iter()
            .position(|channel| channel.label == "Package id 0")
        {
            let channel = matching.remove(index);
            matching.insert(0, channel);
        } else if let Some(index) = matching
            .iter()
            .position(|channel| channel.input.ends_with("temp1_input"))
        {
            let channel = matching.remove(index);
            matching.insert(0, channel);
        }

        for channel in matching {
            match read_millidegrees(&channel.input) {
                Ok(celsius) => return Ok(celsius),
                Err(error) => last_error = Some(error),
            }
        }
    }

    for zone_type in THERMAL_TYPES {
        for zone in thermal_zones(root)
            .iter()
            .filter(|zone| zone.zone_type == *zone_type)
        {
            match read_millidegrees(&zone.temp) {
                Ok(celsius) => return Ok(celsius),
                Err(error) => last_error = Some(error),
            }
        }
    }

    Err(last_error.unwrap_or_else(|| {
        ProviderError::Unavailable(
            "no CPU temperature sensor (coretemp/k10temp/cpu_thermal or x86_pkg_temp)".to_string(),
        )
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::linux::sysfs::fixture::Fixture;

    #[test]
    fn picks_the_coretemp_package_sensor() {
        let fixture = Fixture::new("cpu-package");
        fixture.file("sys/class/hwmon/hwmon0/name", "coretemp\n");
        fixture.file("sys/class/hwmon/hwmon0/temp1_input", "51000\n");
        fixture.file("sys/class/hwmon/hwmon0/temp1_label", "Core 0\n");
        fixture.file("sys/class/hwmon/hwmon0/temp2_input", "62000\n");
        fixture.file("sys/class/hwmon/hwmon0/temp2_label", "Package id 0\n");

        assert_eq!(read(fixture.path()), Ok(62.0));
    }

    #[test]
    fn ignores_chipset_board_and_acpi_sensors() {
        let fixture = Fixture::new("cpu-ignore");
        fixture.file("sys/class/hwmon/hwmon0/name", "gigabyte_wmi\n");
        fixture.file("sys/class/hwmon/hwmon0/temp1_input", "99000\n");
        fixture.file("sys/class/hwmon/hwmon1/name", "acpitz\n");
        fixture.file("sys/class/hwmon/hwmon1/temp1_input", "88000\n");
        fixture.file("sys/class/hwmon/hwmon2/name", "pch_cometlake\n");
        fixture.file("sys/class/hwmon/hwmon2/temp1_input", "77000\n");
        fixture.file("sys/class/thermal/thermal_zone0/type", "acpitz\n");
        fixture.file("sys/class/thermal/thermal_zone0/temp", "27000\n");

        assert!(matches!(
            read(fixture.path()),
            Err(ProviderError::Unavailable(_))
        ));
    }

    #[test]
    fn falls_back_to_the_package_thermal_zone() {
        let fixture = Fixture::new("cpu-thermal");
        fixture.file("sys/class/thermal/thermal_zone0/type", "acpitz\n");
        fixture.file("sys/class/thermal/thermal_zone0/temp", "21000\n");
        fixture.file("sys/class/thermal/thermal_zone1/type", "x86_pkg_temp\n");
        fixture.file("sys/class/thermal/thermal_zone1/temp", "65000\n");

        assert_eq!(read(fixture.path()), Ok(65.0));
    }

    #[test]
    fn continues_when_the_first_channel_is_unreadable() {
        let fixture = Fixture::new("cpu-bad-channel");
        fixture.file("sys/class/hwmon/hwmon0/name", "k10temp\n");
        fixture.file("sys/class/hwmon/hwmon0/temp1_input", "not-a-number\n");
        fixture.file("sys/class/hwmon/hwmon0/temp2_input", "48000\n");

        assert_eq!(read(fixture.path()).unwrap(), 48.0);
    }
}

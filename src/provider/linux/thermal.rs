//! ACPI thermal zone fallback.
//!
//! ACPI is the last rung and is never reported as CPU temperature. The hottest
//! plausible `acpitz` zone is used, matching the approved design decision.

use std::path::Path;

use crate::provider::ProviderError;

use super::sysfs::{read_millidegrees, thermal_zones};

/// Reads the hottest ACPI thermal zone in degrees Celsius.
pub fn read(root: &Path) -> Result<f64, ProviderError> {
    let mut best: Option<f64> = None;
    let mut last_error = None;

    for zone in thermal_zones(root) {
        if !zone.zone_type.to_ascii_lowercase().contains("acpi") {
            continue;
        }
        match read_millidegrees(&zone.temp) {
            Ok(celsius) if celsius.is_finite() && (-40.0..=150.0).contains(&celsius) => {
                best = Some(best.map_or(celsius, |current| current.max(celsius)));
            }
            Ok(_) => {}
            Err(error) => last_error = Some(error),
        }
    }

    best.ok_or_else(|| {
        last_error.unwrap_or_else(|| {
            ProviderError::Unavailable("no ACPI thermal zone (acpitz)".to_string())
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::linux::sysfs::fixture::Fixture;

    #[test]
    fn picks_the_hottest_acpi_zone() {
        let fixture = Fixture::new("acpi-hot");
        fixture.file("sys/class/thermal/thermal_zone0/type", "acpitz\n");
        fixture.file("sys/class/thermal/thermal_zone0/temp", "21000\n");
        fixture.file("sys/class/thermal/thermal_zone1/type", "acpitz\n");
        fixture.file("sys/class/thermal/thermal_zone1/temp", "34500\n");
        fixture.file("sys/class/thermal/thermal_zone2/type", "x86_pkg_temp\n");
        fixture.file("sys/class/thermal/thermal_zone2/temp", "90000\n");

        assert_eq!(read(fixture.path()), Ok(34.5));
    }

    #[test]
    fn skips_out_of_range_zones() {
        let fixture = Fixture::new("acpi-range");
        fixture.file("sys/class/thermal/thermal_zone0/type", "acpitz\n");
        fixture.file("sys/class/thermal/thermal_zone0/temp", "0\n");
        fixture.file("sys/class/thermal/thermal_zone1/type", "acpitz\n");
        fixture.file("sys/class/thermal/thermal_zone1/temp", "30000\n");

        assert_eq!(read(fixture.path()), Ok(30.0));
    }

    #[test]
    fn reports_missing_acpi_zone() {
        let fixture = Fixture::new("acpi-none");
        fixture.file("sys/class/thermal/thermal_zone0/type", "x86_pkg_temp\n");
        fixture.file("sys/class/thermal/thermal_zone0/temp", "70000\n");

        assert!(matches!(
            read(fixture.path()),
            Err(ProviderError::Unavailable(_))
        ));
    }
}

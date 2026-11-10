//! GPU temperature from the graphics driver hwmon interface.
//!
//! NVIDIA, AMD and Intel drivers that expose hwmon all use the standard
//! `tempN_input` files. Proprietary NVIDIA drivers without hwmon are not
//! covered here; that is the optional NVML path and is not required for the
//! Linux baseline.

use std::path::Path;

use crate::provider::ProviderError;

use super::sysfs::{hwmon_channels, preferred_channel, read_millidegrees, HwmonChannel};

/// hwmon drivers that represent a GPU.
const HWMON_NAMES: &[&str] = &["amdgpu", "nouveau", "i915", "xe", "radeon", "nvidia"];

/// Reads the GPU temperature in degrees Celsius.
pub fn read(root: &Path) -> Result<f64, ProviderError> {
    let channels = hwmon_channels(root);
    let mut last_error = None;

    for name in HWMON_NAMES {
        let matching: Vec<HwmonChannel> = channels
            .iter()
            .filter(|channel| channel.device == *name)
            .cloned()
            .collect();
        if matching.is_empty() {
            continue;
        }

        // `edge` is the die temperature reported by amdgpu; temp1 is the same
        // sensor on the other drivers.
        let mut ordered = matching;
        if let Some(index) = ordered.iter().position(|channel| channel.label == "edge") {
            let channel = ordered.remove(index);
            ordered.insert(0, channel);
        } else if let Some(channel) = preferred_channel(&ordered) {
            let input = channel.input.clone();
            ordered.sort_by_key(|candidate| candidate.input != input);
        }

        for channel in ordered {
            match read_millidegrees(&channel.input) {
                Ok(celsius) => return Ok(celsius),
                Err(error) => last_error = Some(error),
            }
        }
    }

    Err(last_error.unwrap_or_else(|| {
        ProviderError::Unavailable(
            "no GPU temperature sensor (amdgpu/nouveau/i915/xe hwmon)".to_string(),
        )
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::linux::sysfs::fixture::Fixture;

    #[test]
    fn reads_the_amdgpu_edge_sensor() {
        let fixture = Fixture::new("gpu-amd");
        fixture.file("sys/class/hwmon/hwmon0/name", "amdgpu\n");
        fixture.file("sys/class/hwmon/hwmon0/temp1_input", "54000\n");
        fixture.file("sys/class/hwmon/hwmon0/temp1_label", "edge\n");
        fixture.file("sys/class/hwmon/hwmon0/temp2_input", "61000\n");
        fixture.file("sys/class/hwmon/hwmon0/temp2_label", "junction\n");

        assert_eq!(read(fixture.path()), Ok(54.0));
    }

    #[test]
    fn falls_back_to_intel_hwmon() {
        let fixture = Fixture::new("gpu-intel");
        fixture.file("sys/class/hwmon/hwmon0/name", "coretemp\n");
        fixture.file("sys/class/hwmon/hwmon0/temp1_input", "70000\n");
        fixture.file("sys/class/hwmon/hwmon1/name", "i915\n");
        fixture.file("sys/class/hwmon/hwmon1/temp1_input", "43000\n");

        assert_eq!(read(fixture.path()), Ok(43.0));
    }

    #[test]
    fn reports_missing_gpu() {
        let fixture = Fixture::new("gpu-none");
        fixture.file("sys/class/hwmon/hwmon0/name", "coretemp\n");
        fixture.file("sys/class/hwmon/hwmon0/temp1_input", "70000\n");

        assert!(matches!(
            read(fixture.path()),
            Err(ProviderError::Unavailable(_))
        ));
    }
}

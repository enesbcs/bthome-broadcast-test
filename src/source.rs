//! Temperature source identity and the short advertisement-name suffixes.

/// A rung of the temperature ladder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Cpu,
    Gpu,
    Disk,
    Acpi,
    /// Area weather forecast used when no real device sensor is available.
    Weather,
    /// Clearly marked test data used as the absolute last resort.
    Fake,
}

impl Source {
    /// Two-character suffix appended to the BLE name.
    pub fn suffix(self) -> &'static str {
        match self {
            Source::Cpu => "-C",
            Source::Gpu => "-G",
            Source::Disk => "-D",
            Source::Acpi => "-A",
            Source::Weather => "-W",
            Source::Fake => "-T",
        }
    }

    /// English label used in logs and diagnostics.
    pub fn label(self) -> &'static str {
        match self {
            Source::Cpu => "CPU",
            Source::Gpu => "GPU",
            Source::Disk => "disk",
            Source::Acpi => "ACPI",
            Source::Weather => "weather",
            Source::Fake => "TEST",
        }
    }

    /// Whether this value is synthetic test data rather than a measurement.
    pub fn is_test_data(self) -> bool {
        matches!(self, Source::Fake)
    }

    /// Whether this value is an area forecast rather than a device sensor.
    pub fn is_forecast(self) -> bool {
        matches!(self, Source::Weather)
    }

    /// Long warning for sources that do not measure this machine.
    pub fn warning(self) -> Option<&'static str> {
        match self {
            Source::Weather => Some(
                "Weather forecast: no device temperature sensor is available; the advertised value is an area forecast.",
            ),
            Source::Fake => Some(
                "TEST DATA: no temperature source is available; the advertised values are generated and are not measurements.",
            ),
            _ => None,
        }
    }

    /// Ladder order used by the application.
    #[cfg(target_os = "windows")]
    pub const ALL: [Source; 6] = [
        Source::Cpu,
        Source::Gpu,
        Source::Disk,
        Source::Acpi,
        Source::Weather,
        Source::Fake,
    ];

    /// Ladder order used by the application.
    #[cfg(not(target_os = "windows"))]
    pub const ALL: [Source; 5] = [
        Source::Cpu,
        Source::Gpu,
        Source::Disk,
        Source::Acpi,
        Source::Fake,
    ];
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suffixes_are_two_chars_and_unique() {
        let mut seen = Vec::new();
        for source in Source::ALL {
            let suffix = source.suffix();
            assert_eq!(suffix.len(), 2, "suffix must be two characters: {suffix}");
            assert!(suffix.starts_with('-'));
            assert!(!seen.contains(&suffix));
            seen.push(suffix);
        }
    }

    #[test]
    fn synthetic_sources_are_marked() {
        assert!(Source::Weather.is_forecast());
        assert!(!Source::Weather.is_test_data());
        assert!(Source::Fake.is_test_data());
        assert!(!Source::Cpu.is_test_data());
        assert!(!Source::Cpu.is_forecast());
    }
}

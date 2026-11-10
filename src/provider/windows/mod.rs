//! Windows temperature providers.
//!
//! The ladder follows the approved order: CPU (unavailable without a driver),
//! NVIDIA NVML, AMD ADLX, Intel IGCL, the system disk, and the ACPI thermal
//! zone, followed by test data. The area weather forecast is inserted before
//! test data only when the user enabled it (console flag or GUI prompt). Each
//! rung is a direct API call; no hardware-monitor framework is involved.

mod acpi;
mod cpu;
mod disk;
mod dll;
mod gpu_amd;
mod gpu_intel;
mod gpu_nvml;

use crate::provider::{BoxedProvider, ProviderError, TemperatureProvider};
use crate::source::Source;

/// Which provider a rung reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Cpu,
    Nvidia,
    Amd,
    Intel,
    Disk,
    Acpi,
}

/// One Windows ladder rung.
#[derive(Clone, Copy, Debug)]
pub struct WindowsProvider {
    kind: Kind,
}

impl WindowsProvider {
    pub fn new(kind: Kind) -> Self {
        Self { kind }
    }

    pub fn kind(self) -> Kind {
        self.kind
    }
}

impl TemperatureProvider for WindowsProvider {
    fn source(&self) -> Source {
        match self.kind {
            Kind::Cpu => Source::Cpu,
            Kind::Nvidia | Kind::Amd | Kind::Intel => Source::Gpu,
            Kind::Disk => Source::Disk,
            Kind::Acpi => Source::Acpi,
        }
    }

    fn label(&self) -> &'static str {
        match self.kind {
            Kind::Cpu => "CPU (Windows)",
            Kind::Nvidia => "NVIDIA NVML",
            Kind::Amd => "AMD ADLX",
            Kind::Intel => "Intel IGCL",
            Kind::Disk => "disk (storage API)",
            Kind::Acpi => "ACPI (PDH)",
        }
    }

    fn read(&self) -> Result<f64, ProviderError> {
        match self.kind {
            Kind::Cpu => cpu::read(),
            Kind::Nvidia => gpu_nvml::read(),
            Kind::Amd => gpu_amd::read(),
            Kind::Intel => gpu_intel::read(),
            Kind::Disk => disk::read(),
            Kind::Acpi => acpi::read(),
        }
    }
}

/// Builds the Windows ladder in priority order.
pub fn build_ladder(options: &crate::cli::Options) -> Vec<BoxedProvider> {
    let mut providers: Vec<BoxedProvider> = vec![
        Box::new(WindowsProvider::new(Kind::Cpu)),
        Box::new(WindowsProvider::new(Kind::Nvidia)),
        Box::new(WindowsProvider::new(Kind::Amd)),
        Box::new(WindowsProvider::new(Kind::Intel)),
        Box::new(WindowsProvider::new(Kind::Disk)),
    ];
    if !options.no_acpi {
        providers.push(Box::new(WindowsProvider::new(Kind::Acpi)));
    }
    if options.forecast_enabled {
        providers.push(Box::new(crate::provider::weather::WeatherProvider::new()));
    } else if options.forecast_prompt {
        providers.push(Box::new(
            crate::provider::weather::WeatherProvider::with_prompt(),
        ));
    }
    providers.push(Box::new(crate::provider::fake::FakeProvider::new()));
    providers
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_mapping_matches_the_ladder() {
        let providers = build_ladder(&crate::cli::Options::default());
        let sources: Vec<Source> = providers.iter().map(|provider| provider.source()).collect();
        assert_eq!(
            sources,
            vec![
                Source::Cpu,
                Source::Gpu,
                Source::Gpu,
                Source::Gpu,
                Source::Disk,
                Source::Acpi,
                Source::Fake,
            ]
        );
    }

    #[test]
    fn gui_prompt_inserts_weather_before_test_data() {
        let options = crate::cli::Options {
            forecast_prompt: true,
            ..crate::cli::Options::default()
        };
        let providers = build_ladder(&options);
        let sources: Vec<Source> = providers.iter().map(|provider| provider.source()).collect();
        assert_eq!(
            sources,
            vec![
                Source::Cpu,
                Source::Gpu,
                Source::Gpu,
                Source::Gpu,
                Source::Disk,
                Source::Acpi,
                Source::Weather,
                Source::Fake,
            ]
        );
    }

    #[test]
    fn forecast_flag_inserts_weather_before_test_data() {
        let options = crate::cli::Options {
            forecast_enabled: true,
            ..crate::cli::Options::default()
        };
        let providers = build_ladder(&options);
        let sources: Vec<Source> = providers.iter().map(|provider| provider.source()).collect();
        assert_eq!(
            sources,
            vec![
                Source::Cpu,
                Source::Gpu,
                Source::Gpu,
                Source::Gpu,
                Source::Disk,
                Source::Acpi,
                Source::Weather,
                Source::Fake,
            ]
        );
    }
}

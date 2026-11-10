//! Linux temperature providers backed by sysfs.
//!
//! The real hardware order is CPU -> GPU -> disk -> ACPI; the platform ladder
//! then appends the weather and test-data fallbacks. Each rung re-reads sysfs
//! on demand; no background thread or cached sample is kept.

mod cpu;
mod disk;
mod gpu;
mod sysfs;
mod thermal;

use std::path::{Path, PathBuf};

use crate::provider::{BoxedProvider, ProviderError, TemperatureProvider};
use crate::source::Source;

/// Which sensor family a provider reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Cpu,
    Gpu,
    Disk,
    Acpi,
}

/// A provider that reads one rung from a sysfs tree.
#[derive(Clone, Debug)]
pub struct SysfsProvider {
    root: PathBuf,
    kind: Kind,
}

impl SysfsProvider {
    pub fn new(root: impl Into<PathBuf>, kind: Kind) -> Self {
        Self {
            root: root.into(),
            kind,
        }
    }

    pub fn kind(&self) -> Kind {
        self.kind
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

impl TemperatureProvider for SysfsProvider {
    fn source(&self) -> Source {
        match self.kind {
            Kind::Cpu => Source::Cpu,
            Kind::Gpu => Source::Gpu,
            Kind::Disk => Source::Disk,
            Kind::Acpi => Source::Acpi,
        }
    }

    fn label(&self) -> &'static str {
        match self.kind {
            Kind::Cpu => "CPU (hwmon/thermal)",
            Kind::Gpu => "GPU (hwmon)",
            Kind::Disk => "disk (hwmon)",
            Kind::Acpi => "ACPI (thermal zone)",
        }
    }

    fn read(&self) -> Result<f64, ProviderError> {
        match self.kind {
            Kind::Cpu => cpu::read(&self.root),
            Kind::Gpu => gpu::read(&self.root),
            Kind::Disk => disk::read(&self.root),
            Kind::Acpi => thermal::read(&self.root),
        }
    }
}

/// Builds the Linux ladder in priority order.
///
/// Linux has no weather fallback: after the real hardware rungs, clearly
/// marked test data is the absolute last resort.
pub fn build_ladder(options: &crate::cli::Options) -> Vec<BoxedProvider> {
    let root = PathBuf::from("/");
    let mut providers: Vec<BoxedProvider> = vec![
        Box::new(SysfsProvider::new(root.clone(), Kind::Cpu)),
        Box::new(SysfsProvider::new(root.clone(), Kind::Gpu)),
        Box::new(SysfsProvider::new(root.clone(), Kind::Disk)),
    ];
    if !options.no_acpi {
        providers.push(Box::new(SysfsProvider::new(root, Kind::Acpi)));
    }
    providers.push(Box::new(crate::provider::fake::FakeProvider::new()));
    providers
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::Options;

    fn options(no_acpi: bool) -> Options {
        Options {
            no_acpi,
            interval_secs: Options::DEFAULT_INTERVAL_SECS,
            ..Options::default()
        }
    }

    #[test]
    fn ladder_matches_the_approved_order() {
        let providers = build_ladder(&options(false));
        let sources: Vec<Source> = providers.iter().map(|provider| provider.source()).collect();
        assert_eq!(
            sources,
            vec![
                Source::Cpu,
                Source::Gpu,
                Source::Disk,
                Source::Acpi,
                Source::Fake,
            ]
        );
    }

    #[test]
    fn no_acpi_removes_the_acpi_rung() {
        let providers = build_ladder(&options(true));
        let sources: Vec<Source> = providers.iter().map(|provider| provider.source()).collect();
        assert_eq!(
            sources,
            vec![Source::Cpu, Source::Gpu, Source::Disk, Source::Fake]
        );
    }
}

//! Temperature provider abstraction and the fallback ladder.
//!
//! The ladder keeps trying rungs until one yields a plausible value and
//! re-probes from the top after repeated failures. It is testable with fake
//! providers. Every platform ends with clearly marked test data; only the
//! Windows build can insert the opt-in area weather forecast before it.

use crate::source::Source;

/// Plausible lower bound in °C.
pub const MIN_CELSIUS: f64 = -40.0;

/// Plausible upper bound in °C.
pub const MAX_CELSIUS: f64 = 150.0;

/// Number of consecutive failures before the ladder re-probes from the top.
pub const MAX_CONSECUTIVE_FAILURES: u32 = 3;

/// Why a provider could not deliver a value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProviderError {
    /// The interface or sensor is not present on this machine.
    Unavailable(String),
    /// The interface exists but the read failed.
    Io(String),
}

impl ProviderError {
    /// Short technical reason for logs and diagnostics.
    pub fn reason(&self) -> &str {
        match self {
            ProviderError::Unavailable(reason) | ProviderError::Io(reason) => reason,
        }
    }
}

#[cfg(target_os = "linux")]
pub mod linux;

#[cfg(target_os = "windows")]
pub mod windows;

pub mod fake;

#[cfg(target_os = "windows")]
pub mod weather;

/// Type-erased provider used by the application ladder.
pub type BoxedProvider = Box<dyn TemperatureProvider>;

impl TemperatureProvider for BoxedProvider {
    fn source(&self) -> Source {
        (**self).source()
    }

    fn label(&self) -> &'static str {
        (**self).label()
    }

    fn read(&self) -> Result<f64, ProviderError> {
        (**self).read()
    }
}

/// Builds the platform ladder in priority order.
pub fn build_platform_ladder(options: &crate::cli::Options) -> Vec<BoxedProvider> {
    #[cfg(target_os = "linux")]
    {
        linux::build_ladder(options)
    }

    #[cfg(target_os = "windows")]
    {
        windows::build_ladder(options)
    }

    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        let _ = options;
        Vec::new()
    }
}

/// A single temperature source.
pub trait TemperatureProvider {
    /// Which name suffix this source maps to.
    fn source(&self) -> Source;

    /// Human-readable rung name for logs, e.g. `"NVIDIA NVML"`.
    fn label(&self) -> &'static str;

    /// Reads the current temperature in °C.
    fn read(&self) -> Result<f64, ProviderError>;
}

/// Whether a value is a plausible temperature.
pub fn is_plausible(celsius: f64) -> bool {
    celsius.is_finite() && (MIN_CELSIUS..=MAX_CELSIUS).contains(&celsius)
}

/// A successful ladder read.
#[derive(Clone, Debug, PartialEq)]
pub struct Reading {
    pub celsius: f64,
    pub source: Source,
    pub label: &'static str,
    pub index: usize,
}

/// Ordered fallback ladder.
///
/// The first provider that returns a plausible value becomes active. While it
/// keeps working, it stays active even if higher-priority providers recover.
/// After [`MAX_CONSECUTIVE_FAILURES`] consecutive failures the ladder re-probes
/// from the top and switches to the first working rung.
pub struct Ladder<P: TemperatureProvider> {
    providers: Vec<P>,
    active: Option<usize>,
    consecutive_failures: u32,
    max_failures: u32,
    last_error: Option<String>,
}

impl<P: TemperatureProvider> Ladder<P> {
    /// Creates a ladder from providers in priority order (highest first).
    pub fn new(providers: Vec<P>) -> Self {
        Self {
            providers,
            active: None,
            consecutive_failures: 0,
            max_failures: MAX_CONSECUTIVE_FAILURES,
            last_error: None,
        }
    }

    /// Number of rungs.
    pub fn len(&self) -> usize {
        self.providers.len()
    }

    /// Whether the ladder has no rungs.
    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }

    /// Index of the active rung, if any.
    pub fn active_index(&self) -> Option<usize> {
        self.active
    }

    /// Last technical error, for diagnostics.
    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    /// Reads the next value according to the fallback rules.
    pub fn read(&mut self) -> Option<Reading> {
        if let Some(index) = self.active {
            if let Some(reading) = self.try_index(index) {
                self.consecutive_failures = 0;
                self.last_error = None;
                return Some(reading);
            }

            self.consecutive_failures += 1;
            if self.consecutive_failures < self.max_failures {
                return None;
            }
            self.active = None;
            self.consecutive_failures = 0;
        }

        self.probe_all()
    }

    fn probe_all(&mut self) -> Option<Reading> {
        for index in 0..self.providers.len() {
            if let Some(reading) = self.try_index(index) {
                self.active = Some(index);
                self.consecutive_failures = 0;
                self.last_error = None;
                return Some(reading);
            }
        }
        None
    }

    fn try_index(&mut self, index: usize) -> Option<Reading> {
        let source = self.providers[index].source();
        let label = self.providers[index].label();
        match self.providers[index].read() {
            Ok(celsius) if is_plausible(celsius) => Some(Reading {
                celsius,
                source,
                label,
                index,
            }),
            Ok(celsius) => {
                self.last_error = Some(format!("{label}: implausible value: {celsius} °C"));
                None
            }
            Err(error) => {
                self.last_error = Some(format!("{label}: {}", error.reason()));
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::VecDeque;

    struct Fake {
        label: &'static str,
        source: Source,
        values: RefCell<VecDeque<Result<f64, ProviderError>>>,
        reads: RefCell<usize>,
    }

    impl Fake {
        fn new(
            label: &'static str,
            source: Source,
            values: Vec<Result<f64, ProviderError>>,
        ) -> Self {
            Self {
                label,
                source,
                values: RefCell::new(values.into()),
                reads: RefCell::new(0),
            }
        }

        fn always(label: &'static str, source: Source, value: f64) -> Self {
            Self {
                label,
                source,
                values: RefCell::new(VecDeque::new()),
                reads: RefCell::new(0),
            }
            .always_value(value)
        }

        fn always_value(self, value: f64) -> Self {
            self.values.borrow_mut().push_back(Ok(value));
            self
        }

        fn reads(&self) -> usize {
            *self.reads.borrow()
        }
    }

    impl TemperatureProvider for Fake {
        fn source(&self) -> Source {
            self.source
        }

        fn label(&self) -> &'static str {
            self.label
        }

        fn read(&self) -> Result<f64, ProviderError> {
            *self.reads.borrow_mut() += 1;
            let mut values = self.values.borrow_mut();
            if values.len() == 1 {
                values.front().cloned().unwrap()
            } else {
                values.pop_front().unwrap_or(Err(ProviderError::Unavailable(
                    "no more values".to_string(),
                )))
            }
        }
    }

    #[test]
    fn picks_first_working_provider() {
        let mut ladder = Ladder::new(vec![
            Fake::new(
                "cpu",
                Source::Cpu,
                vec![Err(ProviderError::Unavailable("missing".into()))],
            ),
            Fake::always("gpu", Source::Gpu, 55.0),
            Fake::always("disk", Source::Disk, 40.0),
        ]);

        let reading = ladder.read().unwrap();
        assert_eq!(reading.source, Source::Gpu);
        assert_eq!(reading.index, 1);
        assert_eq!(ladder.active_index(), Some(1));
    }

    #[test]
    fn rejects_implausible_values() {
        let mut ladder = Ladder::new(vec![
            Fake::always("cpu", Source::Cpu, 300.0),
            Fake::always("gpu", Source::Gpu, -60.0),
            Fake::always("acpi", Source::Acpi, 42.0),
        ]);

        let reading = ladder.read().unwrap();
        assert_eq!(reading.source, Source::Acpi);
        assert_eq!(ladder.active_index(), Some(2));
    }

    #[test]
    fn keeps_active_provider_until_three_failures() {
        let mut ladder = Ladder::new(vec![
            Fake::new(
                "cpu",
                Source::Cpu,
                vec![
                    Ok(50.0),
                    Err(ProviderError::Io("failed".into())),
                    Err(ProviderError::Io("failed".into())),
                    Err(ProviderError::Io("failed".into())),
                ],
            ),
            Fake::always("gpu", Source::Gpu, 60.0),
        ]);

        assert_eq!(ladder.read().unwrap().source, Source::Cpu);
        assert!(ladder.read().is_none());
        assert!(ladder.read().is_none());
        // Third consecutive failure triggers a reprobe; CPU still fails.
        assert_eq!(ladder.read().unwrap().source, Source::Gpu);
    }

    #[test]
    fn reprobes_from_the_top_after_failures() {
        let mut ladder = Ladder::new(vec![
            Fake::new(
                "cpu",
                Source::Cpu,
                vec![
                    Ok(50.0),
                    Err(ProviderError::Io("failed".into())),
                    Err(ProviderError::Io("failed".into())),
                    Err(ProviderError::Io("failed".into())),
                    Ok(51.0),
                ],
            ),
            Fake::always("gpu", Source::Gpu, 60.0),
        ]);

        assert_eq!(ladder.read().unwrap().celsius, 50.0);
        assert!(ladder.read().is_none());
        assert!(ladder.read().is_none());
        // Re-probe from the top: CPU recovers and wins again.
        let reading = ladder.read().unwrap();
        assert_eq!(reading.source, Source::Cpu);
        assert_eq!(reading.celsius, 51.0);
    }

    #[test]
    fn returns_none_when_everything_fails() {
        let mut ladder = Ladder::new(vec![
            Fake::new(
                "cpu",
                Source::Cpu,
                vec![Err(ProviderError::Unavailable("missing".into()))],
            ),
            Fake::new(
                "gpu",
                Source::Gpu,
                vec![Err(ProviderError::Unavailable("missing".into()))],
            ),
        ]);

        assert!(ladder.read().is_none());
        assert_eq!(ladder.active_index(), None);
        assert!(ladder.last_error().is_some());
    }

    #[test]
    fn empty_ladder_is_safe() {
        let mut ladder = Ladder::<Fake>::new(Vec::new());
        assert!(ladder.is_empty());
        assert!(ladder.read().is_none());
    }

    #[test]
    fn plausibility_bounds_are_inclusive() {
        assert!(is_plausible(-40.0));
        assert!(is_plausible(150.0));
        assert!(!is_plausible(-40.01));
        assert!(!is_plausible(150.01));
        assert!(!is_plausible(f64::NAN));
    }

    #[test]
    fn fake_always_reads_the_same_value() {
        let fake = Fake::always("x", Source::Cpu, 42.0);
        assert_eq!(fake.read(), Ok(42.0));
        assert_eq!(fake.read(), Ok(42.0));
        assert_eq!(fake.reads(), 2);
    }
}

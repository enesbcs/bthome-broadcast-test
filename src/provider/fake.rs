//! Clearly marked test data used as the absolute last resort.
//!
//! The value is random in the 20.0–24.9 °C range, rounded to one decimal. It is
//! not a measurement. The source label, the `-T` name suffix, the console log
//! and the GUI all mark it as test data.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::provider::{ProviderError, TemperatureProvider};
use crate::source::Source;

/// Test-data provider.
pub struct FakeProvider {
    state: AtomicU64,
}

impl FakeProvider {
    /// Seeds the generator from the system clock.
    pub fn new() -> Self {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos() as u64)
            .unwrap_or(0x9E37_79B9_7F4A_7C15);
        Self {
            state: AtomicU64::new(seed | 1),
        }
    }

    /// Returns the next value in 20.0–24.9 °C with one decimal.
    pub fn value(&self) -> f64 {
        let random = self.next_random();
        20.0 + (random % 50) as f64 / 10.0
    }

    fn next_random(&self) -> u64 {
        let mut current = self.state.load(Ordering::Relaxed);
        loop {
            let mut next = current;
            next ^= next << 13;
            next ^= next >> 7;
            next ^= next << 17;
            if next == 0 {
                next = 0x9E37_79B9_7F4A_7C15;
            }
            match self.state.compare_exchange_weak(
                current,
                next,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return next,
                Err(actual) => current = actual,
            }
        }
    }
}

impl Default for FakeProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl TemperatureProvider for FakeProvider {
    fn source(&self) -> Source {
        Source::Fake
    }

    fn label(&self) -> &'static str {
        "TEST DATA (fake, not a measurement)"
    }

    fn read(&self) -> Result<f64, ProviderError> {
        Ok(self.value())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_are_in_range_and_one_decimal() {
        let provider = FakeProvider::new();
        for _ in 0..1_000 {
            let value = provider.read().unwrap();
            assert!((20.0..=24.9).contains(&value), "{value}");
            let tenths = value * 10.0;
            assert_eq!(tenths.round(), tenths, "{value} is not one decimal");
        }
    }

    #[test]
    fn generator_state_advances() {
        let provider = FakeProvider::new();
        let first = provider.state.load(Ordering::Relaxed);
        let _ = provider.value();
        let second = provider.state.load(Ordering::Relaxed);
        assert_ne!(first, second, "the generator must advance");
    }
}

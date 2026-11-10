//! Area weather fallback.
//!
//! This rung is used only when no real device sensor works. It fetches the
//! current air temperature for the machine's approximate location. The value is
//! a forecast for the area, not a measurement of any component inside the
//! machine, and the source label and BLE name suffix make that explicit.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::provider::{ProviderError, TemperatureProvider};
use crate::source::Source;

const USER_AGENT: &str = concat!("bthome-broadcast-test/", env!("CARGO_PKG_VERSION"));

/// Budget for one HTTP exchange.
const HTTP_TIMEOUT: Duration = Duration::from_secs(6);

/// How long a resolved location is reused.
const LOCATION_TTL: Duration = Duration::from_secs(6 * 60 * 60);

/// How long a fetched temperature is reused without a new request.
const VALUE_TTL: Duration = Duration::from_secs(15 * 60);

/// A failed refresh may still use a value younger than this.
const STALE_VALUE_MAX: Duration = Duration::from_secs(2 * 60 * 60);

/// Approximate machine location.
#[derive(Clone, Debug)]
struct Location {
    latitude: Option<f64>,
    longitude: Option<f64>,
    country: Option<String>,
    label: String,
}

/// Cached location and temperature.
#[derive(Debug, Default)]
struct Cache {
    location: Option<(Instant, Location)>,
    value: Option<(Instant, f64)>,
}

/// Whether the user allowed the forecast rung for this run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Consent {
    /// The rung may fetch weather immediately (console flag or already accepted).
    Granted,
    /// The GUI must ask before the first fetch.
    Pending,
    /// The user rejected the question; never fetch in this run.
    Denied,
}

/// Weather-backed provider.
pub struct WeatherProvider {
    agent: ureq::Agent,
    cache: Mutex<Cache>,
    consent: Mutex<Consent>,
}

impl WeatherProvider {
    /// Creates a provider with a short HTTP timeout and an empty cache.
    ///
    /// The forecast is already allowed; use [`WeatherProvider::with_prompt`]
    /// when the GUI must ask before the first fetch.
    pub fn new() -> Self {
        Self::with_consent(Consent::Granted)
    }

    /// Creates a provider that asks for consent when the rung is first reached.
    pub fn with_prompt() -> Self {
        Self::with_consent(Consent::Pending)
    }

    fn with_consent(consent: Consent) -> Self {
        Self {
            agent: build_agent(),
            cache: Mutex::new(Cache::default()),
            consent: Mutex::new(consent),
        }
    }

    /// Returns whether the forecast may be fetched, asking the GUI once if
    /// necessary. A rejected answer is cached for the rest of the run.
    fn allowed(&self) -> bool {
        let consent = self
            .consent
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match *consent {
            Consent::Granted => true,
            Consent::Denied => false,
            Consent::Pending => {
                drop(consent);
                let accepted = crate::gui::confirm_forecast();
                let mut consent = self
                    .consent
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                *consent = if accepted {
                    Consent::Granted
                } else {
                    Consent::Denied
                };
                accepted
            }
        }
    }

    fn read_cached(&self) -> Result<f64, ProviderError> {
        let mut cache = self
            .cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let now = Instant::now();

        if let Some((at, celsius)) = cache.value {
            if now.duration_since(at) < VALUE_TTL {
                return Ok(celsius);
            }
        }

        let location = match &cache.location {
            Some((at, location)) if now.duration_since(*at) < LOCATION_TTL => location.clone(),
            _ => {
                let location = detect_location(&self.agent)?;
                cache.location = Some((now, location.clone()));
                location
            }
        };

        match fetch_temperature(&self.agent, &location) {
            Ok(celsius) if celsius.is_finite() => {
                cache.value = Some((now, celsius));
                Ok(celsius)
            }
            Ok(celsius) => Err(ProviderError::Io(format!(
                "weather returned an invalid value: {celsius}"
            ))),
            Err(error) => {
                // A recent forecast is still better than dropping to test data.
                if let Some((at, celsius)) = cache.value {
                    if now.duration_since(at) < STALE_VALUE_MAX {
                        return Ok(celsius);
                    }
                }
                Err(ProviderError::Unavailable(format!(
                    "weather unavailable for {}: {error}",
                    location.label
                )))
            }
        }
    }
}

impl Default for WeatherProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl TemperatureProvider for WeatherProvider {
    fn source(&self) -> Source {
        Source::Weather
    }

    fn label(&self) -> &'static str {
        "weather (area forecast)"
    }

    fn read(&self) -> Result<f64, ProviderError> {
        if !self.allowed() {
            return Err(ProviderError::Unavailable(
                "forecast data was not allowed; using test data".to_string(),
            ));
        }
        self.read_cached()
    }
}

/// Builds the HTTP agent with the platform's TLS provider.
fn build_agent() -> ureq::Agent {
    let builder = ureq::Agent::config_builder().timeout_global(Some(HTTP_TIMEOUT));
    // ureq defaults to Rustls even when the native-tls feature is compiled in,
    // so the provider must be selected explicitly on Windows.
    #[cfg(target_os = "windows")]
    let builder = builder.tls_config(
        ureq::tls::TlsConfig::builder()
            .provider(ureq::tls::TlsProvider::NativeTls)
            .build(),
    );
    builder.build().into()
}

fn detect_location(agent: &ureq::Agent) -> Result<Location, ProviderError> {
    let mut errors = Vec::new();

    match get_json(agent, "https://ipapi.co/json/") {
        Ok(value) => {
            let latitude = value.get("latitude").and_then(Value::as_f64);
            let longitude = value.get("longitude").and_then(Value::as_f64);
            let country = value
                .get("country_code")
                .and_then(Value::as_str)
                .map(|code| code.trim().to_ascii_uppercase())
                .filter(|code| code.len() == 2 && code.chars().all(|ch| ch.is_ascii_alphabetic()));
            let city = value.get("city").and_then(Value::as_str).unwrap_or("");

            if latitude.is_some() && longitude.is_some() {
                let label = if city.is_empty() {
                    country
                        .clone()
                        .unwrap_or_else(|| "approximate location".to_string())
                } else {
                    format!("{city}, {}", country.as_deref().unwrap_or("?"))
                };
                return Ok(Location {
                    latitude,
                    longitude,
                    country,
                    label,
                });
            }
            if let Some(country) = country {
                return Ok(Location {
                    latitude: None,
                    longitude: None,
                    label: country.clone(),
                    country: Some(country),
                });
            }
            errors.push("ipapi.co returned neither coordinates nor a country".to_string());
        }
        Err(error) => errors.push(format!("ipapi.co: {error}")),
    }

    #[cfg(target_os = "windows")]
    if let Some(country) = windows_home_country() {
        return Ok(Location {
            latitude: None,
            longitude: None,
            label: country.clone(),
            country: Some(country),
        });
    }

    Err(ProviderError::Unavailable(errors.join("; ")))
}

fn fetch_temperature(agent: &ureq::Agent, location: &Location) -> Result<f64, String> {
    let mut errors = Vec::new();

    if let (Some(latitude), Some(longitude)) = (location.latitude, location.longitude) {
        let url = format!(
            "https://api.open-meteo.com/v1/forecast?latitude={latitude:.4}&longitude={longitude:.4}&current=temperature_2m"
        );
        match get_json(agent, &url).and_then(|value| {
            value
                .pointer("/current/temperature_2m")
                .and_then(Value::as_f64)
                .or_else(|| {
                    value
                        .pointer("/current_weather/temperature")
                        .and_then(Value::as_f64)
                })
                .ok_or_else(|| "no temperature in the response".to_string())
        }) {
            Ok(celsius) => return Ok(celsius),
            Err(error) => errors.push(format!("open-meteo: {error}")),
        }
    }

    if let Some(country) = &location.country {
        let url = format!("https://wttr.in/{country}?format=%t&m");
        match get_text(agent, &url).and_then(|text| parse_temperature(&text)) {
            Ok(celsius) => return Ok(celsius),
            Err(error) => errors.push(format!("wttr.in: {error}")),
        }
    }

    Err(errors.join("; "))
}

fn get_json(agent: &ureq::Agent, url: &str) -> Result<Value, String> {
    let body = get_text(agent, url)?;
    serde_json::from_str(&body).map_err(|error| format!("invalid JSON: {error}"))
}

fn get_text(agent: &ureq::Agent, url: &str) -> Result<String, String> {
    let mut response = agent
        .get(url)
        .header("User-Agent", USER_AGENT)
        .call()
        .map_err(|error| error.to_string())?;
    response
        .body_mut()
        .read_to_string()
        .map_err(|error| format!("cannot read the response: {error}"))
}

/// Parses the plain-text temperature returned by `wttr.in` (`+6°C`).
fn parse_temperature(text: &str) -> Result<f64, String> {
    let trimmed = text.trim();
    let without_unit = trimmed
        .strip_suffix("°C")
        .or_else(|| trimmed.strip_suffix("°F"))
        .unwrap_or(trimmed)
        .trim();
    let without_sign = without_unit.strip_prefix('+').unwrap_or(without_unit);

    if without_sign.is_empty() || without_sign.starts_with('<') {
        return Err(format!("unexpected response: {text:?}"));
    }
    without_sign
        .parse::<f64>()
        .map_err(|_| format!("cannot parse the temperature: {text:?}"))
}

#[cfg(target_os = "windows")]
fn windows_home_country() -> Option<String> {
    use windows::System::UserProfile::GlobalizationPreferences;
    use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};

    // The weather rung can run before the BLE backend initializes WinRT. A
    // failure here is not fatal: the home-region query below is still tried and
    // reports its own result.
    let _ = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
    let region = GlobalizationPreferences::HomeGeographicRegion().ok()?;
    let code = region.to_string().trim().to_ascii_uppercase();
    if code.len() == 2 && code.chars().all(|ch| ch.is_ascii_alphabetic()) {
        Some(code)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_wttr_temperature() {
        assert_eq!(parse_temperature("+6°C"), Ok(6.0));
        assert_eq!(parse_temperature("-3°C"), Ok(-3.0));
        assert_eq!(parse_temperature("21.5°C\n"), Ok(21.5));
        assert_eq!(parse_temperature("7"), Ok(7.0));
    }

    #[test]
    fn rejects_non_temperature_responses() {
        assert!(parse_temperature("Unknown location").is_err());
        assert!(parse_temperature("<3°C").is_err());
        assert!(parse_temperature("").is_err());
    }
}

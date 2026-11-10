//! Diagnostic output for `--diagnose`.

use crate::ble;
use crate::bthome;
use crate::cli::Options;
use crate::logging::Logger;
use crate::name::{build_name, sanitize_hostname, MAX_NAME_LEN};
use crate::provider::build_platform_ladder;
use crate::source::Source;

/// Prints the diagnostic report to stdout.
pub fn print(options: &Options, raw_hostname: &str, logger: &Logger) {
    println!(
        "BTHome Temperature Broadcaster {}",
        env!("CARGO_PKG_VERSION")
    );
    println!(
        "Platform: {} {}",
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    println!("Kernel: {}", kernel_version());
    println!("Hostname (raw): {raw_hostname}");
    println!("Hostname (sanitized): {}", sanitize_hostname(raw_hostname));
    println!(
        "Log: {}",
        logger
            .path()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "disabled (use --log or --log-file)".to_string())
    );
    println!("Interval: {} s", options.interval_secs);
    println!(
        "ACPI: {}",
        if options.no_acpi {
            "disabled"
        } else {
            "enabled (last resort)"
        }
    );
    #[cfg(target_os = "windows")]
    println!(
        "Weather fallback: {}",
        if options.forecast_enabled {
            "enabled (opt-in)"
        } else {
            "disabled (console: use --forecast-enabled; GUI asks at startup)"
        }
    );
    #[cfg(not(target_os = "windows"))]
    println!("Weather fallback: not included on Linux (test data is the last resort)");
    println!();

    println!("Provider ladder:");
    let providers = build_platform_ladder(options);
    if providers.is_empty() {
        println!("  (no providers on this platform yet)");
    }
    for (index, provider) in providers.iter().enumerate() {
        match provider.read() {
            Ok(celsius) => println!(
                "  {}. {:<36} {:<7} {:.2} °C",
                index + 1,
                provider.label(),
                provider.source().label(),
                celsius
            ),
            Err(error) => println!(
                "  {}. {:<36} {:<7} unavailable ({})",
                index + 1,
                provider.label(),
                provider.source().label(),
                error.reason()
            ),
        }
    }
    println!();

    println!("BLE advertising:");
    match ble::probe() {
        Ok(info) => {
            println!("  adapter: {} ({})", info.adapter, info.address);
            println!("  powered: {}", if info.powered { "yes" } else { "no" });
            println!(
                "  advertisement backend: {}",
                if info.advertising {
                    "available"
                } else {
                    "missing"
                }
            );
            println!("  payload: {}", ble::platform_note());
        }
        Err(error) => println!("  unavailable: {error}"),
    }

    if let Some(celsius) = options.test_temp {
        println!();
        println!("Preview at {celsius:.2} °C:");
        for source in Source::ALL {
            let name = build_name(raw_hostname, source);
            let packet = ble::payload(celsius)
                .map(|bytes| bthome::hex(&bytes))
                .unwrap_or_else(|| "cannot be encoded".to_string());
            println!(
                "  {:<7}  name: {:<18}  packet: {packet}",
                source.label(),
                name
            );
        }
    }
}

/// Toy check that the name budget documented in the README stays true.
pub const ADVERTISEMENT_BUDGET_CHECK: usize = 3 + 8 + 2 + MAX_NAME_LEN;

fn kernel_version() -> String {
    #[cfg(unix)]
    {
        if let Some(release) = unix_release() {
            return release;
        }
    }
    "unknown".to_string()
}

#[cfg(unix)]
fn unix_release() -> Option<String> {
    unsafe {
        let mut uts: libc::utsname = std::mem::zeroed();
        if libc::uname(&mut uts) != 0 {
            return None;
        }
        let bytes: Vec<u8> = uts
            .release
            .iter()
            .take_while(|byte| **byte != 0)
            .map(|byte| *byte as u8)
            .collect();
        Some(String::from_utf8_lossy(&bytes).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_matches_legacy_limit() {
        assert_eq!(ADVERTISEMENT_BUDGET_CHECK, 31);
    }

    #[test]
    fn hostname_is_reported() {
        assert!(!crate::hostname::raw_hostname().is_empty());
    }
}

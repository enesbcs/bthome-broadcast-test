//! Application loop: read the ladder, advertise, sleep, repeat.
//!
//! Real hardware rungs are never invented or estimated. The only synthetic
//! values are the explicit weather and test-data fallbacks at the end of the
//! ladder, and they are marked as such. If even those cannot produce a
//! plausible value for several consecutive intervals, advertising is suspended
//! instead of repeating a stale measurement.

use std::io::Write;
use std::thread;
use std::time::Duration;

use crate::ble;
use crate::bthome;
use crate::cli::Options;
use crate::logging::Logger;
use crate::name::build_name;
use crate::provider::{build_platform_ladder, Ladder, Reading, MAX_CONSECUTIVE_FAILURES};
use crate::shutdown;
use crate::source::Source;

/// Successful exit.
pub const EXIT_OK: i32 = 0;
/// No usable temperature source.
pub const EXIT_NO_SOURCE: i32 = 3;
/// The BLE backend could not be started or the first advertisement failed.
pub const EXIT_BLE: i32 = 5;

/// Sleep slice, short enough to react to signals promptly.
const SLICE: Duration = Duration::from_millis(100);

/// One reading delivered to a graphical front end.
#[derive(Clone, Copy, Debug)]
pub struct ReadingUpdate<'a> {
    /// Temperature in °C.
    pub celsius: f64,
    /// Ladder rung.
    pub source: Source,
    /// Provider name, e.g. `"NVIDIA NVML"`.
    pub provider: &'static str,
    /// Advertisement name derived from the hostname.
    pub name: &'a str,
    /// Encoded BTHome advertisement, or `None` when it cannot be encoded.
    pub packet: Option<&'a [u8]>,
}

/// Structured updates for a graphical front end.
///
/// The callback methods run on the worker thread and must only hand the
/// values to the UI thread; they must not block.
pub trait Reporter: Send + Sync {
    /// A new reading (live advertisement or dry-run preview).
    fn reading(&self, update: &ReadingUpdate<'_>);
    /// A high-level state change shown next to the readings.
    fn status(&self, status: &str);
}

/// Runs the application until shutdown is requested.
pub fn run(options: &Options, logger: &Logger, raw_hostname: &str) -> i32 {
    run_with(options, logger, raw_hostname, None)
}

/// Runs the application and reports structured updates to `reporter`.
pub fn run_with(
    options: &Options,
    logger: &Logger,
    raw_hostname: &str,
    reporter: Option<&dyn Reporter>,
) -> i32 {
    if options.dry_run {
        return run_dry_run(options, logger, raw_hostname, reporter);
    }
    run_live(options, logger, raw_hostname, reporter)
}

fn run_live(
    options: &Options,
    logger: &Logger,
    raw_hostname: &str,
    reporter: Option<&dyn Reporter>,
) -> i32 {
    let mut ladder = Ladder::new(build_platform_ladder(options));

    status(reporter, "Reading the temperature ladder...");
    let first = match ladder.read() {
        Some(reading) => reading,
        None => return report_no_source(logger, ladder.last_error(), reporter),
    };

    status(reporter, "Starting BLE advertising...");
    let mut broadcaster = match ble::start() {
        Ok(broadcaster) => broadcaster,
        Err(error) => {
            logger.error(&format!("BLE advertising could not be started: {error}"));
            status(reporter, "BLE advertising could not be started.");
            return EXIT_BLE;
        }
    };

    let name = build_name(raw_hostname, first.source);
    if let Err(error) = broadcaster.update(&name, first.celsius) {
        logger.error(&format!("the first advertisement failed: {error}"));
        status(reporter, "The first advertisement failed.");
        let _ = broadcaster.shutdown();
        return EXIT_BLE;
    }
    logger.info("BLE advertising initialized.");
    log_reading(logger, raw_hostname, &first, reporter);
    logger.info(&format!(
        "Advertising as {name} (source: {}, {:.2} °C).",
        first.source.label(),
        first.celsius
    ));
    if let Some(warning) = first.source.warning() {
        logger.warn(warning);
    }
    status(reporter, source_status(first.source));

    let mut advertising = true;
    let mut consecutive_failures = 0u32;

    while !wait_interval(options.interval_secs) {
        match ladder.read() {
            Some(reading) => {
                consecutive_failures = 0;
                let name = build_name(raw_hostname, reading.source);
                match broadcaster.update(&name, reading.celsius) {
                    Ok(()) => {
                        if !advertising {
                            logger.info(&format!("Advertising resumed as {name}."));
                            advertising = true;
                        }
                        log_reading(logger, raw_hostname, &reading, reporter);
                        status(reporter, source_status(reading.source));
                    }
                    Err(error) => {
                        logger.error(&format!("advertisement update failed: {error}"));
                        status(reporter, "Advertisement update failed.");
                        advertising = false;
                    }
                }
            }
            None => {
                consecutive_failures += 1;
                if let Some(reason) = ladder.last_error() {
                    logger.warn(&format!("temperature read failed: {reason}"));
                    status(
                        reporter,
                        "Temperature read failed; waiting for the next attempt.",
                    );
                }
                if advertising && consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
                    match broadcaster.suspend() {
                        Ok(()) => {
                            logger.warn("advertising suspended: no usable temperature");
                            status(reporter, "Advertising suspended: no usable temperature.");
                            advertising = false;
                        }
                        Err(error) => {
                            logger.warn(&format!("could not suspend advertising: {error}"));
                            status(reporter, "Advertising could not be suspended.");
                        }
                    }
                }
            }
        }
    }

    logger.info("Shutdown requested.");
    status(reporter, "Stopping...");
    if let Err(error) = broadcaster.shutdown() {
        logger.warn(&format!("could not unregister the advertisement: {error}"));
    }
    status(reporter, "Stopped.");
    EXIT_OK
}

fn run_dry_run(
    options: &Options,
    logger: &Logger,
    raw_hostname: &str,
    reporter: Option<&dyn Reporter>,
) -> i32 {
    if let Some(celsius) = options.test_temp {
        let source = Source::Cpu;
        if options.once {
            print_reading(logger, raw_hostname, celsius, source, reporter);
            return EXIT_OK;
        }
        loop {
            print_reading(logger, raw_hostname, celsius, source, reporter);
            if wait_interval(options.interval_secs) {
                logger.info("Shutdown requested.");
                return EXIT_OK;
            }
        }
    }

    let mut ladder = Ladder::new(build_platform_ladder(options));
    let Some(first) = ladder.read() else {
        return report_no_source(logger, ladder.last_error(), reporter);
    };
    print_reading(logger, raw_hostname, first.celsius, first.source, reporter);
    if options.once {
        return EXIT_OK;
    }

    loop {
        if wait_interval(options.interval_secs) {
            logger.info("Shutdown requested.");
            return EXIT_OK;
        }
        match ladder.read() {
            Some(reading) => print_reading(
                logger,
                raw_hostname,
                reading.celsius,
                reading.source,
                reporter,
            ),
            None => {
                if let Some(reason) = ladder.last_error() {
                    logger.warn(&format!("temperature read failed: {reason}"));
                }
            }
        }
    }
}

fn report_no_source(logger: &Logger, reason: Option<&str>, reporter: Option<&dyn Reporter>) -> i32 {
    let detail = reason.unwrap_or("no provider returned a plausible value");
    logger.error(&format!("No temperature source found: {detail}"));
    status(reporter, "No temperature source found.");
    EXIT_NO_SOURCE
}

fn status(reporter: Option<&dyn Reporter>, message: &str) {
    if let Some(reporter) = reporter {
        reporter.status(message);
    }
}

/// Logs one reading. Dry-run output is printed by [`print_reading`].
fn log_reading(
    logger: &Logger,
    raw_hostname: &str,
    reading: &Reading,
    reporter: Option<&dyn Reporter>,
) {
    let name = build_name(raw_hostname, reading.source);
    let note = source_note(reading.source);
    match ble::payload(reading.celsius) {
        Some(packet) => {
            logger.info(&format!(
                "{:>7.2} °C  source: {:<7}  name: {:<18}  packet: {}{note}",
                reading.celsius,
                reading.source.label(),
                name,
                bthome::hex(&packet)
            ));
            report_reading(reporter, reading, &name, Some(&packet));
        }
        None => {
            logger.warn(&format!(
                "{} °C cannot be encoded as a BTHome temperature",
                reading.celsius
            ));
            report_reading(reporter, reading, &name, None);
        }
    }
}

fn report_reading(
    reporter: Option<&dyn Reporter>,
    reading: &Reading,
    name: &str,
    packet: Option<&[u8]>,
) {
    if let Some(reporter) = reporter {
        reporter.reading(&ReadingUpdate {
            celsius: reading.celsius,
            source: reading.source,
            provider: reading.label,
            name,
            packet,
        });
    }
}

fn print_reading(
    logger: &Logger,
    raw_hostname: &str,
    celsius: f64,
    source: Source,
    reporter: Option<&dyn Reporter>,
) {
    let name = build_name(raw_hostname, source);
    let Some(packet) = ble::payload(celsius) else {
        logger.error(&format!(
            "{celsius} °C cannot be encoded as a BTHome temperature"
        ));
        report_preview(reporter, celsius, source, &name, None);
        return;
    };
    let message = format!(
        "{:>7.2} °C  name: {:<18}  packet: {}{}",
        celsius,
        name,
        bthome::hex(&packet),
        source_note(source)
    );
    logger.info(&message);
    println!("{message}");
    let _ = std::io::stdout().flush();
    report_preview(reporter, celsius, source, &name, Some(&packet));
}

fn report_preview(
    reporter: Option<&dyn Reporter>,
    celsius: f64,
    source: Source,
    name: &str,
    packet: Option<&[u8]>,
) {
    if let Some(reporter) = reporter {
        reporter.reading(&ReadingUpdate {
            celsius,
            source,
            provider: source.label(),
            name,
            packet,
        });
    }
}

/// Short marker appended to a reading line.
fn source_note(source: Source) -> &'static str {
    match source {
        Source::Weather => "  [weather forecast, not a measurement]",
        Source::Fake => "  [TEST DATA, not a measurement]",
        _ => "",
    }
}

/// Status line shown next to the readings while a synthetic source is active.
fn source_status(source: Source) -> &'static str {
    match source {
        Source::Weather => "Advertising an area weather forecast (no device sensor available).",
        Source::Fake => {
            "TEST DATA: advertising generated values; no temperature source is available."
        }
        _ => "Advertising.",
    }
}

/// Waits one interval in slices. Returns `true` if shutdown was requested.
fn wait_interval(interval_secs: u64) -> bool {
    let mut remaining = Duration::from_secs(interval_secs);
    while !remaining.is_zero() {
        if shutdown::requested() {
            return true;
        }
        let slice = remaining.min(SLICE);
        thread::sleep(slice);
        remaining = remaining.saturating_sub(slice);
    }
    shutdown::requested()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_packet_matches_encoder() {
        let packet = ble::payload(48.5).unwrap();
        #[cfg(target_os = "windows")]
        assert_eq!(bthome::hex(&packet), "07 16 D2 FC 40 02 F2 12");
        #[cfg(not(target_os = "windows"))]
        assert_eq!(bthome::hex(&packet), "02 01 06 07 16 D2 FC 40 02 F2 12");
    }

    #[test]
    fn wait_interval_returns_immediately_when_requested() {
        shutdown::reset();
        shutdown::request();
        assert!(wait_interval(10));
        shutdown::reset();
    }
}

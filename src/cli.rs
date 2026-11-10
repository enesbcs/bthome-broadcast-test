//! Command line parsing.
//!
//! Parsing is manual to keep the binary small: no `clap`, no `structopt`.

use std::fmt;
use std::path::PathBuf;

/// What the process should do after parsing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Action {
    #[default]
    Run,
    Help,
    Version,
}

/// Parsed command line options.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Options {
    pub action: Action,
    pub dry_run: bool,
    pub diagnose: bool,
    pub no_acpi: bool,
    /// Windows: allow the area weather forecast.
    ///
    /// Console mode uses test data unless this flag is set. GUI mode asks the
    /// user lazily, right before the weather rung would be used, unless this
    /// flag is set. The Linux build has no weather fallback at all, so the flag
    /// does not exist there.
    #[cfg(target_os = "windows")]
    pub forecast_enabled: bool,
    /// Windows GUI: ask before the weather rung is used.
    ///
    /// Set internally by the GUI when `--forecast-enabled` was not passed. It
    /// is never parsed from the command line.
    #[cfg(target_os = "windows")]
    pub forecast_prompt: bool,
    /// Windows: run in console mode instead of opening the GUI window.
    ///
    /// Accepted on Linux as a no-op because Linux always runs in console mode.
    pub console: bool,
    pub once: bool,
    pub interval_secs: u64,
    pub test_temp: Option<f64>,
    /// Write the rotating log file at the platform default location.
    pub log_to_file: bool,
    /// Write the log to this path; implies [`Options::log_to_file`].
    pub log_file: Option<PathBuf>,
}

impl Options {
    /// Default update interval: 10 seconds.
    pub const DEFAULT_INTERVAL_SECS: u64 = 10;
    /// Minimum accepted interval.
    pub const MIN_INTERVAL_SECS: u64 = 1;
    /// Maximum accepted interval.
    pub const MAX_INTERVAL_SECS: u64 = 3600;
    /// Plausible temperature range in °C.
    pub const MIN_TEMP: f64 = -40.0;
    /// Plausible temperature range in °C.
    pub const MAX_TEMP: f64 = 150.0;

    /// Whether the log should also go to stderr.
    ///
    /// Every console run reports the BLE initialization, each reading and each
    /// error on stderr without any extra switch. The Windows GUI deliberately
    /// mirrors the same lines into its own log pane instead, so it creates the
    /// logger with `console = false`.
    pub fn log_to_stderr(&self) -> bool {
        true
    }
}

/// Command line errors.
#[derive(Clone, Debug, PartialEq)]
pub enum CliError {
    UnknownFlag(String),
    MissingValue(String),
    InvalidNumber { flag: String, value: String },
    IntervalOutOfRange(u64),
    TempOutOfRange(f64),
    TempWithoutDryRun,
    OnceWithoutDryRun,
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CliError::UnknownFlag(flag) => write!(f, "Unknown option: {flag}"),
            CliError::MissingValue(flag) => write!(f, "Missing value for {flag}."),
            CliError::InvalidNumber { flag, value } => {
                write!(f, "The value of {flag} is not a number: {value}")
            }
            CliError::IntervalOutOfRange(value) => write!(
                f,
                "The --interval value {value} is outside {}..={} seconds.",
                Options::MIN_INTERVAL_SECS,
                Options::MAX_INTERVAL_SECS
            ),
            CliError::TempOutOfRange(value) => write!(
                f,
                "The --temp value {value} °C is outside {}..={} °C.",
                Options::MIN_TEMP,
                Options::MAX_TEMP
            ),
            CliError::TempWithoutDryRun => {
                write!(f, "--temp can only be used with --dry-run or --diagnose.")
            }
            CliError::OnceWithoutDryRun => {
                write!(f, "--once can only be used together with --dry-run.")
            }
        }
    }
}

/// Parses arguments without the program name.
pub fn parse(args: &[String]) -> Result<Options, CliError> {
    let mut opts = Options {
        interval_secs: Options::DEFAULT_INTERVAL_SECS,
        ..Options::default()
    };

    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        let (flag, inline) = match arg.split_once('=') {
            Some((flag, value)) => (flag, Some(value.to_string())),
            None => (arg.as_str(), None),
        };

        match flag {
            "-h" | "--help" => opts.action = Action::Help,
            "-V" | "--version" => opts.action = Action::Version,
            "--dry-run" => opts.dry_run = true,
            "--diagnose" => opts.diagnose = true,
            "--no-acpi" => opts.no_acpi = true,
            #[cfg(target_os = "windows")]
            "--forecast-enabled" => opts.forecast_enabled = true,
            "--console" => opts.console = true,
            "--log" => opts.log_to_file = true,
            "--log-file" => {
                let value = take_value(args, &mut index, inline, "--log-file")?;
                opts.log_file = Some(PathBuf::from(value));
                opts.log_to_file = true;
            }
            "--once" => opts.once = true,
            "--interval" => {
                let value = take_value(args, &mut index, inline, "--interval")?;
                opts.interval_secs = value.parse::<u64>().map_err(|_| CliError::InvalidNumber {
                    flag: "--interval".to_string(),
                    value,
                })?;
            }
            "--temp" => {
                let value = take_value(args, &mut index, inline, "--temp")?;
                opts.test_temp =
                    Some(value.parse::<f64>().map_err(|_| CliError::InvalidNumber {
                        flag: "--temp".to_string(),
                        value,
                    })?);
            }
            other => return Err(CliError::UnknownFlag(other.to_string())),
        }

        index += 1;
    }

    if opts.action != Action::Run {
        return Ok(opts);
    }

    if !(Options::MIN_INTERVAL_SECS..=Options::MAX_INTERVAL_SECS).contains(&opts.interval_secs) {
        return Err(CliError::IntervalOutOfRange(opts.interval_secs));
    }

    if let Some(temp) = opts.test_temp {
        if !temp.is_finite() || !(Options::MIN_TEMP..=Options::MAX_TEMP).contains(&temp) {
            return Err(CliError::TempOutOfRange(temp));
        }
        if !opts.dry_run && !opts.diagnose {
            return Err(CliError::TempWithoutDryRun);
        }
    }

    if opts.once && !opts.dry_run {
        return Err(CliError::OnceWithoutDryRun);
    }

    Ok(opts)
}

fn take_value(
    args: &[String],
    index: &mut usize,
    inline: Option<String>,
    flag: &str,
) -> Result<String, CliError> {
    if let Some(value) = inline {
        return Ok(value);
    }
    *index += 1;
    args.get(*index)
        .cloned()
        .ok_or_else(|| CliError::MissingValue(flag.to_string()))
}

/// Help text.
pub fn help_text() -> String {
    #[cfg(target_os = "windows")]
    let ladder = "(CPU -> GPU -> disk -> ACPI -> optional weather forecast -> test data)";
    #[cfg(not(target_os = "windows"))]
    let ladder = "(CPU -> GPU -> disk -> ACPI -> test data)";
    #[cfg(target_os = "windows")]
    let forecast_option = "\
    --forecast-enabled   Allow the area weather forecast (console mode)\n\
                         The GUI asks right before the weather fallback.\n";
    #[cfg(not(target_os = "windows"))]
    let forecast_option = "";

    format!(
        "\
BTHome Temperature Broadcaster {version}
{ladder}

USAGE:
    bthome-broadcast-test [OPTIONS]

OPTIONS:
    --dry-run            Do not advertise; read the source and print the packet
    --temp <°C>          Preview temperature (with --dry-run or --diagnose), {min_temp}..={max_temp}
    --once               With --dry-run: print once and exit
    --diagnose           System and source diagnostics, then exit
    --no-acpi            Disable the ACPI source
{forecast_option}    --interval <sec>     Update interval in seconds ({min_int}..={max_int}, default: {def_int})
    --console            Windows: run in console mode instead of the GUI window
                         (Linux always runs in console mode)
    --log                Write the rotating log file at the default location
    --log-file <path>    Write the log to <path> (implies --log)
    -h, --help           Show this help
    -V, --version        Show the version

Windows note: the WinRT advertiser sends BTHome service data only; the Flags
(0x01) and Local Name (0x08/0x09) AD structures are system-reserved.
",
        version = env!("CARGO_PKG_VERSION"),
        min_temp = Options::MIN_TEMP,
        max_temp = Options::MAX_TEMP,
        min_int = Options::MIN_INTERVAL_SECS,
        max_int = Options::MAX_INTERVAL_SECS,
        def_int = Options::DEFAULT_INTERVAL_SECS,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn defaults() {
        let opts = parse(&[]).unwrap();
        assert_eq!(opts.action, Action::Run);
        assert_eq!(opts.interval_secs, 10);
        assert_eq!(opts.test_temp, None);
        assert!(!opts.dry_run);
        assert!(!opts.diagnose);
        assert!(!opts.no_acpi);
        #[cfg(target_os = "windows")]
        assert!(!opts.forecast_enabled);
        #[cfg(target_os = "windows")]
        assert!(!opts.forecast_prompt);
    }

    #[test]
    fn parses_flags() {
        let opts = parse(&args(&[
            "--dry-run",
            "--no-acpi",
            "--console",
            "--once",
            "--log",
        ]))
        .unwrap();
        assert!(opts.dry_run && opts.no_acpi && opts.console && opts.once);
        assert!(opts.log_to_file);
        assert_eq!(opts.log_file, None);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn parses_forecast_flag() {
        let opts = parse(&args(&["--forecast-enabled"])).unwrap();
        assert!(opts.forecast_enabled);
        assert!(!parse(&args(&[])).unwrap().forecast_enabled);
    }

    #[test]
    fn log_file_implies_file_logging() {
        let opts = parse(&args(&["--log-file", "/tmp/custom.log"])).unwrap();
        assert!(opts.log_to_file);
        assert_eq!(opts.log_file, Some(PathBuf::from("/tmp/custom.log")));
        assert_eq!(
            parse(&args(&["--log-file"])).unwrap_err(),
            CliError::MissingValue("--log-file".to_string())
        );
    }

    #[test]
    fn file_logging_is_off_by_default() {
        let opts = parse(&[]).unwrap();
        assert!(!opts.log_to_file);
        assert_eq!(opts.log_file, None);
    }

    #[test]
    fn parses_values_in_both_forms() {
        let opts = parse(&args(&["--dry-run", "--interval", "30", "--temp=48.5"])).unwrap();
        assert_eq!(opts.interval_secs, 30);
        assert_eq!(opts.test_temp, Some(48.5));
    }

    #[test]
    fn help_and_version_short_circuit_validation() {
        assert_eq!(parse(&args(&["--help"])).unwrap().action, Action::Help);
        assert_eq!(parse(&args(&["-V"])).unwrap().action, Action::Version);
        assert_eq!(
            parse(&args(&["--version"])).unwrap().action,
            Action::Version
        );
    }

    #[test]
    fn rejects_unknown_flag() {
        assert_eq!(
            parse(&args(&["--nope"])).unwrap_err(),
            CliError::UnknownFlag("--nope".to_string())
        );
        #[cfg(not(target_os = "windows"))]
        assert_eq!(
            parse(&args(&["--forecast-enabled"])).unwrap_err(),
            CliError::UnknownFlag("--forecast-enabled".to_string())
        );
    }

    #[test]
    fn rejects_missing_value() {
        assert_eq!(
            parse(&args(&["--interval"])).unwrap_err(),
            CliError::MissingValue("--interval".to_string())
        );
    }

    #[test]
    fn rejects_invalid_number() {
        assert!(matches!(
            parse(&args(&["--temp", "warm"])).unwrap_err(),
            CliError::InvalidNumber { .. }
        ));
    }

    #[test]
    fn rejects_interval_out_of_range() {
        assert_eq!(
            parse(&args(&["--interval", "0"])).unwrap_err(),
            CliError::IntervalOutOfRange(0)
        );
        assert_eq!(
            parse(&args(&["--interval", "3601"])).unwrap_err(),
            CliError::IntervalOutOfRange(3601)
        );
    }

    #[test]
    fn rejects_temp_out_of_range_and_non_finite() {
        assert_eq!(
            parse(&args(&["--dry-run", "--temp", "200"])).unwrap_err(),
            CliError::TempOutOfRange(200.0)
        );
        assert!(matches!(
            parse(&args(&["--dry-run", "--temp", "NaN"])).unwrap_err(),
            CliError::TempOutOfRange(_)
        ));
    }

    #[test]
    fn temp_and_once_require_a_preview_mode() {
        assert_eq!(
            parse(&args(&["--temp", "48.5"])).unwrap_err(),
            CliError::TempWithoutDryRun
        );
        assert_eq!(
            parse(&args(&["--once"])).unwrap_err(),
            CliError::OnceWithoutDryRun
        );
        assert!(parse(&args(&["--diagnose", "--temp", "48.5"])).is_ok());
    }

    #[test]
    fn console_logging_is_always_on() {
        let opts = Options::default();
        assert!(opts.log_to_stderr());
        assert!(Options {
            console: true,
            ..Options::default()
        }
        .log_to_stderr());
    }

    #[test]
    fn help_text_mentions_every_flag() {
        let help = help_text();
        #[cfg(target_os = "windows")]
        let flags = vec![
            "--dry-run",
            "--temp",
            "--once",
            "--diagnose",
            "--no-acpi",
            "--forecast-enabled",
            "--interval",
            "--console",
            "--log",
            "--log-file",
        ];
        #[cfg(not(target_os = "windows"))]
        let flags = vec![
            "--dry-run",
            "--temp",
            "--once",
            "--diagnose",
            "--no-acpi",
            "--interval",
            "--console",
            "--log",
            "--log-file",
        ];
        for flag in flags {
            assert!(help.contains(flag), "missing {flag} in help");
        }
        #[cfg(not(target_os = "windows"))]
        assert!(!help.contains("--forecast-enabled"));
    }
}

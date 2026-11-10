//! Binary entry point.
//!
//! Windows builds are GUI-subsystem binaries: a double click opens the native
//! window. Text modes (`--console`, `--help`, `--version`, `--dry-run`,
//! `--diagnose`) attach to the calling terminal and behave like a console
//! program. Linux always runs in console mode.

#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use std::process::ExitCode;

use bthome_broadcast_test::cli::{self, Action, Options};
use bthome_broadcast_test::{app, diagnostics, hostname, instance, logging, shutdown};

#[cfg(target_os = "windows")]
mod console;

const EXIT_USAGE: u8 = 2;
const EXIT_RUNTIME: u8 = 1;
const EXIT_ALREADY_RUNNING: u8 = 4;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    #[cfg(target_os = "windows")]
    if console::requested(&args) {
        console::attach();
    }

    match cli::parse(&args) {
        Ok(options) => dispatch(options),
        Err(error) => {
            #[cfg(target_os = "windows")]
            console::attach();
            eprintln!("{error}");
            eprintln!("Help: --help");
            ExitCode::from(EXIT_USAGE)
        }
    }
}

fn dispatch(options: Options) -> ExitCode {
    match options.action {
        Action::Help => {
            print!("{}", cli::help_text());
            return ExitCode::SUCCESS;
        }
        Action::Version => {
            println!(
                "BTHome Temperature Broadcaster {}",
                env!("CARGO_PKG_VERSION")
            );
            return ExitCode::SUCCESS;
        }
        Action::Run => {}
    }

    let raw_hostname = hostname::raw_hostname();

    #[cfg(target_os = "windows")]
    if !options.console && !options.dry_run && !options.diagnose {
        return match bthome_broadcast_test::gui::run(&options, &raw_hostname) {
            Ok(code) => ExitCode::from(code as u8),
            Err(error) => {
                console::attach();
                eprintln!("{error}");
                ExitCode::from(EXIT_RUNTIME)
            }
        };
    }

    run_console(options, &raw_hostname)
}

fn run_console(options: Options, raw_hostname: &str) -> ExitCode {
    let log_path = if let Some(path) = &options.log_file {
        Some(path.clone())
    } else if options.log_to_file {
        Some(logging::Logger::default_path())
    } else {
        None
    };
    let logger = match logging::Logger::new(log_path.as_deref(), options.log_to_stderr()) {
        Ok(logger) => logger,
        Err(error) => {
            eprintln!("Cannot open the log file: {error}");
            return ExitCode::from(EXIT_RUNTIME);
        }
    };

    logger.info(&format!(
        "Starting BTHome Temperature Broadcaster {}, platform {} {}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    ));
    #[cfg(target_os = "windows")]
    if options.forecast_enabled {
        logger.info("Forecast data: enabled (weather is used only when no device sensor works).");
    } else {
        logger.info("Forecast data: disabled (test data is the last resort).");
    }

    let _lock = match instance::InstanceLock::acquire(instance::INSTANCE_NAME) {
        Ok(lock) => lock,
        Err(instance::LockError::AlreadyRunning) => {
            logger.warn("The program is already running (single-instance lock).");
            return ExitCode::from(EXIT_ALREADY_RUNNING);
        }
        Err(error) => {
            logger.error(&error.to_string());
            return ExitCode::from(EXIT_RUNTIME);
        }
    };

    if let Err(error) = shutdown::install() {
        logger.warn(&format!("Cannot install signal handlers: {error}"));
    }

    if options.diagnose {
        diagnostics::print(&options, raw_hostname, &logger);
        return ExitCode::SUCCESS;
    }

    let code = app::run(&options, &logger, raw_hostname);
    if code == app::EXIT_OK {
        logger.info("Clean shutdown.");
        ExitCode::SUCCESS
    } else {
        logger.warn(&format!("Exiting with code {code}"));
        ExitCode::from(code as u8)
    }
}

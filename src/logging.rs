//! Minimal rotating file logger.
//!
//! One log file, at most three rotated backups, 1 MiB each. No external
//! crates and no background thread: the file is written synchronously.
//!
//! The file is optional. Without `--log` or `--log-file` the logger only
//! writes to the console (stderr), so starting the program does not create a
//! log file as a side effect.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const APP_DIR: &str = "bthome-broadcast-test";
const LOG_FILE: &str = "app.log";
const MAX_BYTES: u64 = 1024 * 1024;
const MAX_BACKUPS: usize = 3;

/// Callback receiving every formatted log line, without the trailing newline.
///
/// The Windows GUI uses it to mirror the log into its own window; the callback
/// must not block for long because it runs on the logging thread.
pub type LogSink = Box<dyn Fn(&str) + Send + Sync + 'static>;

struct Inner {
    path: Option<PathBuf>,
    file: Option<File>,
    size: u64,
    max_bytes: u64,
    max_backups: usize,
    console: bool,
    sink: Option<LogSink>,
}

/// Thread-safe logger writing to stderr and, when enabled, to a log file.
pub struct Logger {
    inner: Mutex<Inner>,
}

impl Logger {
    /// Creates the logger.
    ///
    /// `path` selects the log file; `None` keeps the logger console-only and
    /// does not touch the filesystem.
    pub fn new(path: Option<&Path>, console: bool) -> io::Result<Self> {
        Self::with_sink(path, console, None)
    }

    /// Creates the logger with an additional in-memory sink.
    ///
    /// The sink is used by the Windows GUI so the window shows the same lines
    /// that would otherwise go to stderr. File logging remains independent of
    /// the sink and stays opt-in.
    pub fn with_sink(
        path: Option<&Path>,
        console: bool,
        sink: Option<LogSink>,
    ) -> io::Result<Self> {
        let (path, file, size) = match path {
            Some(path) => {
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent)?;
                }
                let size = fs::metadata(path).map(|meta| meta.len()).unwrap_or(0);
                let file = OpenOptions::new().create(true).append(true).open(path)?;
                (Some(path.to_path_buf()), Some(file), size)
            }
            None => (None, None, 0),
        };

        Ok(Self {
            inner: Mutex::new(Inner {
                path,
                file,
                size,
                max_bytes: MAX_BYTES,
                max_backups: MAX_BACKUPS,
                console,
                sink,
            }),
        })
    }

    /// Platform default log directory.
    pub fn default_dir() -> PathBuf {
        #[cfg(windows)]
        {
            if let Some(local) = std::env::var_os("LOCALAPPDATA") {
                return PathBuf::from(local).join(APP_DIR);
            }
        }

        #[cfg(unix)]
        {
            if let Some(state) = std::env::var_os("XDG_STATE_HOME") {
                return PathBuf::from(state).join(APP_DIR);
            }
            if let Some(home) = std::env::var_os("HOME") {
                return PathBuf::from(home).join(".local/state").join(APP_DIR);
            }
        }

        std::env::temp_dir().join(APP_DIR)
    }

    /// Default log file path used by `--log`.
    pub fn default_path() -> PathBuf {
        Self::default_dir().join(LOG_FILE)
    }

    /// Path of the active log file, if file logging is enabled.
    pub fn path(&self) -> Option<PathBuf> {
        self.lock().path.clone()
    }

    /// Logs an informational message.
    pub fn info(&self, message: &str) {
        self.write("INFO", message);
    }

    /// Logs a warning.
    pub fn warn(&self, message: &str) {
        self.write("WARN", message);
    }

    /// Logs an error.
    pub fn error(&self, message: &str) {
        self.write("ERROR", message);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn write(&self, level: &str, message: &str) {
        let line = format!("{} [{}] {}\n", timestamp_now(), level, message);
        let mut inner = self.lock();

        if inner.file.is_some() && inner.size + line.len() as u64 > inner.max_bytes {
            if let Err(error) = inner.rotate() {
                // Keep logging into the current file if rotation fails.
                let _ = writeln!(io::stderr(), "Log rotation failed: {error}");
            }
        }

        if let Some(file) = inner.file.as_mut() {
            if file.write_all(line.as_bytes()).is_ok() && file.flush().is_ok() {
                inner.size += line.len() as u64;
            }
        }

        if inner.console {
            let _ = io::stderr().write_all(line.as_bytes());
        }

        if let Some(sink) = &inner.sink {
            sink(line.trim_end_matches(['\r', '\n']));
        }
    }
}

impl Inner {
    fn reopen(&mut self) -> io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        self.size = file.metadata().map(|meta| meta.len()).unwrap_or(0);
        self.file = Some(file);
        Ok(())
    }

    fn rotate(&mut self) -> io::Result<()> {
        // Close the current file first so it can be renamed on Windows too.
        self.file = None;

        let Some(path) = self.path.clone() else {
            return Ok(());
        };

        for index in (1..self.max_backups).rev() {
            let from = backup_path(&path, index);
            let to = backup_path(&path, index + 1);
            if from.exists() {
                let _ = fs::rename(&from, &to);
            }
        }

        let first = backup_path(&path, 1);
        if path.exists() {
            let _ = fs::rename(&path, &first);
        }

        self.reopen()
    }
}

fn backup_path(path: &Path, index: usize) -> PathBuf {
    path.with_extension(format!("log.{index}"))
}

/// UTC timestamp in `YYYY-MM-DDTHH:MM:SSZ` form.
pub fn timestamp_now() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    format_timestamp(seconds)
}

/// Formats Unix seconds as a UTC timestamp.
pub fn format_timestamp(unix_seconds: u64) -> String {
    let days = (unix_seconds / 86_400) as i64;
    let seconds_of_day = unix_seconds % 86_400;
    let (year, month, day) = civil_from_days(days);

    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        seconds_of_day / 3600,
        (seconds_of_day % 3600) / 60,
        seconds_of_day % 60
    )
}

/// Howard Hinnant's `civil_from_days` algorithm.
fn civil_from_days(days_since_epoch: i64) -> (i64, u32, u32) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = (z - era * 146_097) as u64;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era as i64 + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_position = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_position + 2) / 5 + 1) as u32;
    let month = if month_position < 10 {
        month_position + 3
    } else {
        month_position - 9
    } as u32;
    if month <= 2 {
        year += 1;
    }
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_known_timestamps() {
        assert_eq!(format_timestamp(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_timestamp(1_700_000_000), "2023-11-14T22:13:20Z");
        assert_eq!(format_timestamp(951_782_400), "2000-02-29T00:00:00Z");
    }

    #[test]
    fn writes_and_rotates() {
        let dir = std::env::temp_dir().join(format!(
            "ble-log-test-{}-{}",
            std::process::id(),
            timestamp_now().replace([':', '-'], "")
        ));
        let path = dir.join(LOG_FILE);
        let logger = Logger::new(Some(&path), false).unwrap();
        assert_eq!(logger.path(), Some(path.clone()));
        logger.info("first line");
        logger.warn("second line");

        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("first line"));
        assert!(content.contains("[WARN] second line"));

        // Force rotation by shrinking the limit.
        {
            let mut inner = logger.lock();
            inner.max_bytes = 1;
        }
        logger.error("after rotation");

        assert!(dir.join("app.log.1").exists());
        let fresh = fs::read_to_string(&path).unwrap();
        assert!(fresh.contains("after rotation"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn console_only_logger_does_not_create_a_file() {
        let logger = Logger::new(None, false).unwrap();
        assert_eq!(logger.path(), None);
        logger.info("only on the console");
    }

    #[test]
    fn sink_receives_every_line_without_the_newline() {
        let lines = std::sync::Arc::new(Mutex::new(Vec::<String>::new()));
        let target = lines.clone();
        let sink: LogSink = Box::new(move |line| {
            target.lock().unwrap().push(line.to_string());
        });
        let logger = Logger::with_sink(None, false, Some(sink)).unwrap();
        logger.info("hello");
        logger.warn("careful");

        let lines = lines.lock().unwrap();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("[INFO] hello"));
        assert!(lines[1].contains("[WARN] careful"));
        assert!(!lines[0].ends_with('\n'));
    }
}

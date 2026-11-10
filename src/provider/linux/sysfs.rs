//! Small helpers for the Linux sysfs sensor interfaces.
//!
//! Every helper takes the sysfs root as a parameter. Production uses `/`, while
//! the unit tests build a temporary tree that mimics the kernel layout.

use std::fs;
use std::path::{Path, PathBuf};

use crate::provider::ProviderError;

/// One `tempN_input` channel of a hwmon device.
#[derive(Clone, Debug)]
pub struct HwmonChannel {
    /// `name` file of the hwmon device, e.g. `coretemp` or `amdgpu`.
    pub device: String,
    /// Channel label, when the driver provides one.
    pub label: String,
    /// The `tempN_input` file.
    pub input: PathBuf,
    /// The hwmon device directory.
    pub dir: PathBuf,
}

/// Reads a sysfs text file and trims surrounding whitespace.
pub fn read_trimmed(path: &Path) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

/// Reads a millidegree Celsius file and converts it to degrees Celsius.
pub fn read_millidegrees(path: &Path) -> Result<f64, ProviderError> {
    let text = read_trimmed(path)
        .ok_or_else(|| ProviderError::Io(format!("cannot read {}", path.display())))?;
    let raw: i64 = text
        .parse()
        .map_err(|_| ProviderError::Io(format!("invalid value in {}: {text}", path.display())))?;
    Ok(raw as f64 / 1000.0)
}

/// Enumerates all readable hwmon temperature channels below
/// `<root>/sys/class/hwmon`.
pub fn hwmon_channels(root: &Path) -> Vec<HwmonChannel> {
    let base = root.join("sys/class/hwmon");
    let Ok(entries) = fs::read_dir(&base) else {
        return Vec::new();
    };

    let mut channels = Vec::new();
    for entry in entries.flatten() {
        let dir = entry.path();
        let Some(device) = read_trimmed(&dir.join("name")) else {
            continue;
        };
        let Ok(files) = fs::read_dir(&dir) else {
            continue;
        };
        let mut inputs: Vec<PathBuf> = files
            .flatten()
            .map(|file| file.path())
            .filter(|path| is_temperature_input(path))
            .collect();
        inputs.sort();

        for input in inputs {
            let label = input
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| name.strip_suffix("_input"))
                .map(|base| dir.join(format!("{base}_label")))
                .and_then(|path| read_trimmed(&path))
                .unwrap_or_default();
            channels.push(HwmonChannel {
                device: device.clone(),
                label,
                input,
                dir: dir.clone(),
            });
        }
    }
    channels
}

/// Picks the most useful channel of a hwmon device: the package sensor when a
/// driver labels one, otherwise `temp1_input`, otherwise the first channel.
pub fn preferred_channel(channels: &[HwmonChannel]) -> Option<&HwmonChannel> {
    channels
        .iter()
        .find(|channel| channel.label == "Package id 0")
        .or_else(|| {
            channels.iter().find(|channel| {
                channel.input.file_name().and_then(|name| name.to_str()) == Some("temp1_input")
            })
        })
        .or_else(|| channels.first())
}

/// Whether the file name matches `tempN_input`.
fn is_temperature_input(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let Some(rest) = name.strip_prefix("temp") else {
        return false;
    };
    let Some(number) = rest.strip_suffix("_input") else {
        return false;
    };
    !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
}

/// A thermal zone below `<root>/sys/class/thermal`.
#[derive(Clone, Debug)]
pub struct ThermalZone {
    pub zone_type: String,
    pub temp: PathBuf,
}

/// Enumerates all thermal zones that have a `type` and a `temp` file.
pub fn thermal_zones(root: &Path) -> Vec<ThermalZone> {
    let base = root.join("sys/class/thermal");
    let Ok(entries) = fs::read_dir(&base) else {
        return Vec::new();
    };

    let mut zones = Vec::new();
    for entry in entries.flatten() {
        let dir = entry.path();
        let name = dir.file_name().and_then(|name| name.to_str()).unwrap_or("");
        if !name.starts_with("thermal_zone") {
            continue;
        }
        let Some(zone_type) = read_trimmed(&dir.join("type")) else {
            continue;
        };
        let temp = dir.join("temp");
        if !temp.exists() {
            continue;
        }
        zones.push(ThermalZone { zone_type, temp });
    }
    zones
}

/// Canonical path of a symlink or directory, if it exists.
pub fn canonical(path: &Path) -> Option<PathBuf> {
    fs::canonicalize(path).ok()
}

/// Parses the device number of the filesystem mounted at `mount_point` from
/// `/proc/self/mountinfo`.
pub fn root_device_number(root: &Path, mount_point: &str) -> Option<(u32, u32)> {
    let text = fs::read_to_string(root.join("proc/self/mountinfo")).ok()?;
    for line in text.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 5 || fields[4] != mount_point {
            continue;
        }
        let (major, minor) = fields[2].split_once(':')?;
        return Some((major.parse().ok()?, minor.parse().ok()?));
    }
    None
}

/// Reads the `slaves` children of a block device (device-mapper, MD, ...).
pub fn block_slaves(block_dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(block_dir.join("slaves")) else {
        return out;
    };
    for entry in entries.flatten() {
        if let Some(path) = canonical(&entry.path()) {
            out.push(path);
        }
    }
    out
}

#[cfg(test)]
pub mod fixture {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    /// Temporary fake sysfs tree, removed on drop.
    pub struct Fixture {
        pub root: PathBuf,
    }

    impl Fixture {
        pub fn new(tag: &str) -> Self {
            let number = COUNTER.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir()
                .join(format!("ble-sysfs-{tag}-{}-{number}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).expect("create fixture root");
            Self { root }
        }

        /// Creates `<root>/<relative>` with the given contents.
        pub fn file(&self, relative: &str, contents: &str) -> PathBuf {
            let path = self.root.join(relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("create fixture directory");
            }
            fs::write(&path, contents).expect("write fixture file");
            path
        }

        /// Creates a directory below the fixture root.
        pub fn dir(&self, relative: &str) -> PathBuf {
            let path = self.root.join(relative);
            fs::create_dir_all(&path).expect("create fixture directory");
            path
        }

        /// Creates a symbolic link below the fixture root.
        pub fn symlink(&self, relative: &str, target: &Path) {
            let path = self.root.join(relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("create symlink parent");
            }
            std::os::unix::fs::symlink(target, &path).expect("create fixture symlink");
        }

        pub fn path(&self) -> &Path {
            &self.root
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

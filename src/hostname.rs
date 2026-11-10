//! Hostname lookup without external crates.

/// Returns the raw hostname, or `"ble-temp"` when it cannot be determined.
pub fn raw_hostname() -> String {
    #[cfg(target_os = "linux")]
    if let Some(name) = linux_hostname() {
        return name;
    }

    #[cfg(windows)]
    if let Some(name) = windows_hostname() {
        return name;
    }

    FALLBACK.to_string()
}

const FALLBACK: &str = "ble-temp";

#[cfg(target_os = "linux")]
fn linux_hostname() -> Option<String> {
    let raw = std::fs::read_to_string("/proc/sys/kernel/hostname").ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[cfg(windows)]
fn windows_hostname() -> Option<String> {
    // The BLE stack reports this name; the environment variable is the cheapest
    // source and matches what Windows shows for most installations.
    std::env::var("COMPUTERNAME")
        .ok()
        .filter(|name| !name.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hostname_is_not_empty() {
        assert!(!raw_hostname().is_empty());
    }
}

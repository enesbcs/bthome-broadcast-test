//! Advertisement name construction.
//!
//! Legacy advertising allows 31 bytes. The flags AD structure takes 3 bytes
//! and the BTHome service data takes 8, leaving 20 bytes for the name AD
//! structure: `2 + n <= 20`, so the final name is at most 18 characters.

use crate::source::Source;

/// Maximum length of the complete advertisement name.
pub const MAX_NAME_LEN: usize = 18;

/// Fallback hostname part when sanitisation produces nothing.
pub const FALLBACK_HOST: &str = "ble-temp";

/// Converts an arbitrary hostname into a BLE-safe label.
///
/// Every character outside `[A-Za-z0-9-]` becomes a dash, repeated dashes are
/// collapsed, and leading/trailing dashes are removed.
pub fn sanitize_hostname(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut last_was_dash = false;

    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            last_was_dash = false;
        } else if !last_was_dash && !out.is_empty() {
            out.push('-');
            last_was_dash = true;
        }
    }

    while out.ends_with('-') {
        out.pop();
    }
    out
}

/// Builds the final advertisement name: `<hostname><suffix>`.
///
/// The hostname part is truncated so the result never exceeds
/// [`MAX_NAME_LEN`], and trailing dashes left by truncation are removed.
pub fn build_name(raw_hostname: &str, source: Source) -> String {
    let suffix = source.suffix();
    let max_host = MAX_NAME_LEN.saturating_sub(suffix.len());

    let mut host = sanitize_hostname(raw_hostname);
    if host.is_empty() {
        host.push_str(FALLBACK_HOST);
    }
    if host.len() > max_host {
        host.truncate(max_host);
        while host.ends_with('-') {
            host.pop();
        }
        if host.is_empty() {
            host.push_str(FALLBACK_HOST);
        }
    }

    let mut name = String::with_capacity(host.len() + suffix.len());
    name.push_str(&host);
    name.push_str(suffix);
    name.truncate(MAX_NAME_LEN);
    name
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_ascii_and_separators() {
        assert_eq!(sanitize_hostname("My-PC"), "My-PC");
        assert_eq!(sanitize_hostname("my_pc.local"), "my-pc-local");
        assert_eq!(sanitize_hostname("  my pc  "), "my-pc");
        assert_eq!(sanitize_hostname("---a---b---"), "a-b");
        assert_eq!(sanitize_hostname("___"), "");
        assert_eq!(sanitize_hostname(""), "");
    }

    #[test]
    fn builds_short_names_without_truncation() {
        assert_eq!(build_name("desktop", Source::Cpu), "desktop-C");
        assert_eq!(build_name("desktop", Source::Gpu), "desktop-G");
        assert_eq!(build_name("desktop", Source::Disk), "desktop-D");
        assert_eq!(build_name("desktop", Source::Acpi), "desktop-A");
    }

    #[test]
    fn truncates_hostname_to_16_characters() {
        let host = "abcdefghijklmnopqrstuvwxyz";
        for source in Source::ALL {
            let name = build_name(host, source);
            assert_eq!(name.len(), MAX_NAME_LEN, "{name}");
            assert!(name.starts_with("abcdefghijklmnop"));
            assert!(name.ends_with(source.suffix()));
        }
    }

    #[test]
    fn truncation_does_not_leave_a_trailing_dash() {
        // 16th and 17th characters are dashes, so truncation must trim them.
        let name = build_name("abcdefghijklmnop--q", Source::Cpu);
        assert_eq!(name, "abcdefghijklmnop-C");
        assert!(!name.contains("--"));
    }

    #[test]
    fn empty_or_unusable_hostname_falls_back() {
        assert_eq!(build_name("", Source::Cpu), "ble-temp-C");
        assert_eq!(build_name("!!!", Source::Acpi), "ble-temp-A");
    }

    #[test]
    fn name_never_exceeds_limit() {
        let long = "a".repeat(200);
        for source in Source::ALL {
            let name = build_name(&long, source);
            assert!(name.len() <= MAX_NAME_LEN);
        }
    }

    #[test]
    fn full_packet_with_name_fits_legacy_limit() {
        let name = build_name("abcdefghijklmnop", Source::Cpu);
        assert_eq!(name.len(), MAX_NAME_LEN);
        // flags (3) + service data (8) + name AD (2 + 18)
        assert_eq!(3 + 8 + 2 + name.len(), 31);
    }
}

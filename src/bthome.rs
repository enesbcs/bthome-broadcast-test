//! BTHome v2 encoding for a single temperature object.
//!
//! Layout of the advertisement (legacy, 31-byte limit):
//!
//! ```text
//! 02 01 06                      Flags: LE General Discoverable + BR/EDR not supported
//! 07 16 D2 FC 40 02 <lo> <hi>   Service data: UUID 0xFCD2, device info 0x40, temperature 0x02
//! ```
//!
//! The temperature is a signed 16-bit little-endian value in 0.01 °C units.

/// BTHome service UUID.
pub const SERVICE_UUID: u16 = 0xFCD2;

/// AD type for 16-bit service data.
pub const AD_TYPE_SERVICE_DATA: u8 = 0x16;

/// BTHome device information: v2, no encryption, regular updates.
pub const DEVICE_INFO: u8 = 0x40;

/// BTHome object id for temperature.
pub const OBJECT_TEMPERATURE: u8 = 0x02;

/// Flags AD structure: length 2, type 1 (flags), value 0x06.
pub const FLAGS: [u8; 3] = [0x02, 0x01, 0x06];

/// Length of the service data AD structure (type + UUID + payload).
pub const SERVICE_DATA_AD_LEN: u8 = 0x07;

/// Smallest representable value in 0.01 °C units.
pub const RAW_MIN: f64 = i16::MIN as f64;

/// Largest representable value in 0.01 °C units.
pub const RAW_MAX: f64 = i16::MAX as f64;

/// Encodes a temperature in °C as a signed 16-bit little-endian value in
/// 0.01 °C units. Returns `None` for non-finite values or values outside the
/// representable range.
pub fn encode_temperature(celsius: f64) -> Option<[u8; 2]> {
    if !celsius.is_finite() {
        return None;
    }
    // 1e-9 guards against binary rounding surprises at exact .xx5 boundaries.
    let raw = (celsius * 100.0
        + if celsius.is_sign_negative() {
            -1e-9
        } else {
            1e-9
        })
    .round();
    if !(RAW_MIN..=RAW_MAX).contains(&raw) {
        return None;
    }
    let value = raw as i16;
    Some(value.to_le_bytes())
}

/// BTHome service data payload: `40 02 <lo> <hi>`.
pub fn service_data(celsius: f64) -> Option<[u8; 4]> {
    let temp = encode_temperature(celsius)?;
    Some([DEVICE_INFO, OBJECT_TEMPERATURE, temp[0], temp[1]])
}

/// Complete service data AD structure: `07 16 D2 FC 40 02 <lo> <hi>`.
pub fn service_data_ad(celsius: f64) -> Option<[u8; 8]> {
    let payload = service_data(celsius)?;
    let uuid = SERVICE_UUID.to_le_bytes();
    Some([
        SERVICE_DATA_AD_LEN,
        AD_TYPE_SERVICE_DATA,
        uuid[0],
        uuid[1],
        payload[0],
        payload[1],
        payload[2],
        payload[3],
    ])
}

/// Complete BTHome advertisement without a name: flags + service data.
pub fn advertisement(celsius: f64) -> Option<[u8; 11]> {
    let service = service_data_ad(celsius)?;
    let mut out = [0u8; 11];
    out[..3].copy_from_slice(&FLAGS);
    out[3..].copy_from_slice(&service);
    Some(out)
}

/// Uppercase hexadecimal representation separated by single spaces.
pub fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 3);
    for (i, byte) in bytes.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(&format!("{byte:02X}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_vectors() {
        // 48.50 °C -> 4850 -> 0x12F2
        assert_eq!(encode_temperature(48.5), Some([0xF2, 0x12]));
        // -12.34 °C -> -1234 -> 0xFB2E
        assert_eq!(encode_temperature(-12.34), Some([0x2E, 0xFB]));
        // 0 °C
        assert_eq!(encode_temperature(0.0), Some([0x00, 0x00]));
        // 0.01 °C
        assert_eq!(encode_temperature(0.01), Some([0x01, 0x00]));
        // -0.01 °C
        assert_eq!(encode_temperature(-0.01), Some([0xFF, 0xFF]));
    }

    #[test]
    fn rounds_to_nearest_hundredth() {
        assert_eq!(encode_temperature(21.234), Some([0x4B, 0x08])); // 2123 = 0x084B
        assert_eq!(encode_temperature(21.235), Some([0x4C, 0x08])); // 2124 = 0x084C
        assert_eq!(encode_temperature(-21.234), Some([0xB5, 0xF7])); // -2123
        assert_eq!(encode_temperature(-21.235), Some([0xB4, 0xF7])); // -2124
    }

    #[test]
    fn rejects_out_of_range_and_non_finite() {
        assert_eq!(encode_temperature(327.68), None);
        assert_eq!(encode_temperature(-327.69), None);
        assert_eq!(encode_temperature(f64::NAN), None);
        assert_eq!(encode_temperature(f64::INFINITY), None);
        assert_eq!(encode_temperature(f64::NEG_INFINITY), None);
    }

    #[test]
    fn service_data_layout() {
        assert_eq!(service_data(48.5), Some([0x40, 0x02, 0xF2, 0x12]));
        assert_eq!(
            service_data_ad(48.5),
            Some([0x07, 0x16, 0xD2, 0xFC, 0x40, 0x02, 0xF2, 0x12])
        );
    }

    #[test]
    fn full_advertisement() {
        let packet = advertisement(48.5).unwrap();
        assert_eq!(hex(&packet), "02 01 06 07 16 D2 FC 40 02 F2 12".to_string());
        assert_eq!(packet.len(), 11);
    }

    #[test]
    fn negative_advertisement() {
        let packet = advertisement(-12.34).unwrap();
        assert_eq!(hex(&packet), "02 01 06 07 16 D2 FC 40 02 2E FB".to_string());
    }

    #[test]
    fn fits_legacy_advertising_limit_with_full_name() {
        // flags + service data + name AD (2 + 18) = 3 + 8 + 20 = 31
        let packet = advertisement(48.5).unwrap();
        assert_eq!(packet.len() + 2 + 18, 31);
    }
}

//! Lowercase hexadecimal encoding shared by host and desktop capabilities.

pub fn encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let out = bytes
        .iter()
        .flat_map(|byte| [HEX[(byte >> 4) as usize], HEX[(byte & 0x0f) as usize]])
        .collect();
    String::from_utf8(out).expect("hexadecimal digits are ASCII")
}

#[cfg(test)]
mod tests {
    use super::encode;

    #[test]
    fn encodes_empty_and_leading_zero_bytes_without_separators() {
        assert_eq!(encode(&[]), "");
        assert_eq!(
            encode(&[0, 1, 15, 16, 127, 128, 171, 255]),
            "00010f107f80abff"
        );
    }

    #[test]
    fn matches_lowercase_hex_format_for_every_byte() {
        for byte in 0..=255 {
            assert_eq!(encode(&[byte]), format!("{byte:02x}"));
        }
    }

    #[test]
    fn encodes_all_byte_pairs_and_uneven_buffers_with_exact_capacity() {
        for value in 0..=u16::MAX {
            let bytes = value.to_be_bytes();
            let actual = encode(&bytes);
            assert_eq!(actual, format!("{value:04x}"));
            assert_eq!(actual.capacity(), 4);
        }
        for size in [
            0, 1, 15, 16, 17, 31, 32, 33, 255, 256, 257, 4095, 4096, 4097,
        ] {
            let bytes: Vec<u8> = (0..size).map(|i| (i % 256) as u8).collect();
            let expected: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
            let actual = encode(&bytes);
            assert_eq!(actual, expected);
            assert_eq!(actual.len(), size * 2);
            assert_eq!(actual.capacity(), size * 2);
        }
    }
}

//! Noto Sans, built into the app, since neither macOS nor Windows has it. The repository keeps the two
//! faces as base64 text, and they are decoded when the app starts.

/// Noto Sans Regular.
pub fn regular() -> Vec<u8> {
    decode(include_str!("../fonts/NotoSans-Regular.base64"))
}

/// Noto Sans Bold.
pub fn bold() -> Vec<u8> {
    decode(include_str!("../fonts/NotoSans-Bold.base64"))
}

/// The bytes in base64 text. Line ends and padding are skipped.
fn decode(text: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(text.len() / 4 * 3);
    let mut number = 0_u32;
    let mut count = 0;
    for byte in text.bytes() {
        let digit = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => continue,
        };
        number = number << 6 | u32::from(digit);
        count += 1;
        if count == 4 {
            bytes.extend_from_slice(&number.to_be_bytes()[1..]);
            number = 0;
            count = 0;
        }
    }
    if count > 1 {
        number <<= 6 * (4 - count);
        bytes.extend_from_slice(&number.to_be_bytes()[1..count]);
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_decodes_with_and_without_padding() {
        assert_eq!(decode("TWFu"), b"Man");
        assert_eq!(decode("TWE=\n"), b"Ma");
        assert_eq!(decode("TQ=="), b"M");
        assert_eq!(decode(""), b"");
    }

    #[test]
    fn both_faces_are_true_type_fonts() {
        for face in [regular(), bold()] {
            assert_eq!(face[..4], [0, 1, 0, 0]);
            assert!(face.len() > 400_000);
        }
    }
}

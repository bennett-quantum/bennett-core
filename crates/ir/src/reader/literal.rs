//! Decoding of type words, numbers and string literals.

use crate::{Type, Width};

/// The type a word such as `i32` or `q4` names.
pub(super) fn parse_type(word: &str) -> Option<Type> {
    let (kind, digits) = word.split_at_checked(1)?;
    if !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let width = Width::new(digits.parse().ok()?)?;
    match kind {
        "i" => Some(Type::Int(width)),
        "q" if width.bits() <= Width::MAX_QUBITS => Some(Type::Qreg(width)),
        _ => None,
    }
}

/// An integer literal: decimal or `0x` hexadecimal, optionally negative; whether it is negative, and its
/// magnitude.
pub(super) fn parse_integer(text: &str) -> Option<(bool, u128)> {
    let (negative, digits) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let magnitude = match digits.strip_prefix("0x") {
        Some(hex) if hex.bytes().all(|b| b.is_ascii_hexdigit()) => u128::from_str_radix(hex, 16).ok()?,
        None if digits.bytes().all(|b| b.is_ascii_digit()) => digits.parse().ok()?,
        _ => return None,
    };
    Some((negative, magnitude))
}

/// The bytes a string literal denotes. Escapes: `\n`, `\t`, `\r`, `\0`, `\\`, `\"` and `\xHH`.
pub(super) fn decode_string(text: &str) -> Option<Vec<u8>> {
    let mut bytes = Vec::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            bytes.extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes());
            continue;
        }
        let byte = match chars.next()? {
            'n' => b'\n',
            't' => b'\t',
            'r' => b'\r',
            '0' => 0,
            '\\' => b'\\',
            '"' => b'"',
            'x' => {
                let hex: String = chars.by_ref().take(2).collect();
                if hex.len() != 2 {
                    return None;
                }
                u8::from_str_radix(&hex, 16).ok()?
            }
            _ => return None,
        };
        bytes.push(byte);
    }
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn type_words() {
        assert_eq!(parse_type("i32"), Width::new(32).map(Type::Int));
        assert_eq!(parse_type("q1"), Some(Type::Qreg(Width::BIT)));
        assert_eq!(parse_type("i0"), None);
        assert_eq!(parse_type("i128"), Width::new(128).map(Type::Int));
        assert_eq!(parse_type("i129"), None);
        assert_eq!(parse_type("q65"), None);
        assert_eq!(parse_type("i+8"), None);
        assert_eq!(parse_type("x8"), None);
        assert_eq!(parse_type("i"), None);
    }

    #[test]
    fn integers() {
        assert_eq!(parse_integer("42"), Some((false, 42)));
        assert_eq!(parse_integer("-0x10"), Some((true, 16)));
        assert_eq!(parse_integer("340282366920938463463374607431768211455"), Some((false, u128::MAX)));
        assert_eq!(parse_integer("340282366920938463463374607431768211456"), None);
        assert_eq!(parse_integer("0x+1"), None);
        assert_eq!(parse_integer("1.5"), None);
        assert_eq!(parse_integer("1e3"), None);
    }

    #[test]
    fn strings() {
        assert_eq!(decode_string(r"hi\n\x41\\"), Some(b"hi\nA\\".to_vec()));
        assert_eq!(decode_string(r"\q"), None);
        assert_eq!(decode_string(r"\x4"), None);
    }
}

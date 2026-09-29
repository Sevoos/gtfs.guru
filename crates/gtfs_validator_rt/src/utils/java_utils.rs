#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JavaParseFloatError {
    Empty,
    InvalidSyntax,
    InvalidDecimal,
    InvalidHex,
}

impl std::fmt::Display for JavaParseFloatError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::Empty => "empty floating-point string",
            Self::InvalidSyntax => "invalid Java floating-point syntax",
            Self::InvalidDecimal => "invalid Java decimal floating-point literal",
            Self::InvalidHex => "invalid Java hexadecimal floating-point literal",
        };

        f.write_str(message)
    }
}

impl std::error::Error for JavaParseFloatError {}

/// `0x1.8p1`. Java requires the `p` binary exponent, so a bare `0x1` throws.
/// Rounds through f64, so it is not bit-exact with Java's parser at extreme
/// precision — irrelevant for a version string compared against 2.0.
fn parse_java_hex_float(hex: &str) -> Option<f32> {
    let (mantissa_text, exponent_text) = hex.split_once(['p', 'P'])?;
    let (int_part, frac_part) = mantissa_text.split_once('.').unwrap_or((mantissa_text, ""));
    if int_part.is_empty() && frac_part.is_empty() {
        return None;
    }
    let mut mantissa = 0.0f64;
    for byte in int_part.bytes() {
        mantissa = mantissa * 16.0 + f64::from((byte as char).to_digit(16)?);
    }
    let mut scale = 1.0f64 / 16.0;
    for byte in frac_part.bytes() {
        mantissa += f64::from((byte as char).to_digit(16)?) * scale;
        scale /= 16.0;
    }
    let exponent: i32 = exponent_text.parse().ok()?;
    Some((mantissa * 2f64.powi(exponent)) as f32)
}

pub fn java_parse_float(value: &str) -> Result<f32, JavaParseFloatError> {
    // Java String.trim(): remove code units <= U+0020.
    let trimmed = value.trim_matches(|c: char| c <= '\u{20}');
    if trimmed.is_empty() {
        return Err(JavaParseFloatError::Empty);
    }

    let (negative, rest) = match trimmed.as_bytes()[0] {
        b'-' => (true, &trimmed[1..]),
        b'+' => (false, &trimmed[1..]),
        _ => (false, trimmed),
    };

    if rest.is_empty() {
        return Err(JavaParseFloatError::InvalidSyntax);
    }

    // Java special values are case-sensitive and cannot have f/d suffixes.
    if rest == "NaN" {
        return Ok(f32::NAN);
    }
    if rest == "Infinity" {
        return Ok(if negative {
            f32::NEG_INFINITY
        } else {
            f32::INFINITY
        });
    }

    // Prevent Rust's additional case-insensitive inf/nan spellings.
    if !matches!(rest.as_bytes()[0], b'0'..=b'9' | b'.') {
        return Err(JavaParseFloatError::InvalidSyntax);
    }

    // Java permits one trailing float/double suffix.
    let body = match rest.as_bytes()[rest.len() - 1] {
        b'f' | b'F' | b'd' | b'D' => &rest[..rest.len() - 1],
        _ => rest,
    };

    if body.is_empty() {
        return Err(JavaParseFloatError::InvalidSyntax);
    }

    let magnitude = if let Some(hex) = body.strip_prefix("0x").or_else(|| body.strip_prefix("0X")) {
        parse_java_hex_float(hex).ok_or(JavaParseFloatError::InvalidHex)?
    } else {
        body.parse::<f32>()
            .map_err(|_| JavaParseFloatError::InvalidDecimal)?
    };

    Ok(if negative { -magnitude } else { magnitude })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every value verified against `Float.parseFloat` on the JDK, not assumed.
    /// `None` means Java throws `NumberFormatException`.
    #[track_caller]
    fn assert_java(input: &str, expected: Option<f32>) {
        let actual = java_parse_float(input).ok();
        match (actual, expected) {
            // NaN never equals itself, so compare the classification instead.
            (Some(a), Some(e)) if e.is_nan() => {
                assert!(a.is_nan(), "input {input:?}: expected NaN, got {a}")
            }
            _ => assert_eq!(actual, expected, "input {input:?}"),
        }
    }

    #[test]
    fn decimal_forms_match_java() {
        for (input, expected) in [
            ("1.0", Some(1.0)),
            ("2.0", Some(2.0)),
            ("3.0", Some(3.0)),
            ("1", Some(1.0)),
            ("2.0e0", Some(2.0)),
            ("-2.0", Some(-2.0)),
            ("+2.0", Some(2.0)),
            // Java accepts one float/double type suffix; Rust does not.
            ("2.0f", Some(2.0)),
            ("2.0F", Some(2.0)),
            ("2.0d", Some(2.0)),
            // Java's String.trim() strips code units <= U+0020 before parsing.
            (" 2.0 ", Some(2.0)),
            // Case-sensitive, and they must consume the whole string.
            ("Infinity", Some(f32::INFINITY)),
            ("NaN", Some(f32::NAN)),
            ("NaNf", None),
            // Rust would accept these spellings; Java throws.
            ("inf", None),
            ("infinity", None),
            ("nan", None),
            ("abcd", None),
            ("", None),
            (".", None),
        ] {
            assert_java(input, expected);
        }
    }

    #[test]
    fn hex_floats_match_java() {
        for (input, expected) in [
            ("0x1p0", Some(1.0)),
            ("0x1p2", Some(4.0)),
            ("0x1P2", Some(4.0)),
            ("0x1.8p1", Some(3.0)),
            ("0x1.8p0", Some(1.5)),
            ("0x0p0", Some(0.0)),
            ("0xAp0", Some(10.0)),
            ("0xap0", Some(10.0)),
            ("0x1p-1", Some(0.5)),
            ("0x1p+2", Some(4.0)),
            ("0x1.fp3", Some(15.5)),
            // A missing integer or fraction part is still legal.
            ("0x.8p1", Some(1.0)),
            ("0x1.p1", Some(2.0)),
            ("0x1p2f", Some(4.0)),
            ("0x1p2d", Some(4.0)),
            // Java requires the `p` binary exponent.
            ("0x1", None),
            ("0x1.8", None),
            ("0x1p", None),
            ("0x1pz", None),
            // No mantissa, and a non-hex digit.
            ("0xp1", None),
            ("0xGp1", None),
        ] {
            assert_java(input, expected);
        }
    }
}

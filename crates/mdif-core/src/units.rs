//! Byte formatting. Decimal units, like Finder and `diskutil`.

pub fn bytes(n: u64) -> String {
    let n = n as f64;
    if n >= 1e9 {
        format!("{:.1} GB", n / 1e9)
    } else if n >= 1e6 {
        format!("{:.0} MB", n / 1e6)
    } else if n >= 1e3 {
        format!("{:.0} KB", n / 1e3)
    } else {
        format!("{n} B")
    }
}

/// Signed variant for differences such as "measured minus reported".
pub fn signed(n: i64) -> String {
    if n < 0 {
        format!("-{}", bytes(n.unsigned_abs()))
    } else {
        bytes(n as u64)
    }
}

/// Bytes from a plain integer or a number with a decimal unit: `500MB`,
/// `5 GB`, `1.5G`, `2000000000`. `None` for anything else.
pub fn parse_bytes(s: &str) -> Option<u64> {
    let s = s.trim().replace('_', "");
    let split = s
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(s.len());
    let (num, unit) = s.split_at(split);
    let scale: f64 = match unit.trim().to_ascii_lowercase().as_str() {
        "" | "b" => 1.0,
        "k" | "kb" => 1e3,
        "m" | "mb" => 1e6,
        "g" | "gb" => 1e9,
        "t" | "tb" => 1e12,
        _ => return None,
    };
    if num.is_empty() {
        return None;
    }
    if unit.is_empty() || scale == 1.0 {
        return num.parse().ok();
    }
    let v: f64 = num.parse().ok()?;
    let b = (v * scale).round();
    (b.is_finite() && b >= 0.0 && b < u64::MAX as f64).then_some(b as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(1_500), "2 KB");
        assert_eq!(bytes(23_400_000), "23 MB");
        assert_eq!(bytes(494_384_795_648), "494.4 GB");
        assert_eq!(signed(-2_000_000_000), "-2.0 GB");
    }

    #[test]
    fn parses() {
        assert_eq!(parse_bytes("2000000000"), Some(2_000_000_000));
        assert_eq!(parse_bytes("5GB"), Some(5_000_000_000));
        assert_eq!(parse_bytes(" 500 mb "), Some(500_000_000));
        assert_eq!(parse_bytes("1.5G"), Some(1_500_000_000));
        assert_eq!(parse_bytes("1_000 MB"), Some(1_000_000_000));
        assert_eq!(parse_bytes("12B"), Some(12));
        for bad in ["", "GB", "5 GiB", "-1GB", "1.5", "five", "1e9", "1.2.3GB"] {
            assert_eq!(parse_bytes(bad), None, "{bad}");
        }
    }
}

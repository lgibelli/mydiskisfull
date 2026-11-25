//! Numbers the way `mdif` and the Mac app print them (`Format.swift`).

/// Decimal units, like Explorer's "on disk" figures and `mdif`: "24.8 GB",
/// "23 MB".
pub fn bytes(n: i64) -> String {
    let v = n.unsigned_abs() as f64;
    let sign = if n < 0 { "-" } else { "" };
    if v >= 1e9 {
        format!("{sign}{:.1} GB", v / 1e9)
    } else if v >= 1e6 {
        format!("{sign}{:.0} MB", v / 1e6)
    } else if v >= 1e3 {
        format!("{sign}{:.0} KB", v / 1e3)
    } else {
        format!("{sign}{} B", v as i64)
    }
}

/// "12 days", "5 hours", "3 months".
pub fn days(d: f64) -> String {
    let (n, unit) = if d < 1.0 {
        ((d * 24.0).round().max(1.0), "hour")
    } else if d < 100.0 {
        (d.round(), "day")
    } else {
        ((d / 30.4).round(), "month")
    };
    if n == 1.0 {
        format!("1 {unit}")
    } else {
        format!("{} {unit}s", n as i64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn as_the_mac_app_says_them() {
        assert_eq!(bytes(24_800_000_000), "24.8 GB");
        assert_eq!(bytes(23_400_000), "23 MB");
        assert_eq!(bytes(-2_900_000_000), "-2.9 GB");
        assert_eq!(bytes(512), "512 B");
        assert_eq!(days(0.2), "5 hours");
        assert_eq!(days(1.0), "1 day");
        assert_eq!(days(12.4), "12 days");
        assert_eq!(days(200.0), "7 months");
    }
}

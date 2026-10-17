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
}

use std::os::windows::ffi::OsStrExt;
use std::path::Path;

/// UTF-16 with a terminating NUL, for Win32 `PCWSTR` arguments.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

pub fn wide_path(p: &Path) -> Vec<u16> {
    p.as_os_str().encode_wide().chain(Some(0)).collect()
}

/// The text up to the first NUL (or the whole slice).
pub(crate) fn from_wide(w: &[u16]) -> String {
    let n = w.iter().position(|&c| c == 0).unwrap_or(w.len());
    String::from_utf16_lossy(&w[..n])
}

/// 4105298 -> "4,105,298", -1234 -> "-1,234".
pub fn group_thousands(n: i64) -> String {
    let mut d = n.to_string();
    let start = usize::from(n < 0);
    let mut i = d.len();
    while i > start + 3 {
        i -= 3;
        d.insert(i, ',');
    }
    d
}

/// Digits only, 1..=6 chars, value within [lo, hi]; anything else gives None.
pub fn parse_bounded(s: &str, lo: i32, hi: i32) -> Option<i32> {
    if s.is_empty() || s.len() > 6 || !s.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let v: i32 = s.parse().ok()?;
    (lo..=hi).contains(&v).then_some(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_wide_roundtrip() {
        assert_eq!(wide(""), [0]);
        assert_eq!(from_wide(&wide("\u{2248} C$")), "\u{2248} C$");
        assert_eq!(from_wide(&[0x41, 0, 0x42]), "A");
    }

    #[test]
    fn text_group_thousands() {
        assert_eq!(group_thousands(0), "0");
        assert_eq!(group_thousands(999), "999");
        assert_eq!(group_thousands(4105298), "4,105,298");
        assert_eq!(group_thousands(-1234), "-1,234");
    }

    #[test]
    fn text_parse_bounded() {
        assert_eq!(parse_bounded("0", 0, 100), Some(0));
        assert_eq!(parse_bounded("007", 0, 100), Some(7));
        assert_eq!(parse_bounded("10000", 0, 10000), Some(10000));
        for bad in ["", "-1", "1 ", "101"] {
            assert_eq!(parse_bounded(bad, 0, 100), None);
        }
        assert_eq!(parse_bounded("4", 5, 300), None);
        assert_eq!(parse_bounded("9999999", 0, 2_000_000_000), None); // more than 6 digits
    }
}

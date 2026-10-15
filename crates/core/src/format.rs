//! Display formatting for integer quantities.

/// Formats `n` in decimal with a comma between every group of three digits, counted from the right.
///
/// A negative number gets a leading `-`. `i64::MIN` is handled without overflow.
pub fn group_thousands(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let len = digits.len();
    let mut out = String::with_capacity(len + len / 3 + 1);
    if n < 0 {
        out.push('-');
    }
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (len - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::group_thousands;

    #[test]
    fn small_numbers_have_no_separator() {
        assert_eq!(group_thousands(0), "0");
        assert_eq!(group_thousands(7), "7");
        assert_eq!(group_thousands(999), "999");
    }

    #[test]
    fn positive_numbers_are_grouped_from_the_right() {
        assert_eq!(group_thousands(1000), "1,000");
        assert_eq!(group_thousands(12345), "12,345");
        assert_eq!(group_thousands(999_999), "999,999");
        assert_eq!(group_thousands(1_000_000), "1,000,000");
        assert_eq!(group_thousands(1_234_567), "1,234,567");
    }

    #[test]
    fn negative_numbers_keep_the_sign_in_front() {
        assert_eq!(group_thousands(-1), "-1");
        assert_eq!(group_thousands(-1000), "-1,000");
        assert_eq!(group_thousands(-999_999), "-999,999");
        assert_eq!(group_thousands(-1_234_567), "-1,234,567");
    }

    #[test]
    fn extremes_do_not_overflow() {
        assert_eq!(group_thousands(i64::MAX), "9,223,372,036,854,775,807");
        assert_eq!(group_thousands(i64::MIN), "-9,223,372,036,854,775,808");
    }
}

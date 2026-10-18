//! Volumes in m³ as exact integers.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::float_arithmetic,
    clippy::as_conversions,
    clippy::integer_division
)]

use crate::decimal::{Grammar, ParseNumberError, fixed_2, parse_scaled, plain};

/// A volume, counted in ten-thousandths of a cubic metre (0.0001 m³).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Volume(i64);

impl Volume {
    pub const fn from_ten_thousandths(count: i64) -> Volume {
        Volume(count)
    }

    pub const fn ten_thousandths(self) -> i64 {
        self.0
    }

    /// Parses game data text such as `0.01`, `80000.0` or `2.5e-3`.
    ///
    /// The exact value must be a whole number of 0.0001 m³. A volume is never negative.
    pub fn parse_decimal(text: &str) -> Result<Volume, ParseNumberError> {
        let grammar = Grammar {
            negative: false,
            grouping: false,
            game_data: true,
        };
        parse_scaled(text.trim(), grammar).map(Volume)
    }

    pub fn checked_add(self, other: Volume) -> Option<Volume> {
        self.0.checked_add(other.0).map(Volume)
    }

    /// The volume times a whole quantity, exact.
    pub fn checked_mul_qty(self, qty: u64) -> Option<Volume> {
        let product = i128::from(self.0).checked_mul(i128::from(qty))?;
        i64::try_from(product).ok().map(Volume)
    }

    /// The exact volume, ungrouped, without trailing decimal zeros.
    pub fn to_plain(self) -> String {
        plain(self.0)
    }

    /// The volume rounded to 0.01 m³, grouped by thousands, always with two decimals.
    pub fn format_m3(self) -> String {
        fixed_2(self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::Volume;
    use crate::decimal::ParseNumberError::{Empty, Invalid, OutOfRange, TooManyDecimals};

    fn v(count: i64) -> Volume {
        Volume::from_ten_thousandths(count)
    }

    #[test]
    fn volume_wraps_a_count_of_ten_thousandths() {
        assert_eq!(v(7).ten_thousandths(), 7);
        assert_eq!(Volume::default(), v(0));
        assert!(v(2) > v(1));
    }

    #[test]
    fn parse_decimal_reads_game_data() {
        assert_eq!(Volume::parse_decimal("0.01"), Ok(v(100)));
        assert_eq!(Volume::parse_decimal("80000.0"), Ok(v(800_000_000)));
        assert_eq!(Volume::parse_decimal("27289"), Ok(v(272_890_000)));
        assert_eq!(Volume::parse_decimal("0.010000"), Ok(v(100)));
        assert_eq!(Volume::parse_decimal("2.5e-3"), Ok(v(25)));
        assert_eq!(Volume::parse_decimal("1E+2"), Ok(v(1_000_000)));
        assert_eq!(Volume::parse_decimal("0"), Ok(v(0)));
        assert_eq!(Volume::parse_decimal(" 0.01 "), Ok(v(100)));
    }

    #[test]
    fn parse_decimal_names_what_is_wrong() {
        assert_eq!(Volume::parse_decimal("0.00025"), Err(TooManyDecimals));
        assert_eq!(Volume::parse_decimal("2.5e-5"), Err(TooManyDecimals));
        for text in ["-1", "1.2.3", "e5", "1,000", "+1", "1."] {
            assert_eq!(Volume::parse_decimal(text), Err(Invalid), "{text}");
        }
        assert_eq!(Volume::parse_decimal("1e400"), Err(OutOfRange));
        assert_eq!(Volume::parse_decimal("1000000000000000"), Err(OutOfRange));
        assert_eq!(Volume::parse_decimal(""), Err(Empty));
        assert_eq!(Volume::parse_decimal("  "), Err(Empty));
    }

    #[test]
    fn format_m3_rounds_to_hundredths_and_groups() {
        assert_eq!(v(705_000).format_m3(), "70.50");
        assert_eq!(v(1_174_800).format_m3(), "117.48");
        assert_eq!(v(8_485_780_700).format_m3(), "848,578.07");
        assert_eq!(v(705_050).format_m3(), "70.51");
        assert_eq!(v(25).format_m3(), "0.00");
    }

    #[test]
    fn to_plain_is_exact() {
        assert_eq!(v(25).to_plain(), "0.0025");
        assert_eq!(v(272_890_000).to_plain(), "27289");
        assert_eq!(v(0).to_plain(), "0");
        assert_eq!(
            Volume::parse_decimal(&v(i64::MAX).to_plain()),
            Ok(v(i64::MAX))
        );
    }

    #[test]
    fn quantities_and_sums_detect_overflow() {
        assert_eq!(v(25).checked_mul_qty(10_000), Some(v(250_000)));
        assert_eq!(v(25_000_000).checked_mul_qty(31), Some(v(775_000_000)));
        assert_eq!(v(25).checked_mul_qty(0), Some(v(0)));
        assert_eq!(v(i64::MAX).checked_mul_qty(2), None);
        assert_eq!(v(i64::MAX).checked_mul_qty(1), Some(v(i64::MAX)));
        assert_eq!(v(i64::MAX).checked_add(v(1)), None);
        assert_eq!(v(3).checked_add(v(4)), Some(v(7)));
    }
}

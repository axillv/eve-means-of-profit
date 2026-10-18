//! ISK amounts and percentage rates as exact integers.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::float_arithmetic,
    clippy::as_conversions,
    clippy::integer_division
)]

use crate::decimal::{Grammar, ParseNumberError, UNIT, div_round, fixed_2, parse_scaled, plain};
use crate::volume::Volume;

/// A rate of 100% in parts per million.
const FULL_RATE_PPM: u32 = 1_000_000;

/// An amount of ISK, counted in ten-thousandths of ISK (0.0001).
///
/// Products are computed in `i128` and rounded half away from zero, so no total is off by float error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Money(i64);

impl Money {
    pub const ZERO: Money = Money(0);

    pub const fn from_ten_thousandths(count: i64) -> Money {
        Money(count)
    }

    pub const fn ten_thousandths(self) -> i64 {
        self.0
    }

    /// Parses an amount typed by a person: `1,234.5`, `-0.0001`.
    ///
    /// Digits may be grouped with `,`; up to four decimals; no sign other than `-`, no exponent.
    pub fn parse_isk(text: &str) -> Result<Money, ParseNumberError> {
        let grammar = Grammar {
            negative: true,
            grouping: true,
            game_data: false,
        };
        parse_scaled(text.trim(), grammar).map(Money)
    }

    /// Parses number text from game data and ESI JSON: `478.6`, `747.0`, `1e3`.
    pub fn parse_decimal(text: &str) -> Result<Money, ParseNumberError> {
        let grammar = Grammar {
            negative: true,
            grouping: false,
            game_data: true,
        };
        parse_scaled(text.trim(), grammar).map(Money)
    }

    pub fn checked_add(self, other: Money) -> Option<Money> {
        self.0.checked_add(other.0).map(Money)
    }

    pub fn checked_sub(self, other: Money) -> Option<Money> {
        self.0.checked_sub(other.0).map(Money)
    }

    /// The amount times a whole quantity, exact.
    pub fn checked_mul_qty(self, qty: u64) -> Option<Money> {
        self.scaled(i128::from(qty), 1)
    }

    /// The amount times a rate, rounded to 0.0001 ISK.
    pub fn checked_mul_rate(self, rate: Rate) -> Option<Money> {
        self.scaled(i128::from(rate.ppm()), i128::from(FULL_RATE_PPM))
    }

    /// A price per m³ times a volume, rounded to 0.0001 ISK.
    pub fn checked_mul_volume(self, volume: Volume) -> Option<Money> {
        self.scaled(i128::from(volume.ten_thousandths()), i128::from(UNIT))
    }

    /// The amount per unit when split over `qty` units, rounded; `None` for zero units.
    pub fn checked_div_qty(self, qty: u64) -> Option<Money> {
        self.scaled(1, i128::from(qty))
    }

    /// The exact amount, ungrouped, without trailing decimal zeros. `parse_isk` reads it back.
    pub fn to_plain(self) -> String {
        plain(self.0)
    }

    /// The amount rounded to 0.01 ISK, grouped by thousands, always with two decimals.
    pub fn format_isk(self) -> String {
        fixed_2(self.0)
    }

    fn scaled(self, numerator: i128, denominator: i128) -> Option<Money> {
        let product = i128::from(self.0).checked_mul(numerator)?;
        i64::try_from(div_round(product, denominator)?)
            .ok()
            .map(Money)
    }
}

/// A percentage rate in parts per million (1% = 10,000).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Rate(u32);

impl Rate {
    pub const fn from_ppm(ppm: u32) -> Rate {
        Rate(ppm)
    }

    pub const fn ppm(self) -> u32 {
        self.0
    }

    /// Parses a percentage such as `3.375`: up to four decimals, from 0 to 100, no sign.
    pub fn parse_percent(text: &str) -> Result<Rate, ParseNumberError> {
        let grammar = Grammar {
            negative: false,
            grouping: false,
            game_data: false,
        };
        let ppm = parse_scaled(text.trim(), grammar)?;
        u32::try_from(ppm)
            .ok()
            .filter(|ppm| *ppm <= FULL_RATE_PPM)
            .map(Rate)
            .ok_or(ParseNumberError::OutOfRange)
    }

    /// The exact percentage without trailing zeros; `parse_percent` reads it back.
    pub fn to_percent_string(self) -> String {
        plain(i64::from(self.0))
    }
}

#[cfg(test)]
mod tests {
    use super::{Money, Rate};
    use crate::decimal::ParseNumberError::{Empty, Invalid, OutOfRange, TooManyDecimals};
    use crate::volume::Volume;

    fn m(count: i64) -> Money {
        Money::from_ten_thousandths(count)
    }

    #[test]
    fn money_wraps_a_count_of_ten_thousandths() {
        assert_eq!(m(7).ten_thousandths(), 7);
        assert_eq!(Money::ZERO, m(0));
        assert_eq!(Money::default(), Money::ZERO);
        assert!(m(1) > m(0));
        assert!(m(-1) < m(0));
    }

    #[test]
    fn parse_isk_reads_typed_amounts() {
        assert_eq!(Money::parse_isk("478.60"), Ok(m(4_786_000)));
        assert_eq!(Money::parse_isk("1,234.5"), Ok(m(12_345_000)));
        assert_eq!(Money::parse_isk("  31,500,000  "), Ok(m(315_000_000_000)));
        assert_eq!(Money::parse_isk("-0.0001"), Ok(m(-1)));
        assert_eq!(Money::parse_isk("0"), Ok(m(0)));
        assert_eq!(
            Money::parse_isk("922,337,203,685,477.5807"),
            Ok(m(i64::MAX))
        );
    }

    #[test]
    fn parse_isk_names_what_is_wrong() {
        assert_eq!(Money::parse_isk(""), Err(Empty));
        assert_eq!(Money::parse_isk("   "), Err(Empty));
        assert_eq!(Money::parse_isk("1.23456"), Err(TooManyDecimals));
        assert_eq!(Money::parse_isk("922337203685477.5808"), Err(OutOfRange));
        for text in [
            "abc", "1,23", "1,2345", "1.", ".5", "--1", "+1", "1 000", "1e3",
        ] {
            assert_eq!(Money::parse_isk(text), Err(Invalid), "{text}");
        }
    }

    #[test]
    fn parse_decimal_reads_game_data() {
        assert_eq!(Money::parse_decimal("478.6"), Ok(m(4_786_000)));
        assert_eq!(Money::parse_decimal("747.0"), Ok(m(7_470_000)));
        assert_eq!(Money::parse_decimal("1e3"), Ok(m(10_000_000)));
        assert_eq!(Money::parse_decimal("-2.5E-3"), Ok(m(-25)));
        assert_eq!(Money::parse_decimal("0.01"), Ok(m(100)));
        assert_eq!(Money::parse_decimal(" 0.01 "), Ok(m(100)));
        assert_eq!(Money::parse_decimal("478.60001"), Err(TooManyDecimals));
        assert_eq!(Money::parse_decimal("-2.5E-4"), Err(TooManyDecimals));
        assert_eq!(Money::parse_decimal("1,000"), Err(Invalid));
        assert_eq!(Money::parse_decimal("+1"), Err(Invalid));
        assert_eq!(Money::parse_decimal("1e20"), Err(OutOfRange));
        assert_eq!(Money::parse_decimal(""), Err(Empty));
    }

    #[test]
    fn to_plain_is_exact_and_round_trips() {
        assert_eq!(m(21_250).to_plain(), "2.125");
        assert_eq!(m(4_786_000).to_plain(), "478.6");
        assert_eq!(m(100_000).to_plain(), "10");
        assert_eq!(m(-1).to_plain(), "-0.0001");
        assert_eq!(m(0).to_plain(), "0");
        assert_eq!(m(i64::MIN).to_plain(), "-922337203685477.5808");
        for count in [21_250, 4_786_000, 100_000, -1, 0, i64::MIN, i64::MAX] {
            assert_eq!(Money::parse_isk(&m(count).to_plain()), Ok(m(count)));
        }
    }

    #[test]
    fn format_isk_rounds_to_cents_and_groups() {
        assert_eq!(m(315_000_000_000).format_isk(), "31,500,000.00");
        assert_eq!(m(51_000).format_isk(), "5.10");
        assert_eq!(m(21_250).format_isk(), "2.13");
        assert_eq!(m(-12_345_678).format_isk(), "-1,234.57");
        assert_eq!(m(-49).format_isk(), "0.00");
        assert_eq!(m(-50).format_isk(), "-0.01");
        assert_eq!(m(i64::MAX).format_isk(), "922,337,203,685,477.58");
        assert_eq!(m(i64::MIN).format_isk(), "-922,337,203,685,477.58");
    }

    #[test]
    fn adding_and_subtracting_detect_overflow() {
        assert_eq!(m(5).checked_add(m(7)), Some(m(12)));
        assert_eq!(m(5).checked_add(m(-7)), Some(m(-2)));
        assert_eq!(m(i64::MAX).checked_add(m(1)), None);
        assert_eq!(m(i64::MIN).checked_add(m(-1)), None);
        assert_eq!(m(5).checked_sub(m(7)), Some(m(-2)));
        assert_eq!(m(i64::MIN).checked_sub(m(1)), None);
        assert_eq!(m(i64::MAX).checked_sub(m(-1)), None);
    }

    #[test]
    fn multiplying_by_a_quantity_is_exact() {
        assert_eq!(m(4_786_000).checked_mul_qty(10), Some(m(47_860_000)));
        assert_eq!(m(4_786_000).checked_mul_qty(0), Some(m(0)));
        assert_eq!(m(-3).checked_mul_qty(4), Some(m(-12)));
        assert_eq!(m(i64::MAX).checked_mul_qty(2), None);
        assert_eq!(m(i64::MAX).checked_mul_qty(1), Some(m(i64::MAX)));
        assert_eq!(m(1).checked_mul_qty(u64::MAX), None);
    }

    #[test]
    fn multiplying_by_a_rate_rounds_half_away_from_zero() {
        let collateral = Rate::from_ppm(5000);
        assert_eq!(
            m(20_032_025_000).checked_mul_rate(collateral),
            Some(m(100_160_125))
        );
        assert_eq!(m(100).checked_mul_rate(collateral), Some(m(1)));
        assert_eq!(m(-100).checked_mul_rate(collateral), Some(m(-1)));
        assert_eq!(m(99).checked_mul_rate(collateral), Some(m(0)));
        let sales_tax = Rate::from_ppm(33_750);
        assert_eq!(
            m(1_000_000_000_000).checked_mul_rate(sales_tax),
            Some(m(33_750_000_000))
        );
        assert_eq!(
            m(i64::MAX).checked_mul_rate(Rate::from_ppm(1_000_000)),
            Some(m(i64::MAX))
        );
        assert_eq!(m(i64::MAX).checked_mul_rate(Rate::from_ppm(u32::MAX)), None);
    }

    #[test]
    fn multiplying_by_a_volume_rounds_half_away_from_zero() {
        let price = m(8_500_000);
        assert_eq!(
            price.checked_mul_volume(Volume::from_ten_thousandths(705_000)),
            Some(m(599_250_000))
        );
        assert_eq!(
            price.checked_mul_volume(Volume::from_ten_thousandths(25)),
            Some(m(21_250))
        );
        assert_eq!(
            m(1).checked_mul_volume(Volume::from_ten_thousandths(5000)),
            Some(m(1))
        );
        assert_eq!(
            m(1).checked_mul_volume(Volume::from_ten_thousandths(4999)),
            Some(m(0))
        );
        assert_eq!(
            m(-1).checked_mul_volume(Volume::from_ten_thousandths(5000)),
            Some(m(-1))
        );
        assert_eq!(
            m(i64::MAX).checked_mul_volume(Volume::from_ten_thousandths(20_000)),
            None
        );
        assert_eq!(
            m(i64::MAX).checked_mul_volume(Volume::from_ten_thousandths(10_000)),
            Some(m(i64::MAX))
        );
    }

    #[test]
    fn dividing_by_a_quantity_rounds_half_away_from_zero() {
        assert_eq!(m(100_000).checked_div_qty(3), Some(m(33_333)));
        assert_eq!(m(20_000).checked_div_qty(3), Some(m(6667)));
        assert_eq!(m(5).checked_div_qty(2), Some(m(3)));
        assert_eq!(m(-5).checked_div_qty(2), Some(m(-3)));
        assert_eq!(m(4).checked_div_qty(2), Some(m(2)));
        assert_eq!(m(5).checked_div_qty(0), None);
        assert_eq!(m(0).checked_div_qty(0), None);
        assert_eq!(m(i64::MIN).checked_div_qty(1), Some(m(i64::MIN)));
    }

    #[test]
    fn rate_reads_percentages() {
        assert_eq!(Rate::parse_percent("0.5"), Ok(Rate::from_ppm(5000)));
        assert_eq!(Rate::parse_percent("3.375"), Ok(Rate::from_ppm(33_750)));
        assert_eq!(Rate::parse_percent("100"), Ok(Rate::from_ppm(1_000_000)));
        assert_eq!(Rate::parse_percent("0"), Ok(Rate::from_ppm(0)));
        assert_eq!(Rate::parse_percent(" 0 "), Ok(Rate::from_ppm(0)));
        assert_eq!(
            Rate::parse_percent("100.0000"),
            Ok(Rate::from_ppm(1_000_000))
        );
        assert_eq!(Rate::parse_percent("100.0001"), Err(OutOfRange));
        assert_eq!(Rate::parse_percent("0.00001"), Err(TooManyDecimals));
        assert_eq!(Rate::parse_percent("-1"), Err(Invalid));
        assert_eq!(Rate::parse_percent("1%"), Err(Invalid));
        assert_eq!(Rate::parse_percent(""), Err(Empty));
    }

    #[test]
    fn rate_shows_the_exact_percentage() {
        assert_eq!(Rate::from_ppm(5000).to_percent_string(), "0.5");
        assert_eq!(Rate::from_ppm(33_750).to_percent_string(), "3.375");
        assert_eq!(Rate::from_ppm(1_000_000).to_percent_string(), "100");
        assert_eq!(Rate::from_ppm(0).to_percent_string(), "0");
        assert_eq!(Rate::from_ppm(5000).ppm(), 5000);
        assert_eq!(Rate::default(), Rate::from_ppm(0));
        assert!(Rate::from_ppm(2) > Rate::from_ppm(1));
    }
}

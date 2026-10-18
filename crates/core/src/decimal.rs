//! Exact decimal text handling shared by money, rates and volumes.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::float_arithmetic,
    clippy::as_conversions,
    clippy::integer_division
)]

use std::fmt;

use crate::format::group_thousands;

/// Why a piece of text is not a usable number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseNumberError {
    /// The text is empty or only whitespace.
    Empty,
    /// The text does not follow the number format.
    Invalid,
    /// The exact value has more decimals than the unit holds.
    TooManyDecimals,
    /// The value does not fit the type.
    OutOfRange,
}

impl fmt::Display for ParseNumberError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Empty => "The number is empty.",
            Self::Invalid => "The text is not a valid number.",
            Self::TooManyDecimals => "The number has more decimals than are supported.",
            Self::OutOfRange => "The number is outside the supported range.",
        })
    }
}

impl std::error::Error for ParseNumberError {}

/// Decimals every unit holds: amounts are counted in 10^-4.
const SCALE: u32 = 4;
/// One whole unit as a count of 10^-4 units (10^`SCALE`).
pub(crate) const UNIT: u64 = 10_u64.pow(SCALE);

/// Which spellings `parse_scaled` accepts.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Grammar {
    /// A leading `-` is allowed.
    pub(crate) negative: bool,
    /// Integer digits may be grouped with `,`.
    pub(crate) grouping: bool,
    /// Game data text: an exponent and trailing zero decimals are allowed.
    pub(crate) game_data: bool,
}

/// Parses trimmed decimal text into a count of 10^-4 units, exact or an error.
pub(crate) fn parse_scaled(text: &str, grammar: Grammar) -> Result<i64, ParseNumberError> {
    if text.is_empty() {
        return Err(ParseNumberError::Empty);
    }
    let unsigned = text.strip_prefix('-').filter(|_| grammar.negative);
    let negative = unsigned.is_some();
    let body = unsigned.unwrap_or(text);
    let (mantissa, exponent) = match body.split_once(['e', 'E']).filter(|_| grammar.game_data) {
        Some((mantissa, exponent)) => (mantissa, parse_exponent(exponent)?),
        None => (body, 0),
    };
    let (integer, fraction) = split_fraction(mantissa)?;
    let fraction_ok = fraction.is_empty() || all_digits(fraction);
    if !valid_integer(integer, grammar.grouping) || !fraction_ok {
        return Err(ParseNumberError::Invalid);
    }
    if !grammar.game_data && len_i64(fraction)? > i64::from(SCALE) {
        return Err(ParseNumberError::TooManyDecimals);
    }
    let kept = fraction.trim_end_matches('0');
    let digits = integer
        .chars()
        .chain(kept.chars())
        .filter_map(|c| c.to_digit(10));
    let mut value: u128 = 0;
    for digit in digits {
        value = value
            .checked_mul(10)
            .and_then(|v| v.checked_add(u128::from(digit)))
            .ok_or(ParseNumberError::OutOfRange)?;
    }
    let shift = exponent
        .checked_add(i64::from(SCALE))
        .and_then(|shift| shift.checked_sub(len_i64(kept).ok()?))
        .ok_or(ParseNumberError::OutOfRange)?;
    to_signed(shift_decimal(value, shift)?, negative)
}

/// Splits `12.345` into its integer and fraction digits; a fraction is never empty.
fn split_fraction(mantissa: &str) -> Result<(&str, &str), ParseNumberError> {
    match mantissa.split_once('.') {
        Some((_, "")) => Err(ParseNumberError::Invalid),
        Some(parts) => Ok(parts),
        None => Ok((mantissa, "")),
    }
}

/// Parses the digits after `e`, with an optional sign. An exponent beyond `i64` is out of range.
fn parse_exponent(text: &str) -> Result<i64, ParseNumberError> {
    let (negative, digits) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text.strip_prefix('+').unwrap_or(text)),
    };
    if !all_digits(digits) {
        return Err(ParseNumberError::Invalid);
    }
    let mut value: i64 = 0;
    for digit in digits.chars().filter_map(|c| c.to_digit(10)) {
        let digit = i64::from(digit);
        // Accumulating with the final sign lets `-9223372036854775808` fit.
        value = value
            .checked_mul(10)
            .and_then(|v| {
                if negative {
                    v.checked_sub(digit)
                } else {
                    v.checked_add(digit)
                }
            })
            .ok_or(ParseNumberError::OutOfRange)?;
    }
    Ok(value)
}

fn all_digits(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit())
}

/// Digits only, or with `grouping` also comma groups: 1 to 3 digits, then groups of exactly 3.
fn valid_integer(text: &str, grouping: bool) -> bool {
    if !text.contains(',') {
        return all_digits(text);
    }
    if !grouping {
        return false;
    }
    let mut groups = text.split(',');
    groups
        .next()
        .is_some_and(|first| first.len() <= 3 && all_digits(first))
        && groups.all(|group| group.len() == 3 && all_digits(group))
}

fn len_i64(text: &str) -> Result<i64, ParseNumberError> {
    i64::try_from(text.len())
        .ok()
        .ok_or(ParseNumberError::OutOfRange)
}

/// Multiplies by `10^shift`; a negative shift divides and must be exact.
fn shift_decimal(mut value: u128, shift: i64) -> Result<u128, ParseNumberError> {
    if value == 0 {
        return Ok(0);
    }
    if shift >= 0 {
        for _ in 0..shift {
            value = value.checked_mul(10).ok_or(ParseNumberError::OutOfRange)?;
        }
        return Ok(value);
    }
    for _ in 0..shift.unsigned_abs() {
        match (value.checked_rem(10), value.checked_div(10)) {
            (Some(0), Some(quotient)) => value = quotient,
            _ => return Err(ParseNumberError::TooManyDecimals),
        }
    }
    Ok(value)
}

fn to_signed(magnitude: u128, negative: bool) -> Result<i64, ParseNumberError> {
    i128::try_from(magnitude)
        .ok()
        .and_then(|v| if negative { v.checked_neg() } else { Some(v) })
        .and_then(|v| i64::try_from(v).ok())
        .ok_or(ParseNumberError::OutOfRange)
}

/// `numerator / denominator` rounded half away from zero; `None` for a zero denominator.
pub(crate) fn div_round(numerator: i128, denominator: i128) -> Option<i128> {
    let quotient = numerator.checked_div(denominator)?;
    let remainder = numerator.checked_rem(denominator)?;
    if remainder.unsigned_abs().checked_mul(2)? < denominator.unsigned_abs() {
        return Some(quotient);
    }
    // Both are nonzero here, so the product of their signs is the direction away from zero.
    let away = numerator.signum().checked_mul(denominator.signum())?;
    quotient.checked_add(away)
}

/// The exact value of a count of 10^-4 units, without grouping or trailing decimal zeros.
pub(crate) fn plain(units: i64) -> String {
    let magnitude = units.unsigned_abs();
    let whole = magnitude.div_euclid(UNIT);
    let decimals = format!("{:04}", magnitude.rem_euclid(UNIT));
    let decimals = decimals.trim_end_matches('0');
    let sign = if units < 0 { "-" } else { "" };
    if decimals.is_empty() {
        format!("{sign}{whole}")
    } else {
        format!("{sign}{whole}.{decimals}")
    }
}

/// A count of 10^-4 units rounded to 0.01, grouped, with two decimals.
///
/// A value that rounds to zero has no sign.
pub(crate) fn fixed_2(units: i64) -> String {
    // Dividing by 100 cannot overflow and the sums stay below 2^63 / 100 + 1, so no step saturates.
    let mut hundredths = units.saturating_div(100);
    if (units % 100).unsigned_abs() >= 50 {
        hundredths = hundredths.saturating_add(units.signum());
    }
    let sign = if hundredths.is_negative() { "-" } else { "" };
    let whole = hundredths.saturating_div(100).saturating_abs();
    let cents = (hundredths % 100).unsigned_abs();
    format!("{sign}{}.{cents:02}", group_thousands(whole))
}

#[cfg(test)]
mod tests {
    use super::{
        Grammar, ParseNumberError, div_round, fixed_2, parse_exponent, parse_scaled, plain,
        shift_decimal, valid_integer,
    };

    const TYPED: Grammar = Grammar {
        negative: true,
        grouping: true,
        game_data: false,
    };
    const GAME: Grammar = Grammar {
        negative: true,
        grouping: false,
        game_data: true,
    };
    const BARE: Grammar = Grammar {
        negative: false,
        grouping: false,
        game_data: false,
    };

    #[test]
    fn errors_have_plain_sentences() {
        assert_eq!(ParseNumberError::Empty.to_string(), "The number is empty.");
        assert_eq!(
            ParseNumberError::Invalid.to_string(),
            "The text is not a valid number."
        );
        assert_eq!(
            ParseNumberError::TooManyDecimals.to_string(),
            "The number has more decimals than are supported."
        );
        assert_eq!(
            ParseNumberError::OutOfRange.to_string(),
            "The number is outside the supported range."
        );
        let error: &dyn std::error::Error = &ParseNumberError::Empty;
        assert_eq!(error.to_string(), "The number is empty.");
    }

    #[test]
    fn typed_numbers_parse_exactly() {
        assert_eq!(parse_scaled("0", TYPED), Ok(0));
        assert_eq!(parse_scaled("7", TYPED), Ok(70_000));
        assert_eq!(parse_scaled("0.0001", TYPED), Ok(1));
        assert_eq!(parse_scaled("12.3456", TYPED), Ok(123_456));
        assert_eq!(parse_scaled("-12.3456", TYPED), Ok(-123_456));
        assert_eq!(parse_scaled("1,000", TYPED), Ok(10_000_000));
        assert_eq!(parse_scaled("0001.5", TYPED), Ok(15_000));
    }

    #[test]
    fn typed_numbers_reject_bad_shapes() {
        assert_eq!(parse_scaled("", TYPED), Err(ParseNumberError::Empty));
        for text in [
            "-", ".", "1.", ".5", "1.2.3", "1,", ",1", "1,,000", "1e3", "1 2", "٣",
        ] {
            assert_eq!(
                parse_scaled(text, TYPED),
                Err(ParseNumberError::Invalid),
                "{text}"
            );
        }
        assert_eq!(
            parse_scaled("1.50000", TYPED),
            Err(ParseNumberError::TooManyDecimals)
        );
        assert_eq!(
            parse_scaled("1.00001", TYPED),
            Err(ParseNumberError::TooManyDecimals)
        );
    }

    #[test]
    fn the_sign_is_only_taken_when_the_grammar_allows_it() {
        assert_eq!(parse_scaled("-1", BARE), Err(ParseNumberError::Invalid));
        assert_eq!(parse_scaled("-1", GAME), Ok(-10_000));
        assert_eq!(parse_scaled("--1", GAME), Err(ParseNumberError::Invalid));
    }

    #[test]
    fn grouping_needs_the_grammar_and_the_right_group_sizes() {
        assert!(valid_integer("1,234,567", true));
        assert!(valid_integer("123,456", true));
        assert!(valid_integer("1234567", true));
        assert!(!valid_integer("1,234,567", false));
        assert!(!valid_integer("1234,567", true));
        assert!(!valid_integer("12,34", true));
        assert!(!valid_integer("1,2345", true));
        assert!(!valid_integer("", true));
        assert!(!valid_integer("", false));
    }

    #[test]
    fn game_data_allows_exponents_and_trailing_zeros() {
        assert_eq!(parse_scaled("1e3", GAME), Ok(10_000_000));
        assert_eq!(parse_scaled("1E+3", GAME), Ok(10_000_000));
        assert_eq!(parse_scaled("25e-4", GAME), Ok(25));
        assert_eq!(parse_scaled("0.010000", GAME), Ok(100));
        assert_eq!(parse_scaled("1.50000e1", GAME), Ok(150_000));
        assert_eq!(
            parse_scaled("0e99999999999999999999", GAME),
            Err(ParseNumberError::OutOfRange)
        );
        assert_eq!(
            parse_scaled("1e-99999999999999999999", GAME),
            Err(ParseNumberError::OutOfRange)
        );
        assert_eq!(
            parse_scaled("1e99999999999999999999", GAME),
            Err(ParseNumberError::OutOfRange)
        );
        assert_eq!(
            parse_scaled("1e9223372036854775807", GAME),
            Err(ParseNumberError::OutOfRange)
        );
        assert_eq!(
            parse_scaled("1e-9223372036854775808", GAME),
            Err(ParseNumberError::TooManyDecimals)
        );
        assert_eq!(
            parse_scaled("0.00001e-9223372036854775808", GAME),
            Err(ParseNumberError::OutOfRange)
        );
        // Leading fraction zeros and a large exponent cancel exactly.
        let long = format!("0.{}1e100006", "0".repeat(100_001));
        assert_eq!(parse_scaled(&long, GAME), Ok(100_000_000));
        for text in ["1e", "1e+", "1e-", "1e1.5", "e1", "1e1e1", "1ee1"] {
            assert_eq!(
                parse_scaled(text, GAME),
                Err(ParseNumberError::Invalid),
                "{text}"
            );
        }
        assert_eq!(parse_scaled("1e3", TYPED), Err(ParseNumberError::Invalid));
    }

    #[test]
    fn exponents_are_signed_and_checked() {
        assert_eq!(parse_exponent("12"), Ok(12));
        assert_eq!(parse_exponent("+12"), Ok(12));
        assert_eq!(parse_exponent("-12"), Ok(-12));
        assert_eq!(parse_exponent("100001"), Ok(100_001));
        assert_eq!(parse_exponent("9223372036854775807"), Ok(i64::MAX));
        assert_eq!(parse_exponent("-9223372036854775808"), Ok(i64::MIN));
        assert_eq!(
            parse_exponent("9223372036854775808"),
            Err(ParseNumberError::OutOfRange)
        );
        assert_eq!(
            parse_exponent("-99999999999999999999"),
            Err(ParseNumberError::OutOfRange)
        );
        assert_eq!(parse_exponent(""), Err(ParseNumberError::Invalid));
        assert_eq!(parse_exponent("-"), Err(ParseNumberError::Invalid));
    }

    #[test]
    fn the_whole_i64_range_parses() {
        assert_eq!(parse_scaled("922337203685477.5807", TYPED), Ok(i64::MAX));
        assert_eq!(parse_scaled("-922337203685477.5808", TYPED), Ok(i64::MIN));
        assert_eq!(
            parse_scaled("922337203685477.5808", TYPED),
            Err(ParseNumberError::OutOfRange)
        );
        assert_eq!(
            parse_scaled("-922337203685477.5809", TYPED),
            Err(ParseNumberError::OutOfRange)
        );
        let huge = "9".repeat(40);
        assert_eq!(
            parse_scaled(&huge, TYPED),
            Err(ParseNumberError::OutOfRange)
        );
        assert_eq!(
            parse_scaled("340282366920938463463374607431768211455", GAME),
            Err(ParseNumberError::OutOfRange)
        );
    }

    #[test]
    fn shifting_moves_the_decimal_point_exactly() {
        assert_eq!(shift_decimal(0, -5), Ok(0));
        assert_eq!(shift_decimal(0, 5), Ok(0));
        assert_eq!(shift_decimal(7, 0), Ok(7));
        assert_eq!(shift_decimal(7, 3), Ok(7000));
        assert_eq!(shift_decimal(7000, -3), Ok(7));
        assert_eq!(
            shift_decimal(7000, -4),
            Err(ParseNumberError::TooManyDecimals)
        );
        assert_eq!(shift_decimal(10, -1), Ok(1));
        assert_eq!(shift_decimal(5, -1), Err(ParseNumberError::TooManyDecimals));
        assert_eq!(shift_decimal(1, 39), Err(ParseNumberError::OutOfRange));
        assert_eq!(
            shift_decimal(1, 38),
            Ok(100_000_000_000_000_000_000_000_000_000_000_000_000)
        );
    }

    #[test]
    fn rounding_goes_half_away_from_zero() {
        assert_eq!(div_round(5, 2), Some(3));
        assert_eq!(div_round(-5, 2), Some(-3));
        assert_eq!(div_round(4, 2), Some(2));
        assert_eq!(div_round(7, 3), Some(2));
        assert_eq!(div_round(-7, 3), Some(-2));
        assert_eq!(div_round(1, 3), Some(0));
        assert_eq!(div_round(-1, 3), Some(0));
        assert_eq!(div_round(499_999, 1_000_000), Some(0));
        assert_eq!(div_round(500_000, 1_000_000), Some(1));
        assert_eq!(div_round(-500_000, 1_000_000), Some(-1));
        assert_eq!(div_round(5, -2), Some(-3));
        assert_eq!(div_round(-5, -2), Some(3));
        assert_eq!(div_round(3, 0), None);
        assert_eq!(div_round(i128::MIN, -1), None);
        assert_eq!(
            div_round(i128::MAX, 2),
            Some(i128::MAX.div_euclid(2).saturating_add(1))
        );
    }

    #[test]
    fn plain_text_is_exact_and_trimmed() {
        assert_eq!(plain(0), "0");
        assert_eq!(plain(1), "0.0001");
        assert_eq!(plain(-1), "-0.0001");
        assert_eq!(plain(21_250), "2.125");
        assert_eq!(plain(100_000), "10");
        assert_eq!(plain(100_001), "10.0001");
        assert_eq!(plain(1_000_000), "100");
        assert_eq!(plain(i64::MAX), "922337203685477.5807");
        assert_eq!(plain(i64::MIN), "-922337203685477.5808");
    }

    #[test]
    fn fixed_text_rounds_groups_and_never_shows_negative_zero() {
        assert_eq!(fixed_2(0), "0.00");
        assert_eq!(fixed_2(49), "0.00");
        assert_eq!(fixed_2(50), "0.01");
        assert_eq!(fixed_2(-49), "0.00");
        assert_eq!(fixed_2(-50), "-0.01");
        assert_eq!(fixed_2(51_000), "5.10");
        assert_eq!(fixed_2(21_250), "2.13");
        assert_eq!(fixed_2(10_000_000), "1,000.00");
        assert_eq!(fixed_2(-12_345_678), "-1,234.57");
        assert_eq!(fixed_2(i64::MAX), "922,337,203,685,477.58");
        assert_eq!(fixed_2(i64::MIN), "-922,337,203,685,477.58");
    }
}

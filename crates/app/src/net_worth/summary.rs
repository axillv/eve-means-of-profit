//! The headline numbers of the net worth points: the total, the change since the previous point
//! split into price moves and everything else, and the change over the chosen range.

use mop_core::money::Money;
use mop_core::price_history::day_of;

use super::data::ValuedPoint;

/// A signed change, with its size against the earlier total.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Change {
    /// The later total minus the earlier one.
    pub amount: Money,
    /// `amount` over the earlier total in hundredths of a percent: 7,272 is 72.72%. `None` when the
    /// earlier total is zero or below.
    pub percent: Option<i64>,
}

/// The change since the point before the last.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Since {
    /// When the point before the last was recorded.
    pub from_at: i64,
    /// The day of that point.
    pub from_day: i64,
    /// The total change from that point to the last.
    pub change: Change,
    /// The part of the change that repricing that point's holdings at the last point's prices
    /// accounts for.
    pub price_moves: Money,
    /// The rest: holdings added or removed, and the vault.
    pub everything_else: Money,
}

/// The change over all the points.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RangeChange {
    /// When the first point was recorded.
    pub from_at: i64,
    /// The total change from the first point to the last.
    pub change: Change,
}

/// The headline numbers of the points.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Summary {
    /// The last point's total.
    pub total: Money,
    /// The change since the point before the last, when that point's holdings were valued at the
    /// last point's prices.
    pub since: Option<Since>,
    /// The change over the range, with two or more points.
    pub over_range: Option<RangeChange>,
}

/// A headline number that does not fit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SummaryError {
    /// A difference or a percentage is outside `i64`.
    Overflow,
}

/// The headline numbers of `points`, oldest first.
///
/// `previous_at_latest_prices` is the point before the last valued at the last point's prices: with
/// it, [`Summary::since`] splits the change since that point into price moves and everything else.
/// Without points there is nothing to summarise.
pub fn summarise(
    points: &[ValuedPoint],
    previous_at_latest_prices: Option<&ValuedPoint>,
) -> Result<Option<Summary>, SummaryError> {
    let Some((last, before)) = points.split_last() else {
        return Ok(None);
    };

    let over_range = match before.first() {
        Some(first) => Some(RangeChange {
            from_at: first.value.at,
            change: change(first.total, last.total)?,
        }),
        None => None,
    };

    let since = match (before.last(), previous_at_latest_prices) {
        (Some(previous), Some(repriced)) => {
            let change = change(previous.total, last.total)?;
            let price_moves = repriced
                .total
                .checked_sub(previous.total)
                .ok_or(SummaryError::Overflow)?;
            let everything_else = last
                .total
                .checked_sub(repriced.total)
                .ok_or(SummaryError::Overflow)?;
            Some(Since {
                from_at: previous.value.at,
                from_day: day_of(previous.value.at),
                change,
                price_moves,
                everything_else,
            })
        }
        _ => None,
    };

    Ok(Some(Summary {
        total: last.total,
        since,
        over_range,
    }))
}

/// The change from `from` to `to`, and its size against `from`.
fn change(from: Money, to: Money) -> Result<Change, SummaryError> {
    let amount = to.checked_sub(from).ok_or(SummaryError::Overflow)?;
    Ok(Change {
        amount,
        percent: percent(amount, from)?,
    })
}

/// `amount` in hundredths of a percent of `base`, rounded half away from zero.
///
/// `None` when the base is zero or below: the size has no meaning then.
fn percent(amount: Money, base: Money) -> Result<Option<i64>, SummaryError> {
    let base = i128::from(base.ten_thousandths());
    if base <= 0 {
        return Ok(None);
    }
    let amount = i128::from(amount.ten_thousandths());
    let scaled = amount.checked_mul(10_000).ok_or(SummaryError::Overflow)?;
    let quotient = scaled / base;
    let remainder = scaled % base;
    let rounded = if remainder.unsigned_abs().saturating_mul(2) >= base.unsigned_abs() {
        // The remainder is nonzero here, so the direction away from zero is the amount's sign.
        quotient
            .checked_add(amount.signum())
            .ok_or(SummaryError::Overflow)?
    } else {
        quotient
    };
    match i64::try_from(rounded) {
        Ok(percent) => Ok(Some(percent)),
        Err(_) => Err(SummaryError::Overflow),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use mop_core::money::Money;
    use mop_core::net_worth::PointValue;

    use super::{Summary, SummaryError, summarise};
    use crate::net_worth::data::ValuedPoint;
    use crate::net_worth::data::pricer::PriceSource;

    fn isk(text: &str) -> Money {
        Money::parse_isk(text).expect("test amount")
    }

    /// A point with only its moment and total set.
    fn point(at: i64, total: Money) -> ValuedPoint {
        ValuedPoint {
            value: PointValue {
                at,
                by_part: BTreeMap::new(),
                by_character: BTreeMap::new(),
                total: Money::ZERO,
                unpriced: BTreeMap::new(),
                stale: Vec::new(),
            },
            plex_units: 0,
            plex_value: Money::ZERO,
            total,
            unpriced: BTreeMap::new(),
            source: PriceSource::Copy { published_at: None },
            members: BTreeMap::new(),
        }
    }

    fn summarise_two(first: Money, last: Money) -> Result<Option<Summary>, SummaryError> {
        summarise(&[point(0, first), point(86_400, last)], None)
    }

    #[test]
    fn the_owners_example_splits_the_change() {
        let previous = point(999_914_400, isk("10,600,000.00"));
        let last = point(1_000_000_000, isk("11,404,000.00"));
        let repriced = point(999_914_400, isk("10,792,000.00"));

        let summary = summarise(&[previous, last], Some(&repriced))
            .expect("summary")
            .expect("points");
        assert_eq!(summary.total, isk("11,404,000.00"));

        let since = summary.since.expect("since");
        assert_eq!(since.from_at, 999_914_400);
        assert_eq!(since.from_day, 11_573);
        assert_eq!(since.change.amount, isk("804,000.00"));
        assert_eq!(since.change.percent, Some(758));
        assert_eq!(since.price_moves, isk("192,000.00"));
        assert_eq!(since.everything_else, isk("612,000.00"));
        assert_eq!(
            since
                .price_moves
                .checked_add(since.everything_else)
                .expect("split"),
            since.change.amount
        );

        let range = summary.over_range.expect("range");
        assert_eq!(range.from_at, 999_914_400);
        assert_eq!(range.change, since.change);
    }

    #[test]
    fn the_range_runs_from_the_first_point() {
        let first = point(999_395_200, isk("10,600,000.00"));
        let previous = point(999_914_400, isk("10,000,000.00"));
        let last = point(1_000_000_000, isk("11,404,000.00"));
        let repriced = point(999_914_400, isk("10,300,000.00"));
        let points = [first, previous, last];

        let summary = summarise(&points, Some(&repriced))
            .expect("summary")
            .expect("points");
        let since = summary.since.expect("since");
        assert_eq!(since.from_day, 11_573);
        assert_eq!(since.change.amount, isk("1,404,000.00"));
        assert_eq!(since.change.percent, Some(1_404));
        assert_eq!(since.price_moves, isk("300,000.00"));
        assert_eq!(since.everything_else, isk("1,104,000.00"));

        let range = summary.over_range.expect("range");
        assert_eq!(range.from_at, 999_395_200);
        assert_eq!(range.change.amount, isk("804,000.00"));
        assert_eq!(range.change.percent, Some(758));

        // Without the repriced point there is no today split, but the range stays.
        let summary = summarise(&points, None).expect("summary").expect("points");
        assert!(summary.since.is_none());
        assert_eq!(summary.over_range, Some(range));
    }

    #[test]
    fn one_point_has_a_total_but_no_change() {
        let one = point(999_914_400, isk("10,600,000.00"));
        let repriced = point(999_914_400, isk("10,000,000.00"));

        let summary = summarise(&[one], Some(&repriced))
            .expect("summary")
            .expect("points");
        assert_eq!(summary.total, isk("10,600,000.00"));
        assert!(summary.since.is_none());
        assert!(summary.over_range.is_none());

        assert_eq!(summarise(&[], None), Ok(None));
    }

    #[test]
    fn the_percent_rounds_half_away_from_zero() {
        let round = |first: &str, last: &str| {
            let summary = summarise_two(isk(first), isk(last))
                .expect("summary")
                .expect("points");
            let change = summary.over_range.expect("range").change;
            (change.amount, change.percent)
        };

        assert_eq!(round("8.0000", "8.0004"), (isk("0.0004"), Some(1)));
        assert_eq!(round("8.0000", "7.9996"), (isk("-0.0004"), Some(-1)));
        assert_eq!(round("8.0000", "8.0003"), (isk("0.0003"), Some(0)));
        assert_eq!(
            round("10,600,000.00", "9,640,000.00"),
            (isk("-960,000.00"), Some(-906))
        );
    }

    #[test]
    fn a_base_of_zero_or_below_has_no_percent() {
        let summary = summarise_two(Money::ZERO, isk("960,000.00"))
            .expect("summary")
            .expect("points");
        let change = summary.over_range.expect("range").change;
        assert_eq!(change.amount, isk("960,000.00"));
        assert_eq!(change.percent, None);

        let summary = summarise_two(isk("-8.0000"), isk("8.0000"))
            .expect("summary")
            .expect("points");
        let change = summary.over_range.expect("range").change;
        assert_eq!(change.amount, isk("16.0000"));
        assert_eq!(change.percent, None);
    }

    #[test]
    fn numbers_that_do_not_fit_are_an_error() {
        let first = point(0, Money::from_ten_thousandths(1));
        let last = point(86_400, Money::from_ten_thousandths(i64::MAX));
        assert_eq!(summarise(&[first, last], None), Err(SummaryError::Overflow));

        let first = point(0, Money::from_ten_thousandths(i64::MIN));
        let last = point(86_400, Money::from_ten_thousandths(i64::MAX));
        assert_eq!(summarise(&[first, last], None), Err(SummaryError::Overflow));
    }
}

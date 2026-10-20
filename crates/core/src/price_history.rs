//! Which daily price readings a market keeps, and how much room its history takes.
//!
//! Days are counted from 1 January 1970 (day 0). Every day is kept for the daily period, then one
//! reading a week, optionally nothing older than a total limit.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::float_arithmetic,
    clippy::as_conversions,
    clippy::integer_division
)]

use std::fmt;

const SECS_PER_DAY: i64 = 86_400;

/// An estimate of the stored size of one price row, in bytes. Not measured.
pub const ROW_BYTES: u64 = 100;

/// The day a Unix time falls on.
pub fn day_of(unix_secs: i64) -> i64 {
    unix_secs.div_euclid(SECS_PER_DAY)
}

/// How long a market keeps its price readings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Retention {
    /// Days, counted back from today, during which every reading is kept.
    pub daily_days: u32,
    /// Days, counted back from today, after which no reading is kept; `None` keeps weekly readings forever.
    pub total_days: Option<u32>,
}

impl Retention {
    /// Every day for 90 days, then one reading a week, with no end.
    pub const DEFAULT: Retention = Retention {
        daily_days: 90,
        total_days: None,
    };

    /// A rule with at least one daily day, and a total limit that is not shorter than the daily period.
    pub fn new(daily_days: u32, total_days: Option<u32>) -> Result<Retention, RetentionError> {
        if daily_days == 0 {
            return Err(RetentionError::ZeroDaily);
        }
        if total_days.is_some_and(|total| total < daily_days) {
            return Err(RetentionError::TotalBelowDaily);
        }
        Ok(Retention {
            daily_days,
            total_days,
        })
    }
}

/// A retention rule or size estimate that cannot be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetentionError {
    /// The daily period is zero days.
    ZeroDaily,
    /// The total limit is shorter than the daily period.
    TotalBelowDaily,
    /// The size estimate does not fit in a `u64`.
    Overflow,
}

impl fmt::Display for RetentionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::ZeroDaily => "Keep at least one day of daily readings.",
            Self::TotalBelowDaily => "The total limit is shorter than the daily period.",
            Self::Overflow => "The size estimate is too large.",
        })
    }
}

impl std::error::Error for RetentionError {}

/// The days whose readings the rule drops, out of the days held.
///
/// `days` are the days held, in any order, repeats ignored. The result is ascending, without
/// repeats. A day whose age cannot be worked out without overflow is kept: deleting is the risky
/// step, so an unknown day is left alone.
pub fn days_to_drop(days: &[i64], today: i64, retention: &Retention) -> Vec<i64> {
    let mut held = days.to_vec();
    held.sort_unstable();
    held.dedup();

    let daily = i64::from(retention.daily_days);
    let total = retention.total_days.map(i64::from);
    let mut dropped = Vec::new();
    // The week of the earliest older reading seen so far; later readings in that week are dropped.
    let mut survivor_week: Option<i64> = None;

    for day in held {
        let Some(age) = today.checked_sub(day) else {
            continue;
        };
        if age < daily {
            continue;
        }
        let Some(week) = day.checked_add(3).map(|shifted| shifted.div_euclid(7)) else {
            continue;
        };
        let weekly_drop = if survivor_week == Some(week) {
            true
        } else {
            survivor_week = Some(week);
            false
        };
        let over_limit = total.is_some_and(|total| age >= total);
        if weekly_drop || over_limit {
            dropped.push(day);
        }
    }
    dropped
}

/// The readings a market that records every day holds after `age_days` days.
pub fn readings_kept(retention: &Retention, age_days: u32) -> u32 {
    let age = u64::from(age_days);
    let daily = u64::from(retention.daily_days);
    let mut older = days_past(age, daily);
    if let Some(total) = retention.total_days {
        older = older.min(days_past(u64::from(total), daily));
    }
    let weekly = older.div_ceil(7);
    // The sum is at most `age_days` (each weekly reading stands for at least one day), so it
    // cannot saturate and always fits in a `u32`.
    let kept = age.min(daily).saturating_add(weekly);
    u32::try_from(kept).unwrap_or(u32::MAX)
}

/// How many of `length` days lie past `period`: none when `length` is still inside it.
fn days_past(length: u64, period: u64) -> u64 {
    // Clamping at zero is the meaning here: a length inside the period has no days past it.
    length.saturating_sub(period)
}

/// The stored size of a history: rows per reading times readings times `ROW_BYTES`.
pub fn estimated_bytes(rows_per_reading: u64, readings: u64) -> Result<u64, RetentionError> {
    rows_per_reading
        .checked_mul(readings)
        .and_then(|rows| rows.checked_mul(ROW_BYTES))
        .ok_or(RetentionError::Overflow)
}

#[cfg(test)]
mod tests {
    use super::{Retention, RetentionError, day_of, days_to_drop, estimated_bytes, readings_kept};

    const THIN: Retention = Retention {
        daily_days: 30,
        total_days: None,
    };

    /// Weekly survivors for `THIN` on day 100: the earliest reading of each week.
    const THIN_SURVIVORS: [i64; 11] = [0, 4, 11, 18, 25, 32, 39, 46, 53, 60, 67];

    /// The days held after `days_to_drop`, from the held list (sorted, no repeats).
    fn kept_after(held: &[i64], today: i64, retention: &Retention) -> Vec<i64> {
        let dropped = days_to_drop(held, today, retention);
        held.iter()
            .copied()
            .filter(|day| !dropped.contains(day))
            .collect()
    }

    #[test]
    fn day_of_counts_whole_days_from_the_epoch() {
        assert_eq!(day_of(0), 0);
        assert_eq!(day_of(86_399), 0);
        assert_eq!(day_of(86_400), 1);
        assert_eq!(day_of(-1), -1);
    }

    #[test]
    fn weekly_thinning_keeps_one_reading_a_week_after_the_daily_period() {
        let held: Vec<i64> = (0..=100).collect();
        let dropped = days_to_drop(&held, 100, &THIN);
        let expected: Vec<i64> = (0..=70)
            .filter(|day| !THIN_SURVIVORS.contains(day))
            .collect();
        assert_eq!(dropped, expected);
        assert_eq!(dropped.len(), 60);
    }

    #[test]
    fn thinning_is_stable_when_run_again() {
        let mut held: Vec<i64> = THIN_SURVIVORS.to_vec();
        held.extend(71..=101);
        let dropped = days_to_drop(&held, 101, &THIN);
        assert_eq!(dropped, vec![71]);

        let remaining = kept_after(&held, 101, &THIN);
        assert_eq!(days_to_drop(&remaining, 101, &THIN), Vec::<i64>::new());
    }

    #[test]
    fn total_limit_drops_survivors_older_than_it() {
        let rule = Retention::new(30, Some(60)).expect("valid rule");
        let held: Vec<i64> = (0..=100).collect();
        let kept = kept_after(&held, 100, &rule);
        let mut expected = vec![46, 53, 60, 67];
        expected.extend(71..=100);
        assert_eq!(kept, expected);
        assert_eq!(days_to_drop(&held, 100, &rule).len(), 67);
    }

    #[test]
    fn default_rule_keeps_ninety_daily_days_then_weekly_readings() {
        let held: Vec<i64> = (0..=199).collect();
        let kept = kept_after(&held, 199, &Retention::DEFAULT);
        let mut expected = vec![
            0, 4, 11, 18, 25, 32, 39, 46, 53, 60, 67, 74, 81, 88, 95, 102, 109,
        ];
        expected.extend(110..=199);
        assert_eq!(kept, expected);
    }

    #[test]
    fn input_order_and_repeats_do_not_change_the_result() {
        let held: Vec<i64> = (0..=100).collect();
        let shuffled: Vec<i64> = held.iter().rev().chain(held.iter()).copied().collect();
        assert_eq!(
            days_to_drop(&shuffled, 100, &THIN),
            days_to_drop(&held, 100, &THIN)
        );
        assert_eq!(days_to_drop(&[], 100, &THIN), Vec::<i64>::new());
    }

    #[test]
    fn a_day_after_today_is_kept() {
        assert_eq!(days_to_drop(&[101], 100, &THIN), Vec::<i64>::new());
    }

    #[test]
    fn new_rejects_zero_daily_days_and_a_total_below_the_daily_period() {
        assert_eq!(Retention::new(0, None), Err(RetentionError::ZeroDaily));
        assert_eq!(
            Retention::new(30, Some(29)),
            Err(RetentionError::TotalBelowDaily)
        );
        assert_eq!(
            Retention::new(30, Some(30)),
            Ok(Retention {
                daily_days: 30,
                total_days: Some(30)
            })
        );
        assert_eq!(
            Retention::new(1, None),
            Ok(Retention {
                daily_days: 1,
                total_days: None
            })
        );
    }

    #[test]
    fn readings_kept_counts_the_steady_state_history() {
        assert_eq!(readings_kept(&Retention::DEFAULT, 30), 30);
        assert_eq!(readings_kept(&Retention::DEFAULT, 90), 90);
        assert_eq!(readings_kept(&Retention::DEFAULT, 365), 130);
        let limited = Retention::new(30, Some(60)).expect("valid rule");
        assert_eq!(readings_kept(&limited, 365), 35);
        assert_eq!(readings_kept(&Retention::DEFAULT, 0), 0);
    }

    #[test]
    fn retention_errors_say_what_is_wrong() {
        assert_eq!(
            RetentionError::ZeroDaily.to_string(),
            "Keep at least one day of daily readings."
        );
        assert_eq!(
            RetentionError::TotalBelowDaily.to_string(),
            "The total limit is shorter than the daily period."
        );
        assert_eq!(
            RetentionError::Overflow.to_string(),
            "The size estimate is too large."
        );
    }

    #[test]
    fn estimated_bytes_multiplies_rows_readings_and_row_size() {
        assert_eq!(estimated_bytes(35_817, 130), Ok(465_621_000));
        assert_eq!(estimated_bytes(u64::MAX, 2), Err(RetentionError::Overflow));
    }
}

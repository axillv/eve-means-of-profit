//! The Forge's daily sales history from ESI: units sold a day and the 30-day middle price.
//!
//! Days are whole days from 1970-01-01 and times are never read from the clock: the window ends at
//! the newest day ESI returned.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::as_conversions,
    clippy::integer_division
)]

use std::collections::BTreeMap;

use mop_core::industry::TypeId;
use mop_core::money::Money;
use serde::Deserialize;

use crate::error::MarketError;
use crate::fuzzwork::{BytesHttp, FORGE};

/// Calendar days a summary keeps: the window ending at the newest day.
pub const HISTORY_DAYS: i64 = 30;
/// The wait when ESI's rate-limit answer has no `retry-after`.
const DEFAULT_RETRY_SECS: u64 = 60;

/// One day of ESI's sales history for an item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryDay {
    /// Whole days from 1970-01-01.
    pub day: i64,
    pub average: Money,
    pub highest: Money,
    pub lowest: Money,
    pub volume: u64,
    pub orders: u64,
}

/// What the sales history says about an item: how much is selling and for how much.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DayStat {
    /// The newest day the history holds.
    pub last_day: i64,
    /// Units sold a day over the window, rounded down.
    pub units_day: u64,
    /// The median of the daily average prices in the window.
    pub middle: Money,
    /// Days in the window that have a price.
    pub days: u32,
}

/// The history fields we read. ESI's other fields are ignored.
#[derive(Deserialize)]
struct RawHistoryDay {
    average: serde_json::Number,
    date: String,
    highest: serde_json::Number,
    lowest: serde_json::Number,
    order_count: u64,
    volume: u64,
}

impl RawHistoryDay {
    fn into_day(self) -> Result<HistoryDay, MarketError> {
        let day = parse_date(&self.date)
            .ok_or_else(|| MarketError::Json(format!("unreadable history date {:?}", self.date)))?;
        Ok(HistoryDay {
            day,
            average: price(&self.average, "average")?,
            highest: price(&self.highest, "highest")?,
            lowest: price(&self.lowest, "lowest")?,
            volume: self.volume,
            orders: self.order_count,
        })
    }
}

/// Reads ESI's daily history list. The days come back oldest first; a repeated day is an error.
pub fn parse_history(json: &str) -> Result<Vec<HistoryDay>, MarketError> {
    let raw: Vec<RawHistoryDay> =
        serde_json::from_str(json).map_err(|error| MarketError::Json(error.to_string()))?;
    let mut days = BTreeMap::new();
    for raw in raw {
        let day = raw.into_day()?;
        if days.insert(day.day, day).is_some() {
            return Err(MarketError::Json("the history repeats a day".to_owned()));
        }
    }
    Ok(days.into_values().collect())
}

/// A price from one history number. Prices must be at least zero.
fn price(number: &serde_json::Number, field: &str) -> Result<Money, MarketError> {
    let text = number.to_string();
    let money = Money::parse_decimal_rounded(&text)
        .map_err(|error| MarketError::Json(format!("history {field} {text:?}: {error}")))?;
    if money < Money::ZERO {
        return Err(MarketError::Json(format!(
            "history {field} {text:?} is negative"
        )));
    }
    Ok(money)
}

/// A date written `YYYY-MM-DD` as whole days from 1970-01-01.
fn parse_date(text: &str) -> Option<i64> {
    let mut parts = text.split('-');
    let year = parts.next()?;
    let month = parts.next()?;
    let day = parts.next()?;
    if parts.next().is_some() || year.len() != 4 || month.len() != 2 || day.len() != 2 {
        return None;
    }
    let year = digits(year)?;
    let month = digits(month)?;
    let day = digits(day)?;
    let length = days_in_month(year, month)?;
    if day < 1 || day > length {
        return None;
    }
    days_from_epoch(year, month, day)
}

/// A non-empty run of ASCII digits.
fn digits(text: &str) -> Option<i64> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

fn days_in_month(year: i64, month: i64) -> Option<i64> {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => Some(31),
        4 | 6 | 9 | 11 => Some(30),
        2 => Some(if is_leap(year) { 29 } else { 28 }),
        _ => None,
    }
}

fn is_leap(year: i64) -> bool {
    let by_four = year.checked_rem(4) == Some(0);
    let by_hundred = year.checked_rem(100) == Some(0);
    let by_four_hundred = year.checked_rem(400) == Some(0);
    by_four && (!by_hundred || by_four_hundred)
}

/// The days from 1970-01-01 to `year-month-day` in the proleptic Gregorian calendar.
fn days_from_epoch(year: i64, month: i64, day: i64) -> Option<i64> {
    let y = if month <= 2 {
        year.checked_sub(1)?
    } else {
        year
    };
    let era = y.checked_div(400)?;
    let yoe = y.checked_sub(era.checked_mul(400)?)?;
    let mp = if month > 2 {
        month.checked_sub(3)?
    } else {
        month.checked_add(9)?
    };
    let doy = mp
        .checked_mul(153)?
        .checked_add(2)?
        .checked_div(5)?
        .checked_add(day)?
        .checked_sub(1)?;
    let doe = yoe
        .checked_mul(365)?
        .checked_add(yoe.checked_div(4)?)?
        .checked_sub(yoe.checked_div(100)?)?
        .checked_add(doy)?;
    era.checked_mul(146_097)?
        .checked_add(doe)?
        .checked_sub(719_468)
}

/// Summarises a history: units sold a day and the middle price over the 30 days ending at its
/// newest day. Days missing from the history count as zero sales and have no price in the middle.
pub fn summarise(days: &[HistoryDay]) -> Option<DayStat> {
    let last_day = days.iter().map(|day| day.day).max()?;
    let window_days = HISTORY_DAYS.checked_sub(1)?;
    let first_day = last_day.checked_sub(window_days)?;
    let mut volume: u64 = 0;
    let mut averages = Vec::new();
    for day in days {
        if day.day < first_day || day.day > last_day {
            continue;
        }
        volume = volume.checked_add(day.volume)?;
        averages.push(day.average);
    }
    let window = u64::try_from(HISTORY_DAYS).ok()?;
    let units_day = volume.checked_div(window)?;
    let middle = median(&mut averages)?;
    Some(DayStat {
        last_day,
        units_day,
        middle,
        days: u32::try_from(averages.len()).ok()?,
    })
}

/// The median of the prices, an even count averaged half away from zero.
fn median(prices: &mut [Money]) -> Option<Money> {
    if prices.is_empty() {
        return None;
    }
    prices.sort_unstable();
    let count = prices.len();
    let half = count.checked_div(2)?;
    if count.checked_rem(2)? == 1 {
        prices.get(half).copied()
    } else {
        let upper = prices.get(half).copied()?;
        let lower = prices.get(half.checked_sub(1)?).copied()?;
        let sum =
            i128::from(lower.ten_thousandths()).checked_add(i128::from(upper.ten_thousandths()))?;
        let rounded = if sum < 0 {
            sum.checked_sub(1)?.checked_div(2)?
        } else {
            sum.checked_add(1)?.checked_div(2)?
        };
        Some(Money::from_ten_thousandths(i64::try_from(rounded).ok()?))
    }
}

/// The Forge's daily sales history through ESI.
pub struct History<H: BytesHttp> {
    http: H,
    region: u32,
}

impl<H: BytesHttp> History<H> {
    pub fn new(http: H, region: u32) -> Self {
        Self { http, region }
    }

    /// The history of The Forge (region 10000002).
    pub fn forge(http: H) -> Self {
        Self::new(http, FORGE)
    }

    /// Fetches one item's daily history. Nothing is cached and no clock is read.
    pub fn fetch(&self, type_id: TypeId) -> Result<Vec<HistoryDay>, MarketError> {
        let url = format!(
            "https://esi.evetech.net/markets/{}/history/?type_id={type_id}",
            self.region
        );
        let response = self.http.get_bytes(&url, None)?;
        match response.status {
            200 => {
                let text = String::from_utf8(response.body)
                    .map_err(|error| MarketError::Json(error.to_string()))?;
                parse_history(&text)
            }
            420 | 429 => Err(MarketError::RateLimited {
                retry_after_secs: response.retry_after.unwrap_or(DEFAULT_RETRY_SECS),
            }),
            status => Err(MarketError::Http { status }),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use crate::fuzzwork::BytesResponse;

    use super::*;

    struct Fake {
        response: BytesResponse,
        calls: RefCell<Vec<(String, Option<String>)>>,
    }

    impl Fake {
        fn new(status: u16, retry_after: Option<u64>, body: &str) -> Self {
            Self {
                response: BytesResponse {
                    status,
                    etag: None,
                    last_modified: None,
                    retry_after,
                    body: body.as_bytes().to_vec(),
                },
                calls: RefCell::new(Vec::new()),
            }
        }
    }

    impl BytesHttp for Fake {
        fn get_bytes(
            &self,
            url: &str,
            if_none_match: Option<&str>,
        ) -> Result<BytesResponse, MarketError> {
            self.calls
                .borrow_mut()
                .push((url.to_owned(), if_none_match.map(str::to_owned)));
            Ok(self.response.clone())
        }
    }

    fn m(count: i64) -> Money {
        Money::from_ten_thousandths(count)
    }

    fn day(day: i64, average: i64, volume: u64) -> HistoryDay {
        HistoryDay {
            day,
            average: m(average),
            highest: m(average),
            lowest: m(average),
            volume,
            orders: 1,
        }
    }

    const TWO_DAYS: &str = r#"[
        {"average":10.00005,"date":"2026-01-01","highest":12.0,"lowest":9.0,"order_count":7,"volume":10},
        {"average":11.25,"date":"2026-01-02","highest":13.0,"lowest":10.0,"order_count":8,"volume":20}
    ]"#;

    #[test]
    fn parse_history_reads_the_days_in_order() {
        let days = parse_history(TWO_DAYS).expect("parse");
        assert_eq!(days.len(), 2);
        assert_eq!(days[0].day, 20_454);
        assert_eq!(days[1].day, 20_455);
        assert_eq!(days[0].average, m(100_001));
        assert_eq!(days[0].highest, m(120_000));
        assert_eq!(days[0].lowest, m(90_000));
        assert_eq!(days[0].volume, 10);
        assert_eq!(days[0].orders, 7);
    }

    #[test]
    fn parse_history_refuses_bad_days_and_prices() {
        for json in [
            r#"[{"average":1.0,"date":"2026-02-30","highest":2.0,"lowest":1.0,"order_count":1,"volume":1}]"#,
            r#"[{"average":-1,"date":"2026-01-01","highest":2.0,"lowest":1.0,"order_count":1,"volume":1}]"#,
            "[",
        ] {
            assert!(
                matches!(parse_history(json), Err(MarketError::Json(_))),
                "{json}"
            );
        }
        let repeated = r#"[
            {"average":1.0,"date":"2026-01-01","highest":2.0,"lowest":1.0,"order_count":1,"volume":1},
            {"average":2.0,"date":"2026-01-01","highest":2.0,"lowest":1.0,"order_count":1,"volume":1}
        ]"#;
        assert!(matches!(parse_history(repeated), Err(MarketError::Json(_))));
    }

    #[test]
    fn fetch_asks_esi_without_a_cache_header() {
        let fake = Fake::new(200, None, TWO_DAYS);
        let history = History::new(fake, 10_000_002);
        assert_eq!(history.fetch(34).expect("fetch").len(), 2);
        let calls = history.http.calls.borrow();
        assert_eq!(calls.len(), 1);
        assert_eq!(
            calls[0].0,
            "https://esi.evetech.net/markets/10000002/history/?type_id=34"
        );
        assert_eq!(calls[0].1, None);
    }

    #[test]
    fn fetch_reports_rate_limits_and_other_statuses() {
        let limited = Fake::new(429, Some(30), "");
        let err = History::forge(limited).fetch(34).expect_err("limited");
        assert_eq!(
            err,
            MarketError::RateLimited {
                retry_after_secs: 30
            }
        );

        let default_wait = Fake::new(420, None, "");
        let err = History::forge(default_wait).fetch(34).expect_err("limited");
        assert_eq!(
            err,
            MarketError::RateLimited {
                retry_after_secs: 60
            }
        );

        let failed = Fake::new(503, None, "");
        let err = History::forge(failed).fetch(34).expect_err("failed");
        assert_eq!(err, MarketError::Http { status: 503 });
    }

    #[test]
    fn summarise_uses_the_30_days_ending_at_the_newest() {
        let days = vec![
            day(20_454, 100_000, 10),
            day(20_455, 120_000, 20),
            day(20_456, 200_000, 30),
            day(20_457, 300_000, 40),
        ];
        assert_eq!(
            summarise(&days),
            Some(DayStat {
                last_day: 20_457,
                units_day: 3,
                middle: m(160_000),
                days: 4,
            })
        );

        let mut full = (20_455..=20_484)
            .map(|value| day(value, 100_000, 100))
            .collect::<Vec<_>>();
        full.push(day(20_454, 9_990_000, 100_000));
        assert_eq!(
            summarise(&full),
            Some(DayStat {
                last_day: 20_484,
                units_day: 100,
                middle: m(100_000),
                days: 30,
            })
        );

        let sparse = vec![day(20_454, 1_000_000, 30), day(20_484, 200_000, 60)];
        assert_eq!(
            summarise(&sparse),
            Some(DayStat {
                last_day: 20_484,
                units_day: 2,
                middle: m(200_000),
                days: 1,
            })
        );
        assert_eq!(summarise(&[]), None);
    }

    #[test]
    fn the_even_middle_rounds_half_away_from_zero() {
        let days = vec![day(20_484, 10_000, 1), day(20_485, 10_001, 1)];
        assert_eq!(summarise(&days).expect("stat").middle, m(10_001));
    }

    #[test]
    fn parse_history_accepts_zero_prices() {
        let zero = r#"[{"average":0,"date":"2026-01-01","highest":0,"lowest":0,"order_count":1,"volume":0}]"#;
        let days = parse_history(zero).expect("parse");
        assert_eq!(days.len(), 1);
        assert_eq!(days[0].day, 20_454);
        assert_eq!(days[0].average, m(0));
        assert_eq!(days[0].highest, m(0));
        assert_eq!(days[0].lowest, m(0));
    }

    #[test]
    fn parse_date_needs_three_parts_and_two_digit_month_and_day() {
        assert_eq!(parse_date("2026-01-01-05"), None);
        assert_eq!(parse_date("2026-1-01"), None);
        assert_eq!(parse_date("2026-01-1"), None);
        assert_eq!(parse_date("2026-01-31"), Some(20_484));
    }

    #[test]
    fn digits_refuses_a_sign() {
        assert_eq!(digits("+5"), None);
        assert_eq!(digits(""), None);
        assert_eq!(digits("5"), Some(5));
    }

    #[test]
    fn parse_date_keeps_month_ends_and_leap_days() {
        assert_eq!(parse_date("2026-02-01"), Some(20_485));
        assert_eq!(parse_date("2026-02-28"), Some(20_512));
        assert_eq!(parse_date("2026-04-30"), Some(20_573));
        assert_eq!(parse_date("2026-06-30"), Some(20_634));
        assert_eq!(parse_date("2026-09-30"), Some(20_726));
        assert_eq!(parse_date("2026-11-30"), Some(20_787));
        assert_eq!(parse_date("2024-02-29"), Some(19_782));
        assert_eq!(parse_date("2000-02-29"), Some(11_016));
        assert_eq!(parse_date("2026-02-29"), None);
        assert_eq!(parse_date("1900-02-29"), None);
    }

    #[test]
    fn the_even_middle_of_negative_prices_rounds_half_away_from_zero() {
        let days = vec![day(20_484, -3_000, 1), day(20_485, -2_000, 1)];
        assert_eq!(summarise(&days).expect("stat").middle, m(-2_500));
    }
}

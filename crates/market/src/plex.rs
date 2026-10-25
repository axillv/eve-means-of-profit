//! PLEX's price from the one global market. PLEX trades in region 19000001 only, not in The Forge,
//! so the Fuzzwork file has no price for it and the order list is read here instead.
//!
//! No clock and no cache: callers fetch at each refresh and keep the result themselves.

#![deny(
    clippy::arithmetic_side_effects,
    clippy::as_conversions,
    clippy::integer_division
)]

use std::collections::BTreeMap;

use mop_core::industry::TypeId;
use mop_core::money::Money;

use crate::book::{Order, parse_orders};
use crate::cache::{Http, HttpResponse};
use crate::error::MarketError;
use crate::fuzzwork::{Side, SummaryRow};

/// PLEX's type id.
pub const PLEX: TypeId = 44_992;
/// The one global market PLEX trades on.
pub const GLOBAL_PLEX_REGION: u64 = 19_000_001;
/// The wait when ESI's rate-limit answer has no `retry-after`.
const DEFAULT_RETRY_SECS: u64 = 60;
/// Most pages read. A larger `x-pages` is treated as a broken header.
const MAX_PAGES: u32 = 100;

/// The order list for one type in the global region, one page at a time.
fn page_url(page: u32) -> String {
    format!(
        "https://esi.evetech.net/markets/{GLOBAL_PLEX_REGION}/orders/?type_id={PLEX}&order_type=all&page={page}"
    )
}

/// Reads PLEX's orders from every page the first page names. Any error gives no summary.
pub fn fetch_plex(http: &dyn Http) -> Result<BTreeMap<Side, SummaryRow>, MarketError> {
    let first = get_page(http, 1)?;
    let pages = first.pages.unwrap_or(1);
    if pages > MAX_PAGES {
        return Err(MarketError::BadHeader {
            name: "x-pages",
            value: pages.to_string(),
        });
    }
    let mut orders = plex_only(parse_orders(&first.body)?);
    for page in 2..=pages {
        let next = get_page(http, page)?;
        orders.extend(plex_only(parse_orders(&next.body)?));
    }
    summarise(&orders)
}

fn plex_only(orders: Vec<Order>) -> Vec<Order> {
    orders.into_iter().filter(|o| o.type_id == PLEX).collect()
}

/// One GET for a page. 200 is read; a rate limit becomes `RateLimited`; any other status is `Http`.
fn get_page(http: &dyn Http, page: u32) -> Result<HttpResponse, MarketError> {
    let response = http.get(&page_url(page), None)?;
    match response.status {
        200 => Ok(response),
        420 | 429 => Err(MarketError::RateLimited {
            retry_after_secs: response.retry_after.unwrap_or(DEFAULT_RETRY_SECS),
        }),
        status => Err(MarketError::Http { status }),
    }
}

/// The summary of each side. Walks each side from its best price; a side with no units is absent.
pub fn summarise(orders: &[Order]) -> Result<BTreeMap<Side, SummaryRow>, MarketError> {
    let mut out = BTreeMap::new();
    for (side, is_buy) in [(Side::Sell, false), (Side::Buy, true)] {
        let mut walk: Vec<&Order> = orders
            .iter()
            .filter(|o| o.is_buy == is_buy && o.volume_remain > 0)
            .collect();
        walk.sort_by(|a, b| {
            let by_price = if is_buy {
                b.price.cmp(&a.price)
            } else {
                a.price.cmp(&b.price)
            };
            by_price.then(a.order_id.cmp(&b.order_id))
        });
        if let Some(row) = summarise_side(&walk)? {
            out.insert(side, row);
        }
    }
    Ok(out)
}

/// One side's summary, from its orders already in walk order. `None` when there are no units.
fn summarise_side(walk: &[&Order]) -> Result<Option<SummaryRow>, MarketError> {
    let Some(first) = walk.first() else {
        return Ok(None);
    };
    let volume = walk
        .iter()
        .try_fold(0u64, |sum, o| sum.checked_add(o.volume_remain))
        .ok_or(MarketError::Overflow)?;
    let orders = u32::try_from(walk.len())
        .ok()
        .ok_or(MarketError::Overflow)?;
    let (total, _) = take_units(walk, volume)?;
    let five_units = volume
        .checked_mul(5)
        .ok_or(MarketError::Overflow)?
        .div_ceil(100);
    let (five_sum, five_taken) = take_units(walk, five_units)?;
    let min = walk.iter().map(|o| o.price).min().unwrap_or(first.price);
    let max = walk.iter().map(|o| o.price).max().unwrap_or(first.price);
    Ok(Some(SummaryRow {
        min,
        max,
        weighted_average: average(total, volume)?,
        median: price_at_unit(walk, volume.div_ceil(2))?,
        five_percent: average(five_sum, five_taken)?,
        volume,
        orders,
    }))
}

/// The sum of price × units over the first `units` units in walk order, and how many units that was.
fn take_units(walk: &[&Order], units: u64) -> Result<(i128, u64), MarketError> {
    let mut left = units;
    let mut sum: i128 = 0;
    let mut taken: u64 = 0;
    for order in walk {
        if left == 0 {
            break;
        }
        let here = order.volume_remain.min(left);
        let value = i128::from(order.price.ten_thousandths())
            .checked_mul(i128::from(here))
            .ok_or(MarketError::Overflow)?;
        sum = sum.checked_add(value).ok_or(MarketError::Overflow)?;
        taken = taken.checked_add(here).ok_or(MarketError::Overflow)?;
        left = left.checked_sub(here).ok_or(MarketError::Overflow)?;
    }
    Ok((sum, taken))
}

/// The price of the unit at 1-based `position` in walk order. The caller keeps `position` within the volume.
fn price_at_unit(walk: &[&Order], position: u64) -> Result<Money, MarketError> {
    let mut seen: u64 = 0;
    for order in walk {
        seen = seen
            .checked_add(order.volume_remain)
            .ok_or(MarketError::Overflow)?;
        if seen >= position {
            return Ok(order.price);
        }
    }
    Err(MarketError::Overflow)
}

/// `sum / units` rounded half away from zero to whole ten-thousandths. Zero units gives `Overflow`.
fn average(sum: i128, units: u64) -> Result<Money, MarketError> {
    let den = i128::from(units);
    let magnitude = sum.unsigned_abs();
    let rounded = round_half_up(magnitude, den)?;
    let signed = if sum < 0 {
        rounded.checked_neg().ok_or(MarketError::Overflow)?
    } else {
        rounded
    };
    let count = i64::try_from(signed).ok().ok_or(MarketError::Overflow)?;
    Ok(Money::from_ten_thousandths(count))
}

fn round_half_up(magnitude: u128, den: i128) -> Result<i128, MarketError> {
    let den = u128::try_from(den).ok().ok_or(MarketError::Overflow)?;
    let numer = magnitude
        .checked_mul(2)
        .and_then(|x| x.checked_add(den))
        .ok_or(MarketError::Overflow)?;
    let denom = den.checked_mul(2).ok_or(MarketError::Overflow)?;
    let q = numer.checked_div(denom).ok_or(MarketError::Overflow)?;
    i128::try_from(q).ok().ok_or(MarketError::Overflow)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    fn order(id: u64, is_buy: bool, price: &str, units: u64) -> Order {
        Order {
            order_id: id,
            type_id: PLEX,
            location_id: 60_000_001,
            is_buy,
            price: money(price),
            volume_remain: units,
        }
    }

    fn money(text: &str) -> Money {
        Money::parse_decimal(text).expect("test price")
    }

    /// A fake ESI: answers each page URL from a table and records every URL asked.
    struct Fake {
        answers: Vec<(String, HttpResponse)>,
        asked: RefCell<Vec<String>>,
    }

    impl Fake {
        fn new(answers: Vec<(String, HttpResponse)>) -> Self {
            Self {
                answers,
                asked: RefCell::new(Vec::new()),
            }
        }
    }

    impl Http for Fake {
        fn get(
            &self,
            url: &str,
            _if_none_match: Option<&str>,
        ) -> Result<HttpResponse, MarketError> {
            self.asked.borrow_mut().push(url.to_owned());
            self.answers
                .iter()
                .find(|(u, _)| u == url)
                .map(|(_, r)| r.clone())
                .ok_or_else(|| MarketError::Network(format!("no answer for {url}")))
        }
    }

    fn page(status: u16, pages: Option<u32>, body: &str) -> HttpResponse {
        HttpResponse {
            status,
            pages,
            body: body.to_owned(),
            ..HttpResponse::default()
        }
    }

    fn json(id: u64, is_buy: bool, price: &str, units: u64) -> String {
        format!(
            r#"{{"order_id":{id},"type_id":44992,"location_id":60000001,"is_buy_order":{is_buy},"price":{price},"volume_remain":{units}}}"#
        )
    }

    #[test]
    fn sell_side_summary() {
        let orders = [
            order(1, false, "5200000.00", 30),
            order(2, false, "5250000.00", 50),
            order(3, false, "5400000.00", 20),
        ];
        let got = summarise(&orders).expect("summary");
        let sell = &got[&Side::Sell];
        assert_eq!(sell.volume, 100);
        assert_eq!(sell.orders, 3);
        assert_eq!(sell.min, money("5200000.00"));
        assert_eq!(sell.max, money("5400000.00"));
        assert_eq!(sell.weighted_average, money("5265000.00"));
        assert_eq!(sell.median, money("5250000.00"));
        assert_eq!(sell.five_percent, money("5200000.00"));
        assert!(!got.contains_key(&Side::Buy));
    }

    #[test]
    fn five_percent_rounds_units_up_and_buy_side_walks_high_first() {
        let sell = summarise(&[
            order(1, false, "5200000.00", 3),
            order(2, false, "5250000.00", 70),
        ])
        .expect("summary");
        assert_eq!(sell[&Side::Sell].five_percent, money("5212500.00"));

        let buy = summarise(&[
            order(3, true, "5100000.00", 10),
            order(4, true, "5000000.00", 30),
        ])
        .expect("summary");
        let row = &buy[&Side::Buy];
        assert_eq!(row.volume, 40);
        assert_eq!(row.weighted_average, money("5025000.00"));
        assert_eq!(row.median, money("5000000.00"));
        assert_eq!(row.five_percent, money("5100000.00"));
    }

    #[test]
    fn rounding_is_half_away_from_zero() {
        let down = summarise(&[order(1, false, "1.0001", 1), order(2, false, "1.0000", 2)])
            .expect("summary");
        assert_eq!(down[&Side::Sell].weighted_average, money("1.0000"));

        let half = summarise(&[order(3, false, "1.0002", 1), order(4, false, "1.0001", 1)])
            .expect("summary");
        assert_eq!(half[&Side::Sell].weighted_average, money("1.0002"));
    }

    #[test]
    fn zero_unit_orders_are_ignored() {
        let got = summarise(&[order(1, false, "2.0000", 0)]).expect("summary");
        assert!(got.is_empty());
    }

    #[test]
    fn fetch_reads_every_page() {
        let fake = Fake::new(vec![
            (
                page_url(1),
                page(
                    200,
                    Some(2),
                    &format!("[{}]", json(1, false, "5200000.00", 30)),
                ),
            ),
            (
                page_url(2),
                page(
                    200,
                    None,
                    &format!("[{}]", json(2, false, "5250000.00", 70)),
                ),
            ),
        ]);
        let got = fetch_plex(&fake).expect("fetch");
        assert_eq!(got[&Side::Sell].volume, 100);
        assert_eq!(got[&Side::Sell].orders, 2);
        assert_eq!(
            *fake.asked.borrow(),
            vec![
                "https://esi.evetech.net/markets/19000001/orders/?type_id=44992&order_type=all&page=1",
                "https://esi.evetech.net/markets/19000001/orders/?type_id=44992&order_type=all&page=2",
            ]
        );
    }

    #[test]
    fn fetch_errors_give_no_summary() {
        let fail_page_2 = Fake::new(vec![
            (page_url(1), page(200, Some(2), "[]")),
            (page_url(2), page(503, None, "")),
        ]);
        assert_eq!(
            fetch_plex(&fail_page_2),
            Err(MarketError::Http { status: 503 })
        );

        let limited = Fake::new(vec![(
            page_url(1),
            HttpResponse {
                status: 420,
                retry_after: Some(30),
                ..HttpResponse::default()
            },
        )]);
        assert_eq!(
            fetch_plex(&limited),
            Err(MarketError::RateLimited {
                retry_after_secs: 30
            })
        );

        let too_many = Fake::new(vec![(page_url(1), page(200, Some(101), "[]"))]);
        assert_eq!(
            fetch_plex(&too_many),
            Err(MarketError::BadHeader {
                name: "x-pages",
                value: "101".to_owned(),
            })
        );
    }
}

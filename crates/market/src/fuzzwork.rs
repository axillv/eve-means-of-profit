//! Summary prices from Fuzzwork's market file: one download holds the cheapest, 5%, weighted average,
//! middle and highest prices of every item in every region, read here for one region.
//!
//! Times are Unix seconds and always passed in by the caller, so this module never reads the clock.

#![deny(
    clippy::arithmetic_side_effects,
    clippy::as_conversions,
    clippy::integer_division
)]

use std::collections::BTreeMap;
use std::io::Read;
use std::time::UNIX_EPOCH;

use flate2::read::GzDecoder;
use mop_core::industry::TypeId;
use mop_core::money::Money;

use crate::error::MarketError;

/// The Forge (region 10000002), the region whose prices this module reads by default.
pub const FORGE: u32 = 10_000_002;
/// The whole-region aggregates, gzip-compressed CSV.
const AGGREGATES_URL: &str = "https://market.fuzzwork.co.uk/aggregatecsv.csv.gz";
/// The first line of the file. Any other header means the columns changed and nothing is read.
const HEADER: &str =
    "what,weightedaverage,maxval,minval,stddev,median,volume,numorders,fivepercent,orderSet";
/// The least time between two requests that got an answer (200 or 304). A convention, not Fuzzwork's.
pub const MIN_GAP_SECS: i64 = 900;
/// The wait after a failure other than a rate limit, before asking again.
const FAILURE_WAIT_SECS: u64 = 60;
/// The wait when a rate-limit answer has no `retry-after`.
const DEFAULT_RETRY_SECS: u64 = 60;
/// Largest unzipped file read. A convention, not Fuzzwork's.
const MAX_DECODED: u64 = 256 * 1024 * 1024;

/// Which side of the book a price is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Side {
    /// Sell orders: what a buyer pays.
    Sell,
    /// Buy orders: what a seller is paid.
    Buy,
}

/// Which summary price to read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stat {
    /// The cheapest sell price, or the highest buy price.
    Best,
    /// The price at which the cheapest 5% of the volume is sold, or bought.
    FivePercent,
    /// The average price weighted by volume.
    WeightedAverage,
    /// The middle price.
    Median,
}

/// One item's summary on one side of The Forge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SummaryRow {
    /// The lowest price.
    pub min: Money,
    /// The highest price.
    pub max: Money,
    pub weighted_average: Money,
    pub median: Money,
    pub five_percent: Money,
    /// Units on the side.
    pub volume: u64,
    /// Orders on the side.
    pub orders: u32,
}

impl SummaryRow {
    /// The price for `stat`. `Best` is the cheapest on the sell side and the highest on the buy side.
    pub fn price(&self, side: Side, stat: Stat) -> Money {
        match stat {
            Stat::Best => match side {
                Side::Sell => self.min,
                Side::Buy => self.max,
            },
            Stat::FivePercent => self.five_percent,
            Stat::WeightedAverage => self.weighted_average,
            Stat::Median => self.median,
        }
    }
}

/// The answer to one GET of the file, as the caller needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BytesResponse {
    pub status: u16,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    /// `retry-after` in seconds, when present and readable.
    pub retry_after: Option<u64>,
    pub body: Vec<u8>,
}

/// Sends a GET and reads the whole body. The real one is [`crate::http::UreqHttp`]; tests pass fakes.
pub trait BytesHttp {
    /// `if_none_match` is sent as the `If-None-Match` header when present.
    fn get_bytes(
        &self,
        url: &str,
        if_none_match: Option<&str>,
    ) -> Result<BytesResponse, MarketError>;
}

/// What a refresh did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refresh {
    /// The file was downloaded and replaces the rows held.
    Downloaded,
    /// Fuzzwork said the file has not changed; the rows held are kept.
    NotModified,
    /// No request was made because the last answer is less than [`MIN_GAP_SECS`] old.
    TooSoon { next_at: i64 },
}

/// A wait after a failure: no request before `until`, and the same error answered meanwhile.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Hold {
    until: i64,
    error: MarketError,
}

/// The rows of one region, by item and side.
type Rows = BTreeMap<(TypeId, Side), SummaryRow>;

/// The summary prices of one region, from the last download that was read.
pub struct FuzzworkMarket<H: BytesHttp> {
    http: H,
    region: u32,
    rows: Rows,
    etag: Option<String>,
    published_at: Option<i64>,
    order_set: Option<u64>,
    /// When the last request that got an answer (200 or 304) was made.
    answered_at: Option<i64>,
    hold: Option<Hold>,
}

impl<H: BytesHttp> FuzzworkMarket<H> {
    pub fn new(http: H, region: u32) -> Self {
        Self {
            http,
            region,
            rows: BTreeMap::new(),
            etag: None,
            published_at: None,
            order_set: None,
            answered_at: None,
            hold: None,
        }
    }

    /// Downloads the file when the held copy may be out of date. A failure keeps the rows held.
    pub fn refresh(&mut self, now: i64) -> Result<Refresh, MarketError> {
        if let Some(hold) = &self.hold
            && now < hold.until
        {
            return Err(hold.error.clone());
        }
        if let Some(next_at) = self.next_answer_due()
            && now < next_at
        {
            return Ok(Refresh::TooSoon { next_at });
        }
        self.request(now).inspect_err(|error| {
            let wait = if let MarketError::RateLimited { retry_after_secs } = error {
                *retry_after_secs
            } else {
                FAILURE_WAIT_SECS
            };
            let until = i64::try_from(wait)
                .ok()
                .and_then(|secs| now.checked_add(secs))
                .unwrap_or(i64::MAX);
            self.hold = Some(Hold {
                until,
                error: error.clone(),
            });
        })
    }

    /// The row for an item on one side, when it has orders there.
    pub fn row(&self, type_id: TypeId, side: Side) -> Option<&SummaryRow> {
        self.rows.get(&(type_id, side))
    }

    /// One summary price of an item on one side. `None` when the item has no row on that side.
    pub fn price(&self, type_id: TypeId, side: Side, stat: Stat) -> Option<Money> {
        self.row(type_id, side).map(|row| row.price(side, stat))
    }

    /// Every row held, in item and side order.
    pub fn rows(&self) -> impl Iterator<Item = (TypeId, Side, &SummaryRow)> {
        self.rows
            .iter()
            .map(|((type_id, side), row)| (*type_id, *side, row))
    }

    /// When Fuzzwork made the file held, from its `Last-Modified`.
    pub fn published_at(&self) -> Option<i64> {
        self.published_at
    }

    /// The `orderSet` of the file held.
    pub fn order_set(&self) -> Option<u64> {
        self.order_set
    }

    /// When the next request may be made: the last answer plus [`MIN_GAP_SECS`], or the end of a wait
    /// after a failure when that is later.
    pub fn next_refresh_at(&self) -> Option<i64> {
        let hold_end = self.hold.as_ref().map(|hold| hold.until);
        self.next_answer_due().into_iter().chain(hold_end).max()
    }

    /// Whether any row is held.
    pub fn has_rows(&self) -> bool {
        !self.rows.is_empty()
    }

    /// The time after which the last answer allows a new request. A time so large that the gap does not
    /// fit in `i64` is not a real clock reading, so the gap is not enforced there.
    fn next_answer_due(&self) -> Option<i64> {
        self.answered_at.and_then(|at| at.checked_add(MIN_GAP_SECS))
    }

    /// One GET of the file, and what its answer does to the held copy.
    fn request(&mut self, now: i64) -> Result<Refresh, MarketError> {
        let sent = self.etag.clone();
        let response = self.http.get_bytes(AGGREGATES_URL, sent.as_deref())?;
        match response.status {
            200 => {
                let published_at =
                    unix_seconds("last-modified", response.last_modified.as_deref())?;
                let (rows, order_set) = parse_aggregates(&response.body, self.region)?;
                self.rows = rows;
                self.etag = response.etag;
                self.published_at = Some(published_at);
                self.order_set = Some(order_set);
                self.answered_at = Some(now);
                self.hold = None;
                Ok(Refresh::Downloaded)
            }
            304 if sent.is_some() => {
                self.answered_at = Some(now);
                self.hold = None;
                Ok(Refresh::NotModified)
            }
            420 | 429 => Err(MarketError::RateLimited {
                retry_after_secs: response.retry_after.unwrap_or(DEFAULT_RETRY_SECS),
            }),
            status => Err(MarketError::Http { status }),
        }
    }
}

/// Unzips the file and reads the rows of `region`. Returns them with the `orderSet` of the first row of
/// that region (0 when the region has no rows). Rows of other regions are skipped without reading their numbers.
pub fn parse_aggregates(gz: &[u8], region: u32) -> Result<(Rows, u64), MarketError> {
    let text = gunzip(gz)?;
    let mut lines = text.lines();
    let header = lines.next().unwrap_or_default();
    if header != HEADER {
        return Err(MarketError::BadRow {
            line: 1,
            text: header.to_owned(),
        });
    }
    let mut rows = BTreeMap::new();
    let mut order_set = None;
    for (number, line) in (2_usize..).zip(lines) {
        let bad = || MarketError::BadRow {
            line: number,
            text: line.to_owned(),
        };
        let columns: Vec<&str> = line.split(',').collect();
        let &[
            what,
            weighted,
            max,
            min,
            _stddev,
            median,
            volume,
            orders,
            five,
            set,
        ] = columns.as_slice()
        else {
            return Err(bad());
        };
        let (row_region, type_id, side) = parse_what(what).ok_or_else(bad)?;
        if row_region != region {
            continue;
        }
        let set = whole::<u64>(set).ok_or_else(bad)?;
        order_set.get_or_insert(set);
        let row = SummaryRow {
            min: money(min).ok_or_else(bad)?,
            max: money(max).ok_or_else(bad)?,
            weighted_average: money(weighted).ok_or_else(bad)?,
            median: money(median).ok_or_else(bad)?,
            five_percent: money(five).ok_or_else(bad)?,
            volume: whole(volume).ok_or_else(bad)?,
            orders: whole(orders).ok_or_else(bad)?,
        };
        if row.orders == 0 {
            continue;
        }
        if rows.insert((type_id, side), row).is_some() {
            return Err(bad());
        }
    }
    Ok((rows, order_set.unwrap_or(0)))
}

/// The file unzipped to text. More than [`MAX_DECODED`] bytes is [`MarketError::TooLarge`].
fn gunzip(gz: &[u8]) -> Result<String, MarketError> {
    let mut decoded = Vec::new();
    GzDecoder::new(gz)
        .take(MAX_DECODED + 1)
        .read_to_end(&mut decoded)
        .map_err(|error| MarketError::Gzip(error.to_string()))?;
    if u64::try_from(decoded.len()).unwrap_or(u64::MAX) > MAX_DECODED {
        return Err(MarketError::TooLarge);
    }
    String::from_utf8(decoded).map_err(|error| MarketError::Gzip(error.utf8_error().to_string()))
}

/// The `what` column: `<region>|<type>|<true for buy, false for sell>`.
fn parse_what(text: &str) -> Option<(u32, TypeId, Side)> {
    let mut parts = text.split('|');
    let region = parts.next()?.parse().ok()?;
    let type_id = parts.next()?.parse().ok()?;
    let side = match parts.next()? {
        "true" => Side::Buy,
        "false" => Side::Sell,
        _ => return None,
    };
    if parts.next().is_some() {
        return None;
    }
    Some((region, type_id, side))
}

/// A price as written in the file, rounded to 0.0001 ISK.
fn money(text: &str) -> Option<Money> {
    Money::parse_decimal_rounded(text).ok()
}

/// A count written as decimal text whose fraction is zero (`58.0`), or as a plain integer.
fn whole<T: TryFrom<u64>>(text: &str) -> Option<T> {
    let (integer, fraction) = match text.split_once('.') {
        Some((integer, fraction)) => (integer, Some(fraction)),
        None => (text, None),
    };
    if let Some(fraction) = fraction
        && (fraction.is_empty() || fraction.bytes().any(|byte| byte != b'0'))
    {
        return None;
    }
    T::try_from(integer.parse::<u64>().ok()?).ok()
}

/// An HTTP date as Unix seconds. Missing or unreadable dates are a [`MarketError::BadHeader`].
fn unix_seconds(name: &'static str, value: Option<&str>) -> Result<i64, MarketError> {
    let bad = || MarketError::BadHeader {
        name,
        value: value.unwrap_or_default().to_owned(),
    };
    let time = value
        .and_then(|text| httpdate::parse_http_date(text).ok())
        .ok_or_else(bad)?;
    time.duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|since| i64::try_from(since.as_secs()).ok())
        .ok_or_else(bad)
}

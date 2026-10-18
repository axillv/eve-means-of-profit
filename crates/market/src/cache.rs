//! The order-book cache: when to ask ESI again, and what to keep from its answers.
//!
//! Times are Unix seconds and always passed in by the caller, so this module never reads the clock.

#![deny(
    clippy::arithmetic_side_effects,
    clippy::as_conversions,
    clippy::integer_division
)]

use std::collections::BTreeMap;
use std::time::UNIX_EPOCH;

use profit_core::industry::TypeId;

use crate::book::{Order, parse_orders};
use crate::error::MarketError;

/// Market orders in The Forge (region 10000002).
const ORDERS_URL: &str = "https://esi.evetech.net/markets/10000002/orders/";
/// The wait when ESI's rate-limit answer has no `retry-after`.
const DEFAULT_RETRY_SECS: u64 = 60;
/// Most pages of one book that are fetched. A larger `x-pages` is treated as a broken header.
const MAX_PAGES: u32 = 100;

/// ESI's answer to one GET, with the headers the cache reads.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HttpResponse {
    pub status: u16,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub expires: Option<String>,
    pub pages: Option<u32>,
    pub retry_after: Option<u64>,
    pub body: String,
}

/// Sends a GET. The real one is [`crate::http::UreqHttp`]; tests pass fakes.
pub trait Http {
    /// `if_none_match` is sent as the `If-None-Match` header when present.
    fn get(&self, url: &str, if_none_match: Option<&str>) -> Result<HttpResponse, MarketError>;
}

/// The orders for one type, as the cache holds them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BookView<'a> {
    pub orders: &'a [Order],
    /// ESI's `last-modified`, in Unix seconds.
    pub last_modified: i64,
    /// `true` when no request was made for this view.
    pub from_cache: bool,
}

/// What the cache holds for one type.
#[derive(Debug, Default)]
struct Entry {
    orders: Vec<Order>,
    etag: Option<String>,
    last_modified: i64,
    expires: i64,
}

/// The rate-limit wait: no requests before `until`.
#[derive(Debug, Clone, Copy)]
struct Block {
    until: i64,
    retry_after_secs: u64,
}

/// The headers the cache keeps from one response.
struct Headers {
    etag: Option<String>,
    last_modified: i64,
    expires: i64,
}

/// What a fetch of a whole book returned.
enum Fetched {
    /// ESI answered 304: the held orders are still current.
    NotModified(Headers),
    Modified {
        orders: Vec<Order>,
        headers: Headers,
    },
}

/// Order books per type, kept until ESI says they expire.
pub struct Market<H: Http> {
    http: H,
    entries: BTreeMap<TypeId, Entry>,
    blocked: Option<Block>,
}

impl<H: Http> Market<H> {
    pub fn new(http: H) -> Self {
        Self {
            http,
            entries: BTreeMap::new(),
            blocked: None,
        }
    }

    /// The orders for `type_id` at time `now`. Asks ESI only when the held copy has expired.
    ///
    /// Every call fails with [`MarketError::RateLimited`] without asking ESI until a rate-limit wait has passed.
    pub fn book(&mut self, type_id: TypeId, now: i64) -> Result<BookView<'_>, MarketError> {
        self.check_block(now)?;
        let fresh = self
            .entries
            .get(&type_id)
            .is_some_and(|entry| now < entry.expires);
        if !fresh {
            self.refresh(type_id, now)?;
        }
        let entry = self.entries.entry(type_id).or_default();
        Ok(BookView {
            orders: &entry.orders,
            last_modified: entry.last_modified,
            from_cache: fresh,
        })
    }

    /// What is held for `type_id`, however old. `None` until a `book` call has succeeded for it.
    pub fn cached(&self, type_id: TypeId) -> Option<BookView<'_>> {
        self.entries.get(&type_id).map(|entry| BookView {
            orders: &entry.orders,
            last_modified: entry.last_modified,
            from_cache: true,
        })
    }

    fn check_block(&mut self, now: i64) -> Result<(), MarketError> {
        if let Some(block) = self.blocked {
            if now < block.until {
                return Err(MarketError::RateLimited {
                    retry_after_secs: block.retry_after_secs,
                });
            }
            self.blocked = None;
        }
        Ok(())
    }

    /// Asks ESI for the whole book and stores the answer. A failure leaves what is held as it was.
    fn refresh(&mut self, type_id: TypeId, now: i64) -> Result<(), MarketError> {
        let etag = self
            .entries
            .get(&type_id)
            .and_then(|entry| entry.etag.clone());
        match self.fetch(type_id, etag.as_deref()) {
            Ok(Fetched::NotModified(headers)) => {
                let entry = self.entries.entry(type_id).or_default();
                entry.last_modified = headers.last_modified;
                entry.expires = headers.expires;
            }
            Ok(Fetched::Modified { orders, headers }) => {
                let entry = self.entries.entry(type_id).or_default();
                entry.orders = orders;
                entry.etag = headers.etag;
                entry.last_modified = headers.last_modified;
                entry.expires = headers.expires;
            }
            Err(error) => {
                self.note_failure(&error, now);
                return Err(error);
            }
        }
        Ok(())
    }

    /// Starts the rate-limit wait when ESI asked for one.
    fn note_failure(&mut self, error: &MarketError, now: i64) {
        if let MarketError::RateLimited { retry_after_secs } = *error {
            let wait = i64::try_from(retry_after_secs).unwrap_or(i64::MAX);
            self.blocked = Some(Block {
                until: now.saturating_add(wait),
                retry_after_secs,
            });
        }
    }

    /// Fetches page 1, then the rest when `x-pages` says there are more.
    fn fetch(&self, type_id: TypeId, if_none_match: Option<&str>) -> Result<Fetched, MarketError> {
        let first = self.get_page(&orders_url(type_id, None), if_none_match)?;
        let headers = Headers::read(&first)?;
        if first.status == 304 {
            return Ok(Fetched::NotModified(headers));
        }
        let mut orders = parse_orders(&first.body)?;
        let pages = first.pages.unwrap_or(1);
        if pages > MAX_PAGES {
            return Err(MarketError::BadHeader {
                name: "x-pages",
                value: pages.to_string(),
            });
        }
        for page in 2..=pages {
            let next = self.get_page(&orders_url(type_id, Some(page)), None)?;
            orders.extend(parse_orders(&next.body)?);
        }
        Ok(Fetched::Modified { orders, headers })
    }

    /// One GET. Accepts 200, and 304 only when a conditional request was sent.
    fn get_page(
        &self,
        url: &str,
        if_none_match: Option<&str>,
    ) -> Result<HttpResponse, MarketError> {
        let response = self.http.get(url, if_none_match)?;
        let status = response.status;
        match status {
            200 => Ok(response),
            304 if if_none_match.is_some() => Ok(response),
            420 | 429 => Err(MarketError::RateLimited {
                retry_after_secs: response.retry_after.unwrap_or(DEFAULT_RETRY_SECS),
            }),
            _ => Err(MarketError::Http { status }),
        }
    }
}

impl Headers {
    fn read(response: &HttpResponse) -> Result<Headers, MarketError> {
        Ok(Headers {
            etag: response.etag.clone(),
            last_modified: unix_seconds("last-modified", response.last_modified.as_deref())?,
            expires: unix_seconds("expires", response.expires.as_deref())?,
        })
    }
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

fn orders_url(type_id: TypeId, page: Option<u32>) -> String {
    match page {
        None => format!("{ORDERS_URL}?type_id={type_id}&order_type=all"),
        Some(page) => format!("{ORDERS_URL}?type_id={type_id}&order_type=all&page={page}"),
    }
}

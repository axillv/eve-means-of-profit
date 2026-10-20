//! Jita prices for the Build page. The market keeps the order books, and only the worker thread
//! locks it, since a request can take a while. A job copies each book it reads into [`Prices`],
//! and the page reads those copies, so a request never holds up a frame.

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard, PoisonError};

use profit_core::buylist::LinePrice;
use profit_core::industry::TypeId;
use profit_market::book::{Order, PriceRule, Quote, quote as market_quote};
use profit_market::cache::{Http, HttpResponse, Market};
use profit_market::error::MarketError;

use crate::worker::Worker;

/// Jita 4-4, the station the buy list is priced at.
pub const JITA: u64 = 60_003_760;

/// A price older than this, in seconds, is shown in the stale colour.
pub const STALE_SECS: i64 = 600;

/// A book as the page keeps it: its orders, and when ESI last changed them.
#[derive(Debug, Clone)]
pub struct Book {
    pub orders: Vec<Order>,
    pub last_modified: i64,
}

/// The HTTP client the market reads through: the real one, or a fake in tests.
pub struct Feed(Box<dyn Http + Send>);

impl Feed {
    pub fn new(http: Box<dyn Http + Send>) -> Self {
        Self(http)
    }
}

impl Http for Feed {
    fn get(&self, url: &str, if_none_match: Option<&str>) -> Result<HttpResponse, MarketError> {
        self.0.get(url, if_none_match)
    }
}

/// The market and the books the page has read from it.
pub struct Prices {
    /// Locked only by the worker thread, while a job reads the market.
    market: Mutex<Market<Feed>>,
    /// Written by the jobs and read by the page, which only ever holds this lock for a moment.
    books: Mutex<BTreeMap<TypeId, Book>>,
    /// Failures the jobs hit, until the page takes them.
    failures: Mutex<Vec<MarketError>>,
}

impl Prices {
    pub fn new(http: Box<dyn Http + Send>) -> Self {
        Self {
            market: Mutex::new(Market::new(Feed::new(http))),
            books: Mutex::new(BTreeMap::new()),
            failures: Mutex::new(Vec::new()),
        }
    }

    /// Asks for the books of `types` on the worker thread. The market keeps a book that has not
    /// expired, so ESI is asked only for the rest.
    pub fn request(self: &std::sync::Arc<Self>, worker: &Worker, types: Vec<TypeId>, now: i64) {
        let prices = std::sync::Arc::clone(self);
        worker.run(Box::new(move || prices.read(&types, now)));
    }

    /// Reads the books on the worker thread. A failure is kept for the page, and the next book is
    /// still read.
    fn read(&self, types: &[TypeId], now: i64) {
        let mut market = lock(&self.market);
        for &type_id in types {
            match market.book(type_id, now) {
                Ok(view) => {
                    let book = Book {
                        orders: view.orders.to_vec(),
                        last_modified: view.last_modified,
                    };
                    lock(&self.books).insert(type_id, book);
                }
                Err(error) => lock(&self.failures).push(error),
            }
        }
    }

    /// Whether a book is held for `type_id`.
    pub fn held(&self, type_id: TypeId) -> bool {
        lock(&self.books).contains_key(&type_id)
    }

    /// The price of `qty` units of `type_id` under `rule`, as the buy list reads it. `None` while
    /// the book is not held.
    pub fn line_price(
        &self,
        type_id: TypeId,
        rule: PriceRule,
        qty: u64,
    ) -> Option<Result<LinePrice, MarketError>> {
        lock(&self.books)
            .get(&type_id)
            .map(|book| -> Result<LinePrice, MarketError> {
                let priced_at = book.last_modified;
                Ok(match market_quote(&book.orders, JITA, rule, qty)? {
                    Quote::Priced {
                        total, per_unit, ..
                    } => LinePrice::Priced {
                        total,
                        per_unit,
                        priced_at,
                    },
                    Quote::Thin {
                        available,
                        total_for_available,
                        ..
                    } => LinePrice::Thin {
                        available,
                        total_for_available,
                        priced_at,
                    },
                    Quote::NoOrders => LinePrice::Missing,
                })
            })
    }

    /// When the oldest held book last changed on ESI, `None` while no book is held.
    pub fn oldest_change(&self) -> Option<i64> {
        lock(&self.books)
            .values()
            .map(|book| book.last_modified)
            .min()
    }

    /// The failures the jobs have hit since the last call.
    pub fn take_failures(&self) -> Vec<MarketError> {
        std::mem::take(&mut *lock(&self.failures))
    }
}

/// The notice text for a failed read. A rate limit names the wait.
pub fn failure_text(error: &MarketError) -> String {
    if let MarketError::RateLimited { retry_after_secs } = error {
        return format!("Jita prices paused for {retry_after_secs} s (ESI rate limit).");
    }
    format!("Could not get Jita prices: {error}")
}

/// An age in whole units: `<n>s` under a minute, `<n>m` under an hour, `<n>h` under a day, else
/// `<n>d`. Rounded down; a time in the future counts as 0 s.
pub(crate) fn age_text(seconds: i64) -> String {
    let seconds = seconds.max(0);
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3_600 {
        format!("{}m", seconds / 60)
    } else if seconds < 86_400 {
        format!("{}h", seconds / 3_600)
    } else {
        format!("{}d", seconds / 86_400)
    }
}

/// Locks a mutex. A lock poisoned by a panicked job is still used.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::age_text;

    #[test]
    fn age_labels_change_unit_at_each_boundary() {
        assert_eq!(age_text(0), "0s");
        assert_eq!(age_text(59), "59s");
        assert_eq!(age_text(60), "1m");
        assert_eq!(age_text(3_599), "59m");
        assert_eq!(age_text(3_600), "1h");
        assert_eq!(age_text(86_399), "23h");
        assert_eq!(age_text(86_400), "1d");
    }
}

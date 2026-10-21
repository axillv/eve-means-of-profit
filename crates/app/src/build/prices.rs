//! Market prices for the Build page: the summary file of The Forge, as Fuzzwork publishes it. The
//! worker thread owns the market and is the only one to download; after each download it copies
//! the rows into [`Held`], which the page reads, so a download never holds up a frame.
//! Prices the player types are kept here too, per plan, for the rest of the session.

use std::collections::BTreeMap;
use std::panic::{self, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use profit_core::buylist::LinePrice;
use profit_core::industry::TypeId;
use profit_core::money::Money;
use profit_market::book::PriceRule;
use profit_market::error::MarketError;
use profit_market::fuzzwork::{
    BytesHttp, BytesResponse, FORGE, FuzzworkMarket, MIN_GAP_SECS, Refresh, Side, Stat, SummaryRow,
};

use crate::worker::Worker;

/// A price older than this, in seconds, is shown in the stale colour.
pub const STALE_SECS: i64 = 600;

/// The HTTP client the market reads through: the real one, or a fake in tests.
pub struct Feed(Box<dyn BytesHttp + Send>);

impl Feed {
    pub fn new(http: Box<dyn BytesHttp + Send>) -> Self {
        Self(http)
    }
}

impl BytesHttp for Feed {
    fn get_bytes(
        &self,
        url: &str,
        if_none_match: Option<&str>,
    ) -> Result<BytesResponse, MarketError> {
        self.0.get_bytes(url, if_none_match)
    }
}

/// The rows of the file last downloaded, and when Fuzzwork made it.
#[derive(Debug, Clone, Default)]
pub struct Held {
    pub rows: BTreeMap<(TypeId, Side), SummaryRow>,
    pub published_at: Option<i64>,
    pub next_refresh_at: Option<i64>,
}

/// Why a refresh failed: the download answered with an error, or it stopped unexpectedly.
#[derive(Debug, Clone)]
pub enum Failure {
    Market(MarketError),
    Crashed,
}

/// What one refresh answered. `explicit` is set when the player asked for it, so a refresh that
/// the timer made without news stays quiet.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub explicit: bool,
    pub result: Result<Refresh, Failure>,
}

/// A price the player typed for one line, per unit, and when it was typed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Manual {
    pub per_unit: Money,
    pub set_at: i64,
}

/// The market, the copy of its rows the page reads, and the prices typed for the plans.
pub struct Prices {
    /// Locked only by the worker thread, while it downloads.
    market: Mutex<FuzzworkMarket<Feed>>,
    /// Written by the worker after each refresh and read by the page, which only holds it briefly.
    held: Mutex<Arc<Held>>,
    /// Answers the worker has not yet shown to the page.
    outcomes: Mutex<Vec<Outcome>>,
    /// Set while a refresh is queued or running, so a second request is not made meanwhile.
    running: AtomicBool,
    /// Set once the first download has been asked for.
    first_asked: AtomicBool,
    /// Set when the worker thread has stopped, so no refresh can run for the rest of the session.
    stopped: AtomicBool,
    /// Typed prices, by plan id and item.
    manual: Mutex<BTreeMap<i64, BTreeMap<TypeId, Manual>>>,
}

impl Prices {
    /// The market reads through `http`.
    pub fn new(http: Box<dyn BytesHttp + Send>) -> Self {
        Self {
            market: Mutex::new(FuzzworkMarket::new(Feed::new(http), FORGE)),
            held: Mutex::new(Arc::new(Held::default())),
            outcomes: Mutex::new(Vec::new()),
            running: AtomicBool::new(false),
            first_asked: AtomicBool::new(false),
            stopped: AtomicBool::new(false),
            manual: Mutex::new(BTreeMap::new()),
        }
    }

    /// Asks the worker thread to refresh the file. A request already queued or running is not
    /// doubled. The market itself answers [`Refresh::TooSoon`] within 15 minutes of its last answer.
    pub fn refresh(self: &Arc<Self>, worker: &Worker, now: i64, explicit: bool) {
        if self.stopped() || self.running.swap(true, Ordering::SeqCst) {
            return;
        }
        let queued = Queued {
            prices: Arc::clone(self),
            now,
            explicit,
        };
        if !worker.run(Box::new(move || queued.run())) {
            // The worker's thread has stopped: the job was dropped unrun, which cleared the running
            // flag. Nothing is requested again this session.
            self.stopped.store(true, Ordering::SeqCst);
        }
    }

    /// Runs on the worker thread: refreshes the market and keeps the answer for the page to show. A
    /// download that panics is an answer too, so the worker keeps running and the timer tries again
    /// later.
    fn read(&self, now: i64, explicit: bool) {
        let result = match panic::catch_unwind(AssertUnwindSafe(|| self.download(now))) {
            Ok(result) => result.map_err(Failure::Market),
            Err(_) => {
                // Held back as a failed download is, so the timer does not ask again on every frame.
                let mut held = (*self.held()).clone();
                held.next_refresh_at = Some(now + MIN_GAP_SECS);
                *lock(&self.held) = Arc::new(held);
                Err(Failure::Crashed)
            }
        };
        lock(&self.outcomes).push(Outcome { explicit, result });
    }

    /// Refreshes the market and copies the rows it holds for the page.
    fn download(&self, now: i64) -> Result<Refresh, MarketError> {
        let mut market = lock(&self.market);
        let result = market.refresh(now);
        let held = Held {
            rows: market
                .rows()
                .map(|(type_id, side, row)| ((type_id, side), row.clone()))
                .collect(),
            published_at: market.published_at(),
            next_refresh_at: market.next_refresh_at(),
        };
        *lock(&self.held) = Arc::new(held);
        result
    }

    /// Whether a refresh is queued or running.
    pub fn running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Whether the worker thread has stopped, so refreshes cannot run until the app restarts.
    pub fn stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }

    /// The copy of the market the page reads.
    pub fn held(&self) -> Arc<Held> {
        Arc::clone(&lock(&self.held))
    }

    /// The age of the file held, when one has been downloaded.
    pub fn published_at(&self) -> Option<i64> {
        self.held().published_at
    }

    /// The answers since the last call.
    pub fn take_outcomes(&self) -> Vec<Outcome> {
        std::mem::take(&mut *lock(&self.outcomes))
    }

    /// True the first time it is asked, so the first download is asked for once.
    pub fn ask_first(&self) -> bool {
        !self.first_asked.swap(true, Ordering::SeqCst)
    }

    /// The price typed for an item on a plan, if any.
    pub fn manual(&self, plan: i64, type_id: TypeId) -> Option<Manual> {
        lock(&self.manual)
            .get(&plan)
            .and_then(|prices| prices.get(&type_id))
            .copied()
    }

    /// Sets or, with `None`, clears the price typed for an item on a plan.
    pub fn set_manual(&self, plan: i64, type_id: TypeId, price: Option<Manual>) {
        let mut all = lock(&self.manual);
        let prices = all.entry(plan).or_default();
        match price {
            Some(price) => prices.insert(type_id, price),
            None => prices.remove(&type_id),
        };
    }
}

/// A refresh on its way to the worker thread. Dropping it clears the running flag, whether the
/// worker ran it or dropped it unrun because the worker's thread has stopped.
struct Queued {
    prices: Arc<Prices>,
    now: i64,
    explicit: bool,
}

impl Queued {
    fn run(self) {
        self.prices.read(self.now, self.explicit);
    }
}

impl Drop for Queued {
    fn drop(&mut self) {
        self.prices.running.store(false, Ordering::SeqCst);
    }
}

/// The live price of an item under a rule: what the file says now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Live {
    pub per_unit: Money,
    /// Units on the side the rule reads.
    pub volume: u64,
    pub published_at: i64,
}

/// The side and statistic a rule reads. A plan saved with the walk-the-book rule reads the 5% price.
pub fn rule_source(rule: PriceRule) -> (Side, Stat) {
    rule.summary().unwrap_or((Side::Sell, Stat::FivePercent))
}

/// The live price of an item under a rule, from the rows held. `None` when the side has no orders.
pub fn live(held: &Held, type_id: TypeId, rule: PriceRule) -> Option<Live> {
    let (side, stat) = rule_source(rule);
    let row = held.rows.get(&(type_id, side))?;
    Some(Live {
        per_unit: row.price(side, stat),
        volume: row.volume,
        published_at: held.published_at?,
    })
}

/// A price that does not fit in money.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutOfRange;

/// The price of `qty` units of a line: the typed price when there is one, else the live price.
/// A live side with fewer units than `qty` is thin.
pub fn line_price(
    live: Option<&Live>,
    manual: Option<Manual>,
    qty: u64,
) -> Result<LinePrice, OutOfRange> {
    if let Some(manual) = manual {
        let total = manual.per_unit.checked_mul_qty(qty).ok_or(OutOfRange)?;
        return Ok(LinePrice::Priced {
            total,
            per_unit: manual.per_unit,
            priced_at: manual.set_at,
        });
    }
    let Some(live) = live else {
        return Ok(LinePrice::Missing);
    };
    if live.volume >= qty {
        let total = live.per_unit.checked_mul_qty(qty).ok_or(OutOfRange)?;
        Ok(LinePrice::Priced {
            total,
            per_unit: live.per_unit,
            priced_at: live.published_at,
        })
    } else {
        let total_for_available = live
            .per_unit
            .checked_mul_qty(live.volume)
            .ok_or(OutOfRange)?;
        Ok(LinePrice::Thin {
            available: live.volume,
            total_for_available,
            priced_at: live.published_at,
        })
    }
}

/// The notice text for a failed refresh. A rate limit names the wait.
pub fn failure_text(failure: &Failure) -> String {
    match failure {
        Failure::Market(MarketError::RateLimited { retry_after_secs }) => {
            format!("Market prices paused for {retry_after_secs} s (rate limited).")
        }
        Failure::Market(error) => format!("Could not get market prices: {error}"),
        Failure::Crashed => {
            "Could not get market prices: the download stopped unexpectedly.".to_owned()
        }
    }
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
    use profit_core::money::Money;
    use profit_market::book::PriceRule;
    use profit_market::fuzzwork::{Side, Stat};

    use std::io::Write;
    use std::ops::ControlFlow;
    use std::sync::Arc;
    use std::sync::atomic::Ordering;
    use std::thread;
    use std::time::Duration;

    use egui::Context;
    use profit_core::buylist::LinePrice;
    use profit_market::error::MarketError;
    use profit_market::fuzzwork::BytesResponse;
    use profit_sde::{Fetch, FetchError};

    use super::{
        BytesHttp, Live, Manual, OutOfRange, Prices, Queued, age_text, line_price, rule_source,
    };
    use crate::worker::Worker;

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

    #[test]
    fn walk_the_book_reads_the_five_percent_price() {
        assert_eq!(
            rule_source(PriceRule::WalkSell),
            (Side::Sell, Stat::FivePercent)
        );
        assert_eq!(rule_source(PriceRule::HighestBuy), (Side::Buy, Stat::Best));
    }

    #[test]
    fn a_typed_price_beats_the_live_one_and_is_frozen() {
        let live = Live {
            per_unit: Money::from_ten_thousandths(10),
            volume: 1,
            published_at: 5,
        };
        let manual = Manual {
            per_unit: Money::from_ten_thousandths(20),
            set_at: 9,
        };
        assert_eq!(
            line_price(Some(&live), Some(manual), 2),
            Ok(LinePrice::Priced {
                total: Money::from_ten_thousandths(40),
                per_unit: Money::from_ten_thousandths(20),
                priced_at: 9,
            })
        );
    }

    #[test]
    fn a_total_that_does_not_fit_is_out_of_range() {
        let manual = Manual {
            per_unit: Money::from_ten_thousandths(i64::MAX),
            set_at: 0,
        };
        assert_eq!(line_price(None, Some(manual), 2), Err(OutOfRange));
    }

    /// A network that answers nothing: the tests here never reach it.
    struct NoFetch;

    impl Fetch for NoFetch {
        fn get_text(&self, _url: &str) -> Result<String, FetchError> {
            Err(FetchError::Http { status: 503 })
        }

        fn download(
            &self,
            _url: &str,
            _dest: &mut dyn Write,
            _progress: &mut dyn FnMut(u64, Option<u64>) -> ControlFlow<()>,
        ) -> Result<u64, FetchError> {
            Err(FetchError::Http { status: 503 })
        }
    }

    /// A market server that answers nothing: the tests here never reach it.
    struct NoHttp;

    impl BytesHttp for NoHttp {
        fn get_bytes(
            &self,
            _url: &str,
            _if_none_match: Option<&str>,
        ) -> Result<BytesResponse, MarketError> {
            Err(MarketError::TooLarge)
        }
    }

    fn prices() -> Arc<Prices> {
        Arc::new(Prices::new(Box::new(NoHttp)))
    }

    #[test]
    fn a_queued_refresh_that_ends_unrun_clears_the_running_flag() {
        let prices = prices();
        prices.running.store(true, Ordering::SeqCst);
        drop(Queued {
            prices: Arc::clone(&prices),
            now: 0,
            explicit: false,
        });
        assert!(!prices.running());
    }

    #[test]
    fn a_refresh_on_a_stopped_worker_stops_refreshing_for_the_session() {
        let worker = Worker::spawn(Context::default(), Box::new(NoFetch));
        worker.run(Box::new(|| panic!("a job crashed")));
        // The thread stops with the job that crashed; wait until a job can no longer be queued.
        while worker.run(Box::new(|| {})) {
            thread::sleep(Duration::from_millis(1));
        }
        let prices = prices();
        prices.refresh(&worker, 0, true);
        assert!(prices.stopped());
        assert!(!prices.running());
        prices.refresh(&worker, 0, true);
        assert!(!prices.running());
    }
}

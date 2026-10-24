//! The price history while the app is open: the background refresh of the Forge prices, and the
//! saving of one reading a day. The download runs on the worker thread, and the reading is saved
//! from there through a second connection to the same file, so the screen's connection is never
//! held up while a download is written.

use std::path::Path;
use std::sync::{Mutex, MutexGuard, PoisonError};

use mop_core::industry::TypeId;
use mop_core::price_history::day_of;
use mop_market::fuzzwork::{FORGE, Side, SummaryRow};
use mop_store::price_history::MarketKind;
use mop_store::{Result, Store};

use crate::Shared;
use crate::build::prices::Held;
use crate::status::Notice;

/// The background check runs at most this often, in seconds.
const CHECK_SECS: i64 = 60;

/// Saves the readings of the Forge through its own connection to the history file.
pub struct Recorder {
    store: Mutex<Store>,
    /// Failures of the last saves, shown on the UI thread by [`Recorder::take_errors`].
    errors: Mutex<Vec<String>>,
}

impl Recorder {
    /// Opens its own connection to the history file at `path`.
    pub fn open(path: &Path, now: i64) -> Result<Self> {
        let opened = Store::open(path, now)?;
        Ok(Self {
            store: Mutex::new(opened.store),
            errors: Mutex::new(Vec::new()),
        })
    }

    /// Saves one reading of the Forge for the day of `published_at`, for each enabled Forge
    /// market, then keeps the history within the size limit. A failure is kept for the UI.
    /// Runs on the worker thread, right after a download that replaced the rows.
    pub fn record_download(&self, held: &Held) {
        let Some(published_at) = held.published_at else {
            return;
        };
        let rows: Vec<(TypeId, Side, SummaryRow)> = held
            .rows
            .iter()
            .map(|((type_id, side), row)| (*type_id, *side, row.clone()))
            .collect();
        if let Err(error) = self.save(&rows, day_of(published_at)) {
            lock(&self.errors).push(format!("Could not save price history: {error}"));
        }
    }

    fn save(&self, rows: &[(TypeId, Side, SummaryRow)], day: i64) -> Result<()> {
        let mut store = lock(&self.store);
        let forge = u64::from(FORGE);
        for market in store.history_markets()? {
            if market.enabled && market.kind == MarketKind::Region && market.place == forge {
                store.record(market.id, day, rows)?;
            }
        }
        if let Some(limit) = store.history_settings()?.limit_bytes {
            store.enforce_limit(limit)?;
        }
        Ok(())
    }

    /// The failures of the saves since the last call.
    pub fn take_errors(&self) -> Vec<String> {
        std::mem::take(&mut *lock(&self.errors))
    }
}

/// Asks the worker for the Forge prices in the background when the setting is on: once at start,
/// then when the market says the next refresh is due. Checked at most once a minute, whatever page
/// is open. `last_check` holds the time of the last check.
pub fn background_refresh(cx: &mut Shared, last_check: &mut Option<i64>) {
    let now = (cx.clock)();
    let due_check =
        last_check.is_none_or(|at| now.checked_sub(at).is_none_or(|age| age >= CHECK_SECS));
    if !due_check {
        return;
    }
    *last_check = Some(now);
    match cx.store.history_settings() {
        Ok(settings) if settings.auto_refresh => {
            let held = cx.market.held();
            let first = held.published_at.is_none() && cx.market.ask_first();
            let timer = held.next_refresh_at.is_some_and(|at| now >= at);
            if first || timer {
                cx.market.refresh(&cx.worker, now, false);
            }
        }
        Ok(_) => {}
        Err(error) => cx.notices.push(Notice::error(format!(
            "Could not read the price history settings: {error}"
        ))),
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

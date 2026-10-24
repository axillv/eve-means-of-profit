//! Acceptance tests for the price history: what a download saves, the day it is filed under, the
//! market that is turned off, and the sizes the Setup panel shows. The Fuzzwork file is not
//! downloaded; the rows are the ones the fixture holds.

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use eve_means_of_profit::build::prices::Held;
    use eve_means_of_profit::history::Recorder;
    use eve_means_of_profit::setup::history::format_bytes;
    use mop_core::money::Money;
    use mop_core::price_history::Retention;
    use mop_market::fuzzwork::{FORGE, Side, SummaryRow};
    use mop_store::Store;
    use tempfile::TempDir;

    const NOW: i64 = 1_000_000;
    /// Mon, 12 Jan 1970 13:42:40 GMT: the day is 11.
    const PUBLISHED: i64 = 999_760;

    fn row(min: i64) -> SummaryRow {
        SummaryRow {
            min: Money::from_ten_thousandths(min),
            max: Money::ZERO,
            weighted_average: Money::ZERO,
            median: Money::ZERO,
            five_percent: Money::ZERO,
            volume: 1,
            orders: 1,
        }
    }

    /// The Forge's rows of the fixture: items 34 and 11481, three rows in all.
    fn held(min: i64) -> Held {
        let mut rows = BTreeMap::new();
        rows.insert((34, Side::Sell), row(min));
        rows.insert((34, Side::Buy), row(min - 1));
        rows.insert((11481, Side::Sell), row(min + 1));
        Held {
            rows,
            published_at: Some(PUBLISHED),
            next_refresh_at: None,
        }
    }

    /// A temp history file, read through the store and written through the recorder.
    fn history() -> (TempDir, Store, Recorder) {
        let dir = TempDir::new().expect("temp dir");
        let path: PathBuf = dir.path().join("user.sqlite");
        let store = Store::open(&path, NOW).expect("store").store;
        let recorder = Recorder::open(&path, NOW).expect("recorder");
        (dir, store, recorder)
    }

    fn forge_market(store: &Store) -> i64 {
        let markets = store.history_markets().expect("markets");
        markets
            .iter()
            .find(|market| market.place == u64::from(FORGE))
            .expect("the Forge market")
            .id
    }

    #[test]
    fn a_download_saves_one_reading_of_the_forge_for_its_day() {
        let (_dir, store, recorder) = history();
        recorder.record_download(&held(100));

        let usage = store.history_usage().expect("usage");
        let forge = usage
            .iter()
            .find(|u| u.market == forge_market(&store))
            .expect("usage");
        assert_eq!(forge.days, 1);
        assert_eq!(forge.rows, 3);
        assert_eq!(forge.first_day, Some(11));
        assert_eq!(format_bytes(forge.estimated_bytes), "300 B");
        assert!(recorder.take_errors().is_empty());
    }

    #[test]
    fn a_second_download_the_same_day_replaces_the_reading() {
        let (_dir, store, recorder) = history();
        recorder.record_download(&held(100));
        recorder.record_download(&held(500));

        let market = forge_market(&store);
        let usage = store.history_usage().expect("usage");
        let forge = usage.iter().find(|u| u.market == market).expect("usage");
        assert_eq!(forge.days, 1);
        assert_eq!(forge.rows, 3);
        let series = store.series(market, 34, Side::Sell).expect("series");
        assert_eq!(series.len(), 1);
        assert_eq!(series[0].1.min, Money::from_ten_thousandths(500));
    }

    #[test]
    fn a_market_turned_off_records_nothing() {
        let (_dir, store, recorder) = history();
        let market = forge_market(&store);
        let mut store = store;
        store.set_history_enabled(market, false).expect("turn off");
        recorder.record_download(&held(100));

        let series = store.series(market, 34, Side::Sell).expect("series");
        assert!(series.is_empty(), "a market turned off keeps no reading");
    }

    /// Writes one reading of `rows` for each of `days` days from `first`.
    fn seed(
        store: &mut Store,
        market: i64,
        first: i64,
        days: i64,
        rows: &[(u32, Side, SummaryRow)],
    ) {
        for day in first..first + days {
            store
                .record(market, day, rows)
                .expect("the reading is recorded");
        }
    }

    #[test]
    fn a_shorter_rule_thins_the_readings_at_once() {
        let (_dir, mut store, _recorder) = history();
        let market = forge_market(&store);
        seed(
            &mut store,
            market,
            20_000,
            100,
            &[(34, Side::Sell, row(500))],
        );
        let retention = Retention::new(30, Some(60)).expect("a valid rule");
        let outcome = store
            .set_retention(market, retention)
            .expect("the rule is saved");
        assert_eq!(outcome.days_dropped, 58);
        let usage = store.history_usage().expect("usage");
        assert_eq!(usage[0].days, 34);
    }

    #[test]
    fn a_size_limit_keeps_the_newest_readings_that_fit() {
        let (_dir, mut store, _recorder) = history();
        let market = forge_market(&store);
        // The default rule keeps 90 days, so the seed is 92 days under a longer rule.
        store
            .set_retention(market, Retention::new(365, None).expect("a valid rule"))
            .expect("the rule is saved");
        // Two rows of 100 bytes a reading: 92 readings are 18,400 bytes.
        seed(
            &mut store,
            market,
            20_000,
            92,
            &[(34, Side::Sell, row(500)), (34, Side::Buy, row(499))],
        );
        let before = store.history_usage().expect("usage");
        assert_eq!(
            (before[0].days, before[0].rows),
            (92, 184),
            "before the limit"
        );
        let dropped = store.enforce_limit(10_000).expect("the limit is enforced");
        assert_eq!(dropped, 42);
        let usage = store.history_usage().expect("usage");
        assert_eq!(usage[0].days, 50);
    }

    #[test]
    fn removing_a_market_deletes_its_readings() {
        let (_dir, mut store, _recorder) = history();
        let market = forge_market(&store);
        seed(&mut store, market, 20_000, 5, &[(34, Side::Sell, row(500))]);
        store.forget_market(market).expect("the market is removed");
        let markets = store.history_markets().expect("markets");
        assert!(markets.iter().all(|m| m.id != market));
        assert!(store.history_usage().expect("usage").is_empty());
    }

    #[test]
    fn sizes_read_in_decimal_units_rounded_down() {
        assert_eq!(format_bytes(300), "300 B");
        assert_eq!(format_bytes(999), "999 B");
        assert_eq!(format_bytes(1_000), "1.0 kB");
        assert_eq!(format_bytes(39_000), "39.0 kB");
        assert_eq!(format_bytes(999_999), "999.9 kB");
        assert_eq!(format_bytes(1_000_000), "1.0 MB");
        assert_eq!(format_bytes(3_400_000), "3.4 MB");
        assert_eq!(format_bytes(465_621_000), "465.6 MB");
        assert_eq!(format_bytes(1_000_000_000), "1.0 GB");
    }
}

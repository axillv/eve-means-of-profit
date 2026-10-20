use std::path::Path;

use profit_core::industry::TypeId;
use profit_core::money::Money;
use profit_core::price_history::Retention;
use profit_market::fuzzwork::{Side, SummaryRow};
use profit_store::price_history::{HistorySettings, MarketKind, RecordOutcome};
use profit_store::{Store, StoreError};
use rusqlite::Connection;

#[cfg(test)]
mod tests {
    use super::*;

    const FORGE: u64 = 10_000_002;
    const JITA: u64 = 30_000_142;
    const ITEM: TypeId = 34;
    const FIRST_DAY: i64 = 20_000;
    const LAST_DAY: i64 = 20_099;

    fn row_a() -> SummaryRow {
        SummaryRow {
            min: Money::from_ten_thousandths(38_700),
            max: Money::from_ten_thousandths(3_001_000_000_000),
            weighted_average: Money::from_ten_thousandths(44_387),
            median: Money::from_ten_thousandths(48_200),
            five_percent: Money::from_ten_thousandths(38_950),
            volume: 7_073_618_405,
            orders: 44,
        }
    }

    fn row_a2() -> SummaryRow {
        SummaryRow {
            five_percent: Money::from_ten_thousandths(39_000),
            ..row_a()
        }
    }

    fn row_b() -> SummaryRow {
        SummaryRow {
            min: Money::from_ten_thousandths(100),
            max: Money::from_ten_thousandths(40_000),
            weighted_average: Money::from_ten_thousandths(29_864),
            median: Money::from_ten_thousandths(33_000),
            five_percent: Money::from_ten_thousandths(37_330),
            volume: 12_555_835_216,
            orders: 58,
        }
    }

    /// One day's readings of the item: the sell side is `row_a`, the buy side `row_b`.
    fn readings() -> Vec<(TypeId, Side, SummaryRow)> {
        vec![(ITEM, Side::Sell, row_a()), (ITEM, Side::Buy, row_b())]
    }

    fn days_of(series: &[(i64, SummaryRow)]) -> Vec<i64> {
        series.iter().map(|(day, _)| *day).collect()
    }

    fn forge_id(store: &Store) -> i64 {
        store
            .history_markets()
            .expect("markets")
            .into_iter()
            .find(|market| market.place == FORGE)
            .expect("the Forge is in the price history")
            .id
    }

    /// Records `days` for `market` and returns the outcome of the last day.
    fn record_days(
        store: &mut Store,
        market: i64,
        days: std::ops::RangeInclusive<i64>,
    ) -> RecordOutcome {
        let mut last = None;
        for day in days {
            last = Some(store.record(market, day, &readings()).expect("record"));
        }
        last.expect("at least one day")
    }

    /// A Forge history of the 92 days that the default retention keeps after day 20099.
    fn ninety_two_days(path: &Path) -> (Store, i64) {
        let mut store = Store::open(path, 1_000).expect("open");
        let forge = forge_id(&store);
        record_days(&mut store, forge, FIRST_DAY..=LAST_DAY);
        (store, forge)
    }

    fn raw(path: &Path) -> Connection {
        Connection::open(path).expect("open")
    }

    #[test]
    fn a_fresh_store_has_the_forge_and_the_default_settings() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Store::open(&dir.path().join("eve.db"), 1_000).expect("open");

        let markets = store.history_markets().expect("markets");

        assert_eq!(markets.len(), 1);
        let forge = markets.first().expect("the Forge");
        assert_eq!(forge.kind, MarketKind::Region);
        assert_eq!(forge.place, FORGE);
        assert_eq!(forge.retention, Retention::DEFAULT);
        assert!(forge.enabled);
        assert_eq!(
            store.history_settings().expect("settings"),
            HistorySettings {
                limit_bytes: None,
                auto_refresh: true,
            }
        );
    }

    #[test]
    fn a_version_seven_file_upgrades_and_keeps_a_copy_of_itself() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("eve.db");
        drop(Store::open(&path, 1_000).expect("open"));
        raw(&path)
            .execute_batch(
                "PRAGMA foreign_keys = OFF;
             DROP TABLE price_history;
             DROP TABLE history_settings;
             DROP TABLE history_markets;
             DELETE FROM migrations WHERE version = 8;
             PRAGMA user_version = 7;",
            )
            .expect("downgrade");
        let before = std::fs::read(&path).expect("read");

        let store = Store::open(&path, 2_000).expect("upgrade");

        assert_eq!(
            std::fs::read(dir.path().join("eve.db.before-v8")).expect("backup"),
            before
        );
        let markets = store.history_markets().expect("markets");
        assert_eq!(markets.len(), 1);
        assert_eq!(
            markets.first().map(|market| market.retention),
            Some(Retention::DEFAULT)
        );
        assert_eq!(
            store.history_settings().expect("settings"),
            HistorySettings {
                limit_bytes: None,
                auto_refresh: true,
            }
        );
        let applied: (String, i64) = raw(&path)
            .query_row(
                "SELECT name, applied_at FROM migrations WHERE version = 8",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("migration row");
        assert_eq!(applied, ("price history".to_owned(), 2_000));
    }

    #[test]
    fn a_recording_keeps_one_reading_per_side_and_day() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut store = Store::open(&dir.path().join("eve.db"), 1_000).expect("open");
        let forge = forge_id(&store);

        let outcome = store.record(forge, 20_000, &readings()).expect("record");

        assert_eq!(
            outcome,
            RecordOutcome {
                rows_written: 2,
                days_dropped: 0,
            }
        );
        assert_eq!(
            store.series(forge, ITEM, Side::Sell).expect("sell"),
            vec![(20_000, row_a())]
        );
        assert_eq!(
            store.series(forge, ITEM, Side::Buy).expect("buy"),
            vec![(20_000, row_b())]
        );

        store
            .record(forge, 20_000, &[(ITEM, Side::Sell, row_a2())])
            .expect("record again");

        assert_eq!(
            store.series(forge, ITEM, Side::Sell).expect("sell"),
            vec![(20_000, row_a2())]
        );
    }

    #[test]
    fn the_default_retention_keeps_ninety_days_then_weekly_readings() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut store = Store::open(&dir.path().join("eve.db"), 1_000).expect("open");
        let forge = forge_id(&store);
        record_days(&mut store, forge, FIRST_DAY..=LAST_DAY - 1);

        let last = store
            .record(forge, LAST_DAY, &readings())
            .expect("last day");

        assert_eq!(
            last,
            RecordOutcome {
                rows_written: 2,
                days_dropped: 1,
            }
        );
        let days = days_of(&store.series(forge, ITEM, Side::Sell).expect("sell"));
        assert_eq!(days.len(), 92);
        assert_eq!(
            days.iter().take(4).copied().collect::<Vec<_>>(),
            vec![20_000, 20_003, 20_010, 20_011]
        );
        let usage = store
            .history_usage()
            .expect("usage")
            .into_iter()
            .find(|usage| usage.market == forge)
            .expect("the Forge");
        assert_eq!(usage.days, 92);
        assert_eq!(usage.first_day, Some(20_000));
        assert_eq!(usage.last_day, Some(20_099));
        assert_eq!(usage.rows, 184);
        assert_eq!(usage.estimated_bytes, 18_400);
    }

    #[test]
    fn each_market_keeps_its_own_retention() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut store = Store::open(&dir.path().join("eve.db"), 1_000).expect("open");
        let forge = forge_id(&store);
        let rule = Retention::new(30, Some(60)).expect("valid rule");
        let jita = store
            .add_history_market(MarketKind::System, JITA, rule)
            .expect("add");
        record_days(&mut store, jita, FIRST_DAY..=LAST_DAY);

        let jita_days = days_of(&store.series(jita, ITEM, Side::Sell).expect("jita"));
        assert_eq!(jita_days.len(), 34);
        assert_eq!(jita_days.first().copied(), Some(20_045));
        assert_eq!(jita_days.last().copied(), Some(20_099));

        record_days(&mut store, forge, FIRST_DAY..=LAST_DAY);
        let outcome = store.set_retention(forge, rule).expect("set retention");

        assert_eq!(
            outcome,
            RecordOutcome {
                rows_written: 0,
                days_dropped: 58,
            }
        );
        assert_eq!(
            days_of(&store.series(forge, ITEM, Side::Sell).expect("forge")),
            jita_days
        );
    }

    #[test]
    fn a_disabled_market_records_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut store = Store::open(&dir.path().join("eve.db"), 1_000).expect("open");
        let forge = forge_id(&store);
        store.set_history_enabled(forge, false).expect("disable");

        let outcome = store.record(forge, 20_000, &readings()).expect("record");

        assert_eq!(
            outcome,
            RecordOutcome {
                rows_written: 0,
                days_dropped: 0,
            }
        );
        assert!(
            store
                .series(forge, ITEM, Side::Sell)
                .expect("sell")
                .is_empty()
        );
        let usage = store.history_usage().expect("usage");
        assert_eq!(usage.first().map(|usage| usage.rows), Some(0));
    }

    #[test]
    fn the_size_limit_drops_the_oldest_days_but_never_the_newest() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (mut store, forge) = ninety_two_days(&dir.path().join("eve.db"));

        let dropped = store.enforce_limit(10_000).expect("limit");

        assert_eq!(dropped, 42);
        let days = days_of(&store.series(forge, ITEM, Side::Sell).expect("sell"));
        assert_eq!(days.len(), 50);
        assert_eq!(days.first().copied(), Some(20_050));
        let usage = store.history_usage().expect("usage");
        assert_eq!(usage.first().map(|usage| usage.rows), Some(100));
        assert_eq!(
            usage.first().map(|usage| usage.estimated_bytes),
            Some(10_000)
        );
    }

    #[test]
    fn a_zero_limit_keeps_only_the_newest_day() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (mut store, forge) = ninety_two_days(&dir.path().join("eve.db"));

        let dropped = store.enforce_limit(0).expect("limit");

        assert_eq!(dropped, 91);
        assert_eq!(
            days_of(&store.series(forge, ITEM, Side::Sell).expect("sell")),
            vec![LAST_DAY]
        );
    }

    #[test]
    fn a_limit_above_the_size_drops_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (mut store, _forge) = ninety_two_days(&dir.path().join("eve.db"));

        assert_eq!(store.enforce_limit(18_400).expect("limit"), 0);
        assert_eq!(store.enforce_limit(1_000_000).expect("limit"), 0);
    }

    #[test]
    fn forgetting_a_market_removes_its_readings() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("eve.db");
        let (mut store, forge) = ninety_two_days(&path);

        store.forget_market(forge).expect("forget");

        assert!(matches!(
            store.series(forge, ITEM, Side::Sell),
            Err(StoreError::NotFound)
        ));
        let left: i64 = raw(&path)
            .query_row("SELECT COUNT(*) FROM price_history", [], |row| row.get(0))
            .expect("count");
        assert_eq!(left, 0);
    }

    #[test]
    fn a_second_add_of_the_same_market_is_a_duplicate() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut store = Store::open(&dir.path().join("eve.db"), 1_000).expect("open");

        assert!(matches!(
            store.add_history_market(MarketKind::Region, FORGE, Retention::DEFAULT),
            Err(StoreError::Duplicate)
        ));
    }

    #[test]
    fn an_unknown_market_is_not_found() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut store = Store::open(&dir.path().join("eve.db"), 1_000).expect("open");

        assert!(matches!(
            store.record(999, 20_000, &readings()),
            Err(StoreError::NotFound)
        ));
        assert!(matches!(
            store.set_retention(999, Retention::DEFAULT),
            Err(StoreError::NotFound)
        ));
        assert!(matches!(
            store.set_history_enabled(999, false),
            Err(StoreError::NotFound)
        ));
    }

    #[test]
    fn a_stored_side_that_is_not_a_side_is_bad_data() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("eve.db");
        let (store, forge) = ninety_two_days(&path);
        raw(&path)
            .execute(
                "UPDATE price_history SET side = 5 WHERE day = 20000 AND side = 0",
                [],
            )
            .expect("corrupt");

        match store.series(forge, ITEM, Side::Sell) {
            Err(StoreError::BadData(message)) => {
                assert!(message.contains("price_history"), "{message}");
                assert!(message.contains("side"), "{message}");
            }
            other => panic!("expected bad data, got {other:?}"),
        }
    }

    #[test]
    fn a_stored_daily_period_of_zero_is_bad_data() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("eve.db");
        let store = Store::open(&path, 1_000).expect("open");
        raw(&path)
            .execute("UPDATE history_markets SET daily_days = 0", [])
            .expect("corrupt");

        match store.history_markets() {
            Err(StoreError::BadData(message)) => {
                assert!(message.contains("history_markets"), "{message}");
                assert!(message.contains("daily_days"), "{message}");
            }
            other => panic!("expected bad data, got {other:?}"),
        }
    }

    #[test]
    fn the_database_size_grows_after_a_recording() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("eve.db");
        let mut store = Store::open(&path, 1_000).expect("open");
        let before = store.database_bytes().expect("size");
        assert!(before > 0);
        let forge = forge_id(&store);

        record_days(&mut store, forge, FIRST_DAY..=LAST_DAY);

        assert!(store.database_bytes().expect("size") > before);
    }

    #[test]
    fn every_market_kind_reads_back_as_it_was_added() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut store = Store::open(&dir.path().join("eve.db"), 1_000).expect("open");
        let kinds = [
            (MarketKind::System, JITA),
            (MarketKind::Station, 60_003_760),
            (MarketKind::Structure, 1_035_466_617_946),
        ];
        for (kind, place) in kinds {
            store
                .add_history_market(kind, place, Retention::DEFAULT)
                .expect("add");
        }

        let read: Vec<(MarketKind, u64)> = store
            .history_markets()
            .expect("markets")
            .into_iter()
            .map(|market| (market.kind, market.place))
            .collect();

        assert_eq!(
            read,
            vec![
                (MarketKind::Region, FORGE),
                (MarketKind::System, JITA),
                (MarketKind::Station, 60_003_760),
                (MarketKind::Structure, 1_035_466_617_946),
            ]
        );
    }

    #[test]
    fn saved_settings_are_read_back_after_reopening() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("eve.db");
        let saved = HistorySettings {
            limit_bytes: Some(10_000),
            auto_refresh: false,
        };
        let mut store = Store::open(&path, 1_000).expect("open");
        store.save_history_settings(&saved).expect("save");
        drop(store);

        let reopened = Store::open(&path, 2_000).expect("reopen");

        assert_eq!(reopened.history_settings().expect("settings"), saved);
    }

    #[test]
    fn the_limit_drops_from_the_market_with_the_most_rows() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut store = Store::open(&dir.path().join("eve.db"), 1_000).expect("open");
        let forge = forge_id(&store);
        record_days(&mut store, forge, FIRST_DAY..=LAST_DAY);
        let jita = store
            .add_history_market(MarketKind::System, JITA, Retention::DEFAULT)
            .expect("add");
        record_days(&mut store, jita, FIRST_DAY..=FIRST_DAY + 9);

        let dropped = store.enforce_limit(18_400).expect("limit");

        assert_eq!(dropped, 10);
        assert_eq!(
            days_of(&store.series(jita, ITEM, Side::Sell).expect("jita")).len(),
            10
        );
        let forge_days = days_of(&store.series(forge, ITEM, Side::Sell).expect("forge"));
        assert_eq!(forge_days.first().copied(), Some(20_018));
    }

    #[test]
    fn on_equal_rows_the_limit_drops_from_the_lower_id() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut store = Store::open(&dir.path().join("eve.db"), 1_000).expect("open");
        let forge = forge_id(&store);
        record_days(&mut store, forge, FIRST_DAY..=FIRST_DAY + 9);
        let jita = store
            .add_history_market(MarketKind::System, JITA, Retention::DEFAULT)
            .expect("add");
        record_days(&mut store, jita, FIRST_DAY..=FIRST_DAY + 9);

        let dropped = store.enforce_limit(3_900).expect("limit");

        assert_eq!(dropped, 1);
        let forge_days = days_of(&store.series(forge, ITEM, Side::Sell).expect("forge"));
        assert_eq!(forge_days.first().copied(), Some(20_001));
        assert_eq!(
            days_of(&store.series(jita, ITEM, Side::Sell).expect("jita")).len(),
            10
        );
    }
}

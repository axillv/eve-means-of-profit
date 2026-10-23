use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use mop_core::money::Money;
use mop_core::net_worth::{CharacterKey, Holdings, ItemPrices, Part, PriceBasis, Record, Snapshot};
use mop_store::{NetWorthSettings, Store, StoreError};
use rusqlite::{Connection, params};

#[cfg(test)]
mod tests {
    use super::*;

    fn money(text: &str) -> Money {
        Money::parse_decimal(text).expect("money")
    }

    fn a() -> CharacterKey {
        CharacterKey::Id(2_112_625_428)
    }

    fn b() -> CharacterKey {
        CharacterKey::Id(95_465_499)
    }

    fn open_temp() -> (tempfile::TempDir, PathBuf, Store) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("eve.db");
        let store = Store::open(&path, 1_000).expect("open").store;
        (dir, path, store)
    }

    fn raw(path: &Path) -> Connection {
        Connection::open(path).expect("raw connection")
    }

    fn seen(character: CharacterKey, at: i64, holdings: Holdings) -> Record {
        Record {
            character,
            at,
            snapshot: Snapshot::Seen(holdings),
        }
    }

    fn wallet_only(wallet: &str) -> Holdings {
        Holdings {
            wallet: Some(money(wallet)),
            assets: BTreeMap::new(),
            sell_orders: None,
            buy_escrow: None,
            jobs_output: BTreeMap::new(),
        }
    }

    fn with_assets(assets: &[(u32, u64)]) -> Holdings {
        Holdings {
            wallet: Some(money("100.00")),
            assets: assets.iter().copied().collect(),
            sell_orders: None,
            buy_escrow: None,
            jobs_output: BTreeMap::new(),
        }
    }

    fn imported_jeve() -> BTreeMap<Part, Money> {
        BTreeMap::from([
            (Part::Wallet, money("10.00")),
            (Part::Assets, money("9.00")),
            (Part::SellOrders, money("8.00")),
            (Part::BuyEscrow, money("5.00")),
            (Part::IndustryJobs, money("7.00")),
            (Part::ContractCollateral, money("3.00")),
            (Part::Contracts, money("4.00")),
        ])
    }

    fn imported(character: CharacterKey, at: i64, parts: BTreeMap<Part, Money>) -> Record {
        Record {
            character,
            at,
            snapshot: Snapshot::Imported(parts),
        }
    }

    fn who_and_when(records: &[Record]) -> Vec<(CharacterKey, i64)> {
        records
            .iter()
            .map(|record| (record.character.clone(), record.at))
            .collect()
    }

    #[test]
    fn records_read_back_equal_after_reopening() {
        let (_dir, path, mut store) = open_temp();
        let first = seen(
            a(),
            1_000,
            Holdings {
                wallet: Some(money("100.00")),
                assets: BTreeMap::from([(34, 1_000), (11_399, 3)]),
                sell_orders: Some(money("200.00")),
                buy_escrow: None,
                jobs_output: BTreeMap::from([(2_185, 10)]),
            },
        );
        let second = seen(b(), 2_000, wallet_only("50.00"));
        let third = Record {
            character: b(),
            at: 2_500,
            snapshot: Snapshot::Removed,
        };
        store.add_record(&first).expect("add");
        store.add_record(&second).expect("add");
        store.add_record(&third).expect("add");
        drop(store);

        let mut reopened = Store::open(&path, 2_000).expect("reopen").store;

        assert_eq!(
            reopened.records(0, 10_000).expect("records"),
            vec![first, second, third]
        );
    }

    #[test]
    fn an_unchanged_item_set_is_stored_once() {
        let (_dir, path, mut store) = open_temp();
        let mut expected = Vec::new();
        for step in 1..=100 {
            let record = seen(a(), step * 1_000, with_assets(&[(34, 1_000), (11_399, 3)]));
            store.add_record(&record).expect("add");
            expected.push(record);
        }
        assert_eq!(count_rows(&path, "item_set_items"), 2);
        assert_eq!(count_rows(&path, "item_sets"), 2);

        let changed = seen(a(), 101_000, with_assets(&[(34, 999), (11_399, 3)]));
        store.add_record(&changed).expect("add");
        expected.push(changed);

        assert_eq!(count_rows(&path, "item_set_items"), 4);
        assert_eq!(count_rows(&path, "item_sets"), 3);
        assert_eq!(store.records(0, 200_000).expect("records"), expected);
    }

    fn count_rows(path: &Path, table: &str) -> i64 {
        raw(path)
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .expect("count")
    }

    #[test]
    fn a_range_starts_with_each_characters_latest_record_before_it() {
        let (_dir, _path, mut store) = open_temp();
        store
            .add_record(&seen(a(), 1_000, wallet_only("1.00")))
            .expect("add");
        store
            .add_record(&seen(a(), 3_000, wallet_only("2.00")))
            .expect("add");
        store
            .add_record(&seen(b(), 2_000, wallet_only("3.00")))
            .expect("add");

        let middle = store.records(2_500, 3_500).expect("records");
        assert_eq!(
            who_and_when(&middle),
            vec![(a(), 1_000), (b(), 2_000), (a(), 3_000)]
        );

        let late = store.records(3_500, 4_000).expect("records");
        assert_eq!(who_and_when(&late), vec![(b(), 2_000), (a(), 3_000)]);
    }

    #[test]
    fn jeve_records_skip_duplicates_and_app_data() {
        let (_dir, _path, mut store) = open_temp();
        store
            .add_record(&seen(a(), 1_000, wallet_only("100.00")))
            .expect("add");
        let name = CharacterKey::Name("TEST-NAME".to_owned());
        let n = imported(name.clone(), 1_552_492_124, imported_jeve());
        let a_early = imported(a(), 500, BTreeMap::from([(Part::Wallet, money("90.00"))]));
        let a_late = imported(a(), 1_500, BTreeMap::from([(Part::Wallet, money("90.00"))]));
        let list = [n.clone(), a_early.clone(), a_late.clone()];

        let first = store.import_records(&list).expect("import");
        let second = store.import_records(&list).expect("import again");

        assert_eq!(
            first,
            mop_store::ImportReport {
                added: 2,
                duplicates: 0,
                after_app_data: 1
            }
        );
        assert_eq!(
            second,
            mop_store::ImportReport {
                added: 0,
                duplicates: 2,
                after_app_data: 1
            }
        );
        let stored = store.records(0, 2_000_000_000).expect("records");
        let imported_n = stored
            .iter()
            .find(|record| record.character == name)
            .expect("name record");
        assert_eq!(imported_n, &n);
        assert!(stored.contains(&a_early));
        assert!(!stored.contains(&a_late));
    }

    #[test]
    fn a_seen_record_in_an_import_writes_nothing() {
        let (_dir, _path, mut store) = open_temp();
        let name = CharacterKey::Name("TEST-NAME".to_owned());
        let list = [
            imported(name, 1, imported_jeve()),
            seen(b(), 2_000, wallet_only("1.00")),
        ];
        let before = store.records(0, 10_000).expect("records");

        let err = store.import_records(&list).expect_err("must refuse");

        assert!(matches!(err, StoreError::BadData(_)), "{err}");
        assert_eq!(store.records(0, 10_000).expect("records"), before);
    }

    #[test]
    fn imported_records_are_refused_by_add_record() {
        let (_dir, _path, mut store) = open_temp();
        let record = imported(a(), 500, BTreeMap::new());

        let err = store.add_record(&record).expect_err("must refuse");

        assert!(
            matches!(&err, StoreError::BadData(what) if what == "imported records go through import_records"),
            "{err}"
        );
    }

    #[test]
    fn prices_keep_the_stored_side_when_a_fetch_leaves_it_out() {
        let (_dir, _path, mut store) = open_temp();
        let full = ItemPrices {
            lowest_sell: Some(money("4.00")),
            highest_buy: Some(money("3.00")),
            average: Some(money("3.50")),
        };
        store.save_prices(0, &[(34, full)]).expect("save");
        let sell_only = ItemPrices {
            lowest_sell: Some(money("4.10")),
            highest_buy: None,
            average: None,
        };
        store.save_prices(0, &[(34, sell_only)]).expect("save");
        let day_three = ItemPrices {
            lowest_sell: Some(money("5.00")),
            highest_buy: Some(money("4.80")),
            average: None,
        };
        store.save_prices(3, &[(34, day_three)]).expect("save");

        let history = store
            .price_history(&BTreeSet::from([34]), 1, 5)
            .expect("history");

        let day_zero = ItemPrices {
            lowest_sell: Some(money("4.10")),
            highest_buy: Some(money("3.00")),
            average: Some(money("3.50")),
        };
        assert_eq!(history.get(34, 2), Some(day_zero));
        assert_eq!(history.get(34, 3), Some(day_three));
        assert_eq!(history.get(34, 9), Some(day_three));
        assert_eq!(history.get(34, -1), None);
        assert_eq!(history.get(35, 3), None);
    }

    #[test]
    fn settings_default_to_midpoint_and_every_part() {
        let (_dir, path, mut store) = open_temp();
        let defaults = NetWorthSettings {
            basis: PriceBasis::Midpoint,
            shown: BTreeSet::from(Part::ALL),
        };
        assert_eq!(store.net_worth_settings().expect("settings"), defaults);

        let saved = NetWorthSettings {
            basis: PriceBasis::HighestBuy,
            shown: BTreeSet::from([Part::Wallet, Part::Assets]),
        };
        store.save_net_worth_settings(&saved).expect("save");
        drop(store);
        let mut reopened = Store::open(&path, 2_000).expect("reopen").store;

        assert_eq!(reopened.net_worth_settings().expect("settings"), saved);
    }

    #[test]
    fn every_price_basis_is_saved_as_chosen() {
        let (_dir, _path, mut store) = open_temp();
        for basis in [
            PriceBasis::Midpoint,
            PriceBasis::HighestBuy,
            PriceBasis::LowestSell,
            PriceBasis::Average,
        ] {
            let saved = NetWorthSettings {
                basis,
                shown: BTreeSet::from([Part::Implants]),
            };
            store.save_net_worth_settings(&saved).expect("save");

            assert_eq!(store.net_worth_settings().expect("settings"), saved);
        }
    }

    #[test]
    fn the_largest_money_and_the_smallest_price_read_back_exactly() {
        let (_dir, _path, mut store) = open_temp();
        let largest = Money::from_ten_thousandths(i64::MAX);
        let smallest = Money::from_ten_thousandths(1);
        let mut holdings = wallet_only("0.00");
        holdings.wallet = Some(largest);
        store.add_record(&seen(a(), 1_000, holdings)).expect("add");
        let price = ItemPrices {
            lowest_sell: Some(smallest),
            highest_buy: None,
            average: None,
        };
        store.save_prices(0, &[(34, price)]).expect("save");

        let records = store.records(0, 2_000).expect("records");
        let history = store
            .price_history(&BTreeSet::from([34]), 0, 0)
            .expect("history");

        assert_eq!(
            records[0].snapshot,
            Snapshot::Seen(Holdings {
                wallet: Some(largest),
                ..with_assets(&[])
            })
        );
        assert_eq!(history.get(34, 0), Some(price));
    }

    #[test]
    fn the_database_refuses_bad_values_and_a_damaged_kind_is_reported() {
        let (_dir, path, mut store) = open_temp();
        let id = store
            .add_record(&Record {
                character: a(),
                at: 1_000,
                snapshot: Snapshot::Removed,
            })
            .expect("add");
        let conn = raw(&path);

        conn.execute(
            "INSERT INTO item_prices (day, type_id, lowest_sell) VALUES (0, 34, -1)",
            [],
        )
        .expect_err("a negative price is refused");
        conn.execute(
            "INSERT INTO net_worth_records (character_id, character_name, at, kind) \
             VALUES (1, 'X', 1, 2)",
            [],
        )
        .expect_err("a record with two characters is refused");
        conn.execute(
            "UPDATE net_worth_records SET kind = 9 WHERE id = ?1",
            params![id],
        )
        .expect_err("an unknown kind is refused");

        conn.execute_batch("PRAGMA ignore_check_constraints = ON;")
            .expect("pragma");
        conn.execute(
            "UPDATE net_worth_records SET kind = 9 WHERE id = ?1",
            params![id],
        )
        .expect("damage");
        let err = store.records(0, 2_000).expect_err("must refuse");

        assert!(
            matches!(&err, StoreError::BadData(what) if what.contains("net_worth_records")
                && what.contains(&format!("row {id}"))
                && what.contains("column kind")),
            "{err}"
        );
    }
}

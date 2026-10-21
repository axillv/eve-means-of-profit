use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use profit_core::stockpile::{Characters, Filter, Include, Place, Source, Stockpile, Target};
use profit_store::{Store, StoreError};
use rusqlite::Connection;

#[cfg(test)]
mod tests {
    use super::*;

    const MAIN: i64 = 2_112_000_001;
    const JITA_4_4: i64 = 60_003_760;
    const JITA_SYSTEM: u32 = 30_000_142;

    fn open_temp() -> (tempfile::TempDir, PathBuf, Store) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("eve.db");
        let store = Store::open(&path, 1_000).expect("open").store;
        (dir, path, store)
    }

    fn target(type_id: u32, units: u64) -> Target {
        Target {
            type_id,
            units,
            ignore_multiplier: false,
        }
    }

    fn include(stockpile_id: i64, count: u32) -> Include {
        Include {
            stockpile_id,
            count,
        }
    }

    fn sources(listed: &[Source]) -> BTreeSet<Source> {
        listed.iter().copied().collect()
    }

    /// Kit as the spec lists it: F1 is everyone at Jita 4-4 for assets and sell orders; F2 is Main
    /// in the Jita system for assets and jobs.
    fn kit_with(id: i64) -> Stockpile {
        Stockpile {
            id,
            name: "Kit".to_owned(),
            group: Some("Drones".to_owned()),
            multiplier: 1,
            targets: vec![target(100, 10), target(200, 50), target(300, 5)],
            includes: vec![],
            filters: vec![
                Filter {
                    characters: Characters::All,
                    place: Place::Location(JITA_4_4),
                    sources: sources(&[Source::Assets, Source::SellOrders]),
                },
                Filter {
                    characters: Characters::Only(BTreeSet::from([MAIN])),
                    place: Place::System(JITA_SYSTEM),
                    sources: sources(&[Source::Assets, Source::Jobs]),
                },
            ],
        }
    }

    /// Creates Kit, saves it, and returns its id.
    fn saved_kit(store: &mut Store) -> i64 {
        let id = store
            .create_stockpile("Kit", Some("Drones"))
            .expect("create");
        store.save_stockpile(&kit_with(id)).expect("save");
        id
    }

    /// Creates Weekly (multiplier 2) including `kit` three times; returns its id.
    fn saved_weekly(store: &mut Store, kit: i64) -> i64 {
        let id = store.create_stockpile("Weekly", None).expect("create");
        store
            .save_stockpile(&Stockpile {
                id,
                name: "Weekly".to_owned(),
                group: None,
                multiplier: 2,
                targets: vec![target(100, 1)],
                includes: vec![include(kit, 3)],
                filters: vec![],
            })
            .expect("save");
        id
    }

    fn row_count(path: &Path, sql: &str, id: i64) -> i64 {
        let conn = Connection::open(path).expect("open");
        conn.query_row(sql, [id], |row| row.get(0)).expect("count")
    }

    #[test]
    fn a_saved_stockpile_reads_back_exactly() {
        let (_dir, _path, mut store) = open_temp();
        let id = saved_kit(&mut store);

        let read = store.stockpile(id).expect("read").expect("some");

        assert_eq!(read, kit_with(id));
    }

    #[test]
    fn a_new_stockpile_is_empty_with_multiplier_one() {
        let (_dir, _path, mut store) = open_temp();
        let id = store.create_stockpile("Hammer", None).expect("create");

        let read = store.stockpile(id).expect("read").expect("some");

        assert_eq!(
            read,
            Stockpile {
                id,
                name: "Hammer".to_owned(),
                group: None,
                multiplier: 1,
                targets: vec![],
                includes: vec![],
                filters: vec![],
            }
        );
    }

    #[test]
    fn the_list_keeps_creation_order_until_it_is_reordered() {
        let (_dir, _path, mut store) = open_temp();
        let kit = store.create_stockpile("Kit", None).expect("kit");
        let hammer = store.create_stockpile("Hammer", None).expect("hammer");
        let weekly = store.create_stockpile("Weekly", None).expect("weekly");

        let names = |store: &mut Store| -> Vec<String> {
            store
                .stockpiles()
                .expect("list")
                .into_iter()
                .map(|s| s.name)
                .collect()
        };
        assert_eq!(names(&mut store), ["Kit", "Hammer", "Weekly"]);

        store
            .set_stockpile_order(&[weekly, kit, hammer])
            .expect("order");
        assert_eq!(names(&mut store), ["Weekly", "Kit", "Hammer"]);
    }

    #[test]
    fn a_loop_of_includes_is_refused_and_the_stored_stockpile_is_unchanged() {
        let (_dir, _path, mut store) = open_temp();
        let kit = saved_kit(&mut store);
        let weekly = saved_weekly(&mut store, kit);
        let before = store.stockpile(kit).expect("read").expect("some");

        let mut looping = before.clone();
        looping.includes.push(include(weekly, 1));
        let err = store.save_stockpile(&looping).expect_err("must refuse");

        assert!(
            matches!(&err, StoreError::StockpileLoop(names) if names == &["Kit", "Weekly", "Kit"]),
            "{err:?}"
        );
        assert_eq!(err.to_string(), "includes itself: Kit → Weekly → Kit");
        assert_eq!(store.stockpile(kit).expect("read"), Some(before));
    }

    /// One candidate per refusal, each changing one thing in `good`, with the refusal it gets.
    fn refusals(good: &Stockpile, hammer: i64) -> Vec<(Stockpile, StoreError)> {
        vec![
            (
                Stockpile {
                    name: "  ".to_owned(),
                    ..good.clone()
                },
                StoreError::EmptyName,
            ),
            (
                Stockpile {
                    id: hammer,
                    name: "kit".to_owned(),
                    ..good.clone()
                },
                StoreError::NameTaken("kit".to_owned()),
            ),
            (
                Stockpile {
                    multiplier: 0,
                    ..good.clone()
                },
                StoreError::InvalidStockpile("multiplier must be 1 or more".to_owned()),
            ),
            (
                Stockpile {
                    targets: vec![target(100, 0)],
                    ..good.clone()
                },
                StoreError::InvalidStockpile("units must be 1 or more".to_owned()),
            ),
            (
                Stockpile {
                    includes: vec![include(hammer, 0)],
                    ..good.clone()
                },
                StoreError::InvalidStockpile("count must be 1 or more".to_owned()),
            ),
            (
                Stockpile {
                    targets: vec![target(100, 10), target(100, 5)],
                    ..good.clone()
                },
                StoreError::InvalidStockpile("type 100 is listed twice".to_owned()),
            ),
            (
                Stockpile {
                    filters: vec![Filter {
                        characters: Characters::All,
                        place: Place::Everywhere,
                        sources: sources(&[]),
                    }],
                    ..good.clone()
                },
                StoreError::InvalidStockpile("a filter needs at least one source".to_owned()),
            ),
            (
                Stockpile {
                    filters: vec![Filter {
                        characters: Characters::Only(Default::default()),
                        place: Place::Everywhere,
                        sources: sources(&[Source::Assets]),
                    }],
                    ..good.clone()
                },
                StoreError::InvalidStockpile(
                    "a character filter needs at least one character".to_owned(),
                ),
            ),
            (
                Stockpile {
                    includes: vec![include(99, 1)],
                    ..good.clone()
                },
                StoreError::NotFound,
            ),
        ]
    }

    #[test]
    fn each_refusal_writes_nothing() {
        let (_dir, _path, mut store) = open_temp();
        let kit = saved_kit(&mut store);
        let hammer = store.create_stockpile("Hammer", None).expect("hammer");
        let before = store.stockpile(kit).expect("read").expect("some");

        for (candidate, expected) in refusals(&before, hammer) {
            let err = store.save_stockpile(&candidate).expect_err("must refuse");
            assert_eq!(format!("{err:?}"), format!("{expected:?}"));
            assert_eq!(store.stockpile(kit).expect("read"), Some(before.clone()));
        }
    }

    #[test]
    fn a_loop_names_the_stockpiles_by_their_trimmed_names() {
        let (_dir, _path, mut store) = open_temp();
        let kit = saved_kit(&mut store);
        let weekly = saved_weekly(&mut store, kit);
        let before = store.stockpile(kit).expect("read").expect("some");

        let mut looping = before.clone();
        looping.name = "  Kit  ".to_owned();
        looping.includes.push(include(weekly, 1));
        let err = store.save_stockpile(&looping).expect_err("must refuse");

        assert!(
            matches!(&err, StoreError::StockpileLoop(names) if names == &["Kit", "Weekly", "Kit"]),
            "{err:?}"
        );
        assert_eq!(store.stockpile(kit).expect("read"), Some(before));
    }

    #[test]
    fn an_order_with_a_repeat_or_an_unknown_id_is_refused_and_nothing_moves() {
        let (_dir, _path, mut store) = open_temp();
        let kit = store.create_stockpile("Kit", None).expect("kit");
        store.create_stockpile("Hammer", None).expect("hammer");
        let weekly = store.create_stockpile("Weekly", None).expect("weekly");
        let names = |store: &mut Store| -> Vec<String> {
            store
                .stockpiles()
                .expect("list")
                .into_iter()
                .map(|s| s.name)
                .collect()
        };

        let repeat = store
            .set_stockpile_order(&[weekly, kit, kit])
            .expect_err("a repeat");
        let unknown = store
            .set_stockpile_order(&[weekly, kit, 99])
            .expect_err("an unknown id");

        assert!(matches!(repeat, StoreError::NotFound));
        assert!(matches!(unknown, StoreError::NotFound));
        assert_eq!(names(&mut store), ["Kit", "Hammer", "Weekly"]);
    }

    #[test]
    fn a_stockpile_in_use_cannot_be_deleted_until_the_include_is_removed() {
        let (_dir, path, mut store) = open_temp();
        let kit = saved_kit(&mut store);
        let weekly = saved_weekly(&mut store, kit);

        let err = store.delete_stockpile(kit).expect_err("in use");
        assert!(
            matches!(&err, StoreError::InUse { plans } if plans == &["Weekly"]),
            "{err:?}"
        );
        assert!(store.stockpile(kit).expect("read").is_some());

        store
            .save_stockpile(&Stockpile {
                id: weekly,
                name: "Weekly".to_owned(),
                group: None,
                multiplier: 2,
                targets: vec![target(100, 1)],
                includes: vec![],
                filters: vec![],
            })
            .expect("save");
        store.delete_stockpile(kit).expect("delete");

        assert!(store.stockpile(kit).expect("read").is_none());
        assert_eq!(
            row_count(
                &path,
                "SELECT count(*) FROM stockpile_targets WHERE stockpile = ?1",
                kit
            ),
            0
        );
        assert_eq!(
            row_count(
                &path,
                "SELECT count(*) FROM stockpile_filters WHERE stockpile = ?1",
                kit
            ),
            0
        );
    }

    #[test]
    fn the_order_must_name_every_stockpile_once() {
        let (_dir, _path, mut store) = open_temp();
        let kit = store.create_stockpile("Kit", None).expect("kit");
        let hammer = store.create_stockpile("Hammer", None).expect("hammer");
        let weekly = store.create_stockpile("Weekly", None).expect("weekly");
        let names = |store: &mut Store| -> Vec<String> {
            store
                .stockpiles()
                .expect("list")
                .into_iter()
                .map(|s| s.name)
                .collect()
        };

        store
            .set_stockpile_order(&[weekly, kit, hammer])
            .expect("order");
        assert_eq!(names(&mut store), ["Weekly", "Kit", "Hammer"]);

        let err = store
            .set_stockpile_order(&[weekly, kit])
            .expect_err("missing one");
        assert!(matches!(err, StoreError::NotFound));
        assert_eq!(names(&mut store), ["Weekly", "Kit", "Hammer"]);
    }

    /// True when SQLite refused the statement because a CHECK constraint failed.
    fn is_check_failure(result: &rusqlite::Result<usize>) -> bool {
        matches!(
            result,
            Err(rusqlite::Error::SqliteFailure(failure, _))
                if failure.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_CHECK
        )
    }

    #[test]
    fn the_file_refuses_zero_multipliers_units_and_sourceless_filters() {
        let (_dir, path, mut store) = open_temp();
        let kit = saved_kit(&mut store);
        let conn = Connection::open(&path).expect("open");

        let multiplier = conn.execute(
            "INSERT INTO stockpiles (name, multiplier, position) VALUES ('Raw', 0, 9)",
            [],
        );
        let units = conn.execute(
            "INSERT INTO stockpile_targets (stockpile, type, units, ignore_multiplier) \
             VALUES (?1, 500, 0, 0)",
            [kit],
        );
        let no_source = conn.execute(
            "INSERT INTO stockpile_filters (stockpile, position, place_kind, place_id, assets, \
                sell_orders, buy_orders, jobs) VALUES (?1, 9, 0, NULL, 0, 0, 0, 0)",
            [kit],
        );

        assert!(is_check_failure(&multiplier), "{multiplier:?}");
        assert!(is_check_failure(&units), "{units:?}");
        assert!(is_check_failure(&no_source), "{no_source:?}");
    }

    #[test]
    fn a_place_kind_the_app_cannot_read_is_bad_data_naming_the_row() {
        let (_dir, path, mut store) = open_temp();
        let kit = saved_kit(&mut store);
        let filter: i64 = {
            let conn = Connection::open(&path).expect("open");
            conn.query_row(
                "SELECT id FROM stockpile_filters WHERE stockpile = ?1 ORDER BY position LIMIT 1",
                [kit],
                |row| row.get(0),
            )
            .expect("filter")
        };
        let conn = Connection::open(&path).expect("open");
        conn.execute_batch("PRAGMA ignore_check_constraints = ON;")
            .expect("pragma");
        conn.execute(
            "UPDATE stockpile_filters SET place_kind = 7 WHERE id = ?1",
            [filter],
        )
        .expect("update");

        let err = store.stockpiles().expect_err("bad data");

        assert!(
            matches!(&err, StoreError::BadData(what) if what.contains("stockpile_filters")
                && what.contains(&format!("row {filter}"))
                && what.contains("place_kind")),
            "{err}"
        );
    }
}

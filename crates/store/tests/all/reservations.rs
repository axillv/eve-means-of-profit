use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use profit_core::freight::Packing;
use profit_core::reserve::ShortMode;
use profit_core::stock::{Places, StockScope};
use profit_core::stockpile::Target;
use profit_market::book::PriceRule;
use profit_store::{PlanRecord, PlanStock, Store, StoreError};
use rusqlite::{Connection, params};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::support::undo_version_six;

    const MAIN: i64 = 2_112_000_001;
    const ALT: i64 = 2_112_000_002;
    const JITA_4_4: i64 = 60_003_760;
    const STRUCTURE: i64 = 1_035_466_617_946;

    /// beta, Alpha and alpha, so their ids are 1, 2 and 3, plus the Kit stockpile. The three
    /// names differ only in case, so they are written into the file as a migrated file would
    /// carry them; the store would refuse one of them.
    fn store_with_plans() -> (tempfile::TempDir, PathBuf, Store) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("user.sqlite");
        version_four_of(&path);
        let store = Store::open(&path, 2_000).expect("upgrade");
        (dir, path, store)
    }

    /// Makes the file a version-4 one that carries beta 1, Alpha 2, alpha 3 and Kit, the way the
    /// acceptance checks need it.
    fn version_four_of(path: &Path) {
        drop(Store::open(path, 1_000).expect("open"));
        undo_version_six(path);
        undo_version_five(path);
        raw_plan_row(path, 1, "beta");
        raw_plan_row(path, 2, "Alpha");
        raw_plan_row(path, 3, "alpha");
        Connection::open(path)
            .expect("open")
            .execute(
                "INSERT INTO stockpiles (id, name, group_name, multiplier, position) \
                 VALUES (1, 'Kit', NULL, 1, 0)",
                params![],
            )
            .expect("insert stockpile");
    }

    fn raw_plan_row(path: &Path, id: i64, name: &str) {
        Connection::open(path)
            .expect("open")
            .execute(
                "INSERT INTO plans (id, name, product, quantity, price_rule, packing, \
                 pasted_stock, created_at, updated_at) VALUES (?1, ?2, 2456, 1, 0, 0, '', 1_000, 1_000)",
                params![id, name],
            )
            .expect("insert plan");
    }

    fn plan_named(name: &str) -> PlanRecord {
        PlanRecord {
            id: 0,
            name: name.to_owned(),
            product: 2456,
            quantity: 1,
            build: BTreeSet::new(),
            me: BTreeMap::new(),
            manufacturing: None,
            reactions: None,
            price_rule: PriceRule::WalkSell,
            route: None,
            service: None,
            packing: Packing::TopUp,
            pasted_stock: String::new(),
            created_at: 0,
            updated_at: 0,
        }
    }

    fn names(store: &mut Store) -> Vec<String> {
        store
            .plans()
            .expect("plans")
            .into_iter()
            .map(|plan| plan.name)
            .collect()
    }

    fn scope(characters: &[i64], places: Places) -> StockScope {
        StockScope {
            characters: characters.iter().copied().collect(),
            places,
        }
    }

    fn only(places: &[i64]) -> Places {
        Places::Only(places.iter().copied().collect())
    }

    /// A stored count, read straight from the file.
    fn count_of(path: &Path, sql: &str, id: i64) -> i64 {
        Connection::open(path)
            .expect("open")
            .query_row(sql, [id], |row| row.get(0))
            .expect("count")
    }

    /// Undoes what version 5 added, so the file reads as a version-4 file again.
    fn undo_version_five(path: &Path) {
        let conn = Connection::open(path).expect("open");
        conn.execute_batch(
            "PRAGMA foreign_keys = OFF;
             DROP TABLE job_assignments;
             DROP TABLE plan_stock_characters;
             DROP TABLE plan_stock_places;
             ALTER TABLE plans DROP COLUMN position;
             ALTER TABLE plans DROP COLUMN reserve;
             ALTER TABLE plans DROP COLUMN stock_source;
             ALTER TABLE stockpiles DROP COLUMN short_mode;
             DELETE FROM migrations WHERE version = 5;
             PRAGMA user_version = 4;",
        )
        .expect("downgrade");
    }

    /// Drops the checks on the stored codes, so raw SQL can store one Rust cannot map back.
    fn loosen_code_checks(path: &Path) {
        let conn = Connection::open(path).expect("open");
        conn.execute_batch(
            "PRAGMA foreign_keys = OFF;
             ALTER TABLE plans DROP COLUMN stock_source;
             ALTER TABLE plans ADD COLUMN stock_source INTEGER;
             ALTER TABLE stockpiles DROP COLUMN short_mode;
             ALTER TABLE stockpiles ADD COLUMN short_mode INTEGER;",
        )
        .expect("loosen checks");
    }

    #[test]
    fn a_version_four_file_is_kept_aside_and_upgraded() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("user.sqlite");
        version_four_of(&path);
        let before = std::fs::read(&path).expect("read");

        let mut store = Store::open(&path, 2_000).expect("upgrade");

        assert_eq!(
            std::fs::read(dir.path().join("user.sqlite.before-v7")).expect("backup"),
            before
        );
        let version: u32 = Connection::open(&path)
            .expect("open")
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("version");
        assert_eq!(version, 7);
        assert_eq!(names(&mut store), vec!["Alpha", "alpha", "beta"]);
        let conn = Connection::open(&path).expect("open");
        let position = |id: i64| -> i64 {
            conn.query_row("SELECT position FROM plans WHERE id = ?1", [id], |row| {
                row.get(0)
            })
            .expect("position")
        };
        assert_eq!((position(2), position(3), position(1)), (0, 1, 2));
        for id in 1..=3 {
            assert_eq!(store.plan_stock(id).expect("stock"), PlanStock::Pasted);
        }
        assert!(store.reserving_plans().expect("reserving").is_empty());
        assert_eq!(store.short_mode(1).expect("mode"), ShortMode::AfterPlans);
    }

    #[test]
    fn create_plan_goes_last_and_the_order_must_list_every_plan_once() {
        let (_dir, _path, mut store) = store_with_plans();

        store
            .create_plan(&plan_named("Gamma"), 1_000)
            .expect("create");
        assert_eq!(names(&mut store), vec!["Alpha", "alpha", "beta", "Gamma"]);

        store.set_plan_order(&[1, 4, 2, 3]).expect("order");
        assert_eq!(names(&mut store), vec!["beta", "Gamma", "Alpha", "alpha"]);

        for bad in [&[1, 2, 3][..], &[1, 4, 2, 9][..], &[1, 1, 2, 3][..]] {
            assert!(
                matches!(store.set_plan_order(bad), Err(StoreError::NotFound)),
                "{bad:?}"
            );
        }
        assert_eq!(names(&mut store), vec!["beta", "Gamma", "Alpha", "alpha"]);
    }

    #[test]
    fn a_plan_stock_scope_is_set_checked_and_read_back() {
        let (_dir, path, mut store) = store_with_plans();

        let wanted = scope(&[MAIN, ALT], only(&[JITA_4_4, STRUCTURE]));
        store
            .set_plan_stock(1, &PlanStock::Characters(wanted.clone()))
            .expect("set");
        assert_eq!(
            store.plan_stock(1).expect("read"),
            PlanStock::Characters(wanted)
        );

        store
            .set_plan_stock(1, &PlanStock::Characters(scope(&[MAIN], Places::All)))
            .expect("all places");
        assert_eq!(
            store.plan_stock(1).expect("read"),
            PlanStock::Characters(scope(&[MAIN], Places::All))
        );
        assert_eq!(
            count_of(
                &path,
                "SELECT count(*) FROM plan_stock_places WHERE plan_id = ?1",
                1
            ),
            0
        );

        let kept = store.plan_stock(1).expect("read");
        let errors = [
            (scope(&[], Places::All), "pick at least one character"),
            (scope(&[], only(&[JITA_4_4])), "pick at least one character"),
            (
                scope(&[MAIN], only(&[])),
                "pick at least one place, or all places",
            ),
        ];
        for (bad, reason) in errors {
            let Err(StoreError::InvalidPlanStock(message)) =
                store.set_plan_stock(1, &PlanStock::Characters(bad))
            else {
                panic!("expected InvalidPlanStock");
            };
            assert_eq!(message, reason);
        }
        assert_eq!(store.plan_stock(1).expect("unchanged"), kept);
        assert_eq!(
            count_of(
                &path,
                "SELECT count(*) FROM plan_stock_characters WHERE plan_id = ?1",
                1
            ),
            1
        );
        assert_eq!(
            count_of(
                &path,
                "SELECT count(*) FROM plan_stock_places WHERE plan_id = ?1",
                1
            ),
            0
        );
    }

    #[test]
    fn reserve_needs_character_stock_and_plans_are_served_in_list_order() {
        let (_dir, _path, mut store) = store_with_plans();

        let Err(StoreError::InvalidPlanStock(message)) = store.set_reserve(2, true) else {
            panic!("expected InvalidPlanStock");
        };
        assert_eq!(message, "reserve needs stock from characters");

        store
            .set_plan_stock(1, &PlanStock::Characters(scope(&[MAIN, ALT], Places::All)))
            .expect("stock 1");
        store
            .set_plan_stock(3, &PlanStock::Characters(scope(&[MAIN], Places::All)))
            .expect("stock 3");
        store.set_reserve(1, true).expect("reserve 1");
        store.set_reserve(3, true).expect("reserve 3");
        store.set_plan_order(&[3, 1, 2]).expect("order");
        assert_eq!(store.reserving_plans().expect("reserving"), vec![3, 1]);

        store.set_plan_stock(3, &PlanStock::Pasted).expect("pasted");
        assert_eq!(store.reserving_plans().expect("reserving"), vec![1]);
    }

    #[test]
    fn saving_keeps_the_reservation_and_a_duplicate_starts_clean() {
        let (_dir, _path, mut store) = store_with_plans();
        let wanted = scope(&[MAIN, ALT], only(&[JITA_4_4, STRUCTURE]));
        store
            .set_plan_stock(1, &PlanStock::Characters(wanted.clone()))
            .expect("set");
        store.set_reserve(1, true).expect("reserve");
        store.set_plan_order(&[3, 1, 2]).expect("order");

        let mut plan = store.plan(1).expect("plan").expect("exists");
        plan.quantity = 5;
        store.save_plan(&plan, 2_000).expect("save");
        assert_eq!(
            store.plan(1).expect("plan").map(|read| read.quantity),
            Some(5)
        );
        assert_eq!(names(&mut store), vec!["alpha", "beta", "Alpha"]);
        assert_eq!(
            store.plan_stock(1).expect("stock"),
            PlanStock::Characters(wanted.clone())
        );
        assert_eq!(store.reserving_plans().expect("reserving"), vec![1]);

        let copy = store.duplicate_plan(1, 3_000).expect("duplicate");
        assert_eq!(
            names(&mut store).last().map(String::as_str),
            Some("beta (copy)")
        );
        assert_eq!(
            store.plan_stock(copy).expect("stock"),
            PlanStock::Characters(wanted)
        );
        assert_eq!(store.reserving_plans().expect("reserving"), vec![1]);
    }

    #[test]
    fn a_job_is_reassigned_and_its_assignment_is_dropped_with_the_plan() {
        let (_dir, path, mut store) = store_with_plans();

        store.assign_job(5001, 1, 1000).expect("assign");
        store.assign_job(5002, 3, 1001).expect("assign");
        store.assign_job(5001, 3, 1002).expect("replace");
        assert_eq!(
            store.job_assignments().expect("list"),
            BTreeMap::from([(5001, 3), (5002, 3)])
        );
        assert!(matches!(
            store.assign_job(5003, 99, 1),
            Err(StoreError::NotFound)
        ));

        store.unassign_job(5002).expect("unassign");
        store.unassign_job(5002).expect("unassign again");
        assert_eq!(
            store.job_assignments().expect("list"),
            BTreeMap::from([(5001, 3)])
        );

        store
            .set_plan_stock(3, &PlanStock::Characters(scope(&[MAIN], only(&[JITA_4_4]))))
            .expect("scope");
        store.delete_plan(3).expect("delete");
        assert!(store.job_assignments().expect("list").is_empty());
        assert_eq!(
            count_of(
                &path,
                "SELECT count(*) FROM plan_stock_characters WHERE plan_id = ?1",
                3
            ),
            0
        );
        assert_eq!(
            count_of(
                &path,
                "SELECT count(*) FROM plan_stock_places WHERE plan_id = ?1",
                3
            ),
            0
        );
    }

    #[test]
    fn a_stockpile_keeps_its_short_mode_over_saving() {
        let (_dir, _path, mut store) = store_with_plans();

        store.set_short_mode(1, ShortMode::RightNow).expect("set");
        let mut kit = store.stockpile(1).expect("read").expect("kit");
        kit.targets.push(Target {
            type_id: 2454,
            units: 20,
            ignore_multiplier: false,
        });
        store.save_stockpile(&kit).expect("save");
        assert_eq!(store.short_mode(1).expect("read"), ShortMode::RightNow);

        store.set_short_mode(1, ShortMode::AfterPlans).expect("set");
        assert_eq!(store.short_mode(1).expect("read"), ShortMode::AfterPlans);

        assert!(matches!(store.short_mode(99), Err(StoreError::NotFound)));
        assert!(matches!(
            store.set_short_mode(99, ShortMode::RightNow),
            Err(StoreError::NotFound)
        ));
    }

    #[test]
    fn the_database_refuses_unknown_codes_without_rust_checks() {
        let (_dir, path, store) = store_with_plans();
        drop(store);
        let conn = Connection::open(&path).expect("open");

        for sql in [
            "UPDATE plans SET reserve = 2 WHERE id = 1",
            "UPDATE plans SET stock_source = 2 WHERE id = 1",
            "UPDATE stockpiles SET short_mode = 2 WHERE id = 1",
        ] {
            let failed = conn.execute(sql, params![]);
            assert!(
                matches!(failed, Err(rusqlite::Error::SqliteFailure(ref e, _))
                    if e.code == rusqlite::ErrorCode::ConstraintViolation),
                "{sql}"
            );
        }
    }

    #[test]
    fn a_stored_code_rust_cannot_map_back_is_bad_data() {
        let (_dir, path, store) = store_with_plans();
        drop(store);
        loosen_code_checks(&path);
        let conn = Connection::open(&path).expect("open");
        conn.execute("UPDATE plans SET stock_source = 7 WHERE id = 1", params![])
            .expect("set stock_source");
        conn.execute(
            "UPDATE stockpiles SET short_mode = 7 WHERE id = 1",
            params![],
        )
        .expect("set short_mode");

        let mut store = Store::open(&path, 2_000).expect("reopen");

        match store.plan_stock(1) {
            Err(StoreError::BadData(message)) => {
                assert!(message.contains("plans"), "{message}");
                assert!(message.contains("row 1,"), "{message}");
                assert!(message.contains("stock_source"), "{message}");
            }
            other => panic!("expected BadData, got {other:?}"),
        }
        match store.short_mode(1) {
            Err(StoreError::BadData(message)) => {
                assert!(message.contains("stockpiles"), "{message}");
                assert!(message.contains("row 1,"), "{message}");
                assert!(message.contains("short_mode"), "{message}");
            }
            other => panic!("expected BadData, got {other:?}"),
        }
        match store.set_reserve(1, true) {
            Err(StoreError::BadData(message)) => {
                assert!(message.contains("plans"), "{message}");
                assert!(message.contains("row 1,"), "{message}");
                assert!(message.contains("stock_source"), "{message}");
            }
            other => panic!("expected BadData, got {other:?}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::PathBuf;

    use mop_core::freight::Packing;
    use mop_core::money::Money;
    use mop_market::book::PriceRule;
    use mop_store::manual_prices::ManualPrice;
    use mop_store::{PlanRecord, Store, StoreError};
    use rusqlite::{Connection, params};

    const ROBOTICS: u32 = 9848;
    const MORPHITE: u32 = 34;

    /// A whole number of ISK as the stored count of ten-thousandths.
    fn isk(whole: i64) -> Money {
        Money::from_ten_thousandths(whole * 10_000)
    }

    fn open_temp() -> (tempfile::TempDir, PathBuf, Store) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("eve.db");
        let store = Store::open(&path, 1_000).expect("open").store;
        (dir, path, store)
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

    fn price(type_id: u32, per_unit: Money, set_at: i64) -> ManualPrice {
        ManualPrice {
            type_id,
            per_unit,
            set_at,
        }
    }

    #[test]
    fn a_fresh_store_has_no_manual_prices() {
        let (_dir, path, mut store) = open_temp();
        let plan = store.create_plan(&plan_named("A"), 1_000).expect("plan");
        assert_eq!(store.manual_prices(plan).expect("prices"), Vec::new());
        let conn = Connection::open(&path).expect("open");
        let rows: i64 = conn
            .query_row("SELECT count(*) FROM plan_manual_prices", [], |row| {
                row.get(0)
            })
            .expect("count");
        assert_eq!(rows, 0);
    }

    #[test]
    fn manual_prices_survive_a_restart_in_type_order() {
        let (_dir, path, mut store) = open_temp();
        let a = store.create_plan(&plan_named("A"), 1_000).expect("plan");
        store
            .set_manual_price(a, ROBOTICS, isk(1_000_000_000), 2_000_000)
            .expect("robotics");
        store
            .set_manual_price(a, MORPHITE, Money::from_ten_thousandths(42_500), 2_000_100)
            .expect("morphite");
        drop(store);

        let mut reopened = Store::open(&path, 3_000).expect("reopen").store;
        assert_eq!(
            reopened.manual_prices(a).expect("prices"),
            vec![
                price(MORPHITE, Money::from_ten_thousandths(42_500), 2_000_100),
                price(ROBOTICS, isk(1_000_000_000), 2_000_000),
            ]
        );

        reopened
            .set_manual_price(a, ROBOTICS, isk(900_000_000), 2_000_500)
            .expect("replaced");
        assert_eq!(
            reopened.manual_prices(a).expect("prices"),
            vec![
                price(MORPHITE, Money::from_ten_thousandths(42_500), 2_000_100),
                price(ROBOTICS, isk(900_000_000), 2_000_500),
            ]
        );

        reopened.clear_manual_price(a, MORPHITE).expect("cleared");
        assert_eq!(
            reopened.manual_prices(a).expect("prices"),
            vec![price(ROBOTICS, isk(900_000_000), 2_000_500)]
        );
        reopened
            .clear_manual_price(a, MORPHITE)
            .expect("clearing again is fine");
    }

    #[test]
    fn a_file_with_the_old_layout_is_set_aside_and_the_new_one_has_the_table() {
        let (_dir, path, store) = open_temp();
        drop(store);
        Connection::open(&path)
            .expect("open")
            .execute("UPDATE meta SET layout = layout || ' '", [])
            .expect("change layout");

        let opened = Store::open(&path, 2_000).expect("open");

        assert!(opened.started_fresh.is_some());
        drop(opened);
        let rows: i64 = Connection::open(&path)
            .expect("open")
            .query_row("SELECT count(*) FROM plan_manual_prices", [], |row| {
                row.get(0)
            })
            .expect("the table exists");
        assert_eq!(rows, 0);
    }

    #[test]
    fn a_zero_or_negative_price_is_refused_and_nothing_is_written() {
        let (_dir, _path, mut store) = open_temp();
        let a = store.create_plan(&plan_named("A"), 1_000).expect("plan");
        assert!(matches!(
            store.set_manual_price(a, MORPHITE, Money::ZERO, 2_000_100),
            Err(StoreError::BadPrice)
        ));
        assert!(matches!(
            store.set_manual_price(a, MORPHITE, Money::from_ten_thousandths(-1), 2_000_100),
            Err(StoreError::BadPrice)
        ));
        assert_eq!(store.manual_prices(a).expect("prices"), Vec::new());
    }

    #[test]
    fn a_price_for_a_plan_that_is_not_saved_is_not_found() {
        let (_dir, _path, mut store) = open_temp();
        assert!(matches!(
            store.set_manual_price(999, MORPHITE, isk(4), 2_000_100),
            Err(StoreError::NotFound)
        ));
        assert_eq!(store.manual_prices(999).expect("prices"), Vec::new());
    }

    #[test]
    fn deleting_a_plan_deletes_its_manual_prices_and_keeps_the_others() {
        let (_dir, _path, mut store) = open_temp();
        let a = store.create_plan(&plan_named("A"), 1_000).expect("plan");
        let b = store.create_plan(&plan_named("B"), 1_000).expect("plan");
        store
            .set_manual_price(a, ROBOTICS, isk(1_000_000_000), 2_000_000)
            .expect("a");
        store
            .set_manual_price(b, MORPHITE, isk(4), 2_000_100)
            .expect("b");

        store.delete_plan(a).expect("deleted");
        assert_eq!(store.manual_prices(a).expect("prices"), Vec::new());
        assert_eq!(
            store.manual_prices(b).expect("prices"),
            vec![price(MORPHITE, isk(4), 2_000_100)]
        );
    }

    #[test]
    fn a_stored_price_of_zero_is_bad_data_that_names_the_row() {
        let (_dir, path, mut store) = open_temp();
        let a = store.create_plan(&plan_named("A"), 1_000).expect("plan");
        store
            .set_manual_price(a, ROBOTICS, isk(1_000_000_000), 2_000_000)
            .expect("robotics");
        drop(store);

        let raw = Connection::open(&path).expect("open");
        raw.execute(
            "UPDATE plan_manual_prices SET per_unit = 0 WHERE plan = ?1",
            params![a],
        )
        .expect("raw update");
        drop(raw);

        let mut store = Store::open(&path, 3_000).expect("reopen").store;
        match store.manual_prices(a) {
            Err(StoreError::BadData(text)) => {
                assert!(text.contains("plan_manual_prices"), "{text}");
                assert!(text.contains("per_unit"), "{text}");
                assert!(text.contains(&a.to_string()), "{text}");
            }
            other => panic!("expected bad data, got {other:?}"),
        }
    }
}

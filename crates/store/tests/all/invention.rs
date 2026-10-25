#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    use mop_core::invention::AttemptRule;
    use mop_core::money::Rate;
    use mop_store::{InventionChoice, Store, StoreError};
    use rusqlite::{Connection, params};

    const HOBGOBLIN: i64 = 1;
    const HOBGOBLIN_T2: u32 = 2456;
    const DECRYPTOR: u32 = 34202;

    fn open(path: &Path, now: i64) -> Store {
        Store::open(path, now).expect("open").store
    }

    fn fresh() -> (tempfile::TempDir, PathBuf, Store) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("user.sqlite");
        let mut store = open(&path, 1_000);
        store
            .create_plan(&plan_named("Hobgoblin II × 8"), 1_000)
            .expect("plan");
        store
            .create_plan(&plan_named("Rifter 31"), 1_000)
            .expect("plan");
        (dir, path, store)
    }

    fn plan_named(name: &str) -> mop_store::PlanRecord {
        use mop_core::freight::Packing;
        use mop_market::book::PriceRule;
        use std::collections::BTreeSet;
        mop_store::PlanRecord {
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

    fn count(path: &Path, sql: &str) -> i64 {
        Connection::open(path)
            .expect("open")
            .query_row(sql, [], |row| row.get(0))
            .expect("count")
    }

    fn raw_setting(path: &Path, key: &str) -> String {
        Connection::open(path)
            .expect("open")
            .query_row(
                "SELECT value FROM app_settings WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .expect("setting")
    }

    fn invented(decryptor: Option<u32>) -> InventionChoice {
        InventionChoice {
            invent: true,
            decryptor,
        }
    }

    fn one(part: u32, choice: InventionChoice) -> BTreeMap<u32, InventionChoice> {
        BTreeMap::from([(part, choice)])
    }

    #[test]
    fn a_fresh_store_has_the_defaults() {
        let (_dir, _path, mut store) = fresh();
        assert!(store.plan_invention(HOBGOBLIN).expect("read").is_empty());
        assert!(store.invention_skills().expect("skills").is_empty());
        assert_eq!(store.attempt_rule().expect("rule"), AttemptRule::Expected);
    }

    #[test]
    fn a_choice_is_kept_across_reopening_and_the_default_removes_it() {
        let (_dir, path, mut store) = fresh();
        store
            .set_plan_invention(HOBGOBLIN, HOBGOBLIN_T2, invented(Some(DECRYPTOR)))
            .expect("set");
        drop(store);

        let mut reopened = open(&path, 2_000);
        assert_eq!(
            reopened.plan_invention(HOBGOBLIN).expect("read"),
            one(HOBGOBLIN_T2, invented(Some(DECRYPTOR)))
        );

        let not_invented = InventionChoice {
            invent: false,
            decryptor: None,
        };
        reopened
            .set_plan_invention(HOBGOBLIN, HOBGOBLIN_T2, not_invented)
            .expect("set");
        assert_eq!(
            reopened.plan_invention(HOBGOBLIN).expect("read"),
            one(HOBGOBLIN_T2, not_invented)
        );

        reopened
            .set_plan_invention(HOBGOBLIN, HOBGOBLIN_T2, InventionChoice::default())
            .expect("set");
        assert!(reopened.plan_invention(HOBGOBLIN).expect("read").is_empty());
        assert_eq!(count(&path, "SELECT count(*) FROM plan_invention"), 0);
    }

    #[test]
    fn duplicate_copies_the_rows_and_delete_removes_only_the_original() {
        let (_dir, path, mut store) = fresh();
        store
            .set_plan_invention(HOBGOBLIN, HOBGOBLIN_T2, invented(Some(DECRYPTOR)))
            .expect("set");
        let copy = store.duplicate_plan(HOBGOBLIN, 3_000).expect("duplicate");

        assert_eq!(
            store.plan_invention(copy).expect("read"),
            one(HOBGOBLIN_T2, invented(Some(DECRYPTOR)))
        );

        store.delete_plan(HOBGOBLIN).expect("delete");
        assert!(store.plan_invention(HOBGOBLIN).expect("read").is_empty());
        assert_eq!(
            store.plan_invention(copy).expect("read"),
            one(HOBGOBLIN_T2, invented(Some(DECRYPTOR)))
        );
        assert_eq!(count(&path, "SELECT count(*) FROM plan_invention"), 1);
    }

    #[test]
    fn an_unknown_plan_is_not_found_and_nothing_is_written() {
        let (_dir, path, mut store) = fresh();
        let result = store.set_plan_invention(99, HOBGOBLIN_T2, invented(Some(DECRYPTOR)));
        assert!(matches!(result, Err(StoreError::NotFound)));
        assert_eq!(count(&path, "SELECT count(*) FROM plan_invention"), 0);
    }

    #[test]
    fn skills_are_saved_cleared_and_range_checked() {
        let (_dir, path, mut store) = fresh();
        store.set_invention_skill(11453, 4).expect("skill");
        store.set_invention_skill(23121, 0).expect("skill");
        assert_eq!(
            store.invention_skills().expect("skills"),
            BTreeMap::from([(11453, 4), (23121, 0)])
        );
        assert_eq!(raw_setting(&path, "invention.skill.11453"), "4");
        assert_eq!(raw_setting(&path, "invention.skill.23121"), "0");

        store.clear_invention_skill(23121).expect("clear");
        assert_eq!(
            store.invention_skills().expect("skills"),
            BTreeMap::from([(11453, 4)])
        );
        store.clear_invention_skill(23121).expect("clear again");

        let result = store.set_invention_skill(11453, 6);
        assert!(matches!(
            result,
            Err(StoreError::InvalidCosts(ref reason)) if reason == "skill levels are 0 to 5"
        ));
        assert_eq!(
            store.invention_skills().expect("skills"),
            BTreeMap::from([(11453, 4)])
        );
    }

    #[test]
    fn stored_skills_outside_the_rules_are_bad_data() {
        let (_dir, _path, mut store) = fresh();
        store
            .set_setting("invention.skill.11453", "7")
            .expect("raw");
        match store.invention_skills() {
            Err(StoreError::BadData(what)) => assert!(what.contains("invention.skill.11453")),
            other => panic!("expected bad data, got {other:?}"),
        }
        store.clear_setting("invention.skill.11453").expect("clear");
        store.set_setting("invention.skill.abc", "1").expect("raw");
        match store.invention_skills() {
            Err(StoreError::BadData(what)) => assert!(what.contains("invention.skill.abc")),
            other => panic!("expected bad data, got {other:?}"),
        }
        store.clear_setting("invention.skill.abc").expect("clear");
        for value in ["+4", "04", ""] {
            store
                .set_setting("invention.skill.11453", value)
                .expect("raw");
            match store.invention_skills() {
                Err(StoreError::BadData(what)) => {
                    assert!(what.contains("invention.skill.11453"), "value {value:?}");
                }
                other => panic!("value {value:?}: expected bad data, got {other:?}"),
            }
        }
        store.clear_setting("invention.skill.11453").expect("clear");
    }

    #[test]
    fn the_attempts_rule_is_saved_and_read_back() {
        let (_dir, path, mut store) = fresh();
        let sure95 = AttemptRule::Confidence(Rate::from_ppm(950_000));
        store.set_attempt_rule(sure95).expect("rule");
        assert_eq!(raw_setting(&path, "invention.attempts"), "95");
        assert_eq!(store.attempt_rule().expect("rule"), sure95);

        store.set_attempt_rule(AttemptRule::Expected).expect("rule");
        assert_eq!(raw_setting(&path, "invention.attempts"), "expected");

        store.set_attempt_rule(sure95).expect("rule");
        let odd = AttemptRule::Confidence(Rate::from_ppm(900_001));
        match store.set_attempt_rule(odd) {
            Err(StoreError::InvalidCosts(reason)) => {
                assert_eq!(reason, "attempts are expected, 90%, 95% or 99% sure");
            }
            other => panic!("expected invalid costs, got {other:?}"),
        }
        assert_eq!(raw_setting(&path, "invention.attempts"), "95");
    }

    #[test]
    fn every_attempts_rule_reads_back() {
        let (_dir, path, mut store) = fresh();
        let cases = [
            (AttemptRule::Expected, "expected"),
            (AttemptRule::Confidence(Rate::from_ppm(900_000)), "90"),
            (AttemptRule::Confidence(Rate::from_ppm(950_000)), "95"),
            (AttemptRule::Confidence(Rate::from_ppm(990_000)), "99"),
        ];
        for (rule, text) in cases {
            store.set_attempt_rule(rule).expect("rule");
            assert_eq!(raw_setting(&path, "invention.attempts"), text);
            assert_eq!(store.attempt_rule().expect("rule"), rule);
        }

        let sure99 = AttemptRule::Confidence(Rate::from_ppm(990_000));
        store.set_attempt_rule(sure99).expect("rule");
        drop(store);
        let reopened = open(&path, 1_000);
        assert_eq!(reopened.attempt_rule().expect("rule"), sure99);
    }

    #[test]
    fn a_stored_attempts_value_outside_the_rule_is_bad_data() {
        let (_dir, _path, mut store) = fresh();
        store.set_setting("invention.attempts", "80").expect("raw");
        match store.attempt_rule() {
            Err(StoreError::BadData(what)) => assert!(what.contains("invention.attempts")),
            other => panic!("expected bad data, got {other:?}"),
        }
    }
}

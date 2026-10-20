#[cfg(test)]
mod tests {
    use crate::support::undo_version_six;
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::{Path, PathBuf};

    use profit_core::fees::Standing;
    use profit_core::freight::Packing;
    use profit_core::money::Rate;
    use profit_market::book::PriceRule;
    use profit_sde::{Facility, Security};
    use profit_store::{FacilityCosts, FeeSettings, PlanRecord, SellMode, Store, StoreError};
    use rusqlite::Connection;

    const JITA: u32 = 30_000_142;
    const AZBEL: u32 = 35_826;
    const RAITARU: u32 = 35_825;
    const RIG: u32 = 37_157;

    fn raw(path: &Path) -> Connection {
        Connection::open(path).expect("open")
    }

    fn version_of(path: &Path) -> u32 {
        raw(path)
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("version")
    }

    /// A version-5 file with the NPC station and the Raitaru from the acceptance checks, and the
    /// plan Hobgoblin 10.
    fn version_five_of(path: &Path) {
        drop(Store::open(path, 1_000).expect("open"));
        undo_version_six(path);
        raw(path)
            .execute_batch(
                "PRAGMA foreign_keys = OFF;
                 INSERT INTO facilities (id, name, structure, security)
                     VALUES (1, 'NPC hisec', NULL, 0), (2, 'Raitaru null', 35825, 2);
                 INSERT INTO facility_rigs (facility_id, position, type_id) VALUES (2, 0, 37157);
                 INSERT INTO plans (id, name, product, quantity, price_rule, packing, pasted_stock,
                     created_at, updated_at)
                     VALUES (1, 'Hobgoblin 10', 2456, 10, 1, 0, 'Tritanium 1,000', 1000, 1000);
                 INSERT INTO plan_build (plan_id, type_id) VALUES (1, 2454);
                 INSERT INTO plan_me (plan_id, type_id, me) VALUES (1, 2456, 2);",
            )
            .expect("raw rows");
    }

    fn npc_station() -> Facility {
        Facility {
            structure: None,
            security: Security::High,
            rigs: Vec::new(),
        }
    }

    fn raitaru() -> Facility {
        Facility {
            structure: Some(RAITARU),
            security: Security::Null,
            rigs: vec![RIG],
        }
    }

    /// A new file with the NPC station (id 1) and the Raitaru (id 2).
    fn store_with_two_facilities() -> (tempfile::TempDir, PathBuf, Store) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("user.sqlite");
        let mut store = Store::open(&path, 1_000).expect("open");
        assert_eq!(
            store
                .add_facility("NPC hisec", &npc_station())
                .expect("npc"),
            1
        );
        assert_eq!(
            store
                .add_facility("Raitaru null", &raitaru())
                .expect("raitaru"),
            2
        );
        (dir, path, store)
    }

    fn hobgoblin(id: i64) -> PlanRecord {
        PlanRecord {
            id,
            name: "Hobgoblin 10".to_owned(),
            product: 2456,
            quantity: 10,
            build: BTreeSet::from([2454]),
            me: BTreeMap::from([(2456, 2)]),
            manufacturing: None,
            reactions: None,
            price_rule: PriceRule::LowestSell,
            route: None,
            service: None,
            packing: Packing::FillOnly,
            pasted_stock: "Tritanium 1,000".to_owned(),
            created_at: 1_000,
            updated_at: 1_000,
        }
    }

    /// The reason of an `InvalidCosts` refusal; any other error fails the test.
    fn refused(err: StoreError) -> String {
        let text = format!("{err:?}");
        let StoreError::InvalidCosts(reason) = err else {
            panic!("expected InvalidCosts, got {text}")
        };
        reason
    }

    /// The text of a `BadData` error; any other error fails the test.
    fn bad_data(err: StoreError) -> String {
        let text = format!("{err:?}");
        let StoreError::BadData(what) = err else {
            panic!("expected BadData, got {text}")
        };
        what
    }

    fn costs(system: Option<u32>, tax: Option<u32>) -> FacilityCosts {
        FacilityCosts {
            system,
            tax: tax.map(Rate::from_ppm),
        }
    }

    #[test]
    fn a_version_five_file_is_kept_aside_and_upgraded() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("user.sqlite");
        version_five_of(&path);
        let before = std::fs::read(&path).expect("read");

        let mut store = Store::open(&path, 2_000).expect("upgrade");

        assert_eq!(
            std::fs::read(dir.path().join("user.sqlite.before-v7")).expect("backup"),
            before
        );
        assert_eq!(version_of(&path), 7);
        assert_eq!(
            store.facility_costs(1).expect("npc"),
            costs(None, Some(2_500))
        );
        assert_eq!(store.facility_costs(2).expect("raitaru"), costs(None, None));
        assert_eq!(store.fee_settings().expect("fees"), FeeSettings::default());
        assert_eq!(store.plan_sell_mode(1).expect("mode"), SellMode::SellOrder);
        let raitaru_row = store
            .facilities()
            .expect("facilities")
            .into_iter()
            .find(|row| row.id == 2)
            .expect("facility 2");
        assert_eq!(raitaru_row.facility.structure, Some(RAITARU));
        assert_eq!(raitaru_row.facility.rigs, vec![RIG]);
        assert_eq!(store.plan(1).expect("plan"), Some(hobgoblin(1)));
    }

    #[test]
    fn a_facilitys_system_and_tax_are_saved_and_kept_by_a_rename() {
        let (_dir, _path, mut store) = store_with_two_facilities();
        let set = costs(Some(JITA), Some(10_000));

        store.set_facility_costs(2, &set).expect("set");
        assert_eq!(store.facility_costs(2).expect("read"), set);
        assert_eq!(
            store.all_facility_costs().expect("all"),
            BTreeMap::from([(1, costs(None, Some(2_500))), (2, set)])
        );

        store
            .update_facility(2, "Raitaru Jita", &raitaru())
            .expect("rename");
        assert_eq!(store.facility_costs(2).expect("after rename"), set);

        store.delete_facility(2).expect("delete");
        assert_eq!(
            store.all_facility_costs().expect("after delete"),
            BTreeMap::from([(1, costs(None, Some(2_500)))])
        );
    }

    #[test]
    fn refused_facility_costs_leave_the_saved_ones() {
        let (_dir, _path, mut store) = store_with_two_facilities();

        let tax = store
            .set_facility_costs(1, &costs(None, Some(1_000_001)))
            .expect_err("tax above 100%");
        assert_eq!(refused(tax), "tax must be from 0% to 100%");
        let system = store
            .set_facility_costs(1, &costs(Some(0), None))
            .expect_err("system 0");
        assert_eq!(refused(system), "pick a solar system");
        assert!(matches!(
            store.set_facility_costs(99, &costs(Some(JITA), None)),
            Err(StoreError::NotFound)
        ));
        assert_eq!(
            store.facility_costs(1).expect("kept"),
            costs(None, Some(2_500))
        );
    }

    #[test]
    fn exactly_100_percent_and_the_top_skill_level_are_kept() {
        let (_dir, _path, mut store) = store_with_two_facilities();
        let whole = costs(Some(JITA), Some(1_000_000));
        store.set_facility_costs(2, &whole).expect("100% tax");
        assert_eq!(store.facility_costs(2).expect("read"), whole);

        let top = FeeSettings {
            accounting: 5,
            broker_relations: 5,
            broker_override: Some(Rate::from_ppm(1_000_000)),
            ..FeeSettings::default()
        };
        store.save_fee_settings(&top).expect("top levels");
        assert_eq!(store.fee_settings().expect("read"), top);
    }

    #[test]
    fn an_npc_station_gets_the_npc_tax_and_a_structure_none() {
        let (_dir, _path, mut store) = store_with_two_facilities();

        let amarr = store
            .add_facility(
                "NPC Amarr",
                &Facility {
                    structure: None,
                    security: Security::High,
                    rigs: Vec::new(),
                },
            )
            .expect("npc");
        let azbel = store
            .add_facility(
                "Azbel",
                &Facility {
                    structure: Some(AZBEL),
                    security: Security::Null,
                    rigs: Vec::new(),
                },
            )
            .expect("structure");

        assert_eq!(
            store.facility_costs(amarr).expect("npc"),
            costs(None, Some(2_500))
        );
        assert_eq!(
            store.facility_costs(azbel).expect("structure"),
            costs(None, None)
        );
    }

    #[test]
    fn fee_settings_survive_a_reopen_and_refused_ones_keep_the_last_saved() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("user.sqlite");
        let mut store = Store::open(&path, 1_000).expect("open");
        let saved = FeeSettings {
            accounting: 4,
            broker_relations: 5,
            faction_standing: Standing::parse("5.23").expect("faction"),
            corporation_standing: Standing::parse("-1.25").expect("corporation"),
            broker_override: None,
        };
        store.save_fee_settings(&saved).expect("save");
        drop(store);
        let mut store = Store::open(&path, 2_000).expect("reopen");
        assert_eq!(store.fee_settings().expect("read"), saved);

        let with_override = FeeSettings {
            broker_override: Some(Rate::from_ppm(13_431)),
            ..saved
        };
        store
            .save_fee_settings(&with_override)
            .expect("save override");
        assert_eq!(store.fee_settings().expect("read"), with_override);

        let skill = store
            .save_fee_settings(&FeeSettings {
                accounting: 6,
                ..with_override
            })
            .expect_err("skill 6");
        assert_eq!(refused(skill), "skill levels are 0 to 5");
        let fee = store
            .save_fee_settings(&FeeSettings {
                broker_override: Some(Rate::from_ppm(1_000_001)),
                ..with_override
            })
            .expect_err("fee above 100%");
        assert_eq!(refused(fee), "broker fee must be from 0% to 100%");
        assert_eq!(store.fee_settings().expect("kept"), with_override);
    }

    #[test]
    fn plan_sell_mode_is_kept_by_a_save_and_copied_by_a_duplicate() {
        let (_dir, _path, mut store) = store_with_two_facilities();
        let id = store.create_plan(&hobgoblin(0), 1_000).expect("create");
        assert_eq!(store.plan_sell_mode(id).expect("new"), SellMode::SellOrder);

        store
            .set_plan_sell_mode(id, SellMode::BuyOrders)
            .expect("set");
        store
            .save_plan(
                &PlanRecord {
                    quantity: 12,
                    ..hobgoblin(id)
                },
                2_000,
            )
            .expect("save");
        assert_eq!(
            store.plan_sell_mode(id).expect("saved"),
            SellMode::BuyOrders
        );
        store
            .set_plan_sell_mode(id, SellMode::SellOrder)
            .expect("back to sell order");
        assert_eq!(store.plan_sell_mode(id).expect("back"), SellMode::SellOrder);
        store
            .set_plan_sell_mode(id, SellMode::BuyOrders)
            .expect("buy orders again");

        let copy = store.duplicate_plan(id, 3_000).expect("duplicate");
        assert_eq!(
            store.plan_sell_mode(copy).expect("copy"),
            SellMode::BuyOrders
        );

        let fresh = store
            .create_plan(
                &PlanRecord {
                    name: "New".to_owned(),
                    ..hobgoblin(0)
                },
                4_000,
            )
            .expect("create new");
        assert_eq!(
            store.plan_sell_mode(fresh).expect("fresh"),
            SellMode::SellOrder
        );

        assert!(matches!(
            store.plan_sell_mode(99),
            Err(StoreError::NotFound)
        ));
        assert!(matches!(
            store.set_plan_sell_mode(99, SellMode::BuyOrders),
            Err(StoreError::NotFound)
        ));
    }

    #[test]
    fn the_database_refuses_codes_outside_their_range() {
        let (_dir, path, mut store) = store_with_two_facilities();
        store.create_plan(&hobgoblin(0), 1_000).expect("plan");
        store
            .save_fee_settings(&FeeSettings::default())
            .expect("fees");
        let conn = raw(&path);

        for sql in [
            "UPDATE plans SET sell_mode = 2 WHERE id = 1",
            "UPDATE facilities SET tax_ppm = 1000001 WHERE id = 1",
            "UPDATE fee_settings SET accounting = 6 WHERE id = 1",
            "UPDATE fee_settings SET faction_standing = 1001 WHERE id = 1",
        ] {
            assert!(conn.execute(sql, []).is_err(), "{sql}");
        }
    }

    #[test]
    fn stored_values_rust_cannot_read_are_bad_data_naming_the_place() {
        let (_dir, path, mut store) = store_with_two_facilities();
        store.create_plan(&hobgoblin(0), 1_000).expect("plan");
        store
            .save_fee_settings(&FeeSettings::default())
            .expect("fees");
        drop(store);
        raw(&path)
            .execute_batch(
                "PRAGMA ignore_check_constraints = ON;
                 UPDATE plans SET sell_mode = 7 WHERE id = 1;
                 UPDATE fee_settings SET faction_standing = 5000 WHERE id = 1;
                 UPDATE facilities SET tax_ppm = -1 WHERE id = 1;",
            )
            .expect("write unchecked");
        let mut store = Store::open(&path, 2_000).expect("reopen");

        let mode = bad_data(store.plan_sell_mode(1).expect_err("sell mode"));
        assert!(
            mode.contains("plans row 1") && mode.contains("sell_mode"),
            "{mode}"
        );
        let standing = bad_data(store.fee_settings().expect_err("standing"));
        assert!(
            standing.contains("fee_settings row 1") && standing.contains("faction_standing"),
            "{standing}"
        );
        let tax = bad_data(store.facility_costs(1).expect_err("tax"));
        assert!(
            tax.contains("facilities row 1") && tax.contains("tax_ppm"),
            "{tax}"
        );
    }
}

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use profit_core::freight::{Packing, RateCard};
use profit_core::money::{Money, Rate};
use profit_core::volume::Volume;
use profit_market::book::PriceRule;
use profit_sde::{Facility, RouteKind, Security};
use profit_store::{PlanRecord, Store, StoreError};
use rusqlite::{Connection, params};

#[cfg(test)]
mod tests {
    use super::*;

    /// A whole number of ISK as the stored count of ten-thousandths.
    fn isk(whole: i64) -> Money {
        Money::from_ten_thousandths(whole * 10_000)
    }

    fn open_temp() -> (tempfile::TempDir, PathBuf, Store) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("eve.db");
        let store = Store::open(&path, 1_000).expect("open");
        (dir, path, store)
    }

    fn raitaru_null() -> Facility {
        Facility {
            structure: Some(35825),
            security: Security::Null,
            rigs: vec![37157, 37154],
        }
    }

    fn alliance_card() -> RateCard {
        RateCard {
            per_m3: Some(isk(850)),
            per_jump: None,
            collateral: Some(Rate::from_ppm(5_000)),
            minimum: Some(isk(5_000_000)),
            max_m3: Some(Volume::from_ten_thousandths(360_000 * 10_000)),
            max_collateral: Some(isk(10_000_000_000)),
        }
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

    /// A plan that uses a facility, a route and a service, as the Build page saves them.
    fn hobgoblin(facility: i64, route: i64, service: i64) -> PlanRecord {
        PlanRecord {
            quantity: 10,
            build: BTreeSet::from([2454, 11688]),
            me: BTreeMap::from([(2456, 2), (11688, 10)]),
            manufacturing: Some(facility),
            price_rule: PriceRule::LowestSell,
            route: Some(route),
            service: Some(service),
            packing: Packing::FillOnly,
            pasted_stock: "Tritanium\t1,000\nParticle Accelerator Unit\t4".to_owned(),
            ..plan_named("Hobgoblin 10")
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

    #[test]
    fn a_plan_with_its_facility_route_and_service_survives_a_restart() {
        let (_dir, path, mut store) = open_temp();
        let facility = store
            .add_facility("Raitaru null", &raitaru_null())
            .expect("facility");
        let route = store.add_route(30000142, 30000189).expect("route");
        let service = store
            .add_service(
                route,
                "Alliance",
                &alliance_card(),
                RouteKind::HighSecOnly,
                Some(9),
            )
            .expect("service");
        let mut input = hobgoblin(facility, route, service);
        let id = store.create_plan(&input, 1_000).expect("create");
        input.id = id;
        input.created_at = 1_000;
        input.updated_at = 2_000;
        store.save_plan(&input, 2_000).expect("save");
        drop(store);

        let mut reopened = Store::open(&path, 3_000).expect("reopen");

        assert_eq!(reopened.plan(id).expect("plan"), Some(input));
        let facilities = reopened.facilities().expect("facilities");
        assert_eq!(facilities.len(), 1);
        assert_eq!(facilities[0].id, facility);
        assert_eq!(facilities[0].name, "Raitaru null");
        assert_eq!(facilities[0].facility, raitaru_null());
        assert_eq!(facilities[0].facility.rigs, vec![37157, 37154]);
        let routes = reopened.routes().expect("routes");
        assert_eq!(routes.len(), 1);
        assert_eq!(
            (routes[0].from_system, routes[0].to_system),
            (30000142, 30000189)
        );
        let services = reopened.services(route).expect("services");
        assert_eq!(services.len(), 1);
        assert_eq!(services[0].name, "Alliance");
        assert_eq!(services[0].card, alliance_card());
        assert_eq!(services[0].route_kind, RouteKind::HighSecOnly);
        assert_eq!(services[0].jumps_override, Some(9));
    }

    #[test]
    fn money_volume_and_quantity_are_stored_exactly() {
        let (_dir, _path, mut store) = open_temp();
        let route = store.add_route(1, 2).expect("route");
        let card = RateCard {
            per_m3: Some(Money::from_ten_thousandths(21_250)),
            max_m3: Some(Volume::from_ten_thousandths(25)),
            max_collateral: Some(Money::from_ten_thousandths(i64::MAX)),
            ..RateCard::default()
        };
        store
            .add_service(route, "Exact", &card, RouteKind::Shortest, None)
            .expect("service");
        let stored = &store.services(route).expect("services")[0];
        assert_eq!(stored.route_kind, RouteKind::Shortest);
        let read = &stored.card;
        assert_eq!(read.per_m3, Some(Money::from_ten_thousandths(21_250)));
        assert_eq!(read.max_m3, Some(Volume::from_ten_thousandths(25)));
        assert_eq!(
            read.max_collateral,
            Some(Money::from_ten_thousandths(i64::MAX))
        );

        let mut plan = plan_named("Big");
        plan.quantity = 9_223_372_036_854_775_807;
        let id = store.create_plan(&plan, 1_000).expect("create");
        assert_eq!(
            store.plan(id).expect("plan").map(|p| p.quantity),
            Some(9_223_372_036_854_775_807)
        );

        plan.name = "Huge".to_owned();
        plan.quantity = u64::MAX;
        assert!(matches!(
            store.create_plan(&plan, 1_000),
            Err(StoreError::TooLarge)
        ));
    }

    #[test]
    fn names_are_trimmed_and_unique_ignoring_case() {
        let (_dir, _path, mut store) = open_temp();
        store
            .create_plan(&plan_named("Hobgoblin 10"), 1_000)
            .expect("create");
        assert!(matches!(
            store.create_plan(&plan_named(" hobgoblin 10 "), 1_000),
            Err(StoreError::NameTaken(_))
        ));
        assert!(matches!(
            store.create_plan(&plan_named("   "), 1_000),
            Err(StoreError::EmptyName)
        ));

        let b = store
            .create_plan(&plan_named("Avatar 1"), 1_000)
            .expect("create b");
        assert!(matches!(
            store.rename_plan(b, "HOBGOBLIN 10"),
            Err(StoreError::NameTaken(_))
        ));
        assert_eq!(
            store.plan(b).expect("plan").map(|p| p.name),
            Some("Avatar 1".to_owned())
        );
    }

    #[test]
    fn duplicates_are_numbered_and_listed_in_creation_order() {
        let (_dir, _path, mut store) = open_temp();
        let original = store
            .create_plan(&plan_named("Hobgoblin 10"), 1_000)
            .expect("create");
        for _ in 0..3 {
            store.duplicate_plan(original, 1_000).expect("duplicate");
        }
        assert_eq!(
            names(&mut store),
            vec![
                "Hobgoblin 10",
                "Hobgoblin 10 (copy)",
                "Hobgoblin 10 (copy 2)",
                "Hobgoblin 10 (copy 3)",
            ]
        );
    }

    #[test]
    fn a_facility_in_use_by_plans_is_not_deleted() {
        let (_dir, _path, mut store) = open_temp();
        let facility = store
            .add_facility("Raitaru null", &raitaru_null())
            .expect("facility");
        let route = store.add_route(1, 2).expect("route");
        let service = store
            .add_service(
                route,
                "Alliance",
                &alliance_card(),
                RouteKind::Shortest,
                None,
            )
            .expect("service");
        let hob = store
            .create_plan(&hobgoblin(facility, route, service), 1_000)
            .expect("hobgoblin");
        let avatar = store
            .create_plan(
                &PlanRecord {
                    manufacturing: None,
                    reactions: Some(facility),
                    ..plan_named("Avatar 1")
                },
                1_000,
            )
            .expect("avatar");

        match store.delete_facility(facility) {
            Err(StoreError::InUse { plans }) => {
                assert_eq!(
                    plans,
                    vec!["Avatar 1".to_owned(), "Hobgoblin 10".to_owned()]
                );
            }
            other => panic!("expected InUse, got {other:?}"),
        }
        assert_eq!(store.facilities().expect("facilities").len(), 1);

        store.delete_plan(hob).expect("delete hobgoblin");
        store.delete_plan(avatar).expect("delete avatar");
        store.delete_facility(facility).expect("delete facility");
        assert!(store.facilities().expect("facilities").is_empty());
    }

    #[test]
    fn a_route_in_use_is_not_deleted_and_an_unused_route_takes_its_services() {
        let (_dir, _path, mut store) = open_temp();
        let facility = store
            .add_facility("Raitaru null", &raitaru_null())
            .expect("facility");
        let used = store.add_route(1, 2).expect("used route");
        let service = store
            .add_service(
                used,
                "Alliance",
                &alliance_card(),
                RouteKind::Shortest,
                None,
            )
            .expect("service");
        let plan = store
            .create_plan(&hobgoblin(facility, used, service), 1_000)
            .expect("plan");

        assert!(matches!(
            store.delete_route(used),
            Err(StoreError::InUse { .. })
        ));

        let unused = store.add_route(3, 4).expect("unused route");
        store
            .add_service(
                unused,
                "Cheap",
                &RateCard::default(),
                RouteKind::Shortest,
                None,
            )
            .expect("service");
        store.delete_route(unused).expect("delete unused");
        assert!(store.services(unused).expect("services").is_empty());
        assert!(matches!(store.delete_plan(999), Err(StoreError::NotFound)));

        store.delete_plan(plan).expect("delete plan");
        store.delete_route(used).expect("delete used once free");
    }

    #[test]
    fn deleting_the_picked_service_sets_the_plan_back_to_cheapest() {
        let (_dir, _path, mut store) = open_temp();
        let facility = store
            .add_facility("Raitaru null", &raitaru_null())
            .expect("facility");
        let route = store.add_route(1, 2).expect("route");
        let service = store
            .add_service(
                route,
                "Alliance",
                &alliance_card(),
                RouteKind::Shortest,
                None,
            )
            .expect("service");
        let plan = store
            .create_plan(&hobgoblin(facility, route, service), 1_000)
            .expect("plan");

        store.delete_service(service).expect("delete service");

        let read = store.plan(plan).expect("plan").expect("exists");
        assert_eq!(read.service, None);
        assert_eq!(read.route, Some(route));
    }

    #[test]
    fn adding_the_same_route_twice_is_refused() {
        let (_dir, _path, mut store) = open_temp();
        store.add_route(30000142, 30000189).expect("first");
        assert!(matches!(
            store.add_route(30000142, 30000189),
            Err(StoreError::RouteExists)
        ));
    }

    #[test]
    fn a_plan_pointing_at_a_missing_facility_is_refused() {
        let (_dir, _path, mut store) = open_temp();
        let plan = PlanRecord {
            manufacturing: Some(999),
            ..plan_named("Lost")
        };
        assert!(matches!(
            store.create_plan(&plan, 1_000),
            Err(StoreError::NotFound)
        ));
        assert!(names(&mut store).is_empty());
    }

    #[test]
    fn duplicating_or_saving_a_missing_plan_is_not_found() {
        let (_dir, _path, mut store) = open_temp();
        assert!(matches!(
            store.duplicate_plan(999, 1_000),
            Err(StoreError::NotFound)
        ));
        let mut plan = plan_named("Ghost");
        plan.id = 999;
        assert!(matches!(
            store.save_plan(&plan, 1_000),
            Err(StoreError::NotFound)
        ));
    }

    fn raw(path: &Path) -> Connection {
        Connection::open(path).expect("raw open")
    }

    #[test]
    fn the_database_refuses_bad_values_without_rust_checks() {
        let (_dir, path, mut store) = open_temp();
        let plan = store
            .create_plan(&plan_named("Checked"), 1_000)
            .expect("plan");
        let route = store.add_route(1, 2).expect("route");
        store
            .add_service(
                route,
                "Cheap",
                &RateCard::default(),
                RouteKind::Shortest,
                None,
            )
            .expect("service");
        drop(store);
        let conn = raw(&path);

        let zero_quantity = conn.execute(
            "INSERT INTO plans (name, product, quantity, price_rule, packing, pasted_stock, \
            created_at, updated_at) VALUES ('raw', 1, 0, 0, 0, '', 1, 1)",
            [],
        );
        assert!(
            matches!(zero_quantity, Err(rusqlite::Error::SqliteFailure(ref e, _)) if e.code == rusqlite::ErrorCode::ConstraintViolation)
        );
        let me_too_high = conn.execute(
            "INSERT INTO plan_me (plan_id, type_id, me) VALUES (?1, 2456, 11)",
            params![plan],
        );
        assert!(
            matches!(me_too_high, Err(rusqlite::Error::SqliteFailure(ref e, _)) if e.code == rusqlite::ErrorCode::ConstraintViolation)
        );
        let negative_rate = conn.execute(
            "INSERT INTO services (route_id, name, per_m3, route_kind) VALUES (?1, 'x', -1, 0)",
            params![route],
        );
        assert!(
            matches!(negative_rate, Err(rusqlite::Error::SqliteFailure(ref e, _)) if e.code == rusqlite::ErrorCode::ConstraintViolation)
        );
    }

    #[test]
    fn a_stored_code_rust_cannot_read_is_bad_data() {
        let (_dir, path, mut store) = open_temp();
        let plan = store
            .create_plan(&plan_named("Broken"), 1_000)
            .expect("plan");
        drop(store);
        raw(&path)
            .execute(
                "UPDATE plans SET price_rule = 99 WHERE id = ?1",
                params![plan],
            )
            .expect("update");
        let mut store = Store::open(&path, 2_000).expect("reopen");

        match store.plan(plan) {
            Err(StoreError::BadData(message)) => {
                assert!(message.contains("plans"), "{message}");
                assert!(message.contains(&format!("row {plan},")), "{message}");
                assert!(message.contains("price_rule"), "{message}");
            }
            other => panic!("expected BadData, got {other:?}"),
        }
    }

    #[test]
    fn default_packing_and_every_security_come_back_as_saved() {
        let (_dir, _path, mut store) = open_temp();
        let plan = store
            .create_plan(&plan_named("Default"), 1_000)
            .expect("plan");
        assert_eq!(
            store.plan(plan).expect("plan").map(|p| p.packing),
            Some(Packing::TopUp)
        );
        for security in [Security::High, Security::Low, Security::Null] {
            let facility = Facility {
                structure: None,
                security,
                rigs: Vec::new(),
            };
            let id = store.add_facility("Station", &facility).expect("add");
            let read = store
                .facilities()
                .expect("facilities")
                .into_iter()
                .find(|row| row.id == id)
                .expect("row");
            assert_eq!(read.facility, facility);
            store.delete_facility(id).expect("delete");
        }
    }

    #[test]
    fn updated_facilities_services_and_renamed_plans_read_back() {
        let (_dir, _path, mut store) = open_temp();
        let facility = store
            .add_facility("Raitaru null", &raitaru_null())
            .expect("facility");
        let changed = Facility {
            structure: None,
            security: Security::High,
            rigs: vec![37157],
        };
        store
            .update_facility(facility, "Station", &changed)
            .expect("update facility");
        let read = store.facilities().expect("facilities");
        assert_eq!(read[0].name, "Station");
        assert_eq!(read[0].facility, changed);

        let route = store.add_route(1, 2).expect("route");
        let service = store
            .add_service(
                route,
                "Alliance",
                &alliance_card(),
                RouteKind::Shortest,
                None,
            )
            .expect("service");
        store
            .update_service(
                service,
                "Cheap",
                &RateCard::default(),
                RouteKind::HighSecOnly,
                Some(3),
            )
            .expect("update service");
        let read = &store.services(route).expect("services")[0];
        assert_eq!(read.name, "Cheap");
        assert_eq!(read.card, RateCard::default());
        assert_eq!(read.route_kind, RouteKind::HighSecOnly);
        assert_eq!(read.jumps_override, Some(3));

        let plan = store.create_plan(&plan_named("Old"), 1_000).expect("plan");
        store.rename_plan(plan, "New").expect("rename");
        assert_eq!(names(&mut store), vec!["New".to_owned()]);
    }

    #[test]
    fn missing_rows_are_not_found_when_updated_or_renamed() {
        let (_dir, _path, mut store) = open_temp();
        assert!(matches!(
            store.update_facility(999, "Gone", &raitaru_null()),
            Err(StoreError::NotFound)
        ));
        assert!(matches!(
            store.update_service(999, "Gone", &RateCard::default(), RouteKind::Shortest, None),
            Err(StoreError::NotFound)
        ));
        assert!(matches!(
            store.rename_plan(999, "Gone"),
            Err(StoreError::NotFound)
        ));
        assert!(matches!(
            store.delete_facility(999),
            Err(StoreError::NotFound)
        ));
        assert!(matches!(
            store.delete_service(999),
            Err(StoreError::NotFound)
        ));
        assert!(matches!(store.delete_route(999), Err(StoreError::NotFound)));
    }
}

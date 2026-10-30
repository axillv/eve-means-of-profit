use super::*;

#[test]
fn the_freight_hover_lists_every_service() {
    let world = World::new();
    let mut harness = complete_list_sized(&world, &standard_services(), 1640.0);
    hover(&mut harness, "110 K");
    assert!(shown(&harness, "Alliance 109,914.68"));
    assert!(shown(&harness, "Haulers 3,013,959.36"));
}

#[test]
fn choosing_haulers_changes_the_freight_and_saves_the_service() {
    let world = World::new();
    let mut harness = complete_list(&world, &standard_services());
    pick(&mut harness, "Cheapest (Alliance)", "Haulers");
    settle(&mut harness, "3.01 M");
    assert!(shown(&harness, " · collateral 1.40 M · delivered 4.41 M"));
    assert!(has_value(&harness, "Haulers"));
}

#[test]
fn no_freight_is_an_empty_quote() {
    let world = World::new();
    let mut harness = complete_list(&world, &standard_services());
    pick(&mut harness, "Jita → Kiainti", "No freight");
    settle(&mut harness, "No freight");
    assert!(shown(&harness, "freight "));
    // The freight amount itself is 0.00, as is the collateral.
    assert!(shown(&harness, "0.00"));
    assert!(shown(&harness, " · collateral 0.00 · delivered 1.40 M"));
    // The footer row still holds the value of the goods.
    assert!(shown(&harness, "1,395,936.00"));
    assert!(!shown(&harness, " incomplete"));
}

#[test]
fn a_minimum_charge_packs_two_contracts_at_twenty_million_either_way() {
    let world = World::new();
    let bulk = (
        "Bulk",
        RateCard {
            per_m3: Some(per_m3(1_000)),
            minimum: Some(per_m3(10_000_000)),
            max_m3: Some(Volume::from_ten_thousandths(100 * 10_000)),
            ..RateCard::default()
        },
        RouteKind::Shortest,
        None,
    );
    let mut harness = complete_list(&world, std::slice::from_ref(&bulk));
    settle(&mut harness, "2 contracts");
    assert!(shown(&harness, "20.00 M"));
    click(&mut harness, "Top up the last contract");
    settle(&mut harness, "2 contracts");
    assert!(shown(&harness, "20.00 M"));
}

#[test]
fn top_up_moves_units_into_the_last_contract_and_turning_it_off_packs_them_in_order() {
    let world = World::new();
    // Each contract pays at least 60,000 ISK. With top-up on, the last contract is filled to 62 m³
    // and the first, at 58.5 m³, pays its minimum: 122,000.00. Off, 100 m³ and 20.5 m³ pay 160,000.00.
    let minimum = (
        "Minimum",
        RateCard {
            per_m3: Some(per_m3(1_000)),
            minimum: Some(per_m3(60_000)),
            max_m3: Some(Volume::from_ten_thousandths(100 * 10_000)),
            ..RateCard::default()
        },
        RouteKind::Shortest,
        None,
    );
    let mut harness = complete_list(&world, std::slice::from_ref(&minimum));
    // Top-up is on by default.
    settle(&mut harness, "122 K");
    click(&mut harness, "Top up the last contract");
    settle(&mut harness, "160 K");
    assert!(shown(&harness, "2 contracts"));
}

#[test]
fn a_service_that_cannot_carry_an_item_says_why_in_the_freight_hover() {
    let world = World::new();
    // A contract holds 1 m³; one Hobgoblin I is 5 m³.
    let tiny = (
        "Tiny",
        RateCard {
            per_m3: Some(per_m3(850)),
            max_m3: Some(Volume::from_ten_thousandths(10_000)),
            ..RateCard::default()
        },
        RouteKind::Shortest,
        None,
    );
    let mut services = standard_services();
    services.push(tiny);
    let mut harness = complete_list_sized(&world, &services, 1640.0);
    hover(&mut harness, "110 K");
    assert!(shown(&harness, "Tiny: cannot carry Hobgoblin I"));
    assert!(shown(&harness, "Alliance 109,914.68"));
}

#[test]
fn the_earlier_service_wins_a_tie_for_the_cheapest() {
    let world = World::new();
    let mut services = standard_services();
    // The same rate card and route as Alliance, saved after it.
    services.push((
        "Twin",
        RateCard {
            per_m3: Some(per_m3(850)),
            collateral: Some(Rate::from_ppm(5_000)),
            ..RateCard::default()
        },
        RouteKind::Shortest,
        None,
    ));
    let mut harness = complete_list(&world, &services);
    settle(&mut harness, "Cheapest (Alliance)");
}

#[test]
fn haulers_is_saved_and_falls_back_to_the_cheapest_once_deleted() {
    let world = World::new();
    let env = make_env(&standard_services());
    let mut harness = start(&env, &world);
    harness.run();
    buy_list_for(&mut harness);
    settle(&mut harness, "Oldest: 4m");
    pick(&mut harness, "Cheapest (Alliance)", "Haulers");
    settle(&mut harness, "3.01 M");
    click(&mut harness, "Save");

    let mut store = Store::open(&env.data.join("user.sqlite"), NOW)
        .expect("the store opens")
        .store;
    let plan = store.plans().expect("the plans")[0].id;
    let route = store.routes().expect("the routes")[0].id;
    let services = store.services(route).expect("the services");
    let haulers = services
        .iter()
        .find(|service| service.name == "Haulers")
        .expect("Haulers is saved")
        .id;
    let saved = store.plan(plan).expect("the plan").expect("a saved plan");
    assert_eq!(saved.service, Some(haulers));

    store.delete_service(haulers).expect("Haulers is deleted");
    // A new app opens the plan from the store; clicking the plan already on screen would not reload it.
    drop(harness);
    let mut harness = start(&env, &world);
    harness.run();
    open_saved_plan(&mut harness, "New plan");
    // The new app downloads the file at its start, so the price can arrive after the label.
    settle(&mut harness, "110 K");
    settle(&mut harness, "Cheapest (Alliance)");
    assert!(shown(&harness, "110 K"));
}

#[test]
fn a_cheaper_second_service_is_the_cheapest() {
    let world = World::new();
    let cheap = (
        "Cheap",
        RateCard {
            per_m3: Some(per_m3(100)),
            ..RateCard::default()
        },
        RouteKind::Shortest,
        None,
    );
    let mut services = standard_services();
    services.push(cheap);
    let mut harness = complete_list(&world, &services);
    settle(&mut harness, "Cheapest (Cheap)");
    settle(&mut harness, "12.11 K");
}

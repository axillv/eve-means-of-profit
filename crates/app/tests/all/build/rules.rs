use super::*;

#[test]
fn a_plan_shows_its_rule_and_a_pick_is_unsaved_until_it_is_saved() {
    let env = make_env(&standard_facilities());
    let mut store = Store::open(&env.data.join("user.sqlite"), NOW)
        .expect("the store opens")
        .store;
    store
        .create_plan(
            &mop_store::PlanRecord {
                id: 0,
                name: "Buy rule".to_owned(),
                product: 2456,
                quantity: 10,
                build: std::collections::BTreeSet::new(),
                me: std::collections::BTreeMap::new(),
                manufacturing: None,
                reactions: None,
                price_rule: PriceRule::HighestBuy,
                route: None,
                service: None,
                packing: mop_core::freight::Packing::TopUp,
                pasted_stock: String::new(),
                created_at: NOW,
                updated_at: NOW,
            },
            NOW,
        )
        .expect("the plan is saved");
    drop(store);
    let mut harness = start(&env);
    harness.run();
    click(&mut harness, "Buy rule");
    assert!(has_value(&harness, "Forge buy, highest"));

    pick(
        &mut harness,
        "Forge buy, highest",
        "Forge sell, weighted average",
    );
    assert!(
        harness
            .query_all_by_label_contains("(unsaved)")
            .next()
            .is_some(),
        "the plan is marked unsaved"
    );
    click(&mut harness, "Save");
    assert_eq!(
        stored_rule(&env, "Buy rule"),
        PriceRule::SellWeightedAverage
    );
}

#[test]
fn a_new_plan_takes_the_default_rule_and_an_older_plan_keeps_its_own() {
    let env = make_env(&standard_facilities());
    let mut store = Store::open(&env.data.join("user.sqlite"), NOW)
        .expect("the store opens")
        .store;
    let mut settings = store.settings().expect("settings");
    settings.default_price_rule = PriceRule::BuyMedian;
    store.save_settings(&settings).expect("settings saved");
    for (name, rule) in [
        ("Old buy", PriceRule::HighestBuy),
        ("Old walk", PriceRule::WalkSell),
    ] {
        store
            .create_plan(
                &mop_store::PlanRecord {
                    id: 0,
                    name: name.to_owned(),
                    product: 2456,
                    quantity: 10,
                    build: std::collections::BTreeSet::new(),
                    me: std::collections::BTreeMap::new(),
                    manufacturing: None,
                    reactions: None,
                    price_rule: rule,
                    route: None,
                    service: None,
                    packing: mop_core::freight::Packing::TopUp,
                    pasted_stock: String::new(),
                    created_at: NOW,
                    updated_at: NOW,
                },
                NOW,
            )
            .expect("the plan is saved");
    }
    drop(store);
    let mut harness = start(&env);
    harness.run();

    click(&mut harness, "New");
    assert!(has_value(&harness, "Forge buy, middle"));

    click(&mut harness, "Old buy");
    assert!(has_value(&harness, "Forge buy, highest"));

    click(&mut harness, "Old walk");
    assert!(has_value(&harness, "Forge sell, 5% price"));
}

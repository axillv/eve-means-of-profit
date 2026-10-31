use super::*;

#[test]
fn saved_plans_that_use_pau_are_listed() {
    let env = env();
    with_main(&env);
    save_plans(&env);
    let (mut harness, _) = start(
        &env,
        FakeEve {
            characters: vec![character(MAIN, "Main")],
            jeve: None,
        },
        Answer::Down,
    );
    loaded(&mut harness);
    select(&mut harness, PAU);

    assert!(shown(&harness, "Hobgoblin 10 · need 10"));
    assert!(!shown(&harness, "Rifter 31 · need 31"));
    assert!(!shown(&harness, "Rifter 31 · product × 31"));
    assert!(!shown(&harness, "Rifter 31 · could not work out the plan"));
    assert!(shown(&harness, "Missing · could not work out the plan"));
}

#[test]
fn a_plan_on_character_stock_still_lists_its_need() {
    let env = env();
    with_main(&env);
    save_plans(&env);
    let mut store = Store::open(&env.data.join("user.sqlite"), NOW)
        .expect("the store opens")
        .store;
    let id = store
        .plans()
        .expect("the plans")
        .iter()
        .find(|plan| plan.name == "Hobgoblin 10")
        .expect("the plan")
        .id;
    let mut record = store
        .plan(id)
        .expect("the plan reads")
        .expect("the plan is saved");
    record.quantity = 300;
    record.name = "Hobgoblin 300".to_owned();
    store.save_plan(&record, NOW).expect("the plan is saved");
    store
        .set_plan_stock(
            id,
            &PlanStock::Characters(StockScope {
                characters: BTreeSet::from([i64::try_from(MAIN).expect("the id fits")]),
                places: Places::All,
            }),
        )
        .expect("the character stock is saved");
    drop(store);
    let (mut harness, _) = start(
        &env,
        FakeEve {
            characters: vec![character(MAIN, "Main")],
            jeve: None,
        },
        Answer::Down,
    );
    loaded(&mut harness);
    select(&mut harness, PAU);

    assert!(shown(&harness, "Hobgoblin 300 · need 300"));
    assert!(shown(&harness, "Missing · could not work out the plan"));
    assert!(!shown(
        &harness,
        "Hobgoblin 300 · could not work out the plan"
    ));
}

#[test]
fn clicking_a_plan_line_changes_nothing() {
    let env = env();
    with_main(&env);
    save_plans(&env);
    let (mut harness, _) = start(
        &env,
        FakeEve {
            characters: vec![character(MAIN, "Main")],
            jeve: None,
        },
        Answer::Down,
    );
    loaded(&mut harness);
    select(&mut harness, PAU);

    harness.get_by_label("Hobgoblin 10 · need 10").click();
    harness.run();
    assert_eq!(harness.state_mut().shared_mut().selection.item, Some(PAU));
    assert!(shown(&harness, "Hobgoblin 10 · need 10"));
}

#[test]
fn the_product_of_a_plan_lists_its_quantity() {
    let env = env();
    with_main(&env);
    save_plans(&env);
    let product = hobgoblin_id(&env);
    let (mut harness, _) = start(
        &env,
        FakeEve {
            characters: vec![character(MAIN, "Main")],
            jeve: None,
        },
        Answer::Down,
    );
    loaded(&mut harness);
    select(&mut harness, product);

    assert!(shown(&harness, "Hobgoblin 10 · product × 10"));
}

#[test]
fn an_item_nobody_owns_shows_none_owned_and_no_jobs() {
    let env = env();
    with_main(&env);
    let (mut harness, _) = start(
        &env,
        FakeEve {
            characters: vec![character(MAIN, "Main")],
            jeve: None,
        },
        Answer::Down,
    );
    loaded(&mut harness);
    select(&mut harness, MORPHITE);

    assert!(shown(&harness, "None owned."));
    assert!(!shown(&harness, "Running jobs"));
}

#[test]
fn the_item_panel_matches_its_snapshot() {
    let env = env();
    with_main(&env);
    name_home_fort(&env);
    let jeve = jeve_profile();
    let eve = FakeEve {
        characters: vec![character(MAIN, "Main")],
        jeve: Some(jeve.path().to_path_buf()),
    };
    let (mut harness, _) = start(&env, eve, fresh_market(NOW - 240));
    loaded(&mut harness);
    harness
        .get_all_by_label("Assets")
        .next()
        .expect("a rail entry")
        .click();
    harness.run();
    select(&mut harness, PAU);
    harness.snapshot("item-pau");
}

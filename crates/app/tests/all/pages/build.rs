use super::*;

#[test]
fn a_click_on_a_steps_row_or_a_buy_list_row_picks_that_item() {
    let env = env();
    {
        let mut store = Store::open(&env.data.join("user.sqlite"), NOW)
            .expect("the store opens")
            .store;
        store
            .create_plan(
                &PlanRecord {
                    id: 0,
                    name: "Drone 10".to_owned(),
                    product: DRONE,
                    quantity: 10,
                    build: BTreeSet::from([DRONE]),
                    me: BTreeMap::new(),
                    manufacturing: None,
                    reactions: None,
                    price_rule: PriceRule::HighestBuy,
                    route: None,
                    service: None,
                    packing: Packing::TopUp,
                    pasted_stock: String::new(),
                    created_at: NOW,
                    updated_at: NOW,
                },
                NOW,
            )
            .expect("the plan is saved");
    }
    let mut harness = start(&env, FakeEve::new(Vec::new(), None), NOW);
    loaded(&mut harness);
    rail(&mut harness, "Build");
    harness.get_by_label("Drone 10").click();
    harness.run();
    // Steps comes before Buy list in the Build page, so the first PAU row is the Steps one.
    harness
        .get_all_by_label("Particle Accelerator Unit")
        .next()
        .expect("the step row")
        .click();
    harness.run();
    assert_eq!(harness.state_mut().shared_mut().selection.item, Some(PAU));
    // The Buy list is drawn after Steps, so its Morphite row is the last one.
    harness
        .get_all_by_label("Morphite")
        .last()
        .expect("the buy row")
        .click();
    harness.run();
    assert_eq!(
        harness.state_mut().shared_mut().selection.item,
        Some(MORPHITE)
    );
}

#[test]
fn the_build_page_matches_its_snapshot() {
    let env = env();
    let mut harness = start(&env, FakeEve::new(Vec::new(), None), NOW);
    loaded(&mut harness);
    rail(&mut harness, "Build");
    harness.remove_cursor();
    harness.run();
    harness.snapshot("pages-build");
}

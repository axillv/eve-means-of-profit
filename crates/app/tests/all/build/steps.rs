use super::*;

/// The cell texts of the row labelled `row`, left to right: the item name, then each column
/// that has text (the Build box has none). Spare 0 is an empty text.
fn cells(harness: &Harness<'_, App>, row: &str) -> Vec<String> {
    let edge = steps_edge(harness);
    let y = steps_y(harness, row);
    let nodes: Vec<(f32, f32, Option<String>)> = harness
        .query_all(by().predicate(|node| node.bounding_box().is_some()))
        .map(|node| (node.rect().min.x, node.rect().center().y, node.value()))
        .collect();
    // The Steps table is wider than its panel and scrolls sideways, so its cells can reach past
    // the Buy list's edge. Nodes come in drawing order, left to right within a row; the row ends
    // where the x-position falls back, which is where the Buy list's first node starts.
    let start = nodes
        .iter()
        .position(|(x, _, value)| *x < edge && value.as_deref() == Some(row))
        .expect("the row in Steps");
    let mut found: Vec<String> = Vec::new();
    let mut last = f32::MIN;
    for (x, node_y, value) in &nodes[start..] {
        if (node_y - y).abs() >= 4.0 {
            continue;
        }
        if *x < last {
            break;
        }
        last = *x;
        if let Some(value) = value
            && !value.is_empty()
        {
            found.push(value.clone());
        }
    }
    found
}

/// Whether the Build box on the row labelled `row` is disabled.
fn tick_disabled(harness: &Harness<'_, App>, row: &str) -> bool {
    let edge = steps_edge(harness);
    let y = steps_y(harness, row);
    harness
        .get_all_by_role(Role::CheckBox)
        .find(|node| node.rect().min.x < edge && (node.rect().center().y - y).abs() < 4.0)
        .expect("a Build box on the row")
        .accesskit_node()
        .is_disabled()
}

/// Hovers the Build box on the row labelled `row`.
fn hover_tick(harness: &mut Harness<'_, App>, row: &str) {
    let edge = steps_edge(harness);
    let y = steps_y(harness, row);
    harness
        .get_all_by_role(Role::CheckBox)
        .find(|node| node.rect().min.x < edge && (node.rect().center().y - y).abs() < 4.0)
        .expect("a Build box on the row")
        .hover();
    harness.run();
}

#[test]
fn hobgoblin_ii_shows_its_six_parts_in_recipe_order() {
    let env = make_env(&standard_facilities());
    let mut harness = start(&env);
    harness.run();
    hobgoblin_ii_plan(&mut harness);
    assert!(shown(&harness, "PLAN  Hobgoblin II × 10 (unsaved)"));
    let parts = [
        "Morphite",
        "Particle Accelerator Unit",
        "Guidance Systems",
        "Hobgoblin I",
        "Robotics",
        "R.A.M.- Robotics",
    ];
    let tops: Vec<f32> = parts.iter().map(|part| top_of(&harness, part)).collect();
    assert!(tops.windows(2).all(|pair| pair[0] < pair[1]));
    assert_eq!(
        cells(&harness, "Hobgoblin II"),
        ["Hobgoblin II", "2", "10", "0", "10", "—", "10", "1", "…"]
    );
    for part in parts {
        assert_eq!(cells(&harness, part), [part, "—", "10", "0", "—", "10"]);
    }
}

#[rustfmt::skip]
#[test]
fn p3_steps_and_stock_numbers() {
    let env = make_env(&standard_facilities());
    let mut harness = start(&env);
    harness.run();
    p3_plan(&mut harness);
    assert_eq!(
        cells(&harness, "Hobgoblin II"),
        ["Hobgoblin II", "2", "10", "0", "10", "—", "10", "1", "…"]
    );
    assert_eq!(
        cells(&harness, "Hobgoblin I"),
        ["Hobgoblin I", "10", "10", "0", "10", "—", "10", "1", "…"]
    );
    assert_eq!(
        cells(&harness, "Particle Accelerator Unit"),
        [
            "Particle Accelerator Unit",
            "10",
            "10",
            "4",
            "6",
            "—",
            "6",
            "1",
            "…"
        ]
    );
    assert_eq!(
        cells(&harness, "Tritanium"),
        ["Tritanium", "—", "4,307", "1,000", "—", "3,307"]
    );
    assert_eq!(
        cells(&harness, "Pyerite"),
        ["Pyerite", "—", "60", "0", "—", "60"]
    );
    assert_eq!(
        cells(&harness, "Isogen"),
        ["Isogen", "—", "34", "0", "—", "34"]
    );
    assert_eq!(
        cells(&harness, "Nocxium"),
        ["Nocxium", "—", "17", "0", "—", "17"]
    );
    assert_eq!(
        cells(&harness, "Crystalline Carbonide"),
        ["Crystalline Carbonide", "—", "166", "0", "—", "166"]
    );
    assert_eq!(
        cells(&harness, "Fullerides"),
        ["Fullerides", "—", "59", "0", "—", "59"]
    );
    assert_eq!(
        cells(&harness, "Hypersynaptic Fibers"),
        ["Hypersynaptic Fibers", "—", "6", "0", "—", "6"]
    );
    // The materials of Hobgoblin I sit between it and the next part; those of the unit sit
    // between the unit and the part after it.
    assert!(top_of(&harness, "Hobgoblin I") < top_of(&harness, "Tritanium"));
    assert!(top_of(&harness, "Nocxium") < top_of(&harness, "Robotics"));
    assert!(
        top_of(&harness, "Particle Accelerator Unit")
            < top_of(&harness, "Crystalline Carbonide")
    );
    assert!(top_of(&harness, "Hypersynaptic Fibers") < top_of(&harness, "Guidance Systems"));
    assert!(!shown(&harness, "Left out, could not be read:"));
    assert!(!shown(&harness, "does not use"));
}

#[test]
fn paste_problems_are_counted_and_listed() {
    let env = make_env(&standard_facilities());
    let mut harness = start(&env);
    harness.run();
    p3_plan_with(
        &mut harness,
        "Particle Accelerator Unit\t4\nTritanium\t1,000\nNo Such Thing\t4\nRifter\t2",
    );
    click(&mut harness, "Pasted stock…");
    assert!(shown(&harness, "Left out, could not be read:"));
    assert!(shown(
        &harness,
        "Line 3: \"No Such Thing\t4\" (unknown item)"
    ));
    assert!(shown(&harness, "Pasted, but this plan does not use:"));
    assert!(shown(&harness, "Rifter 2"));
}

#[test]
fn quantity_errors_keep_the_last_good_plan() {
    let env = make_env(&standard_facilities());
    let mut harness = start(&env);
    harness.run();
    hobgoblin_ii_plan(&mut harness);
    let last_good = cells(&harness, "Hobgoblin II");
    let mut shown_now = "10";
    for bad in ["0", "abc"] {
        type_into(&mut harness, shown_now, bad);
        shown_now = bad;
        assert!(shown(
            &harness,
            "Quantity must be a whole number of 1 or more"
        ));
        assert!(shown(&harness, "PLAN  Hobgoblin II × 10 (unsaved)"));
        assert_eq!(cells(&harness, "Hobgoblin II"), last_good);
    }
}

#[test]
fn a_facility_with_the_wrong_rig_shows_the_plan_error() {
    let env = make_env(&[("Raitaru bad", facility(35825, &[37157]))]);
    let mut harness = start(&env);
    harness.run();
    hobgoblin_ii_plan(&mut harness);
    pick(
        &mut harness,
        "NPC station · high sec",
        "Raitaru bad · Raitaru · null sec · 1 rigs",
    );
    click(&mut harness, "Save");
    drop(harness);

    let mut store = Store::open(&env.data.join("user.sqlite"), NOW)
        .expect("the store opens")
        .store;
    let row = store.facilities().expect("facilities").remove(0);
    store
        .update_facility(row.id, &row.name, &facility(35825, &[46487]))
        .expect("facility saved");
    drop(store);

    let mut restarted = start(&env);
    restarted.run();
    restarted
        .get_all_by_label("New plan")
        .next()
        .expect("the saved plan's row")
        .click();
    restarted.run_steps(6);
    assert!(shown(
        &restarted,
        "Could not work out the plan: rig 46487 does not fit structure 35825"
    ));
}

#[test]
fn reactions_tick_waits_for_a_reactions_facility() {
    let env = make_env(&standard_facilities());
    let mut harness = start(&env);
    harness.run();
    p3_plan(&mut harness);
    assert!(tick_disabled(&harness, "Crystalline Carbonide"));
    hover_tick(&mut harness, "Crystalline Carbonide");
    assert!(shown(&harness, "Pick a reactions facility first"));
    pick(
        &mut harness,
        "None",
        "Athanor null · Athanor · null sec · 1 rigs",
    );
    tick_row(&mut harness, "Crystalline Carbonide");
    assert_eq!(
        cells(&harness, "Crystalline Carbonide"),
        [
            "Crystalline Carbonide",
            "—",
            "166",
            "0",
            "166",
            "—",
            "1",
            "1",
            "9,834",
            "…"
        ]
    );
    assert_eq!(
        cells(&harness, "Helium Fuel Block"),
        ["Helium Fuel Block", "—", "5", "0", "—", "5"]
    );
    assert_eq!(
        cells(&harness, "Crystallite Alloy"),
        ["Crystallite Alloy", "—", "98", "0", "—", "98"]
    );
    assert_eq!(
        cells(&harness, "Carbon Polymers"),
        ["Carbon Polymers", "—", "98", "0", "—", "98"]
    );
}

#[test]
fn saved_plan_comes_back_the_same_after_a_restart() {
    let env = make_env(&standard_facilities());
    let mut harness = start(&env);
    harness.run();
    p3_plan(&mut harness);
    click(&mut harness, "Rename");
    type_into(&mut harness, "New plan", "Hobgoblin 10");
    click(&mut harness, "Rename");
    click(&mut harness, "Save");
    assert!(!shown(&harness, "PLAN  Hobgoblin II × 10 (unsaved)"));
    click(&mut harness, "New");
    drop(harness);

    let mut restarted = start(&env);
    restarted.run();
    assert!(!shown(&restarted, "PLAN  Hobgoblin II × 10"));
    click(&mut restarted, "Hobgoblin 10");
    assert!(shown(&restarted, "PLAN  Hobgoblin II × 10"));
    assert_eq!(
        cells(&restarted, "Hobgoblin I"),
        ["Hobgoblin I", "10", "10", "0", "10", "—", "10", "1", "…"]
    );
    assert_eq!(
        cells(&restarted, "Particle Accelerator Unit"),
        [
            "Particle Accelerator Unit",
            "10",
            "10",
            "4",
            "6",
            "—",
            "6",
            "1",
            "…"
        ]
    );
    assert!(has_value(
        &restarted,
        "Raitaru null · Raitaru · null sec · 1 rigs"
    ));
    assert!(has_value(&restarted, "Forge sell, 5% price"));
    assert_eq!(
        stored_rule(&env, "Hobgoblin 10"),
        PriceRule::SellFivePercent
    );
    click(&mut restarted, "Pasted stock…");
    assert!(has_value(
        &restarted,
        "Particle Accelerator Unit\t4\nTritanium\t1,000"
    ));
    restarted.get_by_label("Close window").click();
    restarted.run();

    type_into(&mut restarted, "10", "11");
    assert!(shown(&restarted, "PLAN  Hobgoblin II × 11 (unsaved)"));
    click(&mut restarted, "New plan");
    assert!(shown(&restarted, "Save changes to Hobgoblin 10?"));
    click(&mut restarted, "Discard");
    click(&mut restarted, "Hobgoblin 10");
    assert!(shown(&restarted, "PLAN  Hobgoblin II × 10"));
    assert!(!shown(&restarted, "PLAN  Hobgoblin II × 11 (unsaved)"));
}

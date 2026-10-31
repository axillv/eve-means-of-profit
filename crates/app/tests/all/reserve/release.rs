use super::*;

/// Clicks the last node labelled `label`, which is a dialog's button when the panel has one of
/// the same name.
fn click_last(harness: &mut Harness<'_, App>, label: &str) {
    let count = harness.get_all_by_label(label).count();
    assert!(count > 0, "no node labelled {label:?}");
    harness
        .get_all_by_label(label)
        .nth(count - 1)
        .expect("the button")
        .click();
    harness.run_steps(3);
}

#[test]
fn only_reserving_plans_show_the_reserves_label() {
    let env = stock_environment();
    reserving_pair(&env);
    make_plan(&env, "Hobgoblin C", 2, &[HOBGOBLIN_I], "Hobgoblin I\t1");

    let harness = start(&env, login());
    let reserves: Vec<f32> = harness
        .query_all(by().role(Role::Label).predicate(|node| {
            node.label()
                .or_else(|| node.value())
                .is_some_and(|text| text.contains("reserves"))
        }))
        .map(|node| node.rect().center().y)
        .collect();
    assert_eq!(reserves.len(), 2, "{reserves:?}");
    for name in ["Hobgoblin A", "Hobgoblin B"] {
        let row = harness.get_by_label(name).rect().center().y;
        assert!(
            reserves.iter().any(|reserve| (reserve - row).abs() <= 3.0),
            "{name} has no reserves label"
        );
    }
    let c = harness.get_by_label("Hobgoblin C").rect().center().y;
    assert!(
        !reserves.iter().any(|reserve| (reserve - c).abs() <= 3.0),
        "Hobgoblin C has a reserves label"
    );
}

#[test]
fn deleting_a_reserving_plan_releases_its_claims() {
    let env = stock_environment();
    let (_, b) = reserving_pair(&env);

    let mut harness = start(&env, login());
    click(&mut harness, "Hobgoblin A");
    click(&mut harness, "Delete");
    assert!(shown(&harness, "Delete plan Hobgoblin A?"));
    click_last(&mut harness, "Delete");
    assert_eq!(plan_order(&env), vec![b]);

    click(&mut harness, "Hobgoblin B");
    assert_cells(
        &harness,
        "Hobgoblin I",
        &[("Need", "2"), ("Own", "2"), ("Make", "0")],
    );
}

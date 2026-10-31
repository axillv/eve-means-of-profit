use super::*;

/// Right-clicks the row labelled `label`, opening its menu.
fn right_click(harness: &mut Harness<'_, App>, label: &str) {
    harness.get_by_label(label).scroll_to_me();
    harness.run_steps(2);
    harness.get_by_label(label).hover();
    harness.run_steps(2);
    harness.get_by_label(label).click_secondary();
    harness.run_steps(3);
}

/// Whether the menu entry labelled `label` is disabled.
fn menu_disabled(harness: &Harness<'_, App>, label: &str) -> bool {
    harness.get_by_label(label).accesskit_node().is_disabled()
}

/// Drags the row labelled `from` onto the row labelled `to`, moving in steps so egui sees the
/// pointer travel.
fn drag(harness: &mut Harness<'_, App>, from: &str, to: &str) {
    let source = harness.get_by_label(from).rect().center();
    let target = harness.get_by_label(to).rect().center();
    harness.hover_at(source);
    harness.run_steps(2);
    harness.drag_at(source);
    harness.run_steps(2);
    let middle = pos2(source.x, source.y + (target.y - source.y) / 2.0);
    harness.hover_at(middle);
    harness.run_steps(2);
    harness.hover_at(target);
    harness.run_steps(2);
    harness.drop_at(target);
    harness.run_steps(4);
}

#[test]
fn the_menu_moves_a_plan_down_and_the_claims_follow_it() {
    let env = stock_environment();
    let (a, b) = reserving_pair(&env);

    let mut harness = start(&env, login());
    right_click(&mut harness, "Hobgoblin A");
    assert!(!menu_disabled(&harness, "Move down"));
    click(&mut harness, "Move down");

    assert_eq!(plan_order(&env), vec![b, a]);
    assert!(top(&harness, "Hobgoblin B") < top(&harness, "Hobgoblin A"));

    click(&mut harness, "Hobgoblin B");
    assert_cells(
        &harness,
        "Hobgoblin I",
        &[("Need", "2"), ("Own", "2"), ("Make", "0")],
    );
    click(&mut harness, "Hobgoblin A");
    assert_cells(
        &harness,
        "Hobgoblin I",
        &[("Need", "10"), ("Own", "6"), ("Make", "4")],
    );
    assert_cells(
        &harness,
        "Tritanium",
        &[("Need", "2,036"), ("Own", "1,500"), ("Buy", "536")],
    );
    drop(harness);

    let harness = start(&env, login());
    assert!(top(&harness, "Hobgoblin B") < top(&harness, "Hobgoblin A"));
}

#[test]
fn the_fresh_list_ends_cannot_move_further() {
    let env = stock_environment();
    reserving_pair(&env);

    let mut harness = start(&env, login());
    right_click(&mut harness, "Hobgoblin A");
    assert!(menu_disabled(&harness, "Move up"));
    assert!(!menu_disabled(&harness, "Move down"));
    drop(harness);

    let mut harness = start(&env, login());
    right_click(&mut harness, "Hobgoblin B");
    assert!(menu_disabled(&harness, "Move down"));
    assert!(!menu_disabled(&harness, "Move up"));
}

#[test]
fn dragging_a_plan_onto_another_reorders_the_list() {
    let env = stock_environment();
    let (a, b) = reserving_pair(&env);
    let c = make_plan(&env, "Hobgoblin C", 2, &[HOBGOBLIN_I], "Hobgoblin I\t1");

    let mut harness = start(&env, login());
    drag(&mut harness, "Hobgoblin C", "Hobgoblin A");
    assert_eq!(plan_order(&env), vec![c, a, b]);
    assert!(top(&harness, "Hobgoblin C") < top(&harness, "Hobgoblin A"));
    assert!(top(&harness, "Hobgoblin A") < top(&harness, "Hobgoblin B"));

    drag(&mut harness, "Hobgoblin A", "Hobgoblin B");
    assert_eq!(plan_order(&env), vec![c, b, a]);
    assert!(top(&harness, "Hobgoblin C") < top(&harness, "Hobgoblin B"));
    assert!(top(&harness, "Hobgoblin B") < top(&harness, "Hobgoblin A"));
}

use super::*;

const SCOPE_HOVER: &str =
    "Plans that reserve take stock in list order; later plans only see what is left.";
const NEEDS_STOCK: &str = "Reserve needs stock from characters.";

/// Clicks the tick labelled `label`, near its left edge: a long label can run under a panel
/// that takes a click at its middle.
fn click_box(harness: &mut Harness<'_, App>, label: &str) {
    let rect = harness
        .get_all_by_role(Role::CheckBox)
        .find(|node| node.accesskit_node().label().as_deref() == Some(label))
        .unwrap_or_else(|| panic!("no {label:?} tick"))
        .rect();
    let pos = pos2(rect.min.x + 6.0, rect.center().y);
    harness.event(Event::PointerMoved(pos));
    harness.run();
    for pressed in [true, false] {
        harness.event(Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        });
        harness.run();
    }
}

/// Whether the tick labelled `label` is on.
fn ticked(harness: &Harness<'_, App>, label: &str) -> bool {
    harness
        .get_all_by_role(Role::CheckBox)
        .find(|node| node.accesskit_node().label().as_deref() == Some(label))
        .unwrap_or_else(|| panic!("no {label:?} tick"))
        .accesskit_node()
        .toggled()
        == Some(Toggled::True)
}

/// Whether the tick labelled `label` is disabled.
fn disabled(harness: &Harness<'_, App>, label: &str) -> bool {
    harness
        .get_all_by_role(Role::CheckBox)
        .find(|node| node.accesskit_node().label().as_deref() == Some(label))
        .unwrap_or_else(|| panic!("no {label:?} tick"))
        .accesskit_node()
        .is_disabled()
}

/// Opens the dropdown that shows `current` and picks `option` from it.
fn pick(harness: &mut Harness<'_, App>, current: &str, option: &str) {
    harness
        .get_by(|node| node.value().is_some_and(|value| value == current))
        .click();
    harness.run();
    click(harness, option);
}

/// The claimed line in the Plan panel, or `None` when it shows none.
fn claimed(harness: &Harness<'_, App>) -> Option<String> {
    harness
        .query_all(by().predicate(|node| node.bounding_box().is_some()))
        .filter_map(|node| node.value())
        .find(|value| value.starts_with("Claimed by other plans:"))
}

#[test]
fn the_claimed_line_names_the_plans_before_this_one() {
    let env = stock_environment();
    reserving_pair(&env);

    let mut harness = start(&env, login());
    click(&mut harness, "Hobgoblin B");
    assert_eq!(
        claimed(&harness).as_deref(),
        Some(
            "Claimed by other plans: Hobgoblin I 4 (Hobgoblin A), Tritanium 1,500 (Hobgoblin A)"
        )
    );
    assert!(ticked(&harness, "Reserve"));
    let tip = hover(&mut harness, "Reserve");
    assert!(
        tip.iter().any(|line| line == SCOPE_HOVER),
        "the Reserve hover: {tip:?}"
    );

    click(&mut harness, "Hobgoblin A");
    assert!(claimed(&harness).is_none(), "{:?}", claimed(&harness));
    assert!(ticked(&harness, "Reserve"));
    let tip = hover(&mut harness, "Reserve");
    assert!(
        tip.iter().any(|line| line == SCOPE_HOVER),
        "the Reserve hover: {tip:?}"
    );
}

#[test]
fn an_unticked_reserve_leaves_the_plan_out_of_the_order() {
    let env = stock_environment();
    let (a, b) = reserving_pair(&env);

    let mut harness = start(&env, login());
    click(&mut harness, "Hobgoblin A");
    click_box(&mut harness, "Reserve");
    assert!(!ticked(&harness, "Reserve"));
    assert_eq!(store(&env).reserving_plans().expect("reserving"), vec![b]);
    // Out of the order, A is served after B and so sees what B claimed.
    assert!(claimed(&harness).is_some(), "A: {:?}", claimed(&harness));

    click(&mut harness, "Hobgoblin B");
    assert!(claimed(&harness).is_none(), "{:?}", claimed(&harness));
    assert_cells(
        &harness,
        "Hobgoblin I",
        &[("Need", "2"), ("Own", "2"), ("Make", "0")],
    );

    click(&mut harness, "Hobgoblin A");
    click_box(&mut harness, "Reserve");
    assert!(ticked(&harness, "Reserve"));
    assert_eq!(
        store(&env).reserving_plans().expect("reserving"),
        vec![a, b]
    );
    // Back at the head of the order, A has nothing claimed from it.
    assert!(claimed(&harness).is_none(), "A: {:?}", claimed(&harness));

    click(&mut harness, "Hobgoblin B");
    assert_eq!(
        claimed(&harness).as_deref(),
        Some(
            "Claimed by other plans: Hobgoblin I 4 (Hobgoblin A), Tritanium 1,500 (Hobgoblin A)"
        )
    );
    assert_cells(
        &harness,
        "Hobgoblin I",
        &[("Need", "2"), ("Own", "1"), ("Make", "1")],
    );
}

#[test]
fn pasted_stock_cannot_reserve() {
    let env = stock_environment();
    let (_, b) = reserving_pair(&env);

    let mut harness = start(&env, login());
    click(&mut harness, "Hobgoblin A");
    assert!(ticked(&harness, "Reserve"));
    pick(&mut harness, "From characters", "Pasted");

    assert!(!ticked(&harness, "Reserve"));
    assert!(disabled(&harness, "Reserve"));
    let tick = harness
        .get_all_by_role(Role::CheckBox)
        .find(|node| node.accesskit_node().label().as_deref() == Some("Reserve"))
        .expect("the Reserve tick");
    tick.hover();
    for _ in 0..60 {
        harness.run();
    }
    assert!(shown(&harness, NEEDS_STOCK), "{:?}", claimed(&harness));
    assert_eq!(store(&env).reserving_plans().expect("reserving"), vec![b]);

    // A no longer reserves, so B sees the whole shared stock again.
    click(&mut harness, "Hobgoblin B");
    assert!(claimed(&harness).is_none(), "{:?}", claimed(&harness));
    assert_cells(
        &harness,
        "Hobgoblin I",
        &[("Need", "2"), ("Own", "2"), ("Make", "0")],
    );
}

#[test]
fn the_reserve_snapshot_is_taken() {
    let env = stock_environment();
    reserving_pair(&env);
    let mut harness = start(&env, login());
    click(&mut harness, "Hobgoblin B");
    snapshot(&mut harness, "build_reserve");
}

#[test]
fn the_plans_order_snapshot_is_taken() {
    let env = stock_environment();
    reserving_pair(&env);
    make_plan(&env, "Hobgoblin C", 2, &[HOBGOBLIN_I], "Hobgoblin I\t1");
    let mut harness = start(&env, login());
    click(&mut harness, "Hobgoblin A");
    snapshot(&mut harness, "plans_order");
}

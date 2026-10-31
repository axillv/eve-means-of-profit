use super::*;

/// Asserts the Steps table shows no `row`.
fn assert_no_row(harness: &Harness<'_, App>, row: &str) {
    let found = row_nodes(harness, row);
    assert!(found.is_empty(), "{row} is shown: {found:?}");
}

#[test]
fn claims_are_taken_in_the_plan_order_a_then_b() {
    let env = stock_environment();
    let a = make_plan(&env, "Hobgoblin A", 10, &[HOBGOBLIN_I], "");
    use_stock(&env, a, &[MAIN, ALT], only(&[JITA_4_4]));
    reserve(&env, a);
    let b = make_plan(&env, "Hobgoblin B", 2, &[HOBGOBLIN_I], "");
    use_stock(&env, b, &[MAIN], Places::All);
    reserve(&env, b);
    assign(&env, 501, b);

    let mut harness = start(&env, login());
    click(&mut harness, "Hobgoblin A");
    assert_cells(
        &harness,
        "Hobgoblin I",
        &[("Need", "10"), ("Own", "7"), ("Make", "3")],
    );
    assert_cells(
        &harness,
        "Tritanium",
        &[("Need", "1,527"), ("Own", "1,500"), ("Buy", "27")],
    );

    click(&mut harness, "Hobgoblin B");
    assert_cells(
        &harness,
        "Hobgoblin I",
        &[("Need", "2"), ("Own", "1"), ("Make", "1")],
    );
    assert_cells(
        &harness,
        "Tritanium",
        &[("Need", "509"), ("Own", "509"), ("Buy", "0")],
    );
}

#[test]
fn the_plan_served_first_takes_the_shared_units() {
    let env = stock_environment();
    let a = make_plan(&env, "Hobgoblin A", 10, &[HOBGOBLIN_I], "");
    use_stock(&env, a, &[MAIN, ALT], only(&[JITA_4_4]));
    reserve(&env, a);
    let b = make_plan(&env, "Hobgoblin B", 2, &[HOBGOBLIN_I], "");
    use_stock(&env, b, &[MAIN], Places::All);
    reserve(&env, b);
    assign(&env, 501, b);
    set_order(&env, &[b, a]);

    let mut harness = start(&env, login());
    click(&mut harness, "Hobgoblin B");
    assert_cells(
        &harness,
        "Hobgoblin I",
        &[("Need", "2"), ("Own", "2"), ("Make", "0")],
    );
    assert_no_row(&harness, "Tritanium");

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
}

#[test]
fn a_plan_that_does_not_reserve_sees_every_claim() {
    let env = stock_environment();
    let a = make_plan(&env, "Hobgoblin A", 10, &[HOBGOBLIN_I], "");
    use_stock(&env, a, &[MAIN, ALT], only(&[JITA_4_4]));
    let b = make_plan(&env, "Hobgoblin B", 2, &[HOBGOBLIN_I], "");
    use_stock(&env, b, &[MAIN], Places::All);
    reserve(&env, b);
    assign(&env, 501, b);
    set_order(&env, &[b, a]);

    let mut harness = start(&env, login());
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
}

#[test]
fn a_saved_edit_changes_what_the_next_plan_gets() {
    let env = stock_environment();
    let a = make_plan(&env, "Hobgoblin A", 10, &[HOBGOBLIN_I], "");
    use_stock(&env, a, &[MAIN, ALT], only(&[JITA_4_4]));
    reserve(&env, a);
    let b = make_plan(&env, "Hobgoblin B", 2, &[HOBGOBLIN_I], "");
    use_stock(&env, b, &[MAIN], Places::All);
    reserve(&env, b);
    assign(&env, 501, b);

    let mut harness = start(&env, login());
    click(&mut harness, "Hobgoblin A");
    harness
        .get_by(|node| node.role() == Role::TextInput && node.value().as_deref() == Some("10"))
        .click();
    harness.key_press_modifiers(Modifiers::COMMAND, Key::A);
    harness.event(Event::Text("3".to_owned()));
    harness.run();
    click(&mut harness, "Save");

    click(&mut harness, "Hobgoblin B");
    assert_cells(
        &harness,
        "Hobgoblin I",
        &[("Need", "2"), ("Own", "2"), ("Make", "0")],
    );
    assert_no_row(&harness, "Tritanium");
}

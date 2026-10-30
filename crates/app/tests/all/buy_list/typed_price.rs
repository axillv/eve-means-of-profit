use super::*;

#[test]
fn a_typed_price_freezes_the_line_until_it_is_cleared() {
    let world = World::new();
    let mut harness = complete_list(&world, &standard_services());
    type_price(&mut harness, "Robotics", "1,000,000,000.00");
    assert_row(
        &harness,
        "Robotics",
        &[
            "1,000,000,000.00",
            "39,990.00",
            "+2,500,525%",
            "manual",
            "10,000,000,000.00",
        ],
    );
    // The typed price is in the accent colour; the line on the live price is not.
    let boxes = price_boxes(&mut harness, &["Robotics", "Hobgoblin I"]);
    assert!(coloured(&mut harness, boxes[0].0));
    assert!(!coloured(&mut harness, boxes[1].0));
    world.at(NOW + 600);
    lock(&world.forge).publish(
        {
            let mut lines = standard_lines();
            lines
                .iter_mut()
                .find(|line| line.type_id == 9848)
                .expect("Robotics")
                .five = "41000.00";
            lines
        },
        NOW + 600,
    );
    world.at(NOW + 900);
    click(&mut harness, "Refresh now");
    settle(&mut harness, "41,000.00");
    assert_row(
        &harness,
        "Robotics",
        &["1,000,000,000.00", "41,000.00", "manual"],
    );
    type_price(&mut harness, "Robotics", "");
    assert_row(&harness, "Robotics", &["41,000.00", "41,000.00", "5m"]);
}

#[test]
fn a_price_that_is_not_a_number_keeps_the_old_one() {
    let world = World::new();
    let mut harness = complete_list(&world, &standard_services());
    type_price(&mut harness, "Robotics", "abc");
    assert!(shown(&harness, "Not a price"));
    assert_row(&harness, "Robotics", &["39,990.00"]);
    assert!(!shown(&harness, "manual"));
}

#[test]
fn a_negative_price_is_not_a_price() {
    let world = World::new();
    let mut harness = complete_list(&world, &standard_services());
    type_price(&mut harness, "Robotics", "-1");
    assert!(shown(&harness, "Not a price"));
    assert_row(&harness, "Robotics", &["39,990.00"]);
}

#[test]
fn a_typed_price_for_a_missing_side_prices_the_line() {
    let world = World::new();
    lock(&world.forge).lines.retain(|line| line.type_id != 9848);
    let env = make_env(&standard_services());
    let mut harness = start(&env, &world);
    harness.run();
    buy_list_for(&mut harness);
    settle(&mut harness, "Oldest: 4m");
    type_price(&mut harness, "Robotics", "39,990.00");
    assert_row(&harness, "Robotics", &["39,990.00", "399,900.00"]);
    // With no market row there is no live price: Live is `—`, and the age reads `manual`.
    assert_eq!(
        cells(&harness, "Robotics"),
        [
            "Robotics",
            "10",
            "0",
            "10",
            "39,990.00",
            "30.00",
            "399,900.00",
            "—",
            "manual",
        ]
    );
    assert!(!shown(&harness, " incomplete"));
}

#[test]
fn a_typed_price_on_a_saved_plan_is_kept_across_a_restart_until_it_is_cleared() {
    let world = World::new();
    let env = make_env(&standard_services());
    let mut harness = start(&env, &world);
    harness.run();
    buy_list_for(&mut harness);
    settle(&mut harness, "Oldest: 4m");
    click(&mut harness, "Save");
    type_price(&mut harness, "Robotics", "1,000,000,000.00");
    drop(harness);

    let mut harness = start(&env, &world);
    harness.run();
    open_saved_plan(&mut harness, "New plan");
    settle(&mut harness, "Oldest: 4m");
    assert_row(&harness, "Robotics", &["1,000,000,000.00", "manual"]);
    let boxes = price_boxes(&mut harness, &["Robotics", "Hobgoblin I"]);
    assert!(coloured(&mut harness, boxes[0].0));
    assert!(!coloured(&mut harness, boxes[1].0));
    type_price(&mut harness, "Robotics", "");
    drop(harness);

    let mut harness = start(&env, &world);
    harness.run();
    open_saved_plan(&mut harness, "New plan");
    settle(&mut harness, "Oldest: 4m");
    assert_row(&harness, "Robotics", &["39,990.00", "4m"]);
    assert!(!shown(&harness, "manual"));
}

#[test]
fn a_typed_price_that_cannot_be_saved_is_reported_and_stays_on_screen() {
    use std::os::unix::fs::PermissionsExt;

    let world = World::new();
    let mut harness = complete_list(&world, &standard_services());
    let data = lock_folders(&world)[0].data.clone();
    // A data folder that cannot be written to makes the journal, and so the save, fail.
    let writable = fs::metadata(&data).expect("the data folder").permissions();
    let mut read_only = writable.clone();
    read_only.set_mode(0o555);
    fs::set_permissions(&data, read_only).expect("the data folder is read-only");

    type_price(&mut harness, "Robotics", "1,000,000,000.00");
    fs::set_permissions(&data, writable).expect("the data folder is writable again");

    assert!(
        harness
            .query_all(by().label_contains("Could not save the price:"))
            .next()
            .is_some()
    );
    assert_row(&harness, "Robotics", &["1,000,000,000.00", "manual"]);
}

#[test]
fn a_typed_price_is_kept_when_the_plan_is_left_and_opened_again() {
    let world = World::new();
    let env = make_env(&standard_services());
    let mut harness = start(&env, &world);
    harness.run();
    buy_list_for(&mut harness);
    settle(&mut harness, "Oldest: 4m");
    click(&mut harness, "Save");
    type_price(&mut harness, "Robotics", "1,000,000,000.00");
    click(&mut harness, "New");
    harness.run();
    click(&mut harness, "New plan");
    settle(&mut harness, "manual");
    assert_row(&harness, "Robotics", &["1,000,000,000.00", "manual"]);
}

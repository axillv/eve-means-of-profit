use super::*;

#[test]
fn nothing_selected_shows_no_inspector_on_build_or_assets() {
    let env = env();
    let (mut harness, _) = start(
        &env,
        FakeEve {
            characters: Vec::new(),
            jeve: None,
        },
        Answer::Down,
    );
    loaded(&mut harness);
    assert_no_inspector(&harness, "Build");

    harness
        .get_all_by_label("Assets")
        .next()
        .expect("a rail entry")
        .click();
    harness.run();
    assert_no_inspector(&harness, "Assets");
}

/// Neither the window nor its docked place is drawn before an item is picked.
fn assert_no_inspector(harness: &Harness<'_, App>, page: &str) {
    assert!(
        !shown(harness, "Click an item to see it here."),
        "{page}: no click hint"
    );
    assert_eq!(
        harness.query_all_by_label("Item").count(),
        0,
        "{page}: no Item window or tab"
    );
}

#[test]
fn a_click_on_a_step_row_opens_the_inspector_and_escape_or_close_hides_it() {
    let env = env();
    save_plans(&env);
    let (mut harness, _) = start(
        &env,
        FakeEve {
            characters: Vec::new(),
            jeve: None,
        },
        Answer::Down,
    );
    loaded(&mut harness);

    harness.get_by_label("Hobgoblin 10").click();
    harness.run();
    click_first_step(&mut harness);
    assert_eq!(harness.state_mut().shared_mut().selection.item, Some(PAU));
    assert!(shown(&harness, "None owned."));
    assert!(shown(&harness, "Hobgoblin 10 · need 10"));

    harness.key_press(egui::Key::Escape);
    harness.run();
    assert!(!shown(&harness, "None owned."));
    assert!(!shown(&harness, "Hobgoblin 10 · need 10"));
    assert_eq!(harness.state_mut().shared_mut().selection.item, None);

    click_first_step(&mut harness);
    assert!(shown(&harness, "None owned."));

    harness.get_by_label("Close window").click();
    harness.run();
    assert!(!shown(&harness, "None owned."));
    assert_eq!(harness.state_mut().shared_mut().selection.item, None);
}

#[test]
fn pin_keeps_the_inspector_open_through_escape_and_close() {
    let env = env();
    save_plans(&env);
    let (mut harness, _) = start(
        &env,
        FakeEve {
            characters: Vec::new(),
            jeve: None,
        },
        Answer::Down,
    );
    loaded(&mut harness);

    harness.get_by_label("Hobgoblin 10").click();
    harness.run();
    click_first_step(&mut harness);

    harness.get_by_label("Pin").click();
    harness.run();
    assert!(harness.state_mut().shared_mut().selection.item_pinned);
    harness.key_press(egui::Key::Escape);
    harness.run();
    assert!(shown(&harness, "None owned."));
    assert_eq!(harness.state_mut().shared_mut().selection.item, Some(PAU));

    harness.get_by_label("Close window").click();
    harness.run();
    assert!(shown(&harness, "None owned."));
    assert_eq!(harness.state_mut().shared_mut().selection.item, Some(PAU));

    harness.get_by_label("Pin").click();
    harness.run();
    assert!(!harness.state_mut().shared_mut().selection.item_pinned);
    harness.key_press(egui::Key::Escape);
    harness.run();
    assert!(!shown(&harness, "None owned."));
    assert_eq!(harness.state_mut().shared_mut().selection.item, None);
}

/// Clicks the first step row for a Particle Accelerator Unit and runs a frame.
fn click_first_step(harness: &mut Harness<'_, App>) {
    harness
        .get_all_by_label("Particle Accelerator Unit")
        .next()
        .expect("the step row")
        .click();
    harness.run();
}

#[test]
fn the_owned_table_copies_once_from_the_inspector() {
    let env = env();
    with_main(&env);
    name_home_fort(&env);
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
    // The copy button appears on the frame after the table first draws.
    harness.run();
    assert_eq!(harness.query_all_by_label("Copy table").count(), 1);

    harness.get_by_label("Copy table").click();
    harness.step();
    let copied = copied_text(&harness).expect("the owned table is copied");
    assert!(
        copied.starts_with("Character\tPlace\tUnits\tCounted\nMain\tHome Fort\t120\t120\n"),
        "{copied}"
    );
    assert!(copied.contains("\nTotal\t\t"), "{copied}");
}

#[test]
fn the_inspector_draws_the_item_window_only_when_an_item_is_picked() {
    let env = env();
    let mut harness = inspector_harness(&env);
    harness.run();
    assert!(!shown(&harness, "Item"));

    harness.state_mut().selection.item = Some(PAU);
    harness.run();
    for text in [
        "Item",
        "Particle Accelerator Unit",
        "None owned.",
        "Plans that use it",
        "No saved plan uses it.",
        "Pin",
    ] {
        assert!(shown(&harness, text), "{text}");
    }
}

#[test]
fn escape_and_close_clear_the_item_unless_pinned() {
    let env = env();
    let mut harness = inspector_harness(&env);
    harness.state_mut().selection.item = Some(PAU);
    harness.run();
    assert!(shown(&harness, "None owned."));

    harness.key_press(egui::Key::Escape);
    harness.run();
    assert!(!shown(&harness, "None owned."));
    assert_eq!(harness.state().selection.item, None);

    {
        let selection = &mut harness.state_mut().selection;
        selection.item = Some(PAU);
        selection.item_pinned = true;
    }
    harness.run();
    assert!(shown(&harness, "None owned."));

    harness.key_press(egui::Key::Escape);
    harness.run();
    assert!(shown(&harness, "None owned."));
    assert_eq!(harness.state().selection.item, Some(PAU));

    harness.get_by_label("Close window").click();
    harness.run();
    assert!(shown(&harness, "None owned."));
    assert_eq!(harness.state().selection.item, Some(PAU));

    harness.get_by_label("Pin").click();
    harness.run();
    harness.key_press(egui::Key::Escape);
    harness.run();
    assert!(!shown(&harness, "None owned."));
    assert_eq!(harness.state().selection.item, None);
}

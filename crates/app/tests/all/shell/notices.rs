use super::*;

#[test]
fn a_user_file_in_another_layout_is_set_aside_with_a_notice() {
    let folders = Folders::new();
    folders.install_game_data();
    import(&fixture(), &folders.user_db()).unwrap();
    let text = format!(
        "Your saved data came from an earlier test build and was set aside as {}.old-test. Starting fresh.",
        folders.user_db().display()
    );
    let mut harness = start(Fake::new(Latest::Status(503)), &folders);
    settle(&mut harness);
    assert!(shown(&harness, "NOTICE"));
    assert!(
        harness
            .query_all_by_label_contains("Your saved data came from an earlier test build")
            .next()
            .is_some()
    );
    let notices = &harness.state_mut().shared_mut().notices;
    assert_eq!(notices[0].text, text);
    assert!(folders.data.join("user.sqlite.old-test").is_file());
}

#[test]
fn a_new_or_current_user_file_gives_no_start_notice() {
    let folders = Folders::new();
    folders.install_game_data();
    folders.set_last_check(990_000);
    for _ in 0..2 {
        let mut harness = start(Fake::new(Latest::Status(503)), &folders);
        settle(&mut harness);
        assert!(build_page_shown(&harness));
        assert!(!shown(&harness, "Starting fresh."));
        assert!(!folders.data.join("user.sqlite.old-test").exists());
    }
}

#[test]
fn a_newer_build_gives_a_notice_when_the_check_is_due() {
    let folders = Folders::new();
    folders.install_game_data();
    folders.set_last_check(900_000);
    let fake = Fake::new(Latest::Body(latest_body(3_607_800)));
    let asked = fake.asked();
    let mut harness = start(fake, &folders);
    until(&mut harness, "the notice", |h| {
        shown(h, "Game data build 3607800 is available.")
    });
    assert!(shown(&harness, "Update"));
    assert_eq!(*asked.lock().unwrap(), vec![LATEST_URL.to_owned()]);
}

#[test]
fn no_check_is_made_when_the_last_one_was_recent() {
    let folders = Folders::new();
    folders.install_game_data();
    folders.set_last_check(990_000);
    let fake = Fake::new(Latest::Body(latest_body(3_607_800)));
    let asked = fake.asked();
    let mut harness = start(fake, &folders);
    settle(&mut harness);
    assert!(asked.lock().unwrap().is_empty());
    assert!(shown(&harness, "No market data yet"));
}

#[test]
fn clicking_setup_shows_its_panels_and_the_locked_pages_keep_their_layout() {
    let folders = Folders::new();
    folders.install_game_data();
    folders.set_last_check(990_000);
    let mut harness = start(Fake::new(Latest::Body(latest_body(BUILD))), &folders);
    until(&mut harness, "the build pages", build_page_shown);

    let plans = harness.get_by_label("Plans").rect();
    assert_eq!(harness.query_all_by_label("Plans").count(), 1);
    harness.event(Event::PointerMoved(plans.center()));
    harness.event(Event::PointerButton {
        pos: plans.center(),
        button: PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    });
    let elsewhere = Pos2::new(plans.center().x + 400.0, plans.center().y + 200.0);
    harness.event(Event::PointerMoved(elsewhere));
    harness.event(Event::PointerButton {
        pos: elsewhere,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    });
    settle(&mut harness);
    assert_eq!(harness.get_by_label("Plans").rect(), plans);

    harness.get_by_label(Page::Setup.label()).click();
    until(&mut harness, "the setup panels", |h| {
        shown(h, "Facilities") && shown(h, "Freight services") && shown(h, "Price rule")
    });
    harness.get_by_label("Build").click();
    until(&mut harness, "the build pages again", build_page_shown);
}

#[test]
fn the_status_line_shows_the_first_notice_and_dismiss_shows_the_next() {
    let folders = Folders::new();
    folders.install_game_data();
    folders.set_last_check(990_000);
    let mut harness = start(Fake::new(Latest::Body(latest_body(BUILD))), &folders);
    settle(&mut harness);
    assert!(shown(&harness, "No market data yet"));

    let shared = harness.state_mut().shared_mut();
    shared
        .notices
        .push(eve_means_of_profit::status::Notice::info("First notice"));
    shared
        .notices
        .push(eve_means_of_profit::status::Notice::info("Second notice"));
    shared
        .notices
        .push(eve_means_of_profit::status::Notice::info("Third notice"));
    settle(&mut harness);
    assert!(shown(&harness, "First notice (1 of 3)"));

    harness.get_by_label("Dismiss").click();
    settle(&mut harness);
    assert!(shown(&harness, "Second notice (1 of 2)"));
}

#[test]
fn refresh_all_keeps_a_notice_and_dismiss_or_its_action_removes_it() {
    use eve_means_of_profit::status::{self, Action, Notice};

    struct State {
        notices: Vec<Notice>,
        action: Option<Action>,
    }
    let mut harness = Harness::new_ui_state(
        |ui, state: &mut State| {
            state.action = status::show(
                ui,
                &Style::eve_dense(),
                &mut state.notices,
                None,
                "idle",
                "",
            );
        },
        State {
            notices: vec![Notice::info("First notice")],
            action: None,
        },
    );
    harness.run();
    assert!(harness.query_all_by_label("First notice").next().is_some());

    harness.get_by_label("Refresh all").click();
    harness.step();
    assert_eq!(harness.state().notices.len(), 1);
    assert_eq!(harness.state().action, Some(Action::RefreshAll));
    assert!(harness.query_all_by_label("First notice").next().is_some());

    harness.get_by_label("Dismiss").click();
    harness.step();
    assert!(harness.state().notices.is_empty());

    harness.state_mut().notices =
        vec![Notice::info("Update ready").with_action(Action::Update { build: 42 })];
    harness.run();
    harness.get_by_label("Update").click();
    harness.step();
    assert_eq!(harness.state().action, Some(Action::Update { build: 42 }));
    assert!(harness.state().notices.is_empty());
}

#[test]
fn a_failed_daily_check_shows_an_error_notice() {
    let folders = Folders::new();
    folders.install_game_data();
    folders.set_last_check(900_000);
    let mut harness = start(Fake::new(Latest::Status(503)), &folders);
    until(&mut harness, "the check error", |h| {
        shown(
            h,
            "Could not check for new game data: the server answered with status 503",
        )
    });
    assert!(shown(&harness, "ERROR"));
}

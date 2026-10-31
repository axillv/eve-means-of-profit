use super::*;

#[test]
fn the_ui_keeps_drawing_while_a_download_is_pending() {
    let folders = Folders::new();
    let (release, gate) = mpsc::channel();
    let fake = Fake::new(Latest::Body(latest_body(BUILD))).held(0, gate);
    let mut harness = start(fake, &folders);
    until(&mut harness, "the download", |h| {
        shown(h, "Getting the game data")
    });

    let started = Instant::now();
    for _ in 0..30 {
        harness.run_steps(1);
    }
    assert!(started.elapsed() < Duration::from_secs(10));
    assert!(shown(&harness, "Getting the game data"));

    release
        .send(())
        .expect("the download is waiting for the release");
    until(&mut harness, "the welcome", |h| {
        shown(h, "Welcome to EVE Means of Profit")
    });
    finish_first_run(&mut harness);
    until(&mut harness, "the build pages", build_page_shown);
}

#[test]
fn the_snapshots_for_the_owner_are_taken() {
    let folders = Folders::fixed();
    let (release, gate) = mpsc::channel();
    let fake = Fake::new(Latest::Body(latest_body(BUILD))).held(2, gate);
    let mut harness = start(fake, &folders);
    until(&mut harness, "half the download", |h| {
        h.query_all_by_label_contains("Build 3607705 · ")
            .next()
            .is_some()
    });
    settle(&mut harness);
    harness.snapshot("shell-first-run");
    release
        .send(())
        .expect("the download is waiting for the release");
    until(&mut harness, "the welcome", |h| {
        shown(h, "Welcome to EVE Means of Profit")
    });
    harness.snapshot("shell-first-run-welcome");
    finish_first_run(&mut harness);
    until(&mut harness, "the build pages", build_page_shown);
    harness.remove_cursor();
    harness.run();
    harness.snapshot("shell-build-empty");
    harness.get_by_label(Page::Setup.label()).click();
    until(&mut harness, "the setup panels", |h| shown(h, "Price rule"));
    harness.remove_cursor();
    harness.run();
    crate::ui::picture(&mut harness, "shell-setup-empty");
    harness.get_by_label("Build").click();
    until(&mut harness, "the build pages again", build_page_shown);
    harness.key_press_modifiers(Modifiers::CTRL, Key::Comma);
    until(&mut harness, "the settings page", |h| {
        shown(h, "Facilities") && shown(h, "Price history") && shown(h, "Display")
    });
    settle(&mut harness);
    assert_eq!(
        harness
            .query_all_by_role_and_label(egui::accesskit::Role::Window, "Settings")
            .count(),
        0,
        "the Settings page stands in for the window"
    );
    harness.snapshot("shell-settings");
}

#[test]
fn the_error_snapshot_is_taken_on_the_status_line() {
    let folders = Folders::new();
    let mut harness = start(Fake::new(Latest::Status(503)), &folders);
    until(&mut harness, "the error text", |h| {
        shown(
            h,
            "Could not get the game data: the server answered with status 503",
        )
    });
    harness.snapshot("shell-status-error");
}

#[test]
fn update_downloads_in_the_background_and_a_bad_download_is_reported() {
    let folders = Folders::new();
    folders.install_game_data();
    folders.set_last_check(900_000);
    let mut harness = start(Fake::new(Latest::Body(latest_body(3_607_800))), &folders);
    until(&mut harness, "the notice", |h| {
        shown(h, "Game data build 3607800 is available.")
    });
    harness.get_by_label("Update").click();
    // The fixture zip is build 3607705, so the update is refused after the download.
    until(&mut harness, "the update error", |h| {
        shown(
            h,
            "Could not update the game data: the download is build 3607705, but build 3607800 was asked for",
        )
    });
    assert!(!shown(&harness, "Update"));
}

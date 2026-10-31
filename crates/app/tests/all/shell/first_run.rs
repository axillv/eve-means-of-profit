use super::*;

#[test]
fn the_first_run_downloads_imports_and_then_shows_the_welcome() {
    let folders = Folders::new();
    let (release, gate) = mpsc::channel();
    let fake = Fake::new(Latest::Body(latest_body(BUILD))).held(1, gate);
    let mut harness = start(fake, &folders);
    harness.run_steps(1);
    assert!(shown(&harness, "Checking for game data…"));
    assert!(shown(
        &harness,
        "When the game data is ready, try without logging in or import from jEve."
    ));

    until(&mut harness, "the download", |h| {
        shown(h, "Getting the game data")
            && h.query_all_by_label_contains("Build 3607705 · ")
                .next()
                .is_some()
    });
    assert!(shown(
        &harness,
        "When the game data is ready, try without logging in or import from jEve."
    ));
    release
        .send(())
        .expect("the download is waiting for the release");

    until(&mut harness, "the welcome", |h| {
        shown(h, "Welcome to EVE Means of Profit")
    });
    for text in [
        "Welcome to EVE Means of Profit",
        "No EVE login is needed.",
        "Try without logging in",
        "Product, quantity, paste what you own, buy list.",
        "Import from jEve",
        "jEve's characters now; import its history on Net worth and its stockpiles on Stockpiles.",
        "Next: log in on Characters for live data, paste your stock on Build, and pick your facility in Settings.",
    ] {
        assert!(shown(&harness, text), "{text}");
    }
    assert!(!shown(
        &harness,
        "When the game data is ready, try without logging in or import from jEve."
    ));

    assert!(folders.sde_db().is_file());
    let mut store = Store::open(&folders.user_db(), NOW).unwrap().store;
    assert_eq!(store.settings().unwrap().last_sde_check, Some(NOW));
}

#[test]
fn try_without_logging_in_from_the_welcome_opens_build() {
    let folders = Folders::new();
    let mut harness = start(Fake::new(Latest::Body(latest_body(BUILD))), &folders);
    until(&mut harness, "the welcome", |h| {
        shown(h, "Welcome to EVE Means of Profit")
    });
    finish_first_run(&mut harness);
    until(&mut harness, "the build pages", build_page_shown);
    assert!(!shown(&harness, "Welcome to EVE Means of Profit"));
}

#[test]
fn import_from_jeve_from_the_welcome_opens_characters() {
    let folders = Folders::new();
    let mut harness = start(Fake::new(Latest::Body(latest_body(BUILD))), &folders);
    until(&mut harness, "the welcome", |h| {
        shown(h, "Welcome to EVE Means of Profit")
    });
    harness.state_mut().show_page(Page::Today);
    harness.get_by_label("Import from jEve").click();
    until(&mut harness, "the characters page", |h| {
        shown(h, "jEve data") && shown(h, "No jEve data file found.")
    });
    assert!(!shown(&harness, "Welcome to EVE Means of Profit"));
}

#[test]
fn a_later_start_shows_the_build_pages_without_the_welcome() {
    let folders = Folders::new();
    folders.install_game_data();
    folders.set_last_check(990_000);
    let mut harness = start(Fake::new(Latest::Body(latest_body(BUILD))), &folders);
    until(&mut harness, "the build pages", build_page_shown);
    assert!(!shown(&harness, "Welcome to EVE Means of Profit"));
}

#[test]
fn the_rail_offers_build_and_setup_once_the_pages_are_shown() {
    let folders = Folders::new();
    folders.install_game_data();
    folders.set_last_check(990_000);
    let mut harness = start(Fake::new(Latest::Body(latest_body(BUILD))), &folders);
    until(&mut harness, "the build pages", build_page_shown);
    assert_eq!(harness.query_all_by_label("Build").count(), 1);
    assert_eq!(harness.query_all_by_label(Page::Setup.label()).count(), 1);
    harness.get_by_label(Page::Setup.label()).click();
    until(&mut harness, "the setup panels", |h| shown(h, "Price rule"));
    harness.get_by_label("Build").click();
    until(&mut harness, "the build pages again", build_page_shown);
}

#[test]
fn a_saved_icons_only_rail_starts_narrow_with_the_build_entry_short() {
    let folders = Folders::new();
    folders.install_game_data();
    folders.set_last_check(990_000);
    folders.set_rail_icons_only(true);
    let mut harness = start(Fake::new(Latest::Body(latest_body(BUILD))), &folders);
    until(&mut harness, "the build pages", build_page_shown);
    assert!(
        harness.get_by_label("Build").rect().width() <= 48.0,
        "the icons-only Build entry is at most 48 px wide"
    );
}

#[test]
fn a_rail_with_nothing_saved_shows_the_build_entry_wider_than_the_narrow_width() {
    let folders = Folders::new();
    folders.install_game_data();
    folders.set_last_check(990_000);
    let mut harness = start(Fake::new(Latest::Body(latest_body(BUILD))), &folders);
    until(&mut harness, "the build pages", build_page_shown);
    assert!(
        harness.get_by_label("Build").rect().width() > 48.0,
        "the labelled Build entry is wider than 48 px"
    );
}

#[test]
fn the_locked_pages_show_each_panel_title_once_and_no_tab_bar() {
    let folders = Folders::new();
    folders.install_game_data();
    folders.set_last_check(990_000);
    let mut harness = start(Fake::new(Latest::Body(latest_body(BUILD))), &folders);
    until(&mut harness, "the build pages", build_page_shown);
    for panel in ["Plans", "Plan", "Steps", "Buy list"] {
        assert_eq!(harness.query_all_by_label(panel).count(), 1, "{panel}");
    }
    harness.get_by_label(Page::Setup.label()).click();
    until(&mut harness, "the setup panels", |h| shown(h, "Price rule"));
    for panel in ["Facilities", "Freight services", "Price rule"] {
        assert_eq!(harness.query_all_by_label(panel).count(), 1, "{panel}");
    }
}

#[test]
fn a_failed_first_download_shows_the_error_and_try_again() {
    let folders = Folders::new();
    let fake = Fake::new(Latest::Body(latest_body(BUILD))).failing_download("connection reset");
    let mut harness = start(fake, &folders);
    let text = "Could not get the game data: cannot reach the server: connection reset";
    until(&mut harness, "the error text", |h| shown(h, text));
    assert_eq!(harness.query_all_by_label(text).count(), 2);
    assert!(shown(&harness, "ERROR"));
    assert!(shown(&harness, "Try again"));
}

#[test]
fn the_status_line_shows_the_update_progress_while_the_download_runs() {
    let folders = Folders::new();
    folders.install_game_data();
    folders.set_last_check(900_000);
    let (release, gate) = mpsc::channel();
    let fake = Fake::new(Latest::Body(latest_body(3_607_800))).held(1, gate);
    let mut harness = start(fake, &folders);
    until(&mut harness, "the notice", |h| {
        shown(h, "Game data build 3607800 is available.")
    });
    harness.get_by_label("Update").click();
    until(&mut harness, "the update progress", |h| {
        h.query_all_by_label_contains("Updating game data:")
            .next()
            .is_some()
    });
    release
        .send(())
        .expect("the download is waiting for the release");
    until(&mut harness, "the update error", |h| {
        h.query_all_by_label_contains("Could not update the game data: ")
            .next()
            .is_some()
    });
}

#[test]
fn cancel_shows_the_cancelled_text_and_try_again_finishes() {
    let folders = Folders::new();
    let fake = Fake::new(Latest::Body(latest_body(BUILD))).paused(Duration::from_millis(200));
    let mut harness = start(fake, &folders);
    until(&mut harness, "the Cancel button", |h| shown(h, "Cancel"));
    harness.get_by_label("Cancel").click();
    until(&mut harness, "the cancelled text", |h| {
        shown(h, "Download cancelled.")
    });
    harness.get_by_label("Try again").click();
    until(&mut harness, "the welcome", |h| {
        shown(h, "Welcome to EVE Means of Profit")
    });
    finish_first_run(&mut harness);
    until(&mut harness, "the build pages", build_page_shown);
    assert!(folders.sde_db().is_file());
}

#[test]
fn a_server_error_shows_on_the_first_run_screen_and_in_the_status_line() {
    let folders = Folders::new();
    let fake = Fake::new(Latest::Status(503));
    let mut harness = start(fake, &folders);
    let text = "Could not get the game data: the server answered with status 503";
    until(&mut harness, "the error text", |h| shown(h, text));
    assert_eq!(harness.query_all_by_label(text).count(), 2);
    assert!(shown(&harness, "ERROR"));
    assert!(shown(&harness, "Try again"));
}

#[test]
fn the_importing_screen_shows_until_the_import_ends() {
    let folders = Folders::new();
    let (release, gate) = mpsc::channel();
    let fake = Fake::new(Latest::Body(latest_body(BUILD))).held(4, gate);
    let mut harness = start(fake, &folders);
    until(&mut harness, "the importing text", |h| {
        shown(h, "Importing game data…")
    });
    release
        .send(())
        .expect("the download is waiting for the release");
    until(&mut harness, "the welcome", |h| {
        shown(h, "Welcome to EVE Means of Profit")
    });
    finish_first_run(&mut harness);
    until(&mut harness, "the build pages", build_page_shown);
}

use super::*;

/// Scrolls the Display panel with the pointer held at `at`, by `delta` points down (negative)
/// or up (positive). The controls at the bottom of the panel need it.
fn scroll_display(harness: &mut Harness<'_, App>, at: Pos2, delta: f32) {
    harness.hover_at(at);
    harness.event(Event::MouseWheel {
        unit: MouseWheelUnit::Point,
        delta: vec2(0.0, delta),
        phase: TouchPhase::Move,
        modifiers: Modifiers::NONE,
    });
    harness.run_steps(20);
}

/// Whether the rail shows icons alone in the test's data folder.
fn saved_rail_icons_only(folders: &Folders) -> bool {
    let mut store = Store::open(&folders.user_db(), NOW).unwrap().store;
    store.settings().unwrap().rail_icons_only
}

/// The text put on the clipboard by the frame just drawn, if any.
fn copied_text(harness: &Harness<'_, App>) -> Option<String> {
    harness
        .output()
        .platform_output
        .commands
        .iter()
        .find_map(|command| {
            if let egui::OutputCommand::CopyText(text) = command {
                Some(text.clone())
            } else {
                None
            }
        })
}

/// The top edge of the first node labelled `label`, to check what is above what.
fn top_of(harness: &Harness<'_, App>, label: &str) -> f32 {
    harness
        .query_all_by_label(label)
        .next()
        .unwrap_or_else(|| panic!("no node labelled {label}"))
        .rect()
        .min
        .y
}

/// Whether the radio button labelled `label` is the selected one.
fn selected(harness: &Harness<'_, App>, label: &str) -> bool {
    harness
        .query_all_by_label(label)
        .next()
        .unwrap_or_else(|| panic!("no node labelled {label}"))
        .accesskit_node()
        .toggled()
        == Some(egui::accesskit::Toggled::True)
}

/// Whether the checkbox labelled `label` is ticked.
fn ticked(harness: &Harness<'_, App>, label: &str) -> bool {
    harness
        .query_all_by_label(label)
        .find(|node| node.accesskit_node().role() == egui::accesskit::Role::CheckBox)
        .unwrap_or_else(|| panic!("no checkbox labelled {label}"))
        .accesskit_node()
        .toggled()
        == Some(egui::accesskit::Toggled::True)
}

#[test]
fn ctrl_comma_opens_the_settings_page_and_a_density_is_saved() {
    let folders = Folders::new();
    folders.install_game_data();
    folders.set_last_check(990_000);
    let mut harness = start(Fake::new(Latest::Body(latest_body(BUILD))), &folders);
    until(&mut harness, "the build pages", build_page_shown);

    harness.key_press_modifiers(Modifiers::CTRL, Key::Comma);
    until(&mut harness, "the settings page", |h| {
        shown(h, "Interface size")
    });
    harness.get_by_label("Dense").click();
    settle(&mut harness);

    let mut store = Store::open(&folders.user_db(), NOW).unwrap().store;
    assert_eq!(store.settings().unwrap().density, Density::Dense);
}

#[test]
fn ctrl_comma_and_the_view_menu_open_the_settings_page() {
    let folders = Folders::new();
    folders.install_game_data();
    folders.set_last_check(990_000);
    let mut harness = start(Fake::new(Latest::Body(latest_body(BUILD))), &folders);
    until(&mut harness, "the build pages", build_page_shown);

    harness.key_press_modifiers(Modifiers::CTRL, Key::Comma);
    until(&mut harness, "the settings page", |h| {
        shown(h, "Facilities") && shown(h, "Price history") && shown(h, "Display")
    });
    assert!(
        top_of(&harness, "Facilities") < top_of(&harness, "Price history")
            && top_of(&harness, "Price history") < top_of(&harness, "Display"),
        "the three panels sit down the left in that order"
    );
    assert_eq!(
        harness
            .query_all_by_role_and_label(egui::accesskit::Role::Window, "Settings")
            .count(),
        0,
        "no Settings window opens"
    );

    harness.get_by_label("Build").click();
    until(&mut harness, "the build pages again", build_page_shown);
    harness.get_by_label("View").click();
    until(&mut harness, "the View menu", |h| shown(h, "Settings…"));
    harness.get_by_label("Settings…").click();
    until(&mut harness, "the settings page from the menu", |h| {
        shown(h, "Facilities") && shown(h, "Price history") && shown(h, "Display")
    });
    assert!(
        top_of(&harness, "Facilities") < top_of(&harness, "Price history")
            && top_of(&harness, "Price history") < top_of(&harness, "Display"),
        "the menu opens the same page with the panels in that order"
    );
    assert_eq!(
        harness
            .query_all_by_role_and_label(egui::accesskit::Role::Window, "Settings")
            .count(),
        0,
        "the menu opens no Settings window"
    );
}

#[test]
fn ctrl_comma_on_the_first_run_screen_opens_no_settings_page_or_window() {
    let folders = Folders::new();
    let mut harness = start(Fake::new(Latest::Status(503)), &folders);
    until(&mut harness, "the error text", |h| {
        shown(
            h,
            "Could not get the game data: the server answered with status 503",
        )
    });

    harness.key_press_modifiers(Modifiers::CTRL, Key::Comma);
    settle(&mut harness);
    assert!(
        !shown(&harness, "Facilities"),
        "the Settings page stays away"
    );
    assert!(!shown(&harness, "Price history"));
    assert_eq!(
        harness
            .query_all_by_role_and_label(egui::accesskit::Role::Window, "Settings")
            .count(),
        0,
        "no Settings window opens"
    );
}

#[test]
fn the_settings_page_shows_the_interface_sizes_and_the_density_names() {
    let folders = Folders::new();
    folders.install_game_data();
    folders.set_last_check(990_000);
    let mut harness = start(Fake::new(Latest::Body(latest_body(BUILD))), &folders);
    until(&mut harness, "the build pages", build_page_shown);
    harness.key_press_modifiers(Modifiers::CTRL, Key::Comma);
    until(&mut harness, "the settings page", |h| {
        shown(h, "Interface size")
    });

    for label in [
        "100%", "125%", "150%", "175%", "200%", "Roomy", "Normal", "Dense",
    ] {
        assert!(shown(&harness, label), "{label} is listed");
    }
    assert_eq!(
        harness.query_all_by_label("Sparse").count(),
        0,
        "Sparse is named Roomy"
    );
    assert!(
        top_of(&harness, "Interface size") < top_of(&harness, "Density"),
        "the interface size is above the density"
    );
    assert!(
        top_of(&harness, "Roomy") < top_of(&harness, "Normal")
            && top_of(&harness, "Normal") < top_of(&harness, "Dense"),
        "the density choices read Roomy, Normal, Dense"
    );
}

#[test]
fn a_settings_interface_size_click_is_saved_and_sets_the_zoom() {
    let folders = Folders::new();
    folders.install_game_data();
    folders.set_last_check(990_000);
    let mut harness = start(Fake::new(Latest::Body(latest_body(BUILD))), &folders);
    until(&mut harness, "the build pages", build_page_shown);
    harness.key_press_modifiers(Modifiers::CTRL, Key::Comma);
    until(&mut harness, "the settings page", |h| {
        shown(h, "Interface size")
    });

    harness.get_by_label("175%").click();
    settle(&mut harness);
    assert!(
        (zoom(&harness) - 1.75).abs() < 0.001,
        "the click zooms to 175%"
    );
    assert_eq!(saved_size(&folders), InterfaceSize::P175);
    assert!(selected(&harness, "175%"), "175% is the selected one");
}

#[test]
fn the_settings_page_saves_150_percent_and_copies_the_data_folder_path() {
    let folders = Folders::new();
    folders.install_game_data();
    folders.set_last_check(990_000);
    let mut harness = start(Fake::new(Latest::Body(latest_body(BUILD))), &folders);
    until(&mut harness, "the build pages", build_page_shown);
    harness.key_press_modifiers(Modifiers::CTRL, Key::Comma);
    until(&mut harness, "the settings page", |h| {
        shown(h, "Interface size")
    });

    harness.get_by_label("150%").click();
    settle(&mut harness);
    assert!(
        (zoom(&harness) - 1.5).abs() < 0.001,
        "the click zooms to 150%"
    );
    assert_eq!(saved_size(&folders), InterfaceSize::P150);

    // The Display panel is scrollable and Copy path sits at its bottom.
    let at = harness.get_by_label("Interface size").rect().center();
    scroll_display(&mut harness, at, -400.0);
    harness.get_by_label("Copy path").click();
    harness.run_steps(1);
    assert_eq!(
        copied_text(&harness),
        Some(folders.data.display().to_string()),
        "Copy path copies the data folder"
    );
}

#[test]
fn a_saved_interface_size_is_the_selected_settings_choice() {
    let folders = Folders::new();
    folders.install_game_data();
    folders.set_last_check(990_000);
    folders.set_interface_size(InterfaceSize::P125);
    let mut harness = start(Fake::new(Latest::Body(latest_body(BUILD))), &folders);
    until(&mut harness, "the build pages", build_page_shown);
    harness.key_press_modifiers(Modifiers::CTRL, Key::Comma);
    until(&mut harness, "the settings page", |h| {
        shown(h, "Interface size")
    });

    assert!(selected(&harness, "125%"), "125% is the selected one");
    assert!(!selected(&harness, "100%"));
}

#[test]
fn the_settings_page_shows_the_rail_choices_below_the_density() {
    let folders = Folders::new();
    folders.install_game_data();
    folders.set_last_check(990_000);
    let mut harness = start(Fake::new(Latest::Body(latest_body(BUILD))), &folders);
    until(&mut harness, "the build pages", build_page_shown);
    harness.key_press_modifiers(Modifiers::CTRL, Key::Comma);
    until(&mut harness, "the settings page", |h| {
        shown(h, "Interface size")
    });

    assert!(shown(&harness, "Rail"), "Rail is listed");
    assert!(
        shown(&harness, "Icons and labels"),
        "the labelled choice is listed"
    );
    assert!(
        shown(&harness, "Icons only"),
        "the icons-only choice is listed"
    );
    assert!(
        selected(&harness, "Icons and labels"),
        "a new data folder shows icons and labels"
    );
    assert!(!selected(&harness, "Icons only"));
    assert!(
        top_of(&harness, "Density") < top_of(&harness, "Rail"),
        "the rail choice is below the density"
    );
}

#[test]
fn a_rail_choice_is_saved_at_once_and_resizes_the_build_entry() {
    let folders = Folders::new();
    folders.install_game_data();
    folders.set_last_check(990_000);
    let mut harness = start(Fake::new(Latest::Body(latest_body(BUILD))), &folders);
    until(&mut harness, "the build pages", build_page_shown);
    harness.key_press_modifiers(Modifiers::CTRL, Key::Comma);
    until(&mut harness, "the settings page", |h| {
        shown(h, "Interface size")
    });
    assert!(harness.get_by_label("Build").rect().width() > 48.0);

    let at = harness.get_by_label("Interface size").rect().center();
    scroll_display(&mut harness, at, -250.0);
    harness.get_by_label("Icons only").click();
    settle(&mut harness);
    assert!(
        saved_rail_icons_only(&folders),
        "the choice is saved at once"
    );
    assert!(harness.state_mut().shared_mut().rail_icons_only);
    assert!(
        harness.get_by_label("Build").rect().width() <= 48.0,
        "the icons-only Build entry is at most 48 px wide"
    );

    let at = harness.get_by_label("Copy path").rect().center();
    scroll_display(&mut harness, at, 250.0);
    harness.get_by_label("Icons and labels").click_accesskit();
    settle(&mut harness);
    assert!(
        !saved_rail_icons_only(&folders),
        "the other choice is saved at once"
    );
    assert!(!harness.state_mut().shared_mut().rail_icons_only);
    assert!(
        harness.get_by_label("Build").rect().width() > 48.0,
        "the labelled Build entry is wider again"
    );
}

#[test]
fn the_keep_refreshing_box_saves_a_1_and_a_0_and_a_new_app_reads_them() {
    let folders = Folders::new();
    folders.install_game_data();
    folders.set_last_check(990_000);
    let mut harness = start(Fake::new(Latest::Body(latest_body(BUILD))), &folders);
    until(&mut harness, "the build pages", build_page_shown);
    harness.key_press_modifiers(Modifiers::CTRL, Key::Comma);
    until(&mut harness, "the settings page", |h| {
        shown(h, "Interface size")
    });

    assert!(shown(&harness, "Refresh"), "the refresh section is listed");
    assert!(
        shown(&harness, "Keep refreshing while open"),
        "the keep-refreshing choice is listed"
    );
    assert!(
        !ticked(&harness, "Keep refreshing while open"),
        "a fresh start has the choice off"
    );
    assert_eq!(saved_refresh_while_open(&folders).as_deref(), None);

    click_checkbox(&mut harness, "Keep refreshing while open");
    settle(&mut harness);
    assert!(ticked(&harness, "Keep refreshing while open"));
    assert!(harness.state_mut().shared_mut().refresh_while_open);
    assert_eq!(saved_refresh_while_open(&folders).as_deref(), Some("1"));

    // A new app on the same files reads the saved 1 as on.
    drop(harness);
    let mut reopened = start(Fake::new(Latest::Status(503)), &folders);
    until(&mut reopened, "the build pages", build_page_shown);
    assert!(
        reopened.state_mut().shared_mut().refresh_while_open,
        "the saved 1 reads as on"
    );
    drop(reopened);

    // The box unticks to a saved 0.
    let mut harness = start(Fake::new(Latest::Status(503)), &folders);
    until(&mut harness, "the build pages", build_page_shown);
    harness.key_press_modifiers(Modifiers::CTRL, Key::Comma);
    until(&mut harness, "the settings page", |h| {
        shown(h, "Interface size")
    });
    assert!(ticked(&harness, "Keep refreshing while open"));
    click_checkbox(&mut harness, "Keep refreshing while open");
    settle(&mut harness);
    assert!(!ticked(&harness, "Keep refreshing while open"));
    assert!(!harness.state_mut().shared_mut().refresh_while_open);
    assert_eq!(saved_refresh_while_open(&folders).as_deref(), Some("0"));
    drop(harness);

    // A fresh data folder with no key at all reads as off.
    let fresh = Folders::new();
    fresh.install_game_data();
    fresh.set_last_check(990_000);
    let mut harness = start(Fake::new(Latest::Status(503)), &fresh);
    assert!(!harness.state_mut().shared_mut().refresh_while_open);
}

use super::*;

#[test]
fn ctrl_plus_minus_and_zero_change_and_save_the_interface_size() {
    let folders = Folders::new();
    folders.install_game_data();
    folders.set_last_check(990_000);
    let mut harness = start(Fake::new(Latest::Body(latest_body(BUILD))), &folders);
    until(&mut harness, "the build pages", build_page_shown);
    assert!(
        (zoom(&harness) - 1.0).abs() < 0.001,
        "a new folder starts at 100%"
    );

    harness.key_press_modifiers(Modifiers::CTRL, Key::Equals);
    settle(&mut harness);
    assert!(
        (zoom(&harness) - 1.25).abs() < 0.001,
        "Ctrl + goes one size up"
    );
    assert_eq!(saved_size(&folders), InterfaceSize::P125);

    harness.key_press_modifiers(Modifiers::CTRL, Key::Plus);
    settle(&mut harness);
    assert!((zoom(&harness) - 1.5).abs() < 0.001, "Ctrl + goes on up");
    assert_eq!(saved_size(&folders), InterfaceSize::P150);

    harness.key_press_modifiers(Modifiers::CTRL, Key::Minus);
    settle(&mut harness);
    assert!(
        (zoom(&harness) - 1.25).abs() < 0.001,
        "Ctrl − goes one size down"
    );
    assert_eq!(saved_size(&folders), InterfaceSize::P125);

    harness.key_press_modifiers(Modifiers::CTRL, Key::Num0);
    settle(&mut harness);
    assert!(
        (zoom(&harness) - 1.0).abs() < 0.001,
        "Ctrl 0 goes back to 100%"
    );
    assert_eq!(saved_size(&folders), InterfaceSize::P100);
}

#[test]
fn the_interface_size_stops_at_each_end_without_a_notice() {
    for (size, key) in [
        (InterfaceSize::P200, Key::Plus),
        (InterfaceSize::P100, Key::Minus),
    ] {
        let folders = Folders::new();
        folders.install_game_data();
        folders.set_last_check(990_000);
        folders.set_interface_size(size);
        let mut harness = start(Fake::new(Latest::Body(latest_body(BUILD))), &folders);
        until(&mut harness, "the build pages", build_page_shown);
        assert!((zoom(&harness) - (f32::from(size.percent()) / 100.0)).abs() < 0.001);

        let notices = harness.state_mut().shared_mut().notices.len();
        harness.key_press_modifiers(Modifiers::CTRL, key);
        settle(&mut harness);
        assert!((zoom(&harness) - (f32::from(size.percent()) / 100.0)).abs() < 0.001);
        assert_eq!(harness.state_mut().shared_mut().notices.len(), notices);
        assert_eq!(saved_size(&folders), size);
    }
}

#[test]
fn a_saved_interface_size_sets_the_zoom_at_start() {
    let folders = Folders::new();
    folders.install_game_data();
    folders.set_last_check(990_000);
    folders.set_interface_size(InterfaceSize::P150);
    let mut harness = start(Fake::new(Latest::Body(latest_body(BUILD))), &folders);
    until(&mut harness, "the build pages", build_page_shown);
    assert!(
        (zoom(&harness) - 1.5).abs() < 0.001,
        "the app starts at 150%"
    );
    assert!(!harness.ctx.options(|options| options.zoom_with_keyboard));
}

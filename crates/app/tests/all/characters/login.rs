use super::*;

#[test]
fn add_character_opens_the_browser_and_adds_the_character_that_logs_in() {
    let env = env();
    let eve = FakeEve::new(&env, partial);
    let mut harness = open(&env, &eve);
    harness.get_by_label("Add character").click();
    frames(&mut harness);
    until(&mut harness, "the waiting text", |h| {
        shown(h, "Waiting for the EVE login in your browser…")
    });
    assert_eq!(
        eve.log(|log| log.opened.clone()),
        vec!["https://login.example/start".to_owned()]
    );
    assert!(disabled(&harness, "Add character"));
    harness.get_by_label("Add character").hover();
    frames(&mut harness);
    assert!(shown(&harness, "Busy: waiting for the EVE login"));
    eve.release.store(true, Ordering::Relaxed);
    until(&mut harness, "Gamma's row", |h| shown(h, "Gamma"));
    until(&mut harness, "Gamma's refresh", |h| {
        !disabled(h, "Refresh all")
    });
    assert_eq!(eve.log(|log| log.refreshed.clone()), vec![GAMMA]);
    assert!(!shown(
        &harness,
        "Waiting for the EVE login in your browser…"
    ));
}

#[test]
fn cancel_while_waiting_is_a_notice_and_not_an_error() {
    let env = env();
    let eve = FakeEve::new(&env, partial);
    let mut harness = open(&env, &eve);
    harness.get_by_label("Add character").click();
    frames(&mut harness);
    until(&mut harness, "the waiting text", |h| shown(h, "Cancel"));
    harness.get_by_label("Cancel").click();
    until_notice(&mut harness);
    let held = &harness.state_mut().shared_mut().notices;
    assert_eq!(held.len(), 1);
    assert_eq!(
        held.first().map(|n| n.text.as_str()),
        Some("Login cancelled.")
    );
    assert_eq!(held.first().map(|n| n.level), Some(Level::Notice));
    assert!(eve.log(|log| log.refreshed.is_empty()));
}

#[test]
fn a_login_that_cannot_start_is_an_error() {
    let env = env();
    let mut eve = FakeEve::new(&env, partial);
    eve.begin = Err("This build has no EVE login yet.".to_owned());
    let mut harness = open(&env, &eve);
    harness.get_by_label("Add character").click();
    until_notice(&mut harness);
    let held = &harness.state_mut().shared_mut().notices;
    assert_eq!(
        held.first().map(|n| n.text.as_str()),
        Some("Could not log in: This build has no EVE login yet.")
    );
    assert_eq!(held.first().map(|n| n.level), Some(Level::Error));
}

#[test]
fn without_a_client_id_both_login_buttons_are_disabled() {
    let env = env();
    let eve = FakeEve::new(&env, partial);
    let mut harness = open(&env, &eve);
    set_client_id_present(&harness.ctx, false);
    frames(&mut harness);
    frames(&mut harness);
    assert!(disabled(&harness, "Add character"));
    assert!(disabled(&harness, "Log in again"));
    assert!(!disabled(&harness, "Refresh all"));
    harness.get_by_label("Add character").hover();
    frames(&mut harness);
    assert!(shown(&harness, "This build has no EVE login yet."));
}

/// Presses the first `Remove…` button through its accesskit action: the row's action buttons
/// sit past the right edge of the window once the Short name column is drawn, and the action
/// clicks a button that is scrolled out of view.
fn press_remove(harness: &mut Harness<'_, App>) {
    harness
        .get_all_by_label("Remove…")
        .next()
        .expect("a Remove… button")
        .click_accesskit();
}

#[test]
fn remove_asks_first_and_forgets_only_that_character() {
    let env = env();
    let eve = FakeEve::new(&env, partial);
    let mut harness = open(&env, &eve);
    press_remove(&mut harness);
    frames(&mut harness);
    let ask = "Remove Alt? Its saved data is deleted. Net worth history is kept.";
    assert!(shown(&harness, ask));
    harness.get_by_label("Keep").click();
    frames(&mut harness);
    assert!(!shown(&harness, ask));
    assert!(eve.log(|log| log.removed.is_empty()));
    assert!(shown(&harness, "Alt"));

    press_remove(&mut harness);
    frames(&mut harness);
    harness.get_by_label("Remove").click();
    until(&mut harness, "Alt's row to go", |h| !shown(h, "Alt"));
    assert_eq!(eve.log(|log| log.removed.clone()), vec![ALT]);
    let session = &harness.state_mut().shared_mut().session;
    assert!(session.character_data(MAIN).is_some());
    assert_eq!(session.characters().len(), 1);
}

#[test]
fn the_keychain_warning_is_a_notice_once_and_stays_above_the_table() {
    let env = env();
    let mut eve = FakeEve::new(&env, partial);
    eve.warning = Some(KEYCHAIN.to_owned());
    let mut harness = open(&env, &eve);
    until_notice(&mut harness);
    assert_eq!(notices(&mut harness), vec![KEYCHAIN.to_owned()]);
    // The notice can arrive one frame before the table draws its warning line.
    until(&mut harness, "the warning above the table", |h| {
        let Some(main) = h.get_all_by_label("Main").next() else {
            return false;
        };
        let main_y = main.rect().center().y;
        h.query_all_by_label(KEYCHAIN)
            .any(|node| node.rect().center().y < main_y)
    });
    let main_y = row_y(&harness, "Main");
    assert!(
        harness
            .query_all_by_label(KEYCHAIN)
            .any(|node| node.rect().center().y < main_y)
    );
    harness.state_mut().shared_mut().notices.clear();
    rail(&mut harness, "Build");
    rail(&mut harness, "Characters");
    for _ in 0..5 {
        frames(&mut harness);
    }
    assert!(notices(&mut harness).is_empty());
    assert!(shown(&harness, KEYCHAIN));
}

#[test]
fn no_characters_says_how_to_add_one() {
    let env = env();
    let mut eve = FakeEve::new(&env, partial);
    eve.characters.clear();
    let harness = open(&env, &eve);
    assert!(shown(
        &harness,
        "No characters yet. Add character opens the EVE login in your browser."
    ));
}

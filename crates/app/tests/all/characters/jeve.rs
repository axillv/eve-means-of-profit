use super::*;

/// Alt (updated 700,000) and Hauler (never updated), as the jEve profile of the pages tests.
fn jeve_profile() -> TempDir {
    let folder = TempDir::new().expect("a temp folder");
    fill_jeve(folder.path());
    folder
}

/// A second profile, `Other.db`, holding only Zed (updated 900,000).
fn fill_other(folder: &Path) {
    let connection = Connection::open(folder.join("Other.db")).expect("database");
    connection
        .execute_batch(
            "CREATE TABLE owners (accountid TEXT, ownerid INTEGER, name TEXT, corp TEXT, show NUMERIC, \
             invalid NUMERIC, assetslastupdate INTEGER, assetsnextupdate INTEGER, scopes TEXT, \
             UNIQUE(accountid));
             CREATE TABLE assets (accountid TEXT, count INTEGER, flagid INTEGER, flagstring TEXT, \
             itemid INTEGER, typeid INTEGER, locationid INTEGER, singleton NUMERIC, rawquantity INTEGER, \
             UNIQUE(accountid, itemid));
             CREATE TABLE blueprints (accountid TEXT, itemid INTEGER, locationid INTEGER, typeid INTEGER, \
             flagid INTEGER, flagstring TEXT, quantity INTEGER, timeefficiency INTEGER, \
             materialefficiency INTEGER, runs NUMERIC, UNIQUE(accountid, itemid));
             INSERT INTO owners (accountid, ownerid, name, show, invalid, assetslastupdate, scopes) VALUES
              ('j9', 2112000009, 'Zed', 1, 0, 900000000, 'esi-assets.read_assets.v1');",
        )
        .expect("fixture builds");
    drop(connection);
}

/// Saves the profile choice before the app starts.
fn choose_profile(env: &Env, name: &str) {
    let mut store = Store::open(&env.data.join("user.sqlite"), NOW)
        .expect("the store opens")
        .store;
    store
        .set_setting("jeve.profile", name)
        .expect("the choice is saved");
}

/// Runs frames until the app holds a notice whose text is `text`.
fn until_notice_text(harness: &mut Harness<'_, App>, text: &str) {
    for _ in 0..WAIT_STEPS {
        harness.step();
        if notices(harness).iter().any(|held| held == text) {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("the notice {text:?} did not come");
}

#[test]
fn the_jeve_panel_lists_its_characters_and_whether_the_app_uses_them() {
    let env = env();
    let jeve = jeve_profile();
    // Nothing is due, so the start-up refresh leaves Alt's jEve data in use until the test
    // replaces it below.
    let mut eve = FakeEve::new(&env, all_not_yet);
    eve.jeve = Some(jeve.path().to_path_buf());
    let mut harness = open(&env, &eve);
    until(&mut harness, "the jEve rows", |h| shown(h, "Hauler"));
    let on_row = |h: &Harness<'_, App>, anchor: &str, text: &str| {
        let y = h
            .get_all_by_label(anchor)
            .last()
            .expect("an anchor")
            .rect()
            .center()
            .y;
        h.query_all_by_label(text)
            .any(|node| (node.rect().center().y - y).abs() < 2.0)
    };
    assert!(on_row(&harness, "Alt", "3d"));
    assert!(on_row(&harness, "Alt", "yes"));
    assert!(on_row(&harness, "Hauler", "never"));
    assert!(on_row(&harness, "Hauler", "yes"));

    let alt_assets = CharacterData {
        character_id: ALT,
        assets: Some(held(owned(ALT, Vec::new()), NOW, None)),
        orders: None,
        jobs: None,
        wallet: None,
    };
    harness.state_mut().shared_mut().session.replace(alt_assets);
    frames(&mut harness);
    assert!(on_row(&harness, "Alt", "3d"));
    assert!(on_row(&harness, "Alt", "no: logged in to the app"));

    fs::remove_file(jeve.path().join("#Default.db")).expect("the file goes");
    harness.get_by_label("Read again").click();
    let message = format!("No jEve data file found at {}.", jeve.path().display());
    until(&mut harness, "the message", |h| shown(h, &message));
    assert!(!shown(&harness, "Hauler"));
    // Only Alt's login row is left; the jEve row and its age are gone.
    assert_eq!(count(&harness, "Alt"), 1);
    assert!(!shown(&harness, "3d"));
    assert!(harness.state_mut().shared_mut().session.jeve().is_none());
}

#[test]
fn a_jeve_read_during_a_login_wait_does_not_wait_for_the_login() {
    let env = env();
    let jeve = jeve_profile();
    let mut eve = FakeEve::new(&env, partial);
    eve.jeve = Some(jeve.path().to_path_buf());
    let mut harness = open(&env, &eve);
    until(&mut harness, "the jEve rows", |h| shown(h, "Hauler"));
    harness.get_by_label("Add character").click();
    until(&mut harness, "the waiting text", |h| {
        shown(h, "Waiting for the EVE login in your browser…")
    });
    let slot = harness.state_mut().shared_mut().session.eve();
    fs::remove_file(jeve.path().join("#Default.db")).expect("the file goes");
    harness.get_by_label("Read again").click();
    let message = format!("No jEve data file found at {}.", jeve.path().display());
    until(&mut harness, "the message", |h| shown(h, &message));
    assert!(shown(
        &harness,
        "Waiting for the EVE login in your browser…"
    ));
    // The wait holds no lock on the login.
    drop(slot.try_lock().expect("the login is free"));
    eve.release.store(true, Ordering::Relaxed);
    until(&mut harness, "Gamma's row", |h| shown(h, "Gamma"));
    // The real login is back in its slot afterwards.
    assert_eq!(lock_eve(&slot).characters().len(), 2);
}

#[test]
fn the_saved_profile_is_read_and_a_missing_one_falls_back_to_the_default() {
    let env = env();
    let jeve = jeve_profile();
    fill_other(jeve.path());
    choose_profile(&env, "Other");
    let mut eve = FakeEve::new(&env, partial);
    eve.jeve = Some(jeve.path().to_path_buf());
    let mut harness = open(&env, &eve);
    until(&mut harness, "Zed", |h| shown(h, "Zed"));
    assert!(!shown(&harness, "Hauler"));

    let env = self::env();
    choose_profile(&env, "Gone");
    let mut eve = FakeEve::new(&env, partial);
    eve.jeve = Some(jeve.path().to_path_buf());
    let mut harness = open(&env, &eve);
    until(&mut harness, "Hauler", |h| shown(h, "Hauler"));
    frames(&mut harness);
    assert!(!shown(&harness, "Zed"));
}

#[test]
fn picking_a_profile_reads_it_and_keeps_the_choice() {
    let env = env();
    let jeve = jeve_profile();
    fill_other(jeve.path());
    let mut eve = FakeEve::new(&env, partial);
    eve.jeve = Some(jeve.path().to_path_buf());
    let mut harness = open(&env, &eve);
    until(&mut harness, "Hauler", |h| shown(h, "Hauler"));
    // The Characters panel's account dropdowns come first in the tree; the jEve panel's
    // profile dropdown is the last one.
    harness
        .query_all(by().role(Role::ComboBox))
        .last()
        .expect("the profile dropdown")
        .click();
    frames(&mut harness);
    harness.get_by_label("Other").click();
    until(&mut harness, "Zed", |h| shown(h, "Zed"));
    let kept = harness
        .state_mut()
        .shared_mut()
        .store
        .setting("jeve.profile")
        .expect("the setting reads");
    assert_eq!(kept.as_deref(), Some("Other"));
}

#[test]
fn a_cancel_the_player_did_not_cause_is_an_error() {
    let env = env();
    let mut eve = FakeEve::new(&env, partial);
    eve.cancels_alone = true;
    let mut harness = open(&env, &eve);
    harness.state_mut().shared_mut().notices.clear();
    harness.get_by_label("Add character").click();
    let expected = format!("Could not log in: {}", EsiError::Cancelled);
    until_notice_text(&mut harness, &expected);
    let held = &harness.state_mut().shared_mut().notices;
    let cancel = held
        .iter()
        .find(|notice| notice.text == expected)
        .expect("the login error");
    assert_eq!(cancel.level, Level::Error);
    assert!(
        !held.iter().any(|notice| notice.text == "Login cancelled."),
        "the player did not cancel"
    );
}

#[test]
fn a_browser_that_will_not_open_is_an_error_and_the_login_goes_on() {
    let env = env();
    let eve = FakeEve::new(&env, partial);
    let mut harness = open(&env, &eve);
    set_opener(&harness.ctx, Arc::new(|_| Err("no browser".to_owned())));
    harness.get_by_label("Add character").click();
    until_notice(&mut harness);
    let held = &harness.state_mut().shared_mut().notices;
    assert_eq!(
        held.first().map(|n| n.text.as_str()),
        Some(
            "Could not open the browser (no browser). Open this address yourself: https://login.example/start"
        )
    );
    assert_eq!(held.first().map(|n| n.level), Some(Level::Error));
    frames(&mut harness);
    assert!(shown(
        &harness,
        "Waiting for the EVE login in your browser…"
    ));
    harness.get_by_label("Cancel").click();
    until(&mut harness, "the end of the wait", |h| {
        !shown(h, "Waiting for the EVE login in your browser…")
    });
}

#[test]
fn the_buttons_say_what_is_running_while_it_runs() {
    let env = env();
    let eve = FakeEve::new(&env, partial);
    let mut harness = open(&env, &eve);
    eve.hold.store(true, Ordering::Relaxed);
    harness
        .get_all_by_label("Refresh")
        .nth(1)
        .expect("Main's button")
        .click();
    frames(&mut harness);
    assert!(disabled(&harness, "Refresh all"));
    refresh_buttons(&harness)
        .into_iter()
        .next()
        .expect("the page's button")
        .hover();
    frames(&mut harness);
    assert!(shown(&harness, "Busy: refreshing Main"));
    eve.hold.store(false, Ordering::Relaxed);
    until(&mut harness, "the refresh", |h| !disabled(h, "Refresh all"));
}

#[test]
fn this_build_has_no_eve_login_and_the_page_knows_it_without_a_hook() {
    let env = env();
    let eve = FakeEve::new(&env, partial);
    let mut harness = open_with(&env, &eve, false);
    frames(&mut harness);
    assert!(disabled(&harness, "Add character"));
    assert!(disabled(&harness, "Log in again"));
    harness.get_by_label("Add character").hover();
    frames(&mut harness);
    assert!(shown(&harness, "This build has no EVE login yet."));
    assert_eq!(eve.log(|log| log.begun), 0);
}

/// Alt, Hauler and `corporations` corporations (owners whose scopes hold the corporation
/// roles).
fn jeve_with_corporations(corporations: usize) -> TempDir {
    let folder = jeve_profile();
    let connection = Connection::open(folder.path().join("#Default.db")).expect("database");
    for number in 0..corporations {
        connection
            .execute(
                "INSERT INTO owners (accountid, ownerid, name, show, invalid, scopes) \
                 VALUES (?1, ?2, 'Corp', 1, 0, 'esi-characters.read_corporation_roles.v1')",
                (
                    format!("c{number}"),
                    98_000_000 + i64::try_from(number).expect("small"),
                ),
            )
            .expect("a corporation");
    }
    drop(connection);
    folder
}

#[test]
fn corporations_left_out_are_counted_in_the_singular_and_the_plural() {
    for (corporations, line) in [
        (1, "1 corporation left out"),
        (2, "2 corporations left out"),
    ] {
        let env = env();
        let jeve = jeve_with_corporations(corporations);
        let mut eve = FakeEve::new(&env, partial);
        eve.jeve = Some(jeve.path().to_path_buf());
        let mut harness = open(&env, &eve);
        until(&mut harness, "the line", |h| shown(h, line));
    }
    let env = env();
    let jeve = jeve_profile();
    let mut eve = FakeEve::new(&env, partial);
    eve.jeve = Some(jeve.path().to_path_buf());
    let mut harness = open(&env, &eve);
    until(&mut harness, "Hauler", |h| shown(h, "Hauler"));
    assert!(
        harness
            .query_all_by_label_contains("left out")
            .next()
            .is_none()
    );
}

#[test]
fn without_a_jeve_folder_the_panel_says_there_is_no_file() {
    let env = env();
    let eve = FakeEve::new(&env, partial);
    let mut harness = open(&env, &eve);
    until(&mut harness, "the message", |h| {
        shown(h, "No jEve data file found.")
    });
    assert!(shown(&harness, "Read again"));
}

/// Starts a login that waits, then makes the page forget its state, so that its first
/// listing of jEve's folder and its first ask for the keychain warning happen during the wait.
fn first_show_during_a_login(eve: &FakeEve, env: &Env) -> Harness<'static, App> {
    let mut harness = open(env, eve);
    if eve.warning.is_some() {
        // The start-up ask may still be on its way; let it land before the state is forgotten.
        until(&mut harness, "the keychain notice", |h| shown(h, KEYCHAIN));
    }
    harness.get_by_label("Add character").click();
    until(&mut harness, "the waiting text", |h| {
        shown(h, "Waiting for the EVE login in your browser…")
    });
    forget_state(&harness.ctx);
    harness
}

#[test]
fn the_jeve_panel_listed_during_a_login_wait_finds_the_real_folder() {
    let env = env();
    let jeve = jeve_profile();
    let mut eve = FakeEve::new(&env, partial);
    eve.jeve = Some(jeve.path().to_path_buf());
    let mut harness = first_show_during_a_login(&eve, &env);
    let file = jeve.path().join("#Default.db");
    let line = format!("File: {}", file.display());
    until(&mut harness, "the file line", |h| shown(h, &line));
    assert!(!shown(&harness, "No jEve data file found."));
    eve.release.store(true, Ordering::Relaxed);
}

#[test]
fn the_keychain_warning_asked_during_a_login_wait_is_shown() {
    let env = env();
    let mut eve = FakeEve::new(&env, partial);
    eve.warning = Some(KEYCHAIN.to_owned());
    let mut harness = first_show_during_a_login(&eve, &env);
    // The first show gave one notice; the second, during the wait, gives another.
    for _ in 0..WAIT_STEPS {
        frames(&mut harness);
        if notices(&mut harness).len() >= 2 {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        notices(&mut harness),
        vec![KEYCHAIN.to_owned(), KEYCHAIN.to_owned()],
        "keychain asks: {}",
        eve.log(|log| log.keychain)
    );
    frames(&mut harness);
    assert!(shown(&harness, KEYCHAIN));
    eve.release.store(true, Ordering::Relaxed);
}

#[test]
fn a_login_that_panics_ends_the_wait_with_an_error() {
    let env = env();
    let mut eve = FakeEve::new(&env, partial);
    eve.panics = true;
    let mut harness = open(&env, &eve);
    harness.get_by_label("Add character").click();
    until_notice(&mut harness);
    frames(&mut harness);
    let held = &harness.state_mut().shared_mut().notices;
    assert_eq!(
        held.first().map(|n| n.text.as_str()),
        Some("Could not log in: the login stopped unexpectedly.")
    );
    assert_eq!(held.first().map(|n| n.level), Some(Level::Error));
    assert!(!shown(
        &harness,
        "Waiting for the EVE login in your browser…"
    ));
    assert!(!shown(&harness, "Cancel"));
    assert!(!disabled(&harness, "Add character"));
    assert!(!disabled(&harness, "Refresh all"));
}

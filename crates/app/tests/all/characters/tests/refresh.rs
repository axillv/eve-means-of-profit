use super::*;

fn wallet_fails(data: &mut CharacterData) -> Results {
    let mut results = partial(data);
    if let Some(last) = results.last_mut() {
        *last = (Kind::Wallet, Err(EsiError::Network("timed out".to_owned())));
    }
    results
}

/// EVE limits the assets only; the orders are fetched and the rest is not yet due.
fn limited(data: &mut CharacterData) -> Results {
    data.orders = Some(held(Vec::new(), NOW, None));
    vec![
        (
            Kind::Assets,
            Err(EsiError::ErrorLimited { reset_seconds: 42 }),
        ),
        (Kind::Orders, Ok(Refreshed::Fetched)),
        (Kind::Jobs, Ok(Refreshed::NotYet { until: NOW + 90 })),
        (Kind::Wallet, Ok(Refreshed::NotYet { until: NOW + 90 })),
    ]
}

/// Like [`open`], but saves no character file: the session holds only what the fake's login
/// lists.
fn open_without_saved(env: &Env, eve: &FakeEve) -> Harness<'static, App> {
    let mut harness = open_app(env, eve, true);
    drop(settle_start(&mut harness));
    clear_log(&mut harness, eve);
    harness
}

/// Runs frames until `wanted` price answers have arrived, and returns their marks.
fn settled_price_requests(harness: &mut Harness<'_, App>, wanted: usize) -> Vec<bool> {
    let mut all = Vec::new();
    for _ in 0..WAIT_STEPS {
        harness.step();
        all.extend(price_requests(harness));
        if all.len() >= wanted {
            return all;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("only {} price answers arrived, wanted {wanted}", all.len());
}

#[test]
fn the_session_load_refreshes_every_character_and_the_prices_once() {
    let env = env();
    let eve = FakeEve::new(&env, partial);
    let mut harness = open_raw(&env, &eve);
    assert_eq!(settle_start(&mut harness), vec![false]);
    assert_eq!(eve.log(|log| log.refreshed.clone()), vec![ALT, MAIN]);
    for _ in 0..20 {
        frames(&mut harness);
    }
    assert_eq!(eve.log(|log| log.refreshed.clone()), vec![ALT, MAIN]);
    assert!(price_requests(&mut harness).is_empty());
}

#[test]
fn the_status_line_and_the_page_each_refresh_all() {
    let env = env();
    let eve = FakeEve::new(&env, partial);
    let mut harness = open(&env, &eve);
    assert_eq!(refresh_buttons(&harness).len(), 2);

    // The status line's button refreshes the characters and the prices, explicitly.
    press(&mut harness, "Refresh all", 1);
    assert_eq!(eve.log(|log| log.refreshed.clone()), vec![ALT, MAIN]);
    assert_eq!(settled_price_requests(&mut harness, 1), vec![true]);

    // The page's own button stays character-only.
    press(&mut harness, "Refresh all", 0);
    assert_eq!(
        eve.log(|log| log.refreshed.clone()),
        vec![ALT, MAIN, ALT, MAIN]
    );
    assert!(price_requests(&mut harness).is_empty());
}

#[test]
fn refresh_all_without_characters_still_asks_for_prices() {
    let env = env();
    let mut eve = FakeEve::new(&env, partial);
    eve.characters.clear();
    // No character file is saved either, so nothing outside the fake's login can add one.
    let mut harness = open_without_saved(&env, &eve);
    // The login itself is empty, not just the fake's whitelist.
    assert!(
        harness
            .state_mut()
            .shared_mut()
            .session
            .characters()
            .is_empty()
    );
    assert!(eve.log(|log| log.refreshed.is_empty()));
    drop(price_requests(&mut harness));

    press(&mut harness, "Refresh all", 1);
    assert!(eve.log(|log| log.refreshed.is_empty()));
    assert_eq!(settled_price_requests(&mut harness, 1), vec![true]);
}

#[test]
fn a_press_while_a_character_refresh_runs_starts_no_second_job() {
    let env = env();
    let eve = FakeEve::new(&env, partial);
    let mut harness = open(&env, &eve);
    eve.hold.store(true, Ordering::Relaxed);

    refresh_buttons(&harness)
        .into_iter()
        .nth(1)
        .expect("the status line's button")
        .click();
    until(&mut harness, "the first character", |_| {
        eve.log(|log| log.refreshed.len()) == 1
    });

    refresh_buttons(&harness)
        .into_iter()
        .nth(1)
        .expect("the status line's button")
        .click();
    for _ in 0..10 {
        harness.step();
    }
    assert_eq!(eve.log(|log| log.refreshed.len()), 1);

    eve.hold.store(false, Ordering::Relaxed);
    until(&mut harness, "the refresh", |h| !disabled(h, "Refresh all"));
    assert_eq!(eve.log(|log| log.refreshed.clone()), vec![ALT, MAIN]);
}

#[test]
fn refresh_fetches_one_character_names_its_structures_and_saves() {
    let env = env();
    let eve = FakeEve::new(&env, partial);
    let mut harness = open(&env, &eve);
    let (main_before, alt_before) = {
        let session = &harness.state_mut().shared_mut().session;
        (session.revision(MAIN), session.revision(ALT))
    };
    press(&mut harness, "Refresh", 1);
    assert_eq!(eve.log(|log| log.refreshed.clone()), vec![MAIN]);
    assert_eq!(
        eve.log(|log| log.named.clone()),
        vec![(vec![HOME_FORT], vec![MAIN, ALT])]
    );
    let saved_main = saved::load_character(&esi_dir(&env), MAIN)
        .expect("the file reads")
        .expect("Main's file");
    assert!(saved_main.jobs.is_some());
    let session = &harness.state_mut().shared_mut().session;
    assert_eq!(session.revision(MAIN), main_before + 1);
    assert_eq!(session.revision(ALT), alt_before);
    let named = session
        .structures()
        .get(&HOME_FORT)
        .and_then(|entry| entry.name.clone());
    assert_eq!(named.as_deref(), Some("Home Fort"));
    assert!(notices(&mut harness).is_empty());
}

#[test]
fn refresh_all_with_nothing_due_replaces_nothing_and_says_when() {
    let env = env();
    let eve = FakeEve::new(&env, all_not_yet);
    let mut harness = open(&env, &eve);
    clear_notices(&mut harness);
    // The page's own button is the first one: the status line draws one below the page.
    press(&mut harness, "Refresh all", 0);
    assert_eq!(eve.log(|log| log.refreshed.clone()), vec![ALT, MAIN]);
    assert!(price_requests(&mut harness).is_empty());
    let session = &harness.state_mut().shared_mut().session;
    assert_eq!(session.revision(MAIN), 0);
    assert_eq!(session.revision(ALT), 0);
    assert_eq!(
        notices(&mut harness),
        vec!["Nothing to refresh yet: EVE allows it again in 1m.".to_owned()]
    );
}

#[test]
fn a_failed_kind_is_named_and_the_others_are_kept() {
    let env = env();
    let eve = FakeEve::new(&env, wallet_fails);
    let mut harness = open(&env, &eve);
    clear_notices(&mut harness);
    let before = harness.state_mut().shared_mut().session.revision(MAIN);
    press(&mut harness, "Refresh", 1);
    assert_eq!(
        notices(&mut harness),
        vec![format!(
            "Could not refresh Main wallet: {}",
            EsiError::Network("timed out".to_owned())
        )]
    );
    let session = &harness.state_mut().shared_mut().session;
    assert_eq!(session.revision(MAIN), before + 1);
    assert!(
        session
            .character_data(MAIN)
            .is_some_and(|data| data.jobs.is_some())
    );
}

#[test]
fn an_error_limit_gives_one_notice_for_the_character() {
    let env = env();
    let eve = FakeEve::new(&env, limited);
    let mut harness = open(&env, &eve);
    clear_notices(&mut harness);
    press(&mut harness, "Refresh", 1);
    assert_eq!(
        notices(&mut harness),
        vec!["Could not refresh Main: EVE asks to wait 42 s.".to_owned()]
    );
}

#[test]
fn an_expired_login_found_on_refresh_is_marked() {
    let env = env();
    let eve = FakeEve::new(&env, expired);
    let mut harness = open(&env, &eve);
    clear_notices(&mut harness);
    press(&mut harness, "Refresh", 1);
    assert_eq!(
        notices(&mut harness),
        vec!["Could not refresh Main: login expired, log in again.".to_owned()]
    );
    until(&mut harness, "Main's login", |h| {
        count(h, "Log in again") == 2
    });
}

#[test]
fn failing_to_name_structures_keeps_the_refresh() {
    let env = env();
    let mut eve = FakeEve::new(&env, partial);
    eve.failed_names = 1;
    let mut harness = open(&env, &eve);
    clear_notices(&mut harness);
    let before = harness.state_mut().shared_mut().session.revision(MAIN);
    press(&mut harness, "Refresh", 1);
    assert_eq!(
        notices(&mut harness),
        vec!["Could not get 1 structure names from EVE. The last names are kept.".to_owned()]
    );
    let session = &harness.state_mut().shared_mut().session;
    assert_eq!(session.revision(MAIN), before + 1);
}

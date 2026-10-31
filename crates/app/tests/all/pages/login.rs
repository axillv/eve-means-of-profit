use super::*;

#[test]
fn the_stock_takes_the_login_data_then_jeve_and_a_fetched_login_wins() {
    let env = env();
    saved::save_character(&esi_dir(&env), &main_data()).expect("Main's data saved");
    let jeve = jeve_profile();
    let eve = FakeEve::new(
        vec![character(MAIN, "Main"), character(ALT, "Alt")],
        Some(jeve.path().to_path_buf()),
    );
    let mut harness = start(&env, eve, NOW);
    loaded(&mut harness);

    let texts = notices(&mut harness);
    assert!(texts.is_empty(), "{texts:?}");
    let shared = harness.state_mut().shared_mut();
    let stock = shared.session.stock_characters();
    let ids: Vec<i64> = stock.iter().map(|entry| entry.character_id).collect();
    assert_eq!(ids, [2_112_000_001, 2_112_000_002, 2_112_000_003]);
    assert_eq!(stock[0].assets.len(), 2, "Main has two assets");
    assert_eq!(
        shared.session.source(MAIN),
        Some(eve_means_of_profit::session::Source::Login)
    );
    assert_eq!(
        shared.session.source(ALT),
        Some(eve_means_of_profit::session::Source::Jeve)
    );
    assert_eq!(
        shared.session.source(HAULER),
        Some(eve_means_of_profit::session::Source::Jeve)
    );

    shared
        .session
        .replace(assets_data(ALT, NOW, vec![asset(3, JITA_4_4, 7)]));
    let stock = shared.session.stock_characters();
    assert_eq!(stock.len(), 3);
    assert_eq!(stock[1].assets.len(), 1, "Alt's login assets, not jEve's");
    assert_eq!(
        stock[1].assets[0].quantity, 7,
        "7 from the login, not 60 from jEve"
    );
    assert_eq!(
        shared.session.source(ALT),
        Some(eve_means_of_profit::session::Source::Login)
    );
    assert_eq!(shared.session.revision(ALT), 1);
    assert_eq!(shared.session.revision(MAIN), 0);
}

#[test]
fn place_names_come_from_the_game_data_and_the_saved_names() {
    let env = env();
    saved::save_character(&esi_dir(&env), &main_data()).expect("Main's data saved");
    write_places(&env);
    {
        let mut store = Store::open(&env.data.join("user.sqlite"), NOW)
            .expect("the store opens")
            .store;
        store
            .set_structure_name(HOME_FORT, "Home Fort", NOW)
            .expect("the typed name is saved");
    }
    let mut harness = start(&env, FakeEve::new(vec![character(MAIN, "Main")], None), NOW);
    loaded(&mut harness);

    let shared = harness.state_mut().shared_mut();
    let sde = shared.sde.as_ref();
    let session = &shared.session;
    assert_eq!(
        session.place_label(JITA_4_4, sde),
        "Jita IV - Moon 4 - Caldari Navy Assembly Plant"
    );
    assert_eq!(session.place_label(HOME_FORT, sde), "Home Fort");
    assert_eq!(session.place_label(30_000_142, sde), "Jita");
    assert_eq!(
        session.place_label(REFUSED, sde),
        "Structure 1035466617999 (no access)"
    );
    assert_eq!(session.place_label(9, sde), "Location 9");
}

#[test]
fn the_start_refreshes_the_characters_and_old_data_is_noticed() {
    let env = env();
    saved::save_character(&esi_dir(&env), &main_data()).expect("Main's data saved");
    let eve = FakeEve::new(vec![character(MAIN, "Main")], None);
    let refreshes = eve.refreshes();
    let mut harness = start(&env, eve, 1_100_000);
    loaded(&mut harness);
    for _ in 0..10 {
        harness.run();
    }
    assert_eq!(*refreshes.lock().unwrap_or_else(PoisonError::into_inner), 1);
    assert!(
        notices(&mut harness).contains(&OLD_NOTICE.to_owned()),
        "{:?}",
        notices(&mut harness)
    );
}

#[test]
fn the_start_refresh_happens_once_and_never_repeats_on_its_own() {
    let env = env();
    saved::save_character(&esi_dir(&env), &main_data()).expect("Main's data saved");
    let eve = FakeEve::new(vec![character(MAIN, "Main")], None);
    let refreshes = eve.refreshes();
    let mut harness = start(&env, eve, NOW);
    loaded(&mut harness);
    settle_start(&mut harness, &refreshes);
    assert_eq!(*refreshes.lock().unwrap_or_else(PoisonError::into_inner), 1);

    for _ in 0..20 {
        harness.run();
    }
    assert_eq!(*refreshes.lock().unwrap_or_else(PoisonError::into_inner), 1);
}

#[test]
fn data_of_a_day_or_less_gives_no_notice() {
    let env = env();
    saved::save_character(&esi_dir(&env), &main_data()).expect("Main's data saved");
    let mut harness = start(&env, FakeEve::new(vec![character(MAIN, "Main")], None), NOW);
    loaded(&mut harness);
    assert!(harness.state_mut().shared_mut().notices.is_empty());
}

#[test]
fn without_a_jeve_file_only_the_login_shows_and_there_is_no_notice() {
    let env = env();
    saved::save_character(&esi_dir(&env), &main_data()).expect("Main's data saved");
    let mut harness = start(&env, FakeEve::new(vec![character(MAIN, "Main")], None), NOW);
    loaded(&mut harness);
    let shared = harness.state_mut().shared_mut();
    let ids: Vec<i64> = shared
        .session
        .stock_characters()
        .iter()
        .map(|entry| entry.character_id)
        .collect();
    assert_eq!(ids, [2_112_000_001]);
    assert!(shared.notices.is_empty());
}

#[test]
fn a_jeve_file_that_is_not_a_database_gives_its_notice() {
    let env = env();
    let folder = TempDir::new().expect("a temp folder");
    fs::write(folder.path().join("#Default.db"), "not a database").expect("written");
    let mut harness = start(
        &env,
        FakeEve::new(Vec::new(), Some(folder.path().to_path_buf())),
        NOW,
    );
    loaded(&mut harness);
    let texts = notices(&mut harness);
    assert!(
        texts
            .iter()
            .any(|text| text.starts_with("Could not read jEve's data file: ")),
        "{texts:?}"
    );
}

use super::*;

#[test]
fn prices_and_the_age_of_the_file() {
    let env = env();
    with_main(&env);
    let (mut harness, calls) = start(
        &env,
        FakeEve {
            characters: vec![character(MAIN, "Main")],
            jeve: None,
        },
        fresh_market(NOW - 240),
    );
    loaded(&mut harness);
    select(&mut harness, PAU);
    fetch_market(&mut harness);
    harness.run();

    assert!(shown(&harness, "Forge sell 61,200.00 · buy 55,010.00"));
    assert!(shown(&harness, "4m"), "the file is 4m old");
    let after_first = *calls.lock().unwrap_or_else(PoisonError::into_inner);
    for _ in 0..10 {
        harness.run();
    }
    select(&mut harness, MORPHITE);
    select(&mut harness, PAU);
    assert_eq!(
        *calls.lock().unwrap_or_else(PoisonError::into_inner),
        after_first,
        "the panel makes no market request"
    );
}

#[test]
fn an_old_file_shows_its_age_in_amber() {
    let env = env();
    with_main(&env);
    let (mut harness, _) = start(
        &env,
        FakeEve {
            characters: vec![character(MAIN, "Main")],
            jeve: None,
        },
        fresh_market(NOW - 700),
    );
    loaded(&mut harness);
    select(&mut harness, PAU);
    fetch_market(&mut harness);
    harness.run();

    assert!(shown(&harness, "11m"), "the file is 11m old");
}

#[test]
fn no_buy_row_shows_a_dash() {
    let env = env();
    with_main(&env);
    let answer = Answer::File {
        published: NOW - 240,
        rows: row(PAU, false, "61200.00"),
    };
    let (mut harness, _) = start(
        &env,
        FakeEve {
            characters: vec![character(MAIN, "Main")],
            jeve: None,
        },
        answer,
    );
    loaded(&mut harness);
    select(&mut harness, PAU);
    fetch_market(&mut harness);
    harness.run();

    assert!(shown(&harness, "Forge sell 61,200.00 · buy —"));
}

#[test]
fn no_file_yet_says_so() {
    let env = env();
    with_main(&env);
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

    assert!(shown(&harness, "No market data yet"));
}

#[test]
fn a_failed_download_shows_its_error() {
    let env = env();
    with_main(&env);
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
    let shared = harness.state_mut().shared_mut();
    let market = Arc::clone(&shared.market);
    market.refresh(&shared.worker, NOW, true);
    let expected = failure_text(&Failure::Market(MarketError::Network(
        "the file is down".to_owned(),
    )));
    until(&mut harness, "the failure", |h| shown(h, &expected));
    assert!(shown(&harness, &expected));
}

#[test]
fn a_failed_download_still_shows_after_the_build_page_takes_its_answer() {
    let env = env();
    with_main(&env);
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
    let shared = harness.state_mut().shared_mut();
    let market = Arc::clone(&shared.market);
    market.refresh(&shared.worker, NOW, true);
    let expected = failure_text(&Failure::Market(MarketError::Network(
        "the file is down".to_owned(),
    )));
    until(&mut harness, "the failure", |h| shown(h, &expected));
    let taken = harness.state_mut().shared_mut().market.take_outcomes();
    assert!(!taken.is_empty());
    harness.run();
    assert!(shown(&harness, &expected));
}

#[test]
fn a_working_download_after_a_failure_clears_the_line() {
    let env = env();
    with_main(&env);
    let answer = Arc::new(Mutex::new(Answer::Down));
    let (mut harness, _) = start_with(
        &env,
        FakeEve {
            characters: vec![character(MAIN, "Main")],
            jeve: None,
        },
        Arc::clone(&answer),
    );
    loaded(&mut harness);
    select(&mut harness, PAU);
    let shared = harness.state_mut().shared_mut();
    let market = Arc::clone(&shared.market);
    market.refresh(&shared.worker, NOW, true);
    let failed = failure_text(&Failure::Market(MarketError::Network(
        "the file is down".to_owned(),
    )));
    until(&mut harness, "the failure", |h| shown(h, &failed));

    *answer.lock().unwrap_or_else(PoisonError::into_inner) = Answer::File {
        published: NOW - 240,
        rows: format!(
            "{}{}",
            row(PAU, false, "61200.00"),
            row(PAU, true, "55010.00")
        ),
    };
    {
        // The failure holds the market for a minute, so the retry comes an hour later.
        let shared = harness.state_mut().shared_mut();
        let market = Arc::clone(&shared.market);
        market.refresh(&shared.worker, NOW + 3_600, true);
    }
    until(&mut harness, "the file", |h| {
        shown(h, "Forge sell 61,200.00 · buy 55,010.00")
    });
    assert!(!shown(&harness, &failed));
}

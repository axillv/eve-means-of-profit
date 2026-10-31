use super::*;

/// The `explicit` marks of the price answers that arrived since the last call.
fn take_prices(harness: &mut Harness<'_, App>) -> Vec<bool> {
    harness
        .state_mut()
        .shared_mut()
        .market
        .take_outcomes()
        .into_iter()
        .map(|outcome| outcome.explicit)
        .collect()
}

/// Runs frames until `wanted` price answers have arrived, and returns their marks.
fn wait_for_prices(harness: &mut Harness<'_, App>, wanted: usize) -> Vec<bool> {
    let mut all = Vec::new();
    let started = Instant::now();
    while all.len() < wanted {
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "only {} price answers arrived, wanted {wanted}",
            all.len()
        );
        harness.run_steps(1);
        thread::sleep(Duration::from_millis(5));
        all.extend(take_prices(harness));
    }
    all
}

/// Runs frames until no price request is queued or running, so that the next ask cannot be
/// dropped as a duplicate of this one.
fn prices_settled(harness: &mut Harness<'_, App>) {
    let started = Instant::now();
    while harness.state_mut().shared_mut().market.running() {
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "the price request never finished"
        );
        harness.run_steps(1);
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn with_the_keep_refreshing_choice_off_no_price_is_asked_again() {
    let folders = Folders::new();
    folders.install_game_data();
    folders.set_last_check(990_000);
    let clock = Arc::new(AtomicI64::new(NOW));
    let mut harness = start_with_clock(
        Fake::new(Latest::Body(latest_body(BUILD))),
        &folders,
        Arc::clone(&clock),
    );
    until(&mut harness, "the build pages", build_page_shown);

    assert_eq!(
        wait_for_prices(&mut harness, 1),
        vec![false],
        "the start-up ask is quiet"
    );
    prices_settled(&mut harness);
    assert!(
        take_prices(&mut harness).is_empty(),
        "the start-up ask gives one answer"
    );

    clock.store(NOW + 3_600, Ordering::SeqCst);
    settle(&mut harness);
    assert!(
        take_prices(&mut harness).is_empty(),
        "twenty frames an hour on ask for nothing"
    );
}

#[test]
fn with_the_keep_refreshing_choice_on_the_prices_are_asked_again_every_fifteen_minutes() {
    let folders = Folders::new();
    folders.install_game_data();
    folders.set_last_check(990_000);
    folders.set_refresh_while_open("1");
    let clock = Arc::new(AtomicI64::new(NOW));
    let mut harness = start_with_clock(
        Fake::new(Latest::Body(latest_body(BUILD))),
        &folders,
        Arc::clone(&clock),
    );
    until(&mut harness, "the build pages", build_page_shown);

    assert_eq!(
        wait_for_prices(&mut harness, 1),
        vec![false],
        "the start-up ask is quiet"
    );
    prices_settled(&mut harness);
    assert!(
        take_prices(&mut harness).is_empty(),
        "exactly one answer at start"
    );

    clock.store(NOW + 899, Ordering::SeqCst);
    settle(&mut harness);
    assert!(
        take_prices(&mut harness).is_empty(),
        "at +899 the gap has not run out"
    );

    clock.store(NOW + 900, Ordering::SeqCst);
    assert_eq!(
        wait_for_prices(&mut harness, 1),
        vec![false],
        "at +900 one quiet answer"
    );
    prices_settled(&mut harness);
    settle(&mut harness);
    assert!(
        take_prices(&mut harness).is_empty(),
        "at +900 one answer, not two"
    );

    clock.store(NOW + 1_800, Ordering::SeqCst);
    assert_eq!(
        wait_for_prices(&mut harness, 1),
        vec![false],
        "at +1800 one quiet answer"
    );
    prices_settled(&mut harness);
    settle(&mut harness);
    assert!(
        take_prices(&mut harness).is_empty(),
        "at +1800 one answer, not two"
    );
}

#[test]
fn turning_the_keep_refreshing_choice_on_asks_once_then_waits_its_gap() {
    let folders = Folders::new();
    folders.install_game_data();
    folders.set_last_check(990_000);
    let clock = Arc::new(AtomicI64::new(NOW));
    let mut harness = start_with_clock(
        Fake::new(Latest::Body(latest_body(BUILD))),
        &folders,
        Arc::clone(&clock),
    );
    until(&mut harness, "the build pages", build_page_shown);

    assert_eq!(
        wait_for_prices(&mut harness, 1),
        vec![false],
        "the start-up ask is quiet"
    );
    prices_settled(&mut harness);
    assert!(
        take_prices(&mut harness).is_empty(),
        "the start-up ask gives one answer"
    );

    // The choice was off at start, so nothing was timed: ticking it makes the next frame due.
    harness.key_press_modifiers(Modifiers::CTRL, Key::Comma);
    until(&mut harness, "the settings page", |h| {
        shown(h, "Interface size")
    });
    click_checkbox(&mut harness, "Keep refreshing while open");
    settle(&mut harness);
    assert!(
        harness.state_mut().shared_mut().refresh_while_open,
        "the choice is on"
    );

    assert_eq!(
        wait_for_prices(&mut harness, 1),
        vec![false],
        "the frame after the tick asks once, quietly"
    );
    prices_settled(&mut harness);
    settle(&mut harness);
    assert!(
        take_prices(&mut harness).is_empty(),
        "the rest of the minute asks nothing"
    );

    clock.store(NOW + 899, Ordering::SeqCst);
    settle(&mut harness);
    assert!(
        take_prices(&mut harness).is_empty(),
        "the gap from the tick has not run out"
    );

    clock.store(NOW + 900, Ordering::SeqCst);
    assert_eq!(
        wait_for_prices(&mut harness, 1),
        vec![false],
        "the gap from the tick runs out at +900"
    );
}

#[test]
fn turning_the_keep_refreshing_choice_off_and_on_again_asks_at_once() {
    let folders = Folders::new();
    folders.install_game_data();
    folders.set_last_check(990_000);
    folders.set_refresh_while_open("1");
    let clock = Arc::new(AtomicI64::new(NOW));
    let mut harness = start_with_clock(
        Fake::new(Latest::Body(latest_body(BUILD))),
        &folders,
        Arc::clone(&clock),
    );
    until(&mut harness, "the build pages", build_page_shown);

    assert_eq!(
        wait_for_prices(&mut harness, 1),
        vec![false],
        "the start-up ask is quiet"
    );
    prices_settled(&mut harness);
    assert!(
        take_prices(&mut harness).is_empty(),
        "the start-up ask gives one answer"
    );

    // Off while the start-up gap still has time left: nothing is asked, and the time the
    // gap had left is dropped with the choice.
    clock.store(NOW + 120, Ordering::SeqCst);
    harness.key_press_modifiers(Modifiers::CTRL, Key::Comma);
    until(&mut harness, "the settings page", |h| {
        shown(h, "Interface size")
    });
    click_checkbox(&mut harness, "Keep refreshing while open");
    settle(&mut harness);
    assert!(
        !harness.state_mut().shared_mut().refresh_while_open,
        "the choice is off"
    );
    assert_eq!(saved_refresh_while_open(&folders).as_deref(), Some("0"));
    settle(&mut harness);
    assert!(
        take_prices(&mut harness).is_empty(),
        "the choice off asks for nothing"
    );

    // Back on before the gap's old time (NOW + 900) has come: the next frame asks, it does
    // not wait out what the choice had left when it went off.
    clock.store(NOW + 600, Ordering::SeqCst);
    click_checkbox(&mut harness, "Keep refreshing while open");
    settle(&mut harness);
    assert!(
        harness.state_mut().shared_mut().refresh_while_open,
        "the choice is on again"
    );
    assert_eq!(saved_refresh_while_open(&folders).as_deref(), Some("1"));

    assert_eq!(
        wait_for_prices(&mut harness, 1),
        vec![false],
        "the frame after the choice comes back on asks at once, quietly"
    );
    prices_settled(&mut harness);
    settle(&mut harness);
    assert!(
        take_prices(&mut harness).is_empty(),
        "the ask comes once, not on every frame"
    );
}

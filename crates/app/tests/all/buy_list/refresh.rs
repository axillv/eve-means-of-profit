use super::*;

#[test]
fn a_request_that_blocks_does_not_stop_the_frames() {
    let world = World::new();
    lock(&world.forge).hold = true;
    let env = make_env(&standard_services());
    let mut harness = start(&env, &world);
    harness.run();
    buy_list_for(&mut harness);
    for _ in 0..20 {
        harness.run();
    }
    assert!(shown(&harness, "Morphite"));
    assert!(shown(&harness, "…"));
    lock(&world.forge).hold = false;
    settle(&mut harness, "27,500.00");
}

#[test]
fn the_start_download_is_the_only_automatic_one() {
    let world = World::new();
    let mut harness = complete_list(&world, &standard_services());
    assert_eq!(world.requests(), 1);
    world.at(NOW + 899);
    harness.run();
    assert_eq!(world.requests(), 1);
    assert!(!shown(&harness, "Refresh automatically"));
    lock(&world.forge).publish(
        {
            let mut lines = standard_lines();
            lines
                .iter_mut()
                .find(|line| line.type_id == 2454)
                .expect("Hobgoblin I")
                .five = "27600.00";
            lines
        },
        NOW + 600,
    );
    for at in [NOW + 900, NOW + 1_800] {
        world.at(at);
        for _ in 0..10 {
            harness.run();
        }
        assert_eq!(world.requests(), 1, "no timer asks for the file");
        assert!(shown(&harness, "27,500.00"));
    }

    // The player's refresh gets the new file.
    click(&mut harness, "Refresh now");
    settle(&mut harness, "27,600.00");
    assert_eq!(world.requests(), 2);
    assert_eq!(
        lock(&world.forge).calls.last(),
        Some(&Some("\"1\"".to_owned()))
    );
}

#[test]
fn nothing_is_asked_after_the_start_download_until_the_player_asks() {
    let world = World::new();
    let mut harness = complete_list(&world, &standard_services());
    world.at(NOW + 5_000);
    harness.run();
    assert_eq!(world.requests(), 1);
    world.at(NOW + 300);
    click(&mut harness, "Refresh now");
    settle(&mut harness, "Prices are 9m old; next refresh in 10m");
    assert_eq!(world.requests(), 1);
    world.at(NOW + 900);
    click(&mut harness, "Refresh now");
    harness.run();
    assert_eq!(world.requests(), 2);
}

#[test]
fn opening_the_list_with_no_prices_asks_once() {
    let world = World::new();
    let env = make_env(&standard_services());
    let mut harness = start(&env, &world);
    harness.run();
    buy_list_for(&mut harness);
    settle(&mut harness, "Oldest: 4m");
    for _ in 0..20 {
        harness.run();
    }
    assert_eq!(world.requests(), 1);
}

#[test]
fn a_saved_auto_refresh_setting_does_not_add_a_price_request() {
    for auto in [true, false] {
        let world = World::new();
        let env = make_env(&standard_services());
        let mut store = Store::open(&env.data.join("user.sqlite"), NOW)
            .expect("the store opens")
            .store;
        let mut settings = store.history_settings().expect("history settings");
        settings.auto_refresh = auto;
        store
            .save_history_settings(&settings)
            .expect("history settings saved");

        let mut harness = start(&env, &world);
        let mut marks = Vec::new();
        for _ in 0..500 {
            harness.step();
            marks.extend(
                harness
                    .state_mut()
                    .shared_mut()
                    .market
                    .take_outcomes()
                    .into_iter()
                    .map(|outcome| outcome.explicit),
            );
            if !marks.is_empty() && !harness.state_mut().shared_mut().market.running() {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(marks, vec![false], "auto_refresh {auto}");
        buy_list_for(&mut harness);
        settle(&mut harness, "Oldest: 4m");
        for _ in 0..20 {
            harness.run();
        }
        assert_eq!(world.requests(), 1, "auto_refresh {auto}");
    }
}

#[test]
fn refresh_all_at_the_rate_limit_shows_one_notice_and_keeps_the_rows() {
    let world = World::new();
    // The file is one second old when the start download holds it, so at +899 s it is 15m old.
    lock(&world.forge).published = NOW - 1;
    let env = make_env(&standard_services());
    let mut harness = start(&env, &world);
    harness.run();
    buy_list_for(&mut harness);
    settle(&mut harness, "Oldest: 1s");
    assert_eq!(world.requests(), 1);

    world.at(NOW + 899);
    click(&mut harness, "Refresh all");
    let notice = "Prices are 15m old; next refresh in 1s";
    settle(&mut harness, notice);
    assert_eq!(harness.query_all_by_label(notice).count(), 1);
    assert_row(&harness, "Robotics", &["39,990.00"]);
    // Too soon makes no request.
    assert_eq!(world.requests(), 1);
}

#[test]
fn refresh_now_is_disabled_while_a_request_runs() {
    let world = World::new();
    let mut harness = complete_list(&world, &standard_services());
    world.at(NOW + 900);
    lock(&world.forge).hold = true;
    // The player asks for the file; the request waits on the fake.
    click(&mut harness, "Refresh now");
    assert!(shown(&harness, "Refreshing…"));
    assert!(
        harness
            .get_by_label("Refresh now")
            .accesskit_node()
            .is_disabled()
    );
    assert_eq!(world.requests(), 1);
    lock(&world.forge).hold = false;
    for _ in 0..500 {
        harness.run();
        if !shown(&harness, "Refreshing…") {
            break;
        }
    }
    assert_eq!(world.requests(), 2);
    assert!(
        !harness
            .get_by_label("Refresh now")
            .accesskit_node()
            .is_disabled()
    );
}

#[test]
fn a_first_download_that_fails_is_not_asked_again_on_its_own() {
    let world = World::new();
    lock(&world.forge).fault = Fault::Unavailable;
    let env = make_env(&standard_services());
    let mut harness = start(&env, &world);
    harness.run();
    buy_list_for(&mut harness);
    settle(&mut harness, "ERROR");
    world.at(NOW + 900);
    for _ in 0..50 {
        harness.run();
    }
    assert_eq!(world.requests(), 1);
    // The next request is the player's, not a timer's.
    lock(&world.forge).fault = Fault::None;
    click(&mut harness, "Refresh now");
    settle(&mut harness, "27,500.00");
    assert_eq!(world.requests(), 2);
}

#[test]
fn a_crashed_download_is_reported_and_only_the_player_asks_again() {
    let world = World::new();
    let mut harness = complete_list(&world, &standard_services());
    lock(&world.forge).fault = Fault::Crash;
    world.at(NOW + 900);
    click(&mut harness, "Refresh now");
    settle(
        &mut harness,
        "Could not get market prices: the download stopped unexpectedly.",
    );
    assert_row(&harness, "Robotics", &["39,990.00"]);
    assert!(
        !harness
            .get_by_label("Refresh now")
            .accesskit_node()
            .is_disabled()
    );
    // The failed download is not asked again on every frame.
    let asked = world.requests();
    for _ in 0..10 {
        harness.run();
    }
    assert_eq!(world.requests(), asked);
    // Once the download works again, only the player's refresh asks.
    lock(&world.forge).fault = Fault::None;
    world.at(NOW + 1_800);
    for _ in 0..10 {
        harness.run();
    }
    assert_eq!(world.requests(), asked);
    click(&mut harness, "Refresh now");
    for _ in 0..500 {
        harness.run();
        if !shown(&harness, "Refreshing…") {
            break;
        }
    }
    assert_eq!(world.requests(), asked + 1);
}

#[test]
fn a_stopped_worker_says_the_prices_cannot_refresh_until_restart() {
    let world = World::new();
    let mut harness = complete_list(&world, &standard_services());
    // A job that crashes stops the worker's thread for the rest of the session.
    harness
        .state_mut()
        .shared_mut()
        .worker
        .run(Box::new(|| panic!("a job crashed")));
    world.at(NOW + 900);
    click(&mut harness, "Refresh now");
    settle(
        &mut harness,
        "Prices cannot refresh until the app restarts.",
    );
    assert!(
        harness
            .get_by_label("Refresh now")
            .accesskit_node()
            .is_disabled()
    );
    assert_row(&harness, "Robotics", &["39,990.00"]);
}

#[test]
fn with_another_page_open_no_timer_request_runs() {
    let world = World::new();
    let mut harness = complete_list(&world, &standard_services());
    click(&mut harness, Page::Setup.label());
    world.at(NOW + 900);
    for _ in 0..5 {
        harness.run();
    }
    // Only the start download asked, whatever page is open.
    assert_eq!(world.requests(), 1);
    click(&mut harness, "Build");
    harness.run();
    assert_eq!(world.requests(), 1);
    assert!(!shown(&harness, "Next refresh in 15m"));
}

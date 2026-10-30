use super::*;

#[test]
fn the_first_frame_records_nothing_and_the_next_refresh_records_one() {
    let dir = TempDir::new().expect("a temp folder");
    let mut harness = start(&dir);
    let plex = FakePlex::offline();
    harness.state_mut().shared_mut().plex_http = Arc::new(plex.clone());
    assert_eq!(stored_records(&dir), 0);
    // The first frame and the start-up read make no request.
    wait_for_no_request(&mut harness);
    assert!(plex.asked().is_empty());
    harness
        .state_mut()
        .shared_mut()
        .session
        .replace(main_with_wallet(Money::from_ten_thousandths(50_000)));
    harness.run();
    assert_eq!(stored_records(&dir), 1);
    // The record made one request; a further frame, with no new revision, makes no other.
    settle(&mut harness, "the PLEX request", |_| {
        plex.asked().len() == 1
    });
    wait_for_no_request(&mut harness);
    assert_eq!(plex.asked(), vec![PLEX_URL.to_owned()]);
    harness.run();
    assert_eq!(stored_records(&dir), 1);
}

#[test]
fn a_wallet_alone_records_only_the_wallet() {
    let dir = TempDir::new().expect("a temp folder");
    let mut harness = start(&dir);
    harness
        .state_mut()
        .shared_mut()
        .session
        .replace(main_with_wallet(Money::from_ten_thousandths(50_000)));
    harness.run();
    let records = store_of(&dir.path().join("data"))
        .records(0, NOW)
        .expect("records read");
    assert_eq!(records.len(), 1);
    match &records[0].snapshot {
        Snapshot::Seen(holdings) => {
            assert_eq!(holdings.wallet, Some(Money::from_ten_thousandths(50_000)));
            assert!(holdings.assets.is_empty());
            assert_eq!(holdings.sell_orders, None);
            assert_eq!(holdings.buy_escrow, None);
            assert!(holdings.jobs_output.is_empty());
        }
        Snapshot::Imported(_) | Snapshot::Removed => {
            panic!("expected a seen record")
        }
    }
}

#[test]
fn a_character_that_logs_in_after_a_start_without_characters_is_recorded() {
    let dir = TempDir::new().expect("a temp folder");
    let mut harness = start_without_characters(&dir);
    harness.run();
    assert_eq!(stored_records(&dir), 0);
    harness
        .state_mut()
        .shared_mut()
        .session
        .put_character(character(MAIN, "Main"));
    harness
        .state_mut()
        .shared_mut()
        .session
        .replace(main_with_wallet(Money::from_ten_thousandths(50_000)));
    harness.run();
    assert_eq!(stored_records(&dir), 1);
}

#[test]
fn a_job_of_another_activity_than_its_recipe_adds_nothing_and_is_an_error() {
    let dir = TempDir::new().expect("a temp folder");
    let mut harness = start(&dir);
    let mut data = CharacterData::new(MAIN);
    data.jobs = Some(fetched(vec![Job {
        activity_id: 11,
        ..job(1, 10, JobStatus::Active)
    }]));
    harness.state_mut().shared_mut().session.replace(data);
    harness.run();
    let records = store_of(&dir.path().join("data"))
        .records(0, NOW)
        .expect("records read");
    assert_eq!(records.len(), 1);
    match &records[0].snapshot {
        Snapshot::Seen(holdings) => assert!(holdings.jobs_output.is_empty()),
        Snapshot::Imported(_) | Snapshot::Removed => {
            panic!("expected a seen record")
        }
    }
    assert!(errors(&mut harness).iter().any(|text| text.starts_with(
        "Could not record net worth: a job's product has no recipe for its activity"
    )));
}

#[test]
fn a_job_without_a_recipe_adds_nothing_and_the_rest_of_the_record_is_kept() {
    let dir = TempDir::new().expect("a temp folder");
    let mut harness = start(&dir);
    let mut data = main_with_wallet(Money::from_ten_thousandths(50_000));
    data.jobs = Some(fetched(vec![
        job(1, 10, JobStatus::Active),
        Job {
            product_type_id: Some(TRITANIUM),
            ..job(3, 1, JobStatus::Active)
        },
    ]));
    harness.state_mut().shared_mut().session.replace(data);
    harness.run();
    let records = store_of(&dir.path().join("data"))
        .records(0, NOW)
        .expect("records read");
    assert_eq!(records.len(), 1);
    match &records[0].snapshot {
        Snapshot::Seen(holdings) => {
            assert_eq!(holdings.wallet, Some(Money::from_ten_thousandths(50_000)));
            assert_eq!(holdings.jobs_output, BTreeMap::from([(PAU, 10)]));
        }
        Snapshot::Imported(_) | Snapshot::Removed => {
            panic!("expected a seen record")
        }
    }
    assert!(errors(&mut harness).iter().any(|text| text.starts_with(
        "Could not record net worth: a job's product has no recipe for its activity"
    )));
}

#[test]
fn without_game_data_nothing_is_recorded_and_blueprints_are_not_counted() {
    let dir = TempDir::new().expect("a temp folder");
    let mut harness = start(&dir);
    harness.state_mut().shared_mut().sde = None;
    harness
        .state_mut()
        .shared_mut()
        .session
        .replace(main_data());
    harness.run();
    assert_eq!(stored_records(&dir), 0);
    assert!(copies(&dir).is_empty());
    assert!(
        errors(&mut harness)
            .iter()
            .any(|text| text == "Could not record net worth: no game data is loaded")
    );
}

#[test]
#[rustfmt::skip]
fn a_change_that_arrives_without_game_data_is_recorded_once_the_data_loads() {
    let dir = TempDir::new().expect("a temp folder");
    let mut harness = start(&dir);
    harness.state_mut().shared_mut().sde = None;
    harness
        .state_mut()
        .shared_mut()
        .session
        .replace(main_data());
    harness.run();
    assert_eq!(stored_records(&dir), 0);
    assert!(
        errors(&mut harness)
            .iter()
            .any(|text| text == "Could not record net worth: no game data is loaded")
    );
    harness.state_mut().shared_mut().sde = Some(
        Sde::open(&dir.path().join("data").join("sde.sqlite")).expect("the game data opens"),
    );
    harness.run();
    assert_eq!(stored_records(&dir), 1);
}

#[test]
fn a_sell_sum_too_large_is_an_error_and_writes_nothing() {
    let dir = TempDir::new().expect("a temp folder");
    let mut harness = start(&dir);
    let plex = FakePlex::offline();
    harness.state_mut().shared_mut().plex_http = Arc::new(plex.clone());
    let huge = Money::from_ten_thousandths(9_000_000_000_000_000_000);
    let mut data = with_orders(vec![order(false, false, huge, 100, None)]);
    data.wallet = None;
    harness.state_mut().shared_mut().session.replace(data);
    harness.run();
    // No record was written, so no copy job ran and no request was made.
    wait_for_no_request(&mut harness);
    assert!(plex.asked().is_empty());
    assert_eq!(stored_records(&dir), 0);
    assert!(copies(&dir).is_empty());
    assert!(
        errors(&mut harness)
            .iter()
            .any(|text| text == "Could not record net worth: the sum is too large")
    );
}

#[test]
fn a_personal_sell_and_buy_are_summed_and_corporation_orders_are_left_out() {
    let dir = TempDir::new().expect("a temp folder");
    let mut harness = start(&dir);
    let data = with_orders(vec![
        order(
            false,
            false,
            Money::from_ten_thousandths(612_000_000),
            100,
            None,
        ),
        order(
            false,
            true,
            Money::from_ten_thousandths(700_000_000),
            50,
            None,
        ),
        order(
            true,
            false,
            Money::from_ten_thousandths(5_501_000_000),
            1,
            Some(Money::from_ten_thousandths(5_501_000_000)),
        ),
    ]);
    harness.state_mut().shared_mut().session.replace(data);
    harness.run();
    let records = store_of(&dir.path().join("data"))
        .records(0, NOW)
        .expect("records read");
    match &records[0].snapshot {
        Snapshot::Seen(holdings) => {
            assert_eq!(
                holdings.sell_orders,
                Some(Money::from_ten_thousandths(61_200_000_000))
            );
            assert_eq!(
                holdings.buy_escrow,
                Some(Money::from_ten_thousandths(5_501_000_000))
            );
        }
        Snapshot::Imported(_) | Snapshot::Removed => {
            panic!("expected a seen record")
        }
    }
}

#[test]
fn assets_count_units_by_type() {
    let dir = TempDir::new().expect("a temp folder");
    let mut harness = start(&dir);
    let asset = |item_id: i64, quantity: u64| Asset {
        item_id,
        type_id: PAU,
        location_id: 60_003_760,
        flag: Some("Hangar".to_owned()),
        quantity,
        singleton: false,
    };
    let stock = CharacterAssets {
        character_id: i64::try_from(MAIN).expect("an id that fits"),
        updated: Some(990_000),
        assets: vec![asset(1, 40), asset(2, 120)],
    };
    let mut data = CharacterData::new(MAIN);
    data.assets = Some(fetched(OwnedAssets {
        assets: stock,
        names: Default::default(),
    }));
    harness.state_mut().shared_mut().session.replace(data);
    harness.run();
    let records = store_of(&dir.path().join("data"))
        .records(0, NOW)
        .expect("records read");
    match &records[0].snapshot {
        Snapshot::Seen(holdings) => {
            assert_eq!(holdings.assets.get(&PAU), Some(&160));
        }
        Snapshot::Imported(_) | Snapshot::Removed => {
            panic!("expected a seen record")
        }
    }
}

#[test]
fn a_refresh_records_the_scenario_and_copies_the_prices_of_pau() {
    let dir = TempDir::new().expect("a temp folder");
    let mut harness = start_with(&dir, Box::new(FileForge));
    download(&mut harness);
    harness
        .state_mut()
        .shared_mut()
        .session
        .replace(main_data());
    harness.run();
    let records = store_of(&dir.path().join("data"))
        .records(0, NOW)
        .expect("records read");
    assert_eq!(records.len(), 1);
    match &records[0].snapshot {
        Snapshot::Seen(holdings) => {
            assert_eq!(
                holdings.wallet,
                Some(Money::from_ten_thousandths(12_345_678_900))
            );
            assert_eq!(holdings.assets, BTreeMap::from([(PAU, 160)]));
            assert_eq!(
                holdings.sell_orders,
                Some(Money::from_ten_thousandths(61_200_000_000))
            );
            assert_eq!(
                holdings.buy_escrow,
                Some(Money::from_ten_thousandths(5_501_000_000))
            );
            assert_eq!(holdings.jobs_output, BTreeMap::from([(PAU, 10)]));
        }
        Snapshot::Imported(_) | Snapshot::Removed => panic!("expected a seen record"),
    }
    // The copy is written on the worker, a frame after the record.
    settle(&mut harness, "the price copy", |_| copies(&dir).len() == 1);
    let saved = copies(&dir);
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].at, NOW);
    assert_eq!(saved[0].published_at, Some(PUBLISHED));
    let held = harness.state_mut().shared_mut().market.held();
    let pau: BTreeMap<_, _> = held
        .rows
        .iter()
        .filter(|(key, _)| key.0 == PAU)
        .map(|(key, row)| (*key, row.clone()))
        .collect();
    assert_eq!(pau.len(), 2);
    assert!(pau.contains_key(&(PAU, Side::Sell)));
    assert!(pau.contains_key(&(PAU, Side::Buy)));
    assert_eq!(saved[0].rows, pau);
    harness.run();
    assert_eq!(stored_records(&dir), 1);
    assert_eq!(copies(&dir).len(), 1);
}

#[test]
fn a_carried_alt_keeps_its_tritanium_price_in_the_copy() {
    let dir = TempDir::new().expect("a temp folder");
    let mut harness = start_with(&dir, Box::new(FileForge));
    download(&mut harness);
    let alt = Record {
        character: CharacterKey::Id(ALT),
        at: NOW - 90_000,
        snapshot: Snapshot::Seen(Holdings {
            wallet: None,
            assets: BTreeMap::from([(TRITANIUM, 1_000)]),
            sell_orders: None,
            buy_escrow: None,
            jobs_output: BTreeMap::new(),
        }),
    };
    store_of(&dir.path().join("data"))
        .add_record(&alt)
        .expect("the alt record is written");
    harness
        .state_mut()
        .shared_mut()
        .session
        .replace(main_data());
    harness.run();
    // The copy is written on the worker, a frame after the record.
    settle(&mut harness, "the price copy", |_| copies(&dir).len() == 1);
    let saved = copies(&dir);
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].at, NOW);
    assert_eq!(saved[0].published_at, Some(PUBLISHED));
    let held = harness.state_mut().shared_mut().market.held();
    assert_eq!(saved[0].rows.len(), 3);
    assert!(saved[0].rows.contains_key(&(TRITANIUM, Side::Sell)));
    // Every held row the point's types cover: the file's stale PLEX row is not one of them,
    // and the PLEX market fails in this test, so no fetched row takes its place.
    let without_plex: BTreeMap<_, _> = held
        .rows
        .iter()
        .filter(|((type_id, _), _)| *type_id != PLEX)
        .map(|(key, row)| (*key, row.clone()))
        .collect();
    assert_eq!(saved[0].rows, without_plex);
}

#[test]
fn without_a_download_the_record_is_written_and_nothing_is_copied() {
    let dir = TempDir::new().expect("a temp folder");
    let mut harness = start(&dir);
    let plex = FakePlex::offline();
    harness.state_mut().shared_mut().plex_http = Arc::new(plex.clone());
    assert!(
        harness
            .state_mut()
            .shared_mut()
            .market
            .held()
            .published_at
            .is_none()
    );
    harness
        .state_mut()
        .shared_mut()
        .session
        .replace(main_data());
    harness.run();
    assert_eq!(stored_records(&dir), 1);
    // The request is made all the same; its failure leaves no copy.
    settle(&mut harness, "the failed PLEX fetch", |h| {
        errors(h)
            .iter()
            .any(|text| text.starts_with("Could not fetch the PLEX price: "))
    });
    assert_eq!(plex.asked(), vec![PLEX_URL.to_owned()]);
    assert_eq!(stored_records(&dir), 1);
    assert!(copies(&dir).is_empty());
}

#[test]
fn one_frame_records_main_and_alt_and_keeps_pau_and_the_fetched_plex_price() {
    let dir = TempDir::new().expect("a temp folder");
    let mut harness = start_with_characters(
        &dir,
        Box::new(FileForge),
        vec![character(MAIN, "Main"), character(ALT, "Alt")],
    );
    download(&mut harness);
    let plex = FakePlex::page(200, None, &plex_body(false));
    harness.state_mut().shared_mut().plex_http = Arc::new(plex.clone());
    {
        let shared = harness.state_mut().shared_mut();
        shared.session.replace(main_data());
        shared.session.replace(alt_holding_plex());
    }
    harness.run();
    settle(&mut harness, "the price copy", |_| copies(&dir).len() == 1);
    assert_eq!(stored_records(&dir), 2);
    assert_eq!(plex.asked(), vec![PLEX_URL.to_owned()]);
    let saved = copies(&dir);
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].at, NOW);
    assert_eq!(saved[0].published_at, Some(PUBLISHED));
    // PAU's two rows as the file holds them, PLEX's two sides as the orders summarise them,
    // and the file's stale PLEX row gone. Alt holds PLEX, so its stale row would be kept
    // without the fetch replacing it.
    let held = harness.state_mut().shared_mut().market.held();
    let mut wanted: BTreeMap<_, _> = held
        .rows
        .iter()
        .filter(|((type_id, _), _)| *type_id == PAU)
        .map(|(key, row)| (*key, row.clone()))
        .collect();
    assert_eq!(wanted.len(), 2);
    wanted.extend(fetched_plex_rows());
    assert_eq!(saved[0].rows.len(), 4);
    assert_eq!(saved[0].rows, wanted);
    assert!(errors(&mut harness).is_empty());
}

#[test]
fn only_the_sell_side_of_plex_is_kept_and_no_notice_is_shown() {
    let dir = TempDir::new().expect("a temp folder");
    let mut harness = start_with(&dir, Box::new(FileForge));
    download(&mut harness);
    let plex = FakePlex::page(200, None, &plex_body(true));
    harness.state_mut().shared_mut().plex_http = Arc::new(plex.clone());
    harness
        .state_mut()
        .shared_mut()
        .session
        .replace(main_data());
    harness.run();
    settle(&mut harness, "the price copy", |_| copies(&dir).len() == 1);
    assert_eq!(plex.asked(), vec![PLEX_URL.to_owned()]);
    let saved = copies(&dir);
    assert_eq!(saved.len(), 1);
    assert_eq!(
        saved[0].rows.get(&(PLEX, Side::Sell)),
        Some(&fetched_plex()[&Side::Sell])
    );
    assert!(!saved[0].rows.contains_key(&(PLEX, Side::Buy)));
    assert!(saved[0].rows.contains_key(&(PAU, Side::Sell)));
    assert!(saved[0].rows.contains_key(&(PAU, Side::Buy)));
    assert!(errors(&mut harness).is_empty());
}

#[test]
fn a_failed_plex_fetch_saves_the_held_copy_without_plex_and_says_so() {
    let dir = TempDir::new().expect("a temp folder");
    let mut harness = start_with(&dir, Box::new(FileForge));
    download(&mut harness);
    let plex = FakePlex::page(503, None, "");
    harness.state_mut().shared_mut().plex_http = Arc::new(plex.clone());
    // Main holds PLEX, so the file's stale PLEX row would reach the copy without its
    // removal; the failed fetch must still leave the copy with PAU's rows only.
    harness
        .state_mut()
        .shared_mut()
        .session
        .replace(main_holding_plex());
    harness.run();
    settle(&mut harness, "the PLEX error", |h| {
        errors(h)
            .iter()
            .any(|text| text == "Could not fetch the PLEX price: ESI answered with status 503")
    });
    assert_eq!(plex.asked(), vec![PLEX_URL.to_owned()]);
    assert_eq!(stored_records(&dir), 1);
    let saved = copies(&dir);
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].published_at, Some(PUBLISHED));
    // Main holds PLEX and the fetch failed, so without the removal the file's stale
    // 4,000,000.00 row would be here: PAU's two rows only.
    assert_eq!(saved[0].rows.len(), 2);
    assert!(saved[0].rows.contains_key(&(PAU, Side::Sell)));
    assert!(saved[0].rows.contains_key(&(PAU, Side::Buy)));
    assert!(!saved[0].rows.contains_key(&(PLEX, Side::Sell)));
}

#[test]
fn a_rate_limited_plex_fetch_says_so_and_is_not_retried() {
    let dir = TempDir::new().expect("a temp folder");
    let mut harness = start_with(&dir, Box::new(FileForge));
    download(&mut harness);
    let plex = FakePlex::page(420, Some(30), "");
    harness.state_mut().shared_mut().plex_http = Arc::new(plex.clone());
    harness
        .state_mut()
        .shared_mut()
        .session
        .replace(main_data());
    harness.run();
    settle(&mut harness, "the rate limit notice", |h| {
        errors(h).iter().any(|text| {
            text == "Could not fetch the PLEX price: ESI rate limit reached; retry in 30 seconds"
        })
    });
    wait_for_no_request(&mut harness);
    assert_eq!(plex.asked(), vec![PLEX_URL.to_owned()]);
    assert_eq!(stored_records(&dir), 1);
    assert_eq!(copies(&dir).len(), 1);
}

#[test]
fn without_a_download_the_fetched_plex_price_alone_makes_the_copy() {
    let dir = TempDir::new().expect("a temp folder");
    let mut harness = start(&dir);
    let plex = FakePlex::page(200, None, &plex_body(false));
    harness.state_mut().shared_mut().plex_http = Arc::new(plex.clone());
    harness
        .state_mut()
        .shared_mut()
        .session
        .replace(main_data());
    harness.run();
    settle(&mut harness, "the price copy", |_| copies(&dir).len() == 1);
    assert_eq!(plex.asked(), vec![PLEX_URL.to_owned()]);
    assert_eq!(stored_records(&dir), 1);
    let saved = copies(&dir);
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].at, NOW);
    assert_eq!(saved[0].published_at, None);
    assert_eq!(saved[0].rows, fetched_plex_rows());
    assert!(errors(&mut harness).is_empty());
}

#[test]
fn without_a_download_a_failed_plex_fetch_saves_no_copy_and_says_so() {
    let dir = TempDir::new().expect("a temp folder");
    let mut harness = start(&dir);
    let plex = FakePlex::page(503, None, "");
    harness.state_mut().shared_mut().plex_http = Arc::new(plex.clone());
    harness
        .state_mut()
        .shared_mut()
        .session
        .replace(main_data());
    harness.run();
    settle(&mut harness, "the PLEX error", |h| {
        errors(h)
            .iter()
            .any(|text| text == "Could not fetch the PLEX price: ESI answered with status 503")
    });
    assert_eq!(plex.asked(), vec![PLEX_URL.to_owned()]);
    assert_eq!(stored_records(&dir), 1);
    assert!(copies(&dir).is_empty());
}

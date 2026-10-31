use super::*;

// The Net worth block on Today. The clock is 1,000,000,000, and the records are written
// before the app starts, so its first frame reads them.
const NW_NOW: i64 = 1_000_000_000;
const NW_FIRST: i64 = 999_395_200;
const NW_YESTERDAY: i64 = 999_914_400;
const NW_PUBLISHED: i64 = 999_999_000;
/// The Rifter, which the fixture holds but no price in these tests covers.
const NW_RIFTER: u32 = 587;

fn nw_isk(text: &str) -> Money {
    Money::parse_isk(text).expect("a test amount")
}

fn nw_row(price: &str) -> SummaryRow {
    let price = nw_isk(price);
    SummaryRow {
        min: price,
        max: price,
        weighted_average: price,
        median: price,
        five_percent: price,
        volume: 1_000,
        orders: 7,
    }
}

fn nw_seen(
    wallet: Option<&str>,
    assets: &[(u32, u64)],
    sell_orders: Option<&str>,
    buy_escrow: Option<&str>,
    jobs: &[(u32, u64)],
) -> Snapshot {
    Snapshot::Seen(Holdings {
        wallet: wallet.map(nw_isk),
        assets: assets.iter().copied().collect(),
        sell_orders: sell_orders.map(nw_isk),
        buy_escrow: buy_escrow.map(nw_isk),
        jobs_output: jobs.iter().copied().collect(),
    })
}

fn nw_record(at: i64, snapshot: Snapshot) -> Record {
    Record {
        character: CharacterKey::Id(MAIN),
        at,
        snapshot,
    }
}

fn nw_copy(at: i64, price: &str) -> PointPrices {
    PointPrices {
        at,
        published_at: Some(NW_PUBLISHED),
        rows: BTreeMap::from([((PAU, Side::Sell), nw_row(price))]),
    }
}

fn nw_store(env: &Env) -> Store {
    Store::open(&env.data.join("user.sqlite"), NW_NOW)
        .expect("the store opens")
        .store
}

/// The fixture's Forge market, which the history readings below go into.
fn nw_forge_id(store: &Store) -> i64 {
    store
        .history_markets()
        .expect("the markets read")
        .into_iter()
        .find(|market| market.kind == MarketKind::Region && market.place == u64::from(FORGE))
        .expect("the Forge market")
        .id
}

/// PAU's two readings on 2 Sep and on 9 Sep.
fn nw_history(store: &mut Store) {
    let market = nw_forge_id(store);
    store
        .record(
            market,
            11_567,
            &[
                (PAU, Side::Sell, nw_row("60,000.00")),
                (PAU, Side::Buy, nw_row("54,000.00")),
            ],
        )
        .expect("the first reading is written");
    store
        .record(
            market,
            11_574,
            &[
                (PAU, Side::Sell, nw_row("59,500.00")),
                (PAU, Side::Buy, nw_row("53,000.00")),
            ],
        )
        .expect("the second reading is written");
}

/// Main at the first point: a wallet and 160 PAU.
fn nw_add_first(store: &mut Store) {
    store
        .add_record(&nw_record(
            NW_FIRST,
            nw_seen(Some("1,000,000.00"), &[(PAU, 160)], None, None, &[]),
        ))
        .expect("the first record is written");
}

/// Main at the latest point: the whole scenario.
fn nw_add_latest(store: &mut Store) {
    store
        .add_record(&nw_record(
            NW_NOW,
            nw_seen(
                Some("1,234,567.89"),
                &[(PAU, 160)],
                Some("6,120,000.00"),
                Some("550,100.00"),
                &[(PAU, 10)],
            ),
        ))
        .expect("the latest record is written");
}

/// The store with the app's last game-data check at the Net worth clock.
fn nw_env() -> Env {
    let env = env();
    {
        let mut store = nw_store(&env);
        let mut settings = store.settings().expect("settings");
        settings.last_sde_check = Some(NW_NOW);
        store.save_settings(&settings).expect("settings saved");
    }
    env
}

fn nw_start(env: &Env) -> Harness<'static, App> {
    start(
        env,
        FakeEve::new(vec![character(MAIN, "Main")], None),
        NW_NOW,
    )
}

fn shown_starting(harness: &Harness<'_, App>, prefix: &str) -> bool {
    harness
        .root()
        .children_recursive()
        .filter_map(|node| node.accesskit_node().label())
        .any(|label| label.starts_with(prefix))
}

/// Every shown text in the frame, in drawing order.
fn labels(harness: &Harness<'_, App>) -> Vec<String> {
    harness
        .query_all_by_label_contains("")
        .filter_map(|node| {
            let node = node.accesskit_node();
            node.label().or_else(|| node.value())
        })
        .collect()
}

#[test]
fn the_today_net_worth_block_shows_the_total_its_change_and_the_basis() {
    let env = nw_env();
    {
        let mut store = nw_store(&env);
        nw_add_first(&mut store);
        nw_add_latest(&mut store);
        nw_history(&mut store);
        store
            .save_point_prices(&nw_copy(NW_NOW, "61,200.00"))
            .expect("the copy is saved");
    }
    let mut harness = nw_start(&env);
    loaded(&mut harness);

    assert!(shown(&harness, "18.31 M ISK"));
    let tip = crate::ui::hover(&mut harness, "18.31 M ISK");
    assert!(
        tip.iter()
            .any(|text| text == "18,308,667.89 ISK · Forge sell, 5% price · 9 Sep 01:30"),
        "{tip:?}"
    );
    assert!(shown(&harness, "+7.71 M"), "the change shows");
    assert!(shown(&harness, "since 2 Sep"), "the earlier day shows");
    assert!(
        shown(&harness, "Price moves +192 K, everything else +7.52 M"),
        "the change splits into price moves and everything else"
    );
    let change_hover =
        "7,708,667.89 ISK at Forge sell, 5% price, since 2001-09-02 01:46".to_owned();
    assert_eq!(
        crate::ui::hover(&mut harness, "+7.71 M"),
        vec![change_hover],
        "the change's hover names its amount, basis and earlier point"
    );
    assert_eq!(
        crate::ui::hover(&mut harness, "Price moves +192 K, everything else +7.52 M"),
        vec!["Price moves: the same items repriced at both points. Everything else: quantity changes, items that appeared or left, the wallet and PLEX.".to_owned()],
        "the split's hover says what each part counts"
    );
    let labels = labels(&harness);
    let position = |text: &str| {
        labels
            .iter()
            .position(|label| label == text)
            .unwrap_or_else(|| panic!("{text:?} is not shown: {labels:?}"))
    };
    assert!(position("18.31 M ISK") < position("+7.71 M"));
    assert!(position("+7.71 M") < position("since 2 Sep"));
    assert!(position("since 2 Sep") < position("Price moves +192 K, everything else +7.52 M"));
    assert_eq!(
        labels
            .iter()
            .filter(|label| label.starts_with("Price moves "))
            .count(),
        1,
        "the split is one new line: {labels:?}"
    );
    assert!(!shown(
        &harness,
        "No net worth points yet. Refresh your characters to record the first one."
    ));
}

#[test]
fn one_today_net_worth_point_has_no_change_and_names_the_unpriced_types() {
    let env = nw_env();
    {
        let mut store = nw_store(&env);
        store
            .add_record(&nw_record(
                NW_FIRST,
                nw_seen(Some("1,000,000.00"), &[(NW_RIFTER, 3)], None, None, &[]),
            ))
            .expect("the record is written");
    }
    let mut harness = nw_start(&env);
    loaded(&mut harness);

    assert!(shown(&harness, "1.00 M ISK"));
    assert!(
        !shown_starting(&harness, "since "),
        "a single point has no change line"
    );
    assert!(
        !shown_starting(&harness, "Price moves "),
        "a single point has no split line"
    );
    let total_tip = crate::ui::hover(&mut harness, "1.00 M ISK");
    assert!(
        total_tip
            .iter()
            .any(|text| text.starts_with("1,000,000.00 ISK · ")),
        "the total's hover is shown: {total_tip:?}"
    );
    assert!(
        !total_tip
            .iter()
            .any(|text| text.contains(" at Forge sell, 5% price, since ")),
        "a single point has no change hover: {total_tip:?}"
    );
    assert!(
        !total_tip
            .iter()
            .any(|text| text.starts_with("Price moves:")),
        "a single point has no split hover: {total_tip:?}"
    );
    let texts = labels(&harness);
    assert!(
        !texts
            .iter()
            .any(|text| text.contains(" at Forge sell, 5% price, since ")),
        "no change hover exists: {texts:?}"
    );
    assert!(
        !texts.iter().any(|text| text.starts_with("Price moves:")),
        "no split hover exists: {texts:?}"
    );
    assert!(shown(
        &harness,
        "1 item types have no price at this basis and are not counted."
    ));
}

#[test]
fn a_login_with_no_net_worth_records_says_so() {
    let env = nw_env();
    let mut harness = nw_start(&env);
    loaded(&mut harness);

    assert!(shown(
        &harness,
        "No net worth points yet. Refresh your characters to record the first one."
    ));
    assert!(
        !shown(&harness, "Net worth: Add character"),
        "a character without points offers no Add character"
    );
}

#[test]
fn stored_points_show_without_a_login() {
    let env = nw_env();
    {
        let mut store = nw_store(&env);
        nw_add_first(&mut store);
        nw_add_latest(&mut store);
        nw_history(&mut store);
        store
            .save_point_prices(&nw_copy(NW_NOW, "61,200.00"))
            .expect("the copy is saved");
    }
    let mut harness = start(&env, FakeEve::new(Vec::new(), None), NW_NOW);
    loaded(&mut harness);

    assert!(shown(&harness, "18.31 M ISK"));
    assert!(shown(&harness, "+7.71 M"), "the change shows");
    assert!(shown(&harness, "since 2 Sep"), "the earlier day shows");
    assert!(
        shown(&harness, "Price moves +192 K, everything else +7.52 M"),
        "the change splits into price moves and everything else"
    );
    assert_eq!(
        crate::ui::hover(&mut harness, "+7.71 M"),
        vec!["7,708,667.89 ISK at Forge sell, 5% price, since 2001-09-02 01:46".to_owned()],
        "the change's hover names its amount, basis and earlier point"
    );
    assert_eq!(
        crate::ui::hover(&mut harness, "Price moves +192 K, everything else +7.52 M"),
        vec!["Price moves: the same items repriced at both points. Everything else: quantity changes, items that appeared or left, the wallet and PLEX.".to_owned()],
        "the split's hover says what each part counts"
    );
    assert!(
        !shown(&harness, "Starts with your first character."),
        "stored points replace the empty copy"
    );
    assert_eq!(
        harness
            .query_all_by_label("Net worth: Add character")
            .count(),
        0,
        "stored points need no character"
    );
    crate::ui::snapshot(&mut harness, "today-money-no-login");
}

#[test]
fn no_characters_and_no_points_offers_add_character() {
    let env = nw_env();
    let mut harness = start(&env, FakeEve::new(Vec::new(), None), NW_NOW);
    loaded(&mut harness);

    assert!(shown(&harness, "Net worth"));
    assert!(shown(&harness, "Starts with your first character."));
    assert_eq!(
        harness
            .query_all_by_label("Net worth: Add character")
            .count(),
        1,
        "the empty block offers one Add character"
    );
    assert!(
        !shown(
            &harness,
            "No net worth points yet. Refresh your characters to record the first one."
        ),
        "the no-character copy replaces the refresh copy"
    );

    crate::ui::click(&mut harness, "Net worth: Add character");
    assert!(
        shown(&harness, "jEve data"),
        "the Add character row shows the Characters page"
    );
}

#[test]
fn a_missing_net_worth_settings_table_with_no_characters_shows_the_points() {
    let env = nw_env();
    {
        let mut store = nw_store(&env);
        nw_add_first(&mut store);
        nw_add_latest(&mut store);
        nw_history(&mut store);
        store
            .save_point_prices(&nw_copy(NW_NOW, "61,200.00"))
            .expect("the copy is saved");
    }
    Connection::open(env.data.join("user.sqlite"))
        .expect("the database opens")
        .execute_batch("PRAGMA foreign_keys = OFF; DROP TABLE net_worth_settings;")
        .expect("the settings table drops");

    let mut harness = start(&env, FakeEve::new(Vec::new(), None), NW_NOW);
    loaded(&mut harness);
    assert!(
        shown(&harness, "18.31 M ISK"),
        "the defaults still value the points"
    );
    let texts = notices(&mut harness);
    assert!(
        texts.iter().any(|text| {
            text.starts_with("Could not read the net worth settings: ")
                && text.contains("no such table: net_worth_settings")
        }),
        "{texts:?}"
    );
    assert!(
        !shown(&harness, "Net worth: Add character"),
        "a failed settings read with points draws no Add character row"
    );
}

#[test]
fn the_today_net_worth_block_reloads_after_the_page_was_hidden() {
    let env = nw_env();
    {
        let mut store = nw_store(&env);
        store
            .add_record(&nw_record(
                NW_YESTERDAY,
                nw_seen(Some("1,000,000.00"), &[(PAU, 160)], None, None, &[]),
            ))
            .expect("the first record is written");
        store
            .save_point_prices(&nw_copy(NW_YESTERDAY, "60,000.00"))
            .expect("the first copy is saved");
    }
    let mut harness = nw_start(&env);
    loaded(&mut harness);
    assert!(shown(&harness, "10.60 M ISK"));

    {
        let shared = harness.state_mut().shared_mut();
        shared
            .store
            .add_record(&nw_record(
                NW_NOW,
                nw_seen(Some("1,000,000.00"), &[(PAU, 170)], None, None, &[]),
            ))
            .expect("the latest record is written");
        shared
            .store
            .save_point_prices(&nw_copy(NW_NOW, "61,200.00"))
            .expect("the latest copy is saved");
    }
    rail(&mut harness, "Build");
    rail(&mut harness, "Today");

    assert!(shown(&harness, "11.40 M ISK"), "the changed total shows");
    assert!(shown(&harness, "+804 K"), "the change shows");
    assert!(shown(&harness, "since yesterday"));
    assert!(
        shown(&harness, "Price moves +192 K, everything else +612 K"),
        "the change splits into price moves and everything else"
    );
    assert_eq!(
        crate::ui::hover(&mut harness, "+804 K"),
        vec!["804,000.00 ISK at Forge sell, 5% price, since 2001-09-08 02:00".to_owned()],
        "the change's hover names yesterday's point"
    );
    assert_eq!(
        crate::ui::hover(&mut harness, "Price moves +192 K, everything else +612 K"),
        vec!["Price moves: the same items repriced at both points. Everything else: quantity changes, items that appeared or left, the wallet and PLEX.".to_owned()],
        "the split's hover says what each part counts"
    );
}

#[test]
fn a_price_read_failure_on_return_is_reported_with_its_cause() {
    let env = nw_env();
    {
        let mut store = nw_store(&env);
        nw_add_first(&mut store);
        store
            .save_point_prices(&nw_copy(NW_FIRST, "60,000.00"))
            .expect("the copy is saved");
    }
    let mut harness = nw_start(&env);
    loaded(&mut harness);
    assert!(shown(&harness, "10.60 M ISK"));

    Connection::open(env.data.join("user.sqlite"))
        .expect("the database opens")
        .execute_batch("PRAGMA foreign_keys = OFF; DROP TABLE point_prices;")
        .expect("the copy table drops");
    rail(&mut harness, "Build");
    rail(&mut harness, "Today");

    assert!(shown(&harness, "Could not read net worth."));
    let texts = notices(&mut harness);
    assert!(
        texts.iter().any(|text| {
            text.starts_with("Could not price net worth: ")
                && text.contains("no such table: point_prices")
        }),
        "{texts:?}"
    );
}

#[test]
fn a_missing_net_worth_settings_table_is_an_error_and_the_defaults_show() {
    let env = nw_env();
    {
        let mut store = nw_store(&env);
        nw_add_first(&mut store);
        nw_add_latest(&mut store);
        nw_history(&mut store);
        store
            .save_point_prices(&nw_copy(NW_NOW, "61,200.00"))
            .expect("the copy is saved");
    }
    Connection::open(env.data.join("user.sqlite"))
        .expect("the database opens")
        .execute_batch("PRAGMA foreign_keys = OFF; DROP TABLE net_worth_settings;")
        .expect("the settings table drops");

    let mut harness = nw_start(&env);
    loaded(&mut harness);
    assert!(
        shown(&harness, "18.31 M ISK"),
        "the defaults still value the points"
    );
    let texts = notices(&mut harness);
    assert!(
        texts.iter().any(|text| {
            text.starts_with("Could not read the net worth settings: ")
                && text.contains("no such table: net_worth_settings")
        }),
        "{texts:?}"
    );
    crate::ui::snapshot(&mut harness, "today-money");
}

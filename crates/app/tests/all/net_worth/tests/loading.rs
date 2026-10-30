use super::*;

// The loading tests below use M1-134's PAU (12,612), which the game-data fixture does not
// hold: the app-wide price and the type's own rules price it all the same.
pub(super) const LOAD_PAU: u32 = 12_612;
/// The first record's time: exactly 604,800 seconds before `NOW`.
pub(super) const FIRST_AT: i64 = NOW - 604_800;
/// The range's start: 30 days before `NOW`.
pub(super) const LOAD_FROM: i64 = NOW - 2_592_000;

pub(super) fn isk(text: &str) -> Money {
    Money::parse_isk(text).expect("a test price")
}

pub(super) fn load_row(price: &str) -> SummaryRow {
    let price = isk(price);
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

pub(super) fn all_parts() -> BTreeSet<Part> {
    Part::ALL.iter().copied().collect()
}

/// A store and the game data opened from the same temp folder.
pub(super) fn load_store() -> (TempDir, Store, Sde) {
    let dir = TempDir::new().expect("a temp folder");
    let root = dir.path().join("data");
    fs::create_dir_all(&root).expect("the data folder");
    import(&fixture(), &root.join("sde.sqlite")).expect("the fixture imports");
    let store = store_of(&root);
    let sde = Sde::open(&root.join("sde.sqlite")).expect("the game data opens");
    (dir, store, sde)
}

pub(super) fn load_seen(
    wallet: Option<&str>,
    assets: &[(u32, u64)],
    sell_orders: Option<&str>,
    buy_escrow: Option<&str>,
    jobs: &[(u32, u64)],
) -> Snapshot {
    Snapshot::Seen(Holdings {
        wallet: wallet.map(isk),
        assets: assets.iter().copied().collect(),
        sell_orders: sell_orders.map(isk),
        buy_escrow: buy_escrow.map(isk),
        jobs_output: jobs.iter().copied().collect(),
    })
}

pub(super) fn load_record(character: u64, at: i64, snapshot: Snapshot) -> Record {
    Record {
        character: CharacterKey::Id(character),
        at,
        snapshot,
    }
}

/// Main at the first point: a wallet and 160 PAU.
pub(super) fn add_main_first(store: &mut Store) {
    store
        .add_record(&load_record(
            MAIN,
            FIRST_AT,
            load_seen(Some("1,000,000.00"), &[(LOAD_PAU, 160)], None, None, &[]),
        ))
        .expect("the first record is written");
}

/// Main at the latest point: the whole scenario.
pub(super) fn add_main_latest(store: &mut Store) {
    store
        .add_record(&load_record(
            MAIN,
            NOW,
            load_seen(
                Some("1,234,567.89"),
                &[(LOAD_PAU, 160)],
                Some("6,120,000.00"),
                Some("550,100.00"),
                &[(LOAD_PAU, 10)],
            ),
        ))
        .expect("the latest record is written");
}

pub(super) fn add_alt(store: &mut Store, at: i64) {
    store
        .add_record(&load_record(
            ALT,
            at,
            load_seen(Some("500,000.00"), &[], None, None, &[]),
        ))
        .expect("the alt record is written");
}

/// PAU's Forge history: the two sell readings and the two buy readings of M1-134.
pub(super) fn seed_history(store: &mut Store) {
    let market = forge_id(store);
    store
        .record(
            market,
            11_567,
            &[
                (LOAD_PAU, Side::Sell, load_row("60,000.00")),
                (LOAD_PAU, Side::Buy, load_row("54,000.00")),
            ],
        )
        .expect("the first reading is written");
    store
        .record(
            market,
            11_574,
            &[
                (LOAD_PAU, Side::Sell, load_row("59,500.00")),
                (LOAD_PAU, Side::Buy, load_row("53,000.00")),
            ],
        )
        .expect("the second reading is written");
}

pub(super) fn forge_id(store: &Store) -> i64 {
    store
        .history_markets()
        .expect("the markets read")
        .into_iter()
        .find(|market| market.kind == MarketKind::Region && market.place == u64::from(FORGE))
        .expect("the Forge market")
        .id
}

/// The copy at the latest point: PAU's two sides, and PLEX's sell side when asked for.
pub(super) fn load_copy(with_plex: bool) -> PointPrices {
    let mut rows = BTreeMap::from([
        ((LOAD_PAU, Side::Sell), load_row("61,200.00")),
        ((LOAD_PAU, Side::Buy), load_row("55,010.00")),
    ]);
    if with_plex {
        rows.insert((PLEX, Side::Sell), load_row("5,200,000.00"));
    }
    PointPrices {
        at: NOW,
        published_at: Some(PUBLISHED),
        rows,
    }
}

pub(super) fn load_range(
    store: &mut Store,
    sde: &Sde,
    from: i64,
    left_out: &BTreeSet<CharacterKey>,
) -> Loaded {
    load(store, Some(sde), from, NOW, &all_parts(), true, left_out).expect("the points load")
}

#[test]
fn a_range_of_records_is_valued_at_each_points_own_prices() {
    let (_dir, mut store, sde) = load_store();
    add_main_first(&mut store);
    add_main_latest(&mut store);
    seed_history(&mut store);
    store
        .save_point_prices(&load_copy(false))
        .expect("the copy is saved");

    let loaded = load_range(&mut store, &sde, LOAD_FROM, &BTreeSet::new());
    assert_eq!(loaded.error, None);
    assert_eq!(loaded.points.len(), 2);

    let first = &loaded.points[0];
    assert_eq!(first.value.at, FIRST_AT);
    assert_eq!(first.total, isk("10,600,000.00"));
    assert_eq!(
        first.source,
        PriceSource::History {
            newest_day: Some(11_567)
        }
    );

    let latest = &loaded.points[1];
    assert_eq!(latest.value.at, NOW);
    assert_eq!(latest.total, isk("18,308,667.89"));
    assert_eq!(
        latest.source,
        PriceSource::Copy {
            published_at: Some(PUBLISHED)
        }
    );
    assert_eq!(latest.members.len(), 1);
    let main = &latest.members[&CharacterKey::Id(MAIN)];
    assert_eq!(main.parts[&Part::Wallet], isk("1,234,567.89"));
    assert_eq!(main.parts[&Part::Assets], isk("9,792,000.00"));
    assert_eq!(main.parts[&Part::SellOrders], isk("6,120,000.00"));
    assert_eq!(main.parts[&Part::BuyEscrow], isk("550,100.00"));
    assert_eq!(main.parts[&Part::IndustryJobs], isk("612,000.00"));
    assert_eq!(main.record_at, NOW);
    assert!(!main.imported);

    // A disabled market keeps its readings: the first point prices the same.
    let forge = forge_id(&store);
    store
        .set_history_enabled(forge, false)
        .expect("the market is disabled");
    let disabled = load_range(&mut store, &sde, LOAD_FROM, &BTreeSet::new());
    assert_eq!(disabled.points[0].total, isk("10,600,000.00"));
    assert_eq!(disabled.points[1].total, isk("18,308,667.89"));

    // Forgetting the market removes its history: the first point's PAU is unpriced.
    store.forget_market(forge).expect("the market is forgotten");
    let forgotten = load_range(&mut store, &sde, LOAD_FROM, &BTreeSet::new());
    assert_eq!(forgotten.points[0].total, isk("1,000,000.00"));
    assert_eq!(
        forgotten.points[0].unpriced,
        BTreeMap::from([(LOAD_PAU, 160)])
    );

    // A one-day range keeps only the latest point and has no earlier point to reprice.
    let one = load_range(&mut store, &sde, NOW - 86_400, &BTreeSet::new());
    assert_eq!(one.points.len(), 1);
    assert_eq!(one.points[0].value.at, NOW);
    assert_eq!(one.previous_at_latest_prices, None);
}

#[test]
fn the_rules_and_value_price_of_each_points_day_price_it() {
    let (dir, mut store, sde) = load_store();
    add_main_first(&mut store);
    add_main_latest(&mut store);
    seed_history(&mut store);
    store
        .save_point_prices(&load_copy(false))
        .expect("the copy is saved");

    store
        .set_price_rule(
            RuleTarget::Item(LOAD_PAU),
            Some(RuleValue::Fixed(isk("70,000.00"))),
            NOW - 86_400,
        )
        .expect("the fixed rule is set");
    let loaded = load_range(&mut store, &sde, LOAD_FROM, &BTreeSet::new());
    assert_eq!(loaded.points[0].total, isk("10,600,000.00"));
    assert_eq!(loaded.points[1].total, isk("19,804,667.89"));

    store
        .set_price_rule(RuleTarget::Item(LOAD_PAU), None, NOW - 3_600)
        .expect("the rule is removed");
    let loaded = load_range(&mut store, &sde, LOAD_FROM, &BTreeSet::new());
    assert_eq!(loaded.points[1].total, isk("18,308,667.89"));

    store
        .set_value_price(PriceRule::BuyFivePercent)
        .expect("the app-wide price is set");
    let loaded = load_range(&mut store, &sde, LOAD_FROM, &BTreeSet::new());
    assert_eq!(loaded.points[0].total, isk("9,640,000.00"));
    assert_eq!(loaded.points[1].total, isk("17,256,367.89"));

    // A failed price read names the error and values every point with no prices at all.
    let db = dir.path().join("data").join("user.sqlite");
    rusqlite::Connection::open(&db)
        .expect("the database opens")
        .execute_batch("PRAGMA foreign_keys = OFF; DROP TABLE point_prices;")
        .expect("the copy table drops");
    let loaded = load_range(&mut store, &sde, LOAD_FROM, &BTreeSet::new());
    assert_eq!(
        loaded.error.as_deref(),
        Some("Could not read the prices: database error: no such table: point_prices")
    );
    assert_eq!(loaded.points[1].total, isk("7,904,667.89"));
    assert_eq!(loaded.points[1].unpriced, BTreeMap::from([(LOAD_PAU, 170)]));
    assert_eq!(loaded.points[0].total, isk("1,000,000.00"));
    assert_eq!(loaded.points[0].unpriced, BTreeMap::from([(LOAD_PAU, 160)]));
}

#[test]
fn each_points_plex_count_is_priced_at_its_own_moment() {
    let (_dir, mut store, sde) = load_store();
    add_main_first(&mut store);
    add_main_latest(&mut store);
    seed_history(&mut store);
    store
        .save_point_prices(&load_copy(true))
        .expect("the copy is saved");
    let a = store.add_account("A").expect("the first account");
    let b = store.add_account("B").expect("the second account");
    store.set_plex(a, 500, FIRST_AT).expect("A's count");
    store.set_plex(b, 650, NOW - 100).expect("B's count");

    let loaded = load_range(&mut store, &sde, LOAD_FROM, &BTreeSet::new());
    let first = &loaded.points[0];
    assert_eq!(first.plex_units, 500);
    assert_eq!(first.plex_value, Money::ZERO);
    assert_eq!(first.unpriced, BTreeMap::from([(PLEX, 500)]));
    assert_eq!(first.total, isk("10,600,000.00"));

    let latest = &loaded.points[1];
    assert_eq!(latest.plex_units, 1_150);
    assert_eq!(latest.plex_value, isk("5,980,000,000.00"));
    assert_eq!(latest.total, isk("5,998,308,667.89"));

    // A copy holding only PLEX at the first point prices only the vault.
    let plex_copy = PointPrices {
        at: FIRST_AT,
        published_at: None,
        rows: BTreeMap::from([((PLEX, Side::Sell), load_row("5,200,000.00"))]),
    };
    store
        .save_point_prices(&plex_copy)
        .expect("the PLEX-only copy is saved");
    let loaded = load_range(&mut store, &sde, LOAD_FROM, &BTreeSet::new());
    assert_eq!(loaded.points[0].plex_value, isk("2,600,000,000.00"));
    assert_eq!(loaded.points[0].total, isk("2,610,600,000.00"));

    let previous = loaded
        .previous_at_latest_prices
        .as_ref()
        .expect("the point before the last");
    assert_eq!(previous.value.at, FIRST_AT);
    assert_eq!(previous.total, isk("2,610,792,000.00"));
}

#[test]
fn a_left_out_character_goes_only_once_its_data_is_a_week_old() {
    let (_dir, mut store, sde) = load_store();
    add_main_first(&mut store);
    add_main_latest(&mut store);
    add_alt(&mut store, FIRST_AT);
    seed_history(&mut store);
    store
        .save_point_prices(&load_copy(false))
        .expect("the copy is saved");

    let loaded = load_range(&mut store, &sde, LOAD_FROM, &BTreeSet::new());
    assert_eq!(loaded.points[0].total, isk("11,100,000.00"));
    assert_eq!(loaded.points[1].total, isk("18,808,667.89"));

    let left_out = BTreeSet::from([CharacterKey::Id(ALT)]);
    let loaded = load_range(&mut store, &sde, LOAD_FROM, &left_out);
    // Alt is 0 seconds old at the first point, so it stays there.
    assert_eq!(loaded.points[0].total, isk("11,100,000.00"));
    // At the latest point it is exactly 604,800 seconds old, so it goes.
    assert_eq!(loaded.points[1].total, isk("18,308,667.89"));
    assert_eq!(loaded.points[1].members.len(), 1);
    assert!(
        loaded.points[1]
            .members
            .contains_key(&CharacterKey::Id(MAIN))
    );
    assert_eq!(
        loaded
            .previous_at_latest_prices
            .as_ref()
            .expect("the point before the last")
            .total,
        isk("11,292,000.00")
    );
}

#[test]
fn a_fresher_left_out_character_still_counts() {
    let (_dir, mut store, sde) = load_store();
    add_main_first(&mut store);
    add_main_latest(&mut store);
    add_alt(&mut store, FIRST_AT + 1);
    seed_history(&mut store);
    store
        .save_point_prices(&load_copy(false))
        .expect("the copy is saved");

    let left_out = BTreeSet::from([CharacterKey::Id(ALT)]);
    let loaded = load_range(&mut store, &sde, LOAD_FROM, &left_out);
    assert_eq!(
        loaded.points.last().expect("the latest point").total,
        isk("18,808,667.89")
    );
}

// The controls tests below open the Net worth page by its rail label. The controls save at
// once and change nothing that is stored: no record and no price copy moves.

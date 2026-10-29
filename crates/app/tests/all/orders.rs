//! Acceptance tests for the Orders page: the rows of every login character's market orders, the
//! Forge best price beside them, the footer and the chips. The clock is fixed at 1,000,000.
#[cfg(test)]
mod tests {

    use std::collections::BTreeSet;
    use std::fs;
    use std::io::Write;
    use std::ops::ControlFlow;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex, PoisonError};

    use crate::ui::{click, coloured, hover, on_row, shown, snapshot, top, until};
    use egui::vec2;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;
    use eve_means_of_profit::build::prices::Prices;
    use eve_means_of_profit::session::{EveAccess, PendingLogin};
    use eve_means_of_profit::{App, AppConfig};
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use mop_core::money::Money;
    use mop_esi::character_data::{CharacterData, Fetched, Kind, Order, Refreshed};
    use mop_esi::characters::{Character, Login};
    use mop_esi::error::EsiError;
    use mop_esi::places::{NamingReport, StructureNames};
    use mop_esi::saved;
    use mop_market::error::MarketError;
    use mop_market::fuzzwork::{BytesHttp, BytesResponse};
    use mop_sde::{Fetch, FetchError, import};
    use mop_store::Store;
    use tempfile::TempDir;

    const NOW: i64 = 1_000_000;
    const FIXTURE: &str = "../sde/tests/fixtures/sde-3607705";
    const MAIN: u64 = 2_112_000_001;
    const JITA_4_4: i64 = 60_003_760;
    const HOME_FORT: i64 = 1_035_466_617_946;
    const PAU: u32 = 11_688;
    const MORPHITE: u32 = 11_399;
    const HEADER: &str =
        "what,weightedaverage,maxval,minval,stddev,median,volume,numorders,fivepercent,orderSet";
    const EMPTY: &str = "No market orders. Refresh on the Characters page to fetch them.";

    fn fixture() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE)
    }

    /// The game data is installed by the test, so the app never asks the network for anything.
    struct NoNetwork;

    impl Fetch for NoNetwork {
        fn get_text(&self, _url: &str) -> Result<String, FetchError> {
            Err(FetchError::Http { status: 503 })
        }

        fn download(
            &self,
            _url: &str,
            _dest: &mut dyn Write,
            _progress: &mut dyn FnMut(u64, Option<u64>) -> ControlFlow<()>,
        ) -> Result<u64, FetchError> {
            Err(FetchError::Http { status: 503 })
        }
    }

    /// The Forge's summary file: PAU sells cheapest at 61,200.00, Morphite is bought highest at
    /// 2,150.00. Counts the requests, so a test can see that the page asks for nothing.
    struct FakeForge {
        requests: Arc<Mutex<usize>>,
    }

    impl BytesHttp for FakeForge {
        fn get_bytes(
            &self,
            _url: &str,
            _if_none_match: Option<&str>,
        ) -> Result<BytesResponse, MarketError> {
            *self.requests.lock().unwrap_or_else(PoisonError::into_inner) += 1;
            let text = format!(
                "{HEADER}\n\
                 10000002|{PAU}|false,61500.00,65000.00,61200.00,0.0,62000.00,500,3,61300.00,175414\n\
                 10000002|{MORPHITE}|true,2100.00,2150.00,2000.00,0.0,2050.00,40,2,2100.00,175414\n"
            );
            let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
            encoder
                .write_all(text.as_bytes())
                .map_err(|error| MarketError::Network(error.to_string()))?;
            let body = encoder
                .finish()
                .map_err(|error| MarketError::Network(error.to_string()))?;
            Ok(BytesResponse {
                status: 200,
                etag: Some("\"1\"".to_owned()),
                last_modified: Some("Mon, 12 Jan 1970 13:46:40 GMT".to_owned()),
                retry_after: None,
                body,
            })
        }
    }

    /// A data folder with the game data imported and the user's store, with Home Fort named.
    struct Env {
        _dir: TempDir,
        data: PathBuf,
        cache: PathBuf,
    }

    fn env() -> Env {
        let dir = TempDir::new().expect("a temp folder");
        let data = dir.path().join("data");
        let cache = dir.path().join("cache");
        fs::create_dir_all(data.join("esi")).expect("the data folder");
        import(&fixture(), &data.join("sde.sqlite")).expect("the fixture imports");
        let mut store = Store::open(&data.join("user.sqlite"), NOW)
            .expect("the store opens")
            .store;
        let mut settings = store.settings().expect("settings");
        settings.last_sde_check = Some(NOW);
        store.save_settings(&settings).expect("settings saved");
        // The background price refresh stays off: the test asks for each download itself.
        let mut history = store.history_settings().expect("history settings");
        history.auto_refresh = false;
        store
            .save_history_settings(&history)
            .expect("history settings saved");
        let places = format!(
            "{{\"version\":1,\"structures\":{{\"{HOME_FORT}\":{{\"name\":\"Home Fort\",\"system_id\":null,\"refused\":false,\"checked_at\":900000}}}}}}"
        );
        fs::write(data.join("esi").join("places.json"), places).expect("places written");
        Env {
            _dir: dir,
            data,
            cache,
        }
    }

    fn esi_dir(env: &Env) -> PathBuf {
        env.data.join("esi")
    }

    /// A login with the characters given and a count of refreshes that nothing here makes.
    #[derive(Clone)]
    struct FakeEve {
        characters: Vec<Character>,
    }

    impl EveAccess for FakeEve {
        fn characters(&self) -> Vec<Character> {
            self.characters.clone()
        }

        fn begin_login(&mut self) -> Result<PendingLogin, String> {
            Err("Logins are not started in these tests.".to_owned())
        }

        fn finish_login(&mut self, _code: &str) -> Result<Character, EsiError> {
            Err(EsiError::Cancelled)
        }

        fn refresh_all(
            &mut self,
            _data: &mut CharacterData,
            _now: i64,
        ) -> Vec<(Kind, Result<Refreshed, EsiError>)> {
            Vec::new()
        }

        fn name_structures(
            &mut self,
            _names: &mut StructureNames,
            _ids: &[i64],
            _now: i64,
        ) -> Result<NamingReport, EsiError> {
            Ok(NamingReport {
                asked: 0,
                failed: Vec::new(),
            })
        }

        fn save_character(&mut self, _data: &CharacterData) -> Result<(), EsiError> {
            Ok(())
        }

        fn remove(&mut self, character_id: u64) -> Result<(), EsiError> {
            Err(EsiError::UnknownCharacter(character_id))
        }

        fn keychain_warning(&self) -> Option<String> {
            None
        }

        fn jeve_folder(&self) -> Option<PathBuf> {
            None
        }
    }

    fn character(id: u64, name: &str) -> Character {
        Character {
            id,
            name: name.to_owned(),
            scopes: BTreeSet::new(),
            added_at: NOW,
            login: Login::Ok,
        }
    }

    fn order(
        order_id: i64,
        type_id: u32,
        location_id: i64,
        is_buy: bool,
        price: Money,
        (remain, total): (u64, u64),
        expires: i64,
    ) -> Order {
        Order {
            order_id,
            type_id,
            location_id,
            region_id: 10_000_002,
            is_buy,
            price,
            volume_remain: remain,
            volume_total: total,
            issued: 900_000,
            expires,
            range: "station".to_owned(),
            escrow: is_buy.then(|| price.checked_mul_qty(total).expect("an escrow that fits")),
            min_volume: None,
            is_corporation: false,
        }
    }

    /// Main's orders, fetched at `NOW - 120`: 100 of 200 PAU for sale, 10 of 10 Morphite bought
    /// (expired), and 50 of 50 PAU for sale by the corporation.
    fn main_data(fetched: i64) -> CharacterData {
        let mut corp = order(
            3,
            PAU,
            JITA_4_4,
            false,
            Money::from_ten_thousandths(700_000_000),
            (50, 50),
            NOW + 260_000,
        );
        corp.is_corporation = true;
        let orders = vec![
            order(
                1,
                PAU,
                JITA_4_4,
                false,
                Money::from_ten_thousandths(615_000_000),
                (100, 200),
                NOW + 260_000,
            ),
            order(
                2,
                MORPHITE,
                JITA_4_4,
                true,
                Money::from_ten_thousandths(21_000_000),
                (10, 10),
                NOW - 5,
            ),
            corp,
        ];
        CharacterData {
            character_id: MAIN,
            assets: None,
            orders: Some(Fetched {
                data: orders,
                fetched_at: fetched,
                last_modified: None,
                expires: None,
            }),
            jobs: None,
            wallet: None,
        }
    }

    /// A character's single order for 100 of 200 PAU for sale, fetched at `fetched_at`.
    fn data_fetched_at(character_id: u64, order_id: i64, fetched_at: i64) -> CharacterData {
        CharacterData {
            character_id,
            assets: None,
            orders: Some(Fetched {
                data: vec![order(
                    order_id,
                    PAU,
                    JITA_4_4,
                    false,
                    Money::from_ten_thousandths(615_000_000),
                    (100, 200),
                    NOW + 260_000,
                )],
                fetched_at,
                last_modified: None,
                expires: None,
            }),
            jobs: None,
            wallet: None,
        }
    }

    fn start(
        env: &Env,
        characters: Vec<Character>,
        requests: Arc<Mutex<usize>>,
    ) -> Harness<'static, App> {
        let config = AppConfig {
            data_dir: env.data.clone(),
            cache_dir: env.cache.clone(),
            fetch: Box::new(NoNetwork),
            clock: Box::new(|| NOW),
            eve: Box::new(FakeEve { characters }),
            market: Box::new(crate::offline::Offline),
        };
        let mut harness = Harness::builder()
            .with_size(vec2(1280.0, 800.0))
            .with_max_steps(crate::SETTLE_STEPS)
            .build_eframe(move |cc| App::new(&cc.egui_ctx, config).expect("the app starts"));
        crate::ui::no_images(&mut harness);
        harness.state_mut().shared_mut().market =
            Arc::new(Prices::new(Box::new(FakeForge { requests })));
        harness
            .state_mut()
            .show_page(eve_means_of_profit::rail::Page::Build);
        harness
    }

    /// Runs frames until the start-up read has arrived: the page's loading line is gone.
    fn loaded(harness: &mut Harness<'_, App>) {
        until(harness, "the saved data", |h| {
            !shown(h, "Loading character data…")
        });
    }

    /// Asks the Forge for its file and runs frames until the file is held.
    fn hold_forge(harness: &mut Harness<'_, App>) {
        let market = {
            let shared = harness.state_mut().shared_mut();
            let market = Arc::clone(&shared.market);
            market.refresh(&shared.worker, NOW, true);
            market
        };
        until(harness, "the Forge file", |_| {
            market.published_at().is_some()
        });
    }

    #[test]
    fn the_rows_show_each_order_against_the_forge_best() {
        let env = env();
        saved::save_character(&esi_dir(&env), &main_data(NOW - 120)).expect("Main's orders saved");
        let requests = Arc::new(Mutex::new(0));
        let mut harness = start(&env, vec![character(MAIN, "Main")], Arc::clone(&requests));
        loaded(&mut harness);
        hold_forge(&mut harness);
        click(&mut harness, "Orders");

        assert!(shown(&harness, "Sell"));
        assert!(shown(&harness, "Sell (corp)"));
        assert!(shown(&harness, "Buy"));
        assert!(shown(&harness, "100 / 200"));
        assert!(shown(&harness, "50 / 50"));
        assert!(shown(&harness, "61,500.00"));
        assert!(shown(&harness, "70,000.00"));
        assert!(shown(&harness, "2,100.00"));
        assert!(shown(&harness, "in 3d"));
        assert!(shown(&harness, "expired"));
        assert!(shown(&harness, "61,200.00"));
        assert!(shown(&harness, "2,150.00"));
        assert!(shown(&harness, "1970-01-11 10:00"));
        assert!(shown(&harness, "2m"));
        assert!(shown(&harness, "Main"));
    }

    #[test]
    fn the_age_is_plain_until_six_hours_then_amber_and_old_past_a_day() {
        const ALT: u64 = 2_112_000_002;
        const THIRD: u64 = 2_112_000_003;
        let env = env();
        saved::save_character(&esi_dir(&env), &data_fetched_at(MAIN, 1, NOW - 7_200))
            .expect("Main's order saved");
        saved::save_character(&esi_dir(&env), &data_fetched_at(ALT, 2, NOW - 25_200))
            .expect("Alt's order saved");
        saved::save_character(&esi_dir(&env), &data_fetched_at(THIRD, 3, NOW - 90_000))
            .expect("Third's order saved");
        let requests = Arc::new(Mutex::new(0));
        let mut harness = start(
            &env,
            vec![
                character(MAIN, "Main"),
                character(ALT, "Alt"),
                character(THIRD, "Third"),
            ],
            requests,
        );
        loaded(&mut harness);
        click(&mut harness, "Orders");

        let two_hours = harness
            .get_all_by_label("2h")
            .next()
            .expect("the 2h age")
            .rect();
        assert!(!coloured(&mut harness, two_hours));
        let seven_hours = harness
            .get_all_by_label("7h")
            .next()
            .expect("the 7h age")
            .rect();
        assert!(coloured(&mut harness, seven_hours));
        let a_day = harness
            .get_all_by_label("1d old")
            .next()
            .expect("the 1d old age")
            .rect();
        assert!(coloured(&mut harness, a_day));
    }

    #[test]
    fn an_age_over_a_day_says_to_refresh_on_hover() {
        let env = env();
        saved::save_character(&esi_dir(&env), &data_fetched_at(MAIN, 1, NOW - 90_000))
            .expect("Main's order saved");
        let requests = Arc::new(Mutex::new(0));
        let mut harness = start(&env, vec![character(MAIN, "Main")], requests);
        loaded(&mut harness);
        click(&mut harness, "Orders");

        assert_eq!(
            hover(&mut harness, "1d old"),
            vec!["Over a day old. Refresh before you rely on it."]
        );
    }

    #[test]
    fn an_age_under_a_day_has_no_hover() {
        let env = env();
        saved::save_character(&esi_dir(&env), &data_fetched_at(MAIN, 1, NOW - 25_200))
            .expect("Main's order saved");
        let requests = Arc::new(Mutex::new(0));
        let mut harness = start(&env, vec![character(MAIN, "Main")], requests);
        loaded(&mut harness);
        click(&mut harness, "Orders");

        assert!(hover(&mut harness, "7h").is_empty());
    }

    #[test]
    fn each_row_holds_its_own_values_and_the_sells_come_first() {
        let env = env();
        saved::save_character(&esi_dir(&env), &main_data(NOW - 120)).expect("Main's orders saved");
        let requests = Arc::new(Mutex::new(0));
        let mut harness = start(&env, vec![character(MAIN, "Main")], requests);
        loaded(&mut harness);
        hold_forge(&mut harness);
        click(&mut harness, "Orders");

        let pau = "Particle Accelerator Unit";
        assert!(top(&harness, pau) < top(&harness, "Morphite"));
        assert!(on_row(&harness, pau, &["100 / 200"]));
        assert!(on_row(&harness, pau, &["61,500.00"]));
        assert!(on_row(&harness, pau, &["in 3d"]));
        assert!(on_row(&harness, pau, &["61,200.00"]));
        assert!(on_row(&harness, pau, &["2m"]));
        assert!(on_row(&harness, pau, &["Main"]));
        assert!(on_row(&harness, "Morphite", &["10 / 10"]));
        assert!(on_row(&harness, "Morphite", &["2,100.00"]));
        assert!(on_row(&harness, "Morphite", &["expired"]));
        assert!(on_row(&harness, "Morphite", &["2,150.00"]));
        assert!(on_row(&harness, "Morphite", &["1970-01-11 10:00"]));
        assert!(on_row(&harness, "Sell (corp)", &["50 / 50"]));
    }

    #[test]
    fn a_beaten_buy_says_by_how_much_on_hover() {
        let env = env();
        saved::save_character(&esi_dir(&env), &main_data(NOW - 120)).expect("Main's orders saved");
        let requests = Arc::new(Mutex::new(0));
        let mut harness = start(&env, vec![character(MAIN, "Main")], requests);
        loaded(&mut harness);
        hold_forge(&mut harness);
        click(&mut harness, "Orders");

        assert_eq!(hover(&mut harness, "2,150.00"), vec!["Beaten by 50.00"]);
    }

    #[test]
    fn a_beaten_forge_best_is_amber() {
        let env = env();
        saved::save_character(&esi_dir(&env), &main_data(NOW - 120)).expect("Main's orders saved");
        let requests = Arc::new(Mutex::new(0));
        let mut harness = start(&env, vec![character(MAIN, "Main")], requests);
        loaded(&mut harness);
        hold_forge(&mut harness);
        click(&mut harness, "Orders");

        let area = harness
            .get_all_by_label("2,150.00")
            .next()
            .expect("the Forge best cell of the buy order")
            .rect();
        assert!(coloured(&mut harness, area));
    }

    #[test]
    fn a_beaten_order_says_by_how_much_on_hover() {
        let env = env();
        saved::save_character(&esi_dir(&env), &main_data(NOW - 120)).expect("Main's orders saved");
        let requests = Arc::new(Mutex::new(0));
        let mut harness = start(&env, vec![character(MAIN, "Main")], requests);
        loaded(&mut harness);
        hold_forge(&mut harness);
        click(&mut harness, "Orders");

        assert_eq!(hover(&mut harness, "61,200.00"), vec!["Beaten by 300.00"]);
    }

    #[test]
    fn the_footer_sums_the_shown_sells_and_escrows() {
        let env = env();
        saved::save_character(&esi_dir(&env), &main_data(NOW - 120)).expect("Main's orders saved");
        let requests = Arc::new(Mutex::new(0));
        let mut harness = start(&env, vec![character(MAIN, "Main")], requests);
        loaded(&mut harness);
        hold_forge(&mut harness);
        click(&mut harness, "Orders");

        assert!(shown(
            &harness,
            "3 orders · sell 9,650,000.00 · escrow 21,000.00"
        ));
        harness
            .get_all_by_label("Buy")
            .next()
            .expect("the Buy chip")
            .click();
        harness.run();
        assert!(shown(
            &harness,
            "2 orders · sell 9,650,000.00 · escrow 0.00"
        ));
    }

    #[test]
    fn a_sell_priced_under_the_forge_best_is_not_beaten() {
        let env = env();
        let mut data = main_data(NOW - 120);
        if let Some(fetched) = data.orders.as_mut() {
            fetched.data = vec![order(
                4,
                PAU,
                JITA_4_4,
                false,
                Money::from_ten_thousandths(610_000_000),
                (100, 100),
                NOW + 260_000,
            )];
        }
        saved::save_character(&esi_dir(&env), &data).expect("the order saved");
        let requests = Arc::new(Mutex::new(0));
        let mut harness = start(&env, vec![character(MAIN, "Main")], requests);
        loaded(&mut harness);
        hold_forge(&mut harness);
        click(&mut harness, "Orders");

        assert!(shown(&harness, "61,000.00"));
        let area = harness
            .get_all_by_label("61,200.00")
            .next()
            .expect("the Forge best cell")
            .rect();
        assert!(!coloured(&mut harness, area));
        assert!(hover(&mut harness, "61,200.00").is_empty());
    }

    #[test]
    fn without_a_forge_file_the_best_is_a_dash_and_nothing_is_asked() {
        let env = env();
        saved::save_character(&esi_dir(&env), &main_data(NOW - 120)).expect("Main's orders saved");
        let requests = Arc::new(Mutex::new(0));
        let mut harness = start(&env, vec![character(MAIN, "Main")], Arc::clone(&requests));
        loaded(&mut harness);
        click(&mut harness, "Orders");
        for _ in 0..20 {
            harness.run();
        }

        assert!(shown(
            &harness,
            "3 orders · sell 9,650,000.00 · escrow 21,000.00"
        ));
        assert_eq!(*requests.lock().unwrap_or_else(PoisonError::into_inner), 0);
        assert!(!shown(&harness, "61,200.00"));
        assert!(on_row(&harness, "Particle Accelerator Unit", &["—"]));
    }

    #[test]
    fn a_click_on_a_row_picks_its_item() {
        let env = env();
        saved::save_character(&esi_dir(&env), &main_data(NOW - 120)).expect("Main's orders saved");
        let requests = Arc::new(Mutex::new(0));
        let mut harness = start(&env, vec![character(MAIN, "Main")], requests);
        loaded(&mut harness);
        click(&mut harness, "Orders");

        harness
            .get_all_by_label("100 / 200")
            .next()
            .expect("the Left cell of the PAU sell order")
            .click();
        harness.run();
        assert_eq!(harness.state_mut().shared_mut().selection.item, Some(PAU));
    }

    #[test]
    fn with_no_characters_the_page_says_so() {
        let env = env();
        let requests = Arc::new(Mutex::new(0));
        let mut harness = start(&env, Vec::new(), requests);
        loaded(&mut harness);
        click(&mut harness, "Orders");

        assert!(shown(&harness, EMPTY));
    }

    #[test]
    fn the_orders_page_matches_its_snapshot() {
        let env = env();
        saved::save_character(&esi_dir(&env), &main_data(NOW - 120)).expect("Main's orders saved");
        let requests = Arc::new(Mutex::new(0));
        let mut harness = start(&env, vec![character(MAIN, "Main")], requests);
        loaded(&mut harness);
        hold_forge(&mut harness);
        click(&mut harness, "Orders");
        snapshot(&mut harness, "orders-page");
    }
}

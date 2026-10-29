//! Acceptance tests for the recording of net worth at each refresh: what a character held, the
//! sums of its orders, and the copy of the prices of the latest point. The clock is fixed at
//! 1,000,000,000. The login and the market are fakes, and the app's data is in a temp folder.
#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;
    use std::io::Write;
    use std::ops::ControlFlow;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    use egui::vec2;
    use egui_kittest::Harness;
    use eve_means_of_profit::build::prices::Prices;
    use eve_means_of_profit::session::{EveAccess, PendingLogin, Saved};
    use eve_means_of_profit::{App, AppConfig};
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use mop_core::money::Money;
    use mop_core::net_worth::{CharacterKey, Holdings, Record, Snapshot};
    use mop_core::stock::{Asset, CharacterAssets};
    use mop_esi::character_data::{
        CharacterData, Fetched, Job, JobStatus, Kind, Order, OwnedAssets, Refreshed,
    };
    use mop_esi::characters::{Character, Login};
    use mop_esi::error::EsiError;
    use mop_esi::places::{NamingReport, StructureNames};
    use mop_market::error::MarketError;
    use mop_market::fuzzwork::{BytesHttp, BytesResponse, Side};
    use mop_sde::{Fetch, FetchError, Sde, import};
    use mop_store::Store;
    use tempfile::TempDir;

    const NOW: i64 = 1_000_000_000;
    const MAIN: u64 = 2_112_000_001;
    const ALT: u64 = 2_112_000_002;
    const PAU: u32 = 11_688;
    const TRITANIUM: u32 = 34;
    /// The Rifter Blueprint: a type in the blueprint category (9) of the fixture.
    const BLUEPRINT: u32 = 691;
    const JITA_4_4: i64 = 60_003_760;
    const HOME_FORT: i64 = 1_035_466_617_946;
    const FIXTURE: &str = "../sde/tests/fixtures/sde-3607705";
    const HEADER: &str =
        "what,weightedaverage,maxval,minval,stddev,median,volume,numorders,fivepercent,orderSet";
    /// The held download's Last-Modified time: `Sun, 09 Sep 2001 01:30:00 GMT`.
    const PUBLISHED: i64 = 999_999_000;
    const PUBLISHED_HEADER: &str = "Sun, 09 Sep 2001 01:30:00 GMT";

    /// The game data is not needed here, so the app never asks the network for anything.
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

    /// A market that answers with no rows.
    #[derive(Clone)]
    struct EmptyForge;

    impl BytesHttp for EmptyForge {
        fn get_bytes(
            &self,
            _url: &str,
            _if_none_match: Option<&str>,
        ) -> Result<BytesResponse, MarketError> {
            Err(MarketError::Network("no market in these tests".to_owned()))
        }
    }

    /// A login with the one character given.
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

    fn store_of(data: &Path) -> Store {
        Store::open(&data.join("user.sqlite"), NOW)
            .expect("the store opens")
            .store
    }

    /// Opens the app with Main saved and the login given, and runs the frames that load it. The
    /// market answers with no rows, so no download is held.
    fn start(data: &TempDir) -> Harness<'static, App> {
        start_with(data, Box::new(EmptyForge))
    }

    /// As `start`, with no login character at start.
    fn start_without_characters(data: &TempDir) -> Harness<'static, App> {
        start_with_characters(data, Box::new(EmptyForge), Vec::new())
    }

    /// As `start`, with the market given.
    fn start_with(data: &TempDir, forge: Box<dyn BytesHttp + Send>) -> Harness<'static, App> {
        start_with_characters(data, forge, vec![character(MAIN, "Main")])
    }

    /// As `start_with`, with the login characters given.
    fn start_with_characters(
        data: &TempDir,
        forge: Box<dyn BytesHttp + Send>,
        characters: Vec<Character>,
    ) -> Harness<'static, App> {
        let root = data.path().join("data");
        let cache = data.path().join("cache");
        fs::create_dir_all(root.join("esi")).expect("the data folder");
        import(&fixture(), &root.join("sde.sqlite")).expect("the fixture imports");
        {
            let mut store = store_of(&root);
            let mut settings = store.settings().expect("settings");
            settings.last_sde_check = Some(NOW);
            store.save_settings(&settings).expect("settings saved");
        }
        let loaded = characters.clone();
        let config = AppConfig {
            data_dir: root,
            cache_dir: cache,
            fetch: Box::new(NoNetwork),
            clock: Box::new(|| NOW),
            eve: Box::new(FakeEve { characters }),
            market: Box::new(crate::offline::Offline),
        };
        let mut harness = Harness::builder()
            .with_size(vec2(1280.0, 800.0))
            .build_eframe(move |cc| App::new(&cc.egui_ctx, config).expect("the app starts"));
        harness.state_mut().shared_mut().market = Arc::new(Prices::new(forge));
        // The login characters are already loaded at the first frame, so the first frame sees
        // them as the baseline. The start-up read that arrives later holds the same characters
        // with no data, so it changes no revision and records nothing.
        harness.state_mut().shared_mut().session.set_saved(Saved {
            characters: loaded,
            data: Vec::new(),
            structures: BTreeMap::new(),
        });
        harness.run();
        harness.run();
        // Waits for the start-up read to be applied, so no later read replaces a test's data.
        for _ in 0..500 {
            if harness.state_mut().shared_mut().session.loaded() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
            harness.run();
        }
        assert!(
            harness.state_mut().shared_mut().session.loaded(),
            "the start-up read arrives"
        );
        harness
    }

    fn fixture() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE)
    }

    /// The held Fuzzwork file: a Sell and a Buy row of PAU and a Sell row of Tritanium.
    struct FileForge;

    impl BytesHttp for FileForge {
        fn get_bytes(
            &self,
            _url: &str,
            _if_none_match: Option<&str>,
        ) -> Result<BytesResponse, MarketError> {
            let text = format!(
                "{HEADER}\n{}{}{}",
                row(PAU, false, "61200.00"),
                row(PAU, true, "55010.00"),
                row(TRITANIUM, false, "5.10"),
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
                last_modified: Some(PUBLISHED_HEADER.to_owned()),
                retry_after: None,
                body,
            })
        }
    }

    /// One row of the file: a side (`true` is buy) with its prices in ISK.
    fn row(type_id: u32, buy: bool, price: &str) -> String {
        format!(
            "10000002|{type_id}|{buy},{price},{price},{price},0.0,{price},10,2,{price},175414\n"
        )
    }

    /// Asks the market for the file and runs frames until the download is held.
    fn download(harness: &mut Harness<'static, App>) {
        {
            let shared = harness.state_mut().shared_mut();
            let market = Arc::clone(&shared.market);
            market.refresh(&shared.worker, NOW, true);
        }
        for _ in 0..500 {
            harness.run();
            if harness
                .state_mut()
                .shared_mut()
                .market
                .held()
                .published_at
                .is_some()
            {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("the market file did not arrive");
    }

    fn stack(item_id: i64, type_id: u32, quantity: u64, singleton: bool) -> Asset {
        Asset {
            item_id,
            type_id,
            location_id: JITA_4_4,
            flag: Some("Hangar".to_owned()),
            quantity,
            singleton,
        }
    }

    fn job(job_id: i64, runs: u32, status: JobStatus) -> Job {
        Job {
            job_id,
            activity_id: 1,
            blueprint_type_id: 1,
            product_type_id: Some(PAU),
            runs,
            status,
            start: NOW - 100,
            end: NOW + 7_500,
            facility_id: HOME_FORT,
            station_id: JITA_4_4,
            output_location_id: HOME_FORT,
            installer_id: i64::try_from(MAIN).expect("an id that fits"),
            cost: None,
            licensed_runs: None,
        }
    }

    /// Main at the scenario: a wallet of 1,234,567.89, 40 PAU and 120 PAU, a blueprint, a personal
    /// and a corporation sell order, a personal buy order, an active and a delivered job.
    fn main_data() -> CharacterData {
        let stock = CharacterAssets {
            character_id: i64::try_from(MAIN).expect("an id that fits"),
            updated: Some(990_000),
            assets: vec![
                stack(1, PAU, 40, false),
                stack(2, PAU, 120, false),
                stack(3, BLUEPRINT, 1, true),
            ],
        };
        let mut data = CharacterData::new(MAIN);
        data.wallet = Some(fetched(Money::from_ten_thousandths(12_345_678_900)));
        data.assets = Some(fetched(OwnedAssets {
            assets: stock,
            names: BTreeMap::new(),
        }));
        data.orders = Some(fetched(vec![
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
        ]));
        data.jobs = Some(fetched(vec![
            job(1, 10, JobStatus::Active),
            job(2, 5, JobStatus::Delivered),
        ]));
        data
    }

    fn copies(data: &TempDir) -> Vec<mop_store::price_history::PointPrices> {
        store_of(&data.path().join("data"))
            .point_prices(0, NOW)
            .expect("the copies read")
    }

    fn fetched<T>(data: T) -> Fetched<T> {
        Fetched {
            data,
            fetched_at: 990_000,
            last_modified: None,
            expires: None,
        }
    }

    fn main_with_wallet(wallet: Money) -> CharacterData {
        let mut data = CharacterData::new(MAIN);
        data.wallet = Some(fetched(wallet));
        data
    }

    fn order(
        is_buy: bool,
        is_corporation: bool,
        price: Money,
        volume: u64,
        escrow: Option<Money>,
    ) -> Order {
        Order {
            order_id: 1,
            type_id: PAU,
            location_id: 60_003_760,
            region_id: 10_000_002,
            is_buy,
            price,
            volume_remain: volume,
            volume_total: volume,
            issued: NOW,
            expires: NOW,
            range: "station".to_owned(),
            escrow,
            min_volume: None,
            is_corporation,
        }
    }

    fn with_orders(orders: Vec<Order>) -> CharacterData {
        let mut data = CharacterData::new(MAIN);
        data.orders = Some(fetched(orders));
        data
    }

    fn stored_records(data: &TempDir) -> usize {
        store_of(&data.path().join("data"))
            .records(0, NOW)
            .expect("records read")
            .len()
    }

    fn errors(harness: &mut Harness<'static, App>) -> Vec<String> {
        harness
            .state_mut()
            .shared_mut()
            .notices
            .iter()
            .map(|notice| notice.text.clone())
            .collect()
    }

    #[test]
    fn the_first_frame_records_nothing_and_the_next_refresh_records_one() {
        let dir = TempDir::new().expect("a temp folder");
        let mut harness = start(&dir);
        assert_eq!(stored_records(&dir), 0);
        harness
            .state_mut()
            .shared_mut()
            .session
            .replace(main_with_wallet(Money::from_ten_thousandths(50_000)));
        harness.run();
        assert_eq!(stored_records(&dir), 1);
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
        let huge = Money::from_ten_thousandths(9_000_000_000_000_000_000);
        let mut data = with_orders(vec![order(false, false, huge, 100, None)]);
        data.wallet = None;
        harness.state_mut().shared_mut().session.replace(data);
        harness.run();
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
        let saved = copies(&dir);
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].at, NOW);
        assert_eq!(saved[0].published_at, Some(PUBLISHED));
        let held = harness.state_mut().shared_mut().market.held();
        assert_eq!(saved[0].rows.len(), 3);
        assert!(saved[0].rows.contains_key(&(TRITANIUM, Side::Sell)));
        assert_eq!(saved[0].rows, held.rows);
    }

    #[test]
    fn without_a_download_the_record_is_written_and_nothing_is_copied() {
        let dir = TempDir::new().expect("a temp folder");
        let mut harness = start(&dir);
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
        assert!(copies(&dir).is_empty());
    }
}

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
    use std::sync::{Arc, Mutex, PoisonError};
    use std::thread;
    use std::time::Duration;

    use egui::vec2;
    use egui_kittest::Harness;
    use eve_means_of_profit::build::prices::Prices;
    use eve_means_of_profit::net_worth::data::load::{Loaded, load};
    use eve_means_of_profit::net_worth::data::pricer::PriceSource;
    use eve_means_of_profit::session::{EveAccess, PendingLogin, Saved};
    use eve_means_of_profit::{App, AppConfig};
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use mop_core::industry::TypeId;
    use mop_core::money::Money;
    use mop_core::net_worth::{CharacterKey, Holdings, Part, Record, Snapshot};
    use mop_core::stock::{Asset, CharacterAssets};
    use mop_esi::character_data::{
        CharacterData, Fetched, Job, JobStatus, Kind, Order, OwnedAssets, Refreshed,
    };
    use mop_esi::characters::{Character, Login};
    use mop_esi::error::EsiError;
    use mop_esi::places::{NamingReport, StructureNames};
    use mop_market::book::PriceRule;
    use mop_market::cache::{Http, HttpResponse};
    use mop_market::error::MarketError;
    use mop_market::fuzzwork::{BytesHttp, BytesResponse, FORGE, Side, SummaryRow};
    use mop_market::plex::PLEX;
    use mop_sde::{Fetch, FetchError, Sde, import};
    use mop_store::Store;
    use mop_store::price_history::{MarketKind, PointPrices};
    use mop_store::settings::{RuleTarget, RuleValue};
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
    /// The one page of PLEX's orders the fake answers.
    const PLEX_URL: &str =
        "https://esi.evetech.net/markets/19000001/orders/?type_id=44992&order_type=all&page=1";

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

    /// The PLEX market for these tests: it records every URL it is asked and answers with the
    /// page it is scripted to give, or with the failure to report. The test keeps a clone, so it
    /// can read the URLs the app's client was asked.
    #[derive(Clone)]
    struct FakePlex {
        answer: Result<HttpResponse, MarketError>,
        asked: Arc<Mutex<Vec<String>>>,
    }

    impl FakePlex {
        /// A market that fails every request, as on a machine with no network.
        fn offline() -> Self {
            Self::new(Err(MarketError::Network(
                "no PLEX market in these tests".to_owned(),
            )))
        }

        /// A market that answers the one page with `status`, its `retry_after` header, and
        /// `body`. No `x-pages` header, so one page is read.
        fn page(status: u16, retry_after: Option<u64>, body: &str) -> Self {
            Self::new(Ok(HttpResponse {
                status,
                retry_after,
                body: body.to_owned(),
                ..HttpResponse::default()
            }))
        }

        fn new(answer: Result<HttpResponse, MarketError>) -> Self {
            Self {
                answer,
                asked: Arc::new(Mutex::new(Vec::new())),
            }
        }

        /// The URLs asked so far, in order.
        fn asked(&self) -> Vec<String> {
            self.asked
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }
    }

    impl Http for FakePlex {
        fn get(
            &self,
            url: &str,
            _if_none_match: Option<&str>,
        ) -> Result<HttpResponse, MarketError> {
            self.asked
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(url.to_owned());
            self.answer.clone()
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
    /// market answers with no rows, so no download is held, and the PLEX market fails every
    /// request, as on a machine with no network.
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
        harness.state_mut().shared_mut().plex_http = Arc::new(FakePlex::offline());
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

    /// The held Fuzzwork file: a Sell and a Buy row of PAU, a Sell row of Tritanium, and PLEX's
    /// stale Sell row, left from before PLEX left the file in July 2025.
    struct FileForge;

    impl BytesHttp for FileForge {
        fn get_bytes(
            &self,
            _url: &str,
            _if_none_match: Option<&str>,
        ) -> Result<BytesResponse, MarketError> {
            let text = format!(
                "{HEADER}\n{}{}{}{}",
                row(PAU, false, "61200.00"),
                row(PAU, true, "55010.00"),
                row(TRITANIUM, false, "5.10"),
                row(PLEX, false, "4000000.00"),
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

    /// One order of the PLEX answer: `is_buy_order` is `buy`, the price in ISK.
    fn plex_order(id: u64, buy: bool, price: &str, units: u64) -> String {
        format!(
            r#"{{"order_id":{id},"type_id":44992,"location_id":60003760,"is_buy_order":{buy},"price":{price},"volume_remain":{units}}}"#
        )
    }

    /// The one page of PLEX orders: three sells, then, unless `sells_only`, two buys.
    fn plex_body(sells_only: bool) -> String {
        let mut orders = vec![
            plex_order(1, false, "5200000.0", 30),
            plex_order(2, false, "5250000.0", 50),
            plex_order(3, false, "5400000.0", 20),
        ];
        if !sells_only {
            orders.push(plex_order(4, true, "5100000.0", 10));
            orders.push(plex_order(5, true, "5000000.0", 30));
        }
        format!("[{}]", orders.join(","))
    }

    fn money(text: &str) -> Money {
        Money::parse_decimal(text).expect("a test price")
    }

    /// PLEX's two sides as the scripted orders summarise them.
    fn fetched_plex() -> BTreeMap<Side, SummaryRow> {
        BTreeMap::from([
            (
                Side::Sell,
                SummaryRow {
                    min: money("5200000.00"),
                    max: money("5400000.00"),
                    weighted_average: money("5265000.00"),
                    median: money("5250000.00"),
                    five_percent: money("5200000.00"),
                    volume: 100,
                    orders: 3,
                },
            ),
            (
                Side::Buy,
                SummaryRow {
                    min: money("5000000.00"),
                    max: money("5100000.00"),
                    weighted_average: money("5025000.00"),
                    median: money("5000000.00"),
                    five_percent: money("5100000.00"),
                    volume: 40,
                    orders: 2,
                },
            ),
        ])
    }

    /// PLEX's two sides keyed as the copy holds them.
    fn fetched_plex_rows() -> BTreeMap<(TypeId, Side), SummaryRow> {
        fetched_plex()
            .into_iter()
            .map(|(side, row)| ((PLEX, side), row))
            .collect()
    }

    /// Alt: a stack of PLEX, so the copy's types include PLEX and the file's stale PLEX row
    /// would be kept without the fetch replacing it.
    fn alt_holding_plex() -> CharacterData {
        let stock = CharacterAssets {
            character_id: i64::try_from(ALT).expect("an id that fits"),
            updated: Some(990_000),
            assets: vec![stack(4, PLEX, 500, false)],
        };
        let mut data = CharacterData::new(ALT);
        data.assets = Some(fetched(OwnedAssets {
            assets: stock,
            names: BTreeMap::new(),
        }));
        data
    }

    /// Main's scenario, with a stack of PLEX among the assets: the copy's types then include
    /// PLEX, so the file's stale PLEX row reaches the copy unless it is removed.
    fn main_holding_plex() -> CharacterData {
        let mut data = main_data();
        data.assets
            .as_mut()
            .expect("Main's assets")
            .data
            .assets
            .assets
            .push(stack(4, PLEX, 500, false));
        data
    }

    /// Runs frames until `done`, failing after a while. The price copy is written on the worker,
    /// so its save and its notices arrive a frame later.
    fn settle(
        harness: &mut Harness<'static, App>,
        what: &str,
        mut done: impl FnMut(&mut Harness<'static, App>) -> bool,
    ) {
        for _ in 0..500 {
            harness.run();
            if done(harness) {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("{what} did not happen");
    }

    /// Runs frames long enough for a queued price-copy job to have made its request, so "no
    /// request" can be asserted.
    fn wait_for_no_request(harness: &mut Harness<'static, App>) {
        for _ in 0..50 {
            harness.run();
            thread::sleep(Duration::from_millis(10));
        }
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

    fn copies(data: &TempDir) -> Vec<PointPrices> {
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

    // The loading tests below use M1-134's PAU (12,612), which the game-data fixture does not
    // hold: the app-wide price and the type's own rules price it all the same.
    const LOAD_PAU: u32 = 12_612;
    /// The first record's time: exactly 604,800 seconds before `NOW`.
    const FIRST_AT: i64 = NOW - 604_800;
    /// The range's start: 30 days before `NOW`.
    const LOAD_FROM: i64 = NOW - 2_592_000;

    fn isk(text: &str) -> Money {
        Money::parse_isk(text).expect("a test price")
    }

    fn load_row(price: &str) -> SummaryRow {
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

    fn all_parts() -> BTreeSet<Part> {
        Part::ALL.iter().copied().collect()
    }

    /// A store and the game data opened from the same temp folder.
    fn load_store() -> (TempDir, Store, Sde) {
        let dir = TempDir::new().expect("a temp folder");
        let root = dir.path().join("data");
        fs::create_dir_all(&root).expect("the data folder");
        import(&fixture(), &root.join("sde.sqlite")).expect("the fixture imports");
        let store = store_of(&root);
        let sde = Sde::open(&root.join("sde.sqlite")).expect("the game data opens");
        (dir, store, sde)
    }

    fn load_seen(
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

    fn load_record(character: u64, at: i64, snapshot: Snapshot) -> Record {
        Record {
            character: CharacterKey::Id(character),
            at,
            snapshot,
        }
    }

    /// Main at the first point: a wallet and 160 PAU.
    fn add_main_first(store: &mut Store) {
        store
            .add_record(&load_record(
                MAIN,
                FIRST_AT,
                load_seen(Some("1,000,000.00"), &[(LOAD_PAU, 160)], None, None, &[]),
            ))
            .expect("the first record is written");
    }

    /// Main at the latest point: the whole scenario.
    fn add_main_latest(store: &mut Store) {
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

    fn add_alt(store: &mut Store, at: i64) {
        store
            .add_record(&load_record(
                ALT,
                at,
                load_seen(Some("500,000.00"), &[], None, None, &[]),
            ))
            .expect("the alt record is written");
    }

    /// PAU's Forge history: the two sell readings and the two buy readings of M1-134.
    fn seed_history(store: &mut Store) {
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

    fn forge_id(store: &Store) -> i64 {
        store
            .history_markets()
            .expect("the markets read")
            .into_iter()
            .find(|market| market.kind == MarketKind::Region && market.place == u64::from(FORGE))
            .expect("the Forge market")
            .id
    }

    /// The copy at the latest point: PAU's two sides, and PLEX's sell side when asked for.
    fn load_copy(with_plex: bool) -> PointPrices {
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

    fn load_range(
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
}

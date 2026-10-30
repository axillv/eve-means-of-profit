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

    use crate::ui::{coloured, hover, hover_at, on_row, shown, snapshot};
    use egui::accesskit::{Role, Toggled};
    use egui::{pos2, vec2};
    use egui_kittest::Harness;
    use egui_kittest::kittest::{NodeT, Queryable};
    use eve_means_of_profit::build::prices::Prices;
    use eve_means_of_profit::net_worth::data::load::{Loaded, load};
    use eve_means_of_profit::net_worth::data::pricer::PriceSource;
    use eve_means_of_profit::session::{EveAccess, PendingLogin, Saved};
    use eve_means_of_profit::status::Level;
    use eve_means_of_profit::{App, AppConfig};
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use mop_core::industry::TypeId;
    use mop_core::money::Money;
    use mop_core::net_worth::{CharacterKey, Holdings, Part, PriceBasis, Record, Snapshot};
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
    use mop_store::price_history::{MarketKind, PointPrices};
    use mop_store::settings::{RuleTarget, RuleValue};
    use mop_store::{NetWorthSettings, Store};
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

    /// A login with the one character given, and jEve's profile folder when the test has one.
    #[derive(Clone)]
    struct FakeEve {
        characters: Vec<Character>,
        jeve_folder: Option<PathBuf>,
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
            self.jeve_folder.clone()
        }
    }

    fn character(id: u64, name: &str) -> Character {
        character_at(id, name, NOW)
    }

    fn character_at(id: u64, name: &str, at: i64) -> Character {
        Character {
            id,
            name: name.to_owned(),
            scopes: BTreeSet::new(),
            added_at: at,
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
        start_at(data, forge, characters, NOW, None)
    }

    /// As `start_with_characters`, at the clock given and with jEve's profile folder given.
    fn start_at(
        data: &TempDir,
        forge: Box<dyn BytesHttp + Send>,
        characters: Vec<Character>,
        now: i64,
        jeve_folder: Option<PathBuf>,
    ) -> Harness<'static, App> {
        let root = data.path().join("data");
        let cache = data.path().join("cache");
        fs::create_dir_all(root.join("esi")).expect("the data folder");
        import(&fixture(), &root.join("sde.sqlite")).expect("the fixture imports");
        {
            let mut store = store_of(&root);
            let mut settings = store.settings().expect("settings");
            settings.last_sde_check = Some(now);
            store.save_settings(&settings).expect("settings saved");
        }
        let loaded = characters.clone();
        let config = AppConfig {
            data_dir: root,
            cache_dir: cache,
            fetch: Box::new(NoNetwork),
            clock: Box::new(move || now),
            eve: Box::new(FakeEve {
                characters,
                jeve_folder,
            }),
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

    /// Opens the Net worth page by its rail entry. The Today page has a shortcut with the same
    /// label, and the rail is drawn first.
    fn open_net_worth(harness: &mut Harness<'static, App>) {
        harness
            .get_all_by_label("Net worth")
            .next()
            .expect("a rail entry")
            .click();
        harness.run();
    }

    /// Whether a dropdown shows `value`. A combo box has no label of its own, only its value.
    fn has_combo(harness: &Harness<'static, App>, value: &str) -> bool {
        fn find(node: &egui_kittest::Node<'_>, value: &str) -> bool {
            let accesskit = node.accesskit_node();
            if accesskit.role() == Role::ComboBox && accesskit.value().as_deref() == Some(value) {
                return true;
            }
            node.children().any(|child| find(&child, value))
        }
        find(&harness.root(), value)
    }

    /// Whether any drawn label starts with `prefix`.
    fn labelled_with_prefix(harness: &Harness<'static, App>, prefix: &str) -> bool {
        harness
            .root()
            .children_recursive()
            .filter_map(|node| node.accesskit_node().label())
            .any(|label| label.starts_with(prefix))
    }

    /// Whether the checkbox labelled `label` is ticked.
    fn ticked(harness: &Harness<'static, App>, label: &str) -> bool {
        harness
            .query_all_by_label(label)
            .find(|node| node.accesskit_node().role() == Role::CheckBox)
            .unwrap_or_else(|| panic!("no checkbox labelled {label}"))
            .accesskit_node()
            .toggled()
            == Some(Toggled::True)
    }

    fn click(harness: &mut Harness<'static, App>, label: &str) {
        harness.get_by_label(label).hover();
        // A pick closes its list for a frame; the press must come after that frame.
        harness.run_steps(2);
        harness.get_by_label(label).click();
        harness.run_steps(3);
    }

    fn click_checkbox(harness: &mut Harness<'static, App>, label: &str) {
        {
            let target = harness
                .query_all_by_label(label)
                .find(|node| node.accesskit_node().role() == Role::CheckBox)
                .unwrap_or_else(|| panic!("no checkbox labelled {label}"));
            target.scroll_to_me();
        }
        harness.run();
        {
            let target = harness
                .query_all_by_label(label)
                .find(|node| node.accesskit_node().role() == Role::CheckBox)
                .unwrap_or_else(|| panic!("no checkbox labelled {label}"));
            target.hover();
        }
        harness.run_steps(2);
        {
            let target = harness
                .query_all_by_label(label)
                .find(|node| node.accesskit_node().role() == Role::CheckBox)
                .unwrap_or_else(|| panic!("no checkbox labelled {label}"));
            target.click();
        }
        harness.run_steps(3);
    }

    /// Clicks the `index`th combo box that shows `value`, counting from the top of the page.
    fn click_combo(harness: &mut Harness<'static, App>, value: &str, index: usize) {
        let mut seen = 0;
        let found = reveal_combo_in(&harness.root(), value, index, &mut seen);
        assert!(found, "no combo box shows {value} number {index}");
        harness.run_steps(2);
        // A drop-down that was just closed by a pick is still open for a frame: move onto this one
        // and let that frame pass before the press, or the press only closes the old list.
        let mut seen = 0;
        hover_combo_in(&harness.root(), value, index, &mut seen);
        harness.run_steps(2);
        let mut seen = 0;
        click_combo_in(&harness.root(), value, index, &mut seen);
        harness.run_steps(3);
    }

    fn hover_combo_in(
        node: &egui_kittest::Node<'_>,
        value: &str,
        index: usize,
        seen: &mut usize,
    ) -> bool {
        let accesskit = node.accesskit_node();
        if accesskit.role() == Role::ComboBox && accesskit.value().as_deref() == Some(value) {
            if *seen == index {
                node.hover();
                return true;
            }
            *seen += 1;
        }
        node.children()
            .any(|child| hover_combo_in(&child, value, index, seen))
    }

    fn reveal_combo_in(
        node: &egui_kittest::Node<'_>,
        value: &str,
        index: usize,
        seen: &mut usize,
    ) -> bool {
        let accesskit = node.accesskit_node();
        if accesskit.role() == Role::ComboBox && accesskit.value().as_deref() == Some(value) {
            if *seen == index {
                node.scroll_to_me();
                return true;
            }
            *seen += 1;
        }
        node.children()
            .any(|child| reveal_combo_in(&child, value, index, seen))
    }

    fn click_combo_in(
        node: &egui_kittest::Node<'_>,
        value: &str,
        index: usize,
        seen: &mut usize,
    ) -> bool {
        let accesskit = node.accesskit_node();
        if accesskit.role() == Role::ComboBox && accesskit.value().as_deref() == Some(value) {
            if *seen == index {
                node.click();
                return true;
            }
            *seen += 1;
        }
        node.children()
            .any(|child| click_combo_in(&child, value, index, seen))
    }

    mod chart;
    mod controls;
    mod headline;
    mod history;
    mod loading;
    mod recording;
    mod tables;
}

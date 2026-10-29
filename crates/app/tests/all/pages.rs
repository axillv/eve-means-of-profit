//! Acceptance tests for the rail's pages: the panels each page shows, the saved character data and
//! jEve's profile the pages read, the place names, and the item picked in a table. The game data is
//! the build 3607705 fixture, imported in the test. The login is a fake, and jEve's profile is a
//! made-up file in a temp folder. The clock is fixed at 1,000,000.
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

    use egui::{OutputCommand, Rect, vec2};
    use egui_kittest::Harness;
    use egui_kittest::kittest::{NodeT, Queryable};
    use eve_means_of_profit::build::prices::Prices;
    use eve_means_of_profit::rail::{self, Page};
    use eve_means_of_profit::session::{EveAccess, PendingLogin};
    use eve_means_of_profit::theme::Style;
    use eve_means_of_profit::{App, AppConfig};
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use mop_core::freight::Packing;
    use mop_core::money::Money;
    use mop_core::net_worth::{CharacterKey, Holdings, Record, Snapshot};
    use mop_core::stock::{Asset, CharacterAssets};
    use mop_esi::character_data::{
        CharacterData, Fetched, Job, JobStatus, Kind, Order, OwnedAssets, Refreshed,
    };
    use mop_esi::characters::{Character, Login};
    use mop_esi::error::EsiError;
    use mop_esi::places::{NamingReport, StructureNames};
    use mop_esi::saved;
    use mop_market::book::PriceRule;
    use mop_market::error::MarketError;
    use mop_market::fuzzwork::{BytesHttp, BytesResponse, FORGE, Side, SummaryRow};
    use mop_sde::{Fetch, FetchError, import};
    use mop_store::price_history::{MarketKind, PointPrices};
    use mop_store::{PlanRecord, Store};
    use rusqlite::Connection;
    use tempfile::TempDir;

    const NOW: i64 = 1_000_000;
    const FIXTURE: &str = "../sde/tests/fixtures/sde-3607705";
    const MAIN: u64 = 2_112_000_001;
    const ALT: u64 = 2_112_000_002;
    const HAULER: u64 = 2_112_000_003;
    const JITA_4_4: i64 = 60_003_760;
    const HOME_FORT: i64 = 1_035_466_617_946;
    const REFUSED: i64 = 1_035_466_617_999;
    /// The type the saved data and jEve's profile hold: R.A.M.- Robotics, as the fixture names it.
    const RAM: u32 = 11_481;
    const PAU: u32 = 11_688;
    const MORPHITE: u32 = 11_399;
    /// The Light Scout Drone, built from Morphite and Particle Accelerator Units.
    const DRONE: u32 = 2_456;
    /// The invention job's product in the Today fixture.
    const HOBGOBLIN_II_BLUEPRINT: u32 = 2_457;
    /// The ready reaction's product in the Today fixture.
    const CRYSTALLINE_CARBONIDE: u32 = 16_670;
    /// The blueprint the fixture's research, copying and reaction jobs use.
    const RIFTER_BLUEPRINT: u32 = 691;
    /// The Notice text when the saved data is a day old.
    const OLD_NOTICE: &str = "Character data is 1d old. Refresh it on the Characters page.";

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

    /// A market file with no rows, so the Buy list never reaches the network in these tests.
    struct EmptyForge;

    impl BytesHttp for EmptyForge {
        fn get_bytes(
            &self,
            _url: &str,
            _if_none_match: Option<&str>,
        ) -> Result<BytesResponse, MarketError> {
            let header = "what,weightedaverage,maxval,minval,stddev,median,volume,numorders,fivepercent,orderSet\n";
            let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
            encoder
                .write_all(header.as_bytes())
                .map_err(|error| MarketError::Network(error.to_string()))?;
            let body = encoder
                .finish()
                .map_err(|error| MarketError::Network(error.to_string()))?;
            Ok(BytesResponse {
                status: 200,
                etag: Some("\"0\"".to_owned()),
                last_modified: Some("Mon, 12 Jan 1970 13:46:40 GMT".to_owned()),
                retry_after: None,
                body,
            })
        }
    }

    /// The Forge summary file the Today tests read: PAU sells cheapest at 61,200.00 and Morphite is
    /// bought highest at 2,150.00.
    struct Forge;

    impl BytesHttp for Forge {
        fn get_bytes(
            &self,
            _url: &str,
            _if_none_match: Option<&str>,
        ) -> Result<BytesResponse, MarketError> {
            let text = format!(
                "what,weightedaverage,maxval,minval,stddev,median,volume,numorders,fivepercent,orderSet\n\
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

    /// A data folder with the game data imported and the user's store. The login data is saved by
    /// each test.
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

    /// A login with the characters given, a jEve folder when there is one, and a count of refreshes.
    #[derive(Clone)]
    struct FakeEve {
        characters: Vec<Character>,
        jeve: Option<PathBuf>,
        refreshes: Arc<Mutex<usize>>,
    }

    impl FakeEve {
        fn new(characters: Vec<Character>, jeve: Option<PathBuf>) -> Self {
            Self {
                characters,
                jeve,
                refreshes: Arc::new(Mutex::new(0)),
            }
        }

        fn refreshes(&self) -> Arc<Mutex<usize>> {
            Arc::clone(&self.refreshes)
        }
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
            *self
                .refreshes
                .lock()
                .unwrap_or_else(PoisonError::into_inner) += 1;
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
            self.jeve.clone()
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

    fn asset(item_id: i64, location_id: i64, quantity: u64) -> Asset {
        Asset {
            item_id,
            type_id: RAM,
            location_id,
            flag: Some("Hangar".to_owned()),
            quantity,
            singleton: false,
        }
    }

    /// One character's saved data with assets fetched at `fetched`.
    fn assets_data(character_id: u64, fetched: i64, assets: Vec<Asset>) -> CharacterData {
        let stock = CharacterAssets {
            character_id: i64::try_from(character_id).expect("an id that fits"),
            updated: Some(fetched),
            assets,
        };
        CharacterData {
            character_id,
            assets: Some(Fetched {
                data: OwnedAssets {
                    assets: stock,
                    names: BTreeMap::new(),
                },
                fetched_at: fetched,
                last_modified: None,
                expires: None,
            }),
            orders: None,
            jobs: None,
            wallet: None,
        }
    }

    /// Main's saved data: 40 in Jita 4-4 and 120 in Home Fort, fetched at 990,000.
    fn main_data() -> CharacterData {
        assets_data(
            MAIN,
            990_000,
            vec![asset(1, JITA_4_4, 40), asset(2, HOME_FORT, 120)],
        )
    }

    /// One industry job at Home Fort, as the jobs page's fixture makes them.
    fn job(
        job_id: i64,
        activity_id: u32,
        product: Option<u32>,
        blueprint: u32,
        runs: u32,
        status: JobStatus,
        end: i64,
    ) -> Job {
        Job {
            job_id,
            activity_id,
            blueprint_type_id: blueprint,
            product_type_id: product,
            runs,
            status,
            start: end - 86_400,
            end,
            facility_id: HOME_FORT,
            station_id: HOME_FORT,
            output_location_id: HOME_FORT,
            installer_id: i64::try_from(MAIN).expect("an id that fits"),
            cost: None,
            licensed_runs: None,
        }
    }

    /// M1-26's jobs: an active PAU job, an invention of a Hobgoblin II Blueprint that has ended, a
    /// ready reaction and a paused copy.
    fn ready_jobs() -> Vec<Job> {
        vec![
            job(
                1,
                1,
                Some(PAU),
                RIFTER_BLUEPRINT,
                10,
                JobStatus::Active,
                NOW + 7_500,
            ),
            job(
                2,
                8,
                Some(HOBGOBLIN_II_BLUEPRINT),
                HOBGOBLIN_II_BLUEPRINT,
                1,
                JobStatus::Active,
                NOW - 10,
            ),
            job(
                3,
                11,
                Some(CRYSTALLINE_CARBONIDE),
                RIFTER_BLUEPRINT,
                5,
                JobStatus::Ready,
                NOW - 3_600,
            ),
            job(
                4,
                5,
                None,
                RIFTER_BLUEPRINT,
                1,
                JobStatus::Paused,
                NOW + 20_000,
            ),
        ]
    }

    /// One market order at Jita 4-4, as the orders page's fixture makes them.
    fn order(
        order_id: i64,
        type_id: u32,
        is_buy: bool,
        price: Money,
        (remain, total): (u64, u64),
        expires: i64,
    ) -> Order {
        Order {
            order_id,
            type_id,
            location_id: JITA_4_4,
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

    /// M1-26's orders, with the Morphite buy still open, plus a PAU sell at 61,000.00: two beaten
    /// PAU sells, a beaten corp PAU sell and a beaten Morphite buy.
    fn beaten_orders() -> Vec<Order> {
        let mut corp = order(
            3,
            PAU,
            false,
            Money::from_ten_thousandths(700_000_000),
            (50, 50),
            NOW + 260_000,
        );
        corp.is_corporation = true;
        vec![
            order(
                1,
                PAU,
                false,
                Money::from_ten_thousandths(615_000_000),
                (100, 200),
                NOW + 260_000,
            ),
            order(
                2,
                MORPHITE,
                true,
                Money::from_ten_thousandths(21_000_000),
                (10, 10),
                NOW + 100_000,
            ),
            corp,
            order(
                4,
                PAU,
                false,
                Money::from_ten_thousandths(610_000_000),
                (1, 1),
                NOW + 260_000,
            ),
        ]
    }

    /// Main's saved data with the jobs and orders given, fetched at the clock's time.
    fn data_with(jobs: Option<Vec<Job>>, orders: Option<Vec<Order>>) -> CharacterData {
        CharacterData {
            character_id: MAIN,
            assets: None,
            orders: orders.map(|data| Fetched {
                data,
                fetched_at: NOW,
                last_modified: None,
                expires: None,
            }),
            jobs: jobs.map(|data| Fetched {
                data,
                fetched_at: NOW,
                last_modified: None,
                expires: None,
            }),
            wallet: None,
        }
    }

    /// The places file with one structure EVE refused, as the naming step writes it.
    fn write_places(env: &Env) {
        let text = format!(
            "{{\"version\":1,\"structures\":{{\"{REFUSED}\":{{\"name\":null,\"system_id\":null,\"refused\":true,\"checked_at\":900000}}}}}}"
        );
        fs::write(esi_dir(env).join("places.json"), text).expect("places written");
    }

    /// A jEve profile folder with `#Default.db` built from `sql`.
    fn jeve_folder(sql: &str) -> TempDir {
        let folder = TempDir::new().expect("a temp folder");
        let connection = Connection::open(folder.path().join("#Default.db")).expect("database");
        connection.execute_batch(sql).expect("fixture builds");
        drop(connection);
        folder
    }

    /// Alt (60 in Jita 4-4, updated 700,000) and Hauler (5 in Jita 4-4, never updated).
    fn jeve_profile() -> TempDir {
        jeve_folder(
            "CREATE TABLE owners (accountid TEXT, ownerid INTEGER, name TEXT, corp TEXT, show NUMERIC, \
             invalid NUMERIC, assetslastupdate INTEGER, assetsnextupdate INTEGER, scopes TEXT, \
             UNIQUE(accountid));
             CREATE TABLE assets (accountid TEXT, count INTEGER, flagid INTEGER, flagstring TEXT, \
             itemid INTEGER, typeid INTEGER, locationid INTEGER, singleton NUMERIC, rawquantity INTEGER, \
             UNIQUE(accountid, itemid));
             CREATE TABLE blueprints (accountid TEXT, itemid INTEGER, locationid INTEGER, typeid INTEGER, \
             flagid INTEGER, flagstring TEXT, quantity INTEGER, timeefficiency INTEGER, \
             materialefficiency INTEGER, runs NUMERIC, UNIQUE(accountid, itemid));
             INSERT INTO owners (accountid, ownerid, name, show, invalid, assetslastupdate, scopes) VALUES
              ('j2', 2112000002, 'Alt', 1, 0, 700000000, 'esi-assets.read_assets.v1'),
              ('j3', 2112000003, 'Hauler', 1, 0, NULL, 'esi-assets.read_assets.v1');
             INSERT INTO assets (accountid, itemid, typeid, locationid, flagid, flagstring, count, singleton, rawquantity) VALUES
              ('j2', 5001, 11481, 60003760, 4, 'Hangar', 60, 0, NULL),
              ('j3', 6001, 11481, 60003760, 4, 'Hangar', 5, 0, NULL);",
        )
    }

    fn start(env: &Env, eve: FakeEve, clock: i64) -> Harness<'static, App> {
        let config = AppConfig {
            data_dir: env.data.clone(),
            cache_dir: env.cache.clone(),
            fetch: Box::new(NoNetwork),
            clock: Box::new(move || clock),
            eve: Box::new(eve),
            market: Box::new(crate::offline::Offline),
        };
        let mut harness = Harness::builder()
            .with_size(vec2(1280.0, 800.0))
            .with_max_steps(crate::SETTLE_STEPS)
            .build_eframe(move |cc| App::new(&cc.egui_ctx, config).expect("the app starts"));
        harness.state_mut().shared_mut().market = Arc::new(Prices::new(Box::new(EmptyForge)));
        harness
    }

    /// A harness whose orders read the Forge summary file.
    fn start_with_forge(env: &Env, eve: FakeEve, clock: i64) -> Harness<'static, App> {
        let mut harness = start(env, eve, clock);
        harness.state_mut().shared_mut().market = Arc::new(Prices::new(Box::new(Forge)));
        harness
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

    /// The text the last drawn frame put on the clipboard, if any.
    fn copied_text(harness: &Harness<'_, App>) -> Option<String> {
        harness
            .output()
            .platform_output
            .commands
            .iter()
            .find_map(|command| {
                if let OutputCommand::CopyText(text) = command {
                    Some(text.clone())
                } else {
                    None
                }
            })
    }

    /// The texts of the notices the app holds.
    fn notices(harness: &mut Harness<'_, App>) -> Vec<String> {
        harness
            .state_mut()
            .shared_mut()
            .notices
            .iter()
            .map(|notice| notice.text.clone())
            .collect()
    }

    fn shown(harness: &Harness<'_, App>, text: &str) -> bool {
        harness.query_all_by_label(text).next().is_some()
    }

    /// Runs frames until `done` holds, letting the worker finish between them.
    fn until(harness: &mut Harness<'_, App>, what: &str, done: impl Fn(&Harness<'_, App>) -> bool) {
        for _ in 0..500 {
            harness.run();
            if done(harness) {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("{what} did not show");
    }

    /// Waits until the start-up read has arrived: the Item panel stops saying it is loading.
    fn loaded(harness: &mut Harness<'_, App>) {
        until(harness, "the saved data", |h| {
            !shown(h, "Loading character data…")
        });
    }

    /// Clicks the rail entry `label`. The rail is drawn first, so its entry is the first node with
    /// that label.
    fn rail(harness: &mut Harness<'_, App>, label: &str) {
        harness
            .get_all_by_label(label)
            .next()
            .expect("a rail entry")
            .click();
        harness.run();
    }

    #[test]
    fn the_rail_lists_the_pages_and_each_shows_its_panels() {
        let env = env();
        let mut harness = start(&env, FakeEve::new(Vec::new(), None), NOW);
        loaded(&mut harness);
        let pages: [(&str, &[&str]); 8] = [
            ("Today", &["Today"]),
            ("Build", &["Plans", "Plan", "Steps", "Buy list"]),
            ("Industry jobs", &["Industry jobs"]),
            ("Orders", &["Orders"]),
            ("Assets", &["Places", "Assets"]),
            ("Net worth", &["Net worth", "By character", "Right now"]),
            ("Characters", &["Characters", "jEve data"]),
            (
                "Settings",
                &["Facilities", "Freight services", "Price rule"],
            ),
        ];
        for (page, panels) in pages {
            rail(&mut harness, page);
            for panel in panels {
                // Once each (a tab bar or a second copy would show twice); a title that is also
                // a rail label shows once more, on the rail.
                let on_rail = [
                    "Today",
                    "Assets",
                    "Net worth",
                    "Orders",
                    "Industry jobs",
                    "Characters",
                ]
                .contains(panel);
                assert_eq!(
                    harness.query_all_by_label(panel).count(),
                    if on_rail { 2 } else { 1 },
                    "{page}: {panel} is shown once"
                );
            }
        }
        rail(&mut harness, "Build");
        for panel in ["Plans", "Plan", "Steps", "Buy list"] {
            assert_eq!(harness.query_all_by_label(panel).count(), 1, "{panel}");
        }
    }

    #[test]
    fn the_last_rail_entry_is_settings_and_shows_the_settings_panels() {
        let env = env();
        let mut harness = start(&env, FakeEve::new(Vec::new(), None), NOW);
        loaded(&mut harness);
        assert_eq!(Page::ALL.last().map(|page| page.label()), Some("Settings"));
        let left = rail_left(&harness);
        let settings: Vec<Rect> = harness
            .query_all_by_label("Settings")
            .filter(|node| (node.rect().min.x - left).abs() < 1.0)
            .map(|node| node.rect())
            .collect();
        assert_eq!(settings.len(), 1, "Settings is on the rail exactly once");
        assert!(
            harness.query_all_by_label("Setup").next().is_none(),
            "no entry is labelled Setup"
        );

        rail(&mut harness, "Settings");
        for panel in ["Facilities", "Freight services", "Price rule"] {
            assert!(shown(&harness, panel), "{panel} is shown");
        }
    }

    /// The rail's left edge. Every rail entry starts there; panel titles start to its right.
    fn rail_left(harness: &Harness<'_, App>) -> f32 {
        harness.get_by_label("Build").rect().min.x
    }

    /// The rect of the rail entry named `label`, not a same-named page title.
    fn rail_entry(harness: &Harness<'_, App>, label: &str) -> Rect {
        let left = rail_left(harness);
        harness
            .get_all_by_label(label)
            .find(|node| (node.rect().min.x - left).abs() < 1.0)
            .unwrap_or_else(|| panic!("no rail entry labelled {label:?}"))
            .rect()
    }

    #[test]
    fn the_rail_groups_the_pages_and_wraps_only_at_spaces() {
        let env = env();
        let mut harness = start(&env, FakeEve::new(Vec::new(), None), NOW);
        loaded(&mut harness);

        let labels = [
            "Today",
            "Build",
            "Industry jobs",
            "Orders",
            "Assets",
            "Net worth",
            "Characters",
            "Settings",
        ];
        let entries: Vec<Rect> = labels
            .iter()
            .map(|label| rail_entry(&harness, label))
            .collect();
        for pair in entries.windows(2) {
            assert!(
                pair[1].top() > pair[0].top(),
                "each rail entry is below the one before it"
            );
        }
        let left = rail_left(&harness);
        for label in labels {
            assert_eq!(
                harness
                    .query_all_by_label(label)
                    .filter(|node| (node.rect().min.x - left).abs() < 1.0)
                    .count(),
                1,
                "{label} is on the rail exactly once"
            );
        }

        // Every entry is the same width; the rail adds the panel's padding around them.
        let widest = entries.iter().map(|rect| rect.width()).fold(0.0, f32::max);
        assert!(widest + 16.0 < 140.0, "the rail is under 140 px wide");
        assert_eq!(entries[2].width(), widest, "Industry jobs fits its label");

        let gap = |above: Rect, below: Rect| below.top() - above.bottom();
        let ordinary = gap(
            rail_entry(&harness, "Build"),
            rail_entry(&harness, "Industry jobs"),
        );
        for (above, below) in [("Orders", "Assets"), ("Net worth", "Characters")] {
            let grouped = gap(rail_entry(&harness, above), rail_entry(&harness, below));
            assert!(
                grouped >= ordinary + 16.0,
                "{above} to {below} is a group gap"
            );
        }
    }

    /// A plain harness that draws only the rail, with `show_with` and either its labels or its
    /// icons.
    fn rail_only(icons_only: bool) -> Harness<'static, Page> {
        let style = Style::eve_dense();
        Harness::builder()
            .with_size(vec2(400.0, 800.0))
            .build_ui_state(
                move |ui, page| {
                    style.apply(ui.ctx());
                    rail::show_with(ui, &style, page, icons_only);
                },
                Page::Build,
            )
    }

    /// The rail's labels, top to bottom.
    fn rail_labels(harness: &Harness<'_, Page>) -> Vec<&'static str> {
        let mut labels: Vec<(&'static str, f32)> = Page::ALL
            .iter()
            .map(|page| {
                (
                    page.label(),
                    harness.get_by_label(page.label()).rect().top(),
                )
            })
            .collect();
        labels.sort_by(|left, right| left.1.total_cmp(&right.1));
        labels.into_iter().map(|(label, _)| label).collect()
    }

    #[test]
    fn the_icons_only_rail_is_narrow_and_lists_the_pages_in_the_same_order() {
        let labelled = rail_only(false);
        let icons = rail_only(true);
        let expected = [
            "Today",
            "Build",
            "Industry jobs",
            "Orders",
            "Assets",
            "Net worth",
            "Characters",
            "Settings",
        ];
        assert_eq!(rail_labels(&labelled), expected);
        assert_eq!(
            rail_labels(&icons),
            expected,
            "icons-only keeps the labels' order"
        );
        for label in expected {
            let rect = icons.get_by_label(label).rect();
            assert!(
                rect.width() <= 48.0,
                "{label} is at most 48 px wide, not {}",
                rect.width()
            );
        }
    }

    #[test]
    fn hovering_an_icons_only_entry_shows_its_label() {
        let mut harness = rail_only(true);
        let tip = crate::ui::hover(&mut harness, "Orders");
        assert!(
            tip.iter().any(|text| text == "Orders"),
            "Orders is its hover text: {tip:?}"
        );
    }

    #[test]
    fn clicking_an_icons_only_entry_shows_its_page() {
        let mut harness = rail_only(true);
        harness.get_by_label("Net worth").click();
        harness.run();
        assert_eq!(*harness.state(), Page::NetWorth);
    }

    #[test]
    fn the_stock_takes_the_login_data_then_jeve_and_a_fetched_login_wins() {
        let env = env();
        saved::save_character(&esi_dir(&env), &main_data()).expect("Main's data saved");
        let jeve = jeve_profile();
        let eve = FakeEve::new(
            vec![character(MAIN, "Main"), character(ALT, "Alt")],
            Some(jeve.path().to_path_buf()),
        );
        let mut harness = start(&env, eve, NOW);
        loaded(&mut harness);

        let texts = notices(&mut harness);
        assert!(texts.is_empty(), "{texts:?}");
        let shared = harness.state_mut().shared_mut();
        let stock = shared.session.stock_characters();
        let ids: Vec<i64> = stock.iter().map(|entry| entry.character_id).collect();
        assert_eq!(ids, [2_112_000_001, 2_112_000_002, 2_112_000_003]);
        assert_eq!(stock[0].assets.len(), 2, "Main has two assets");
        assert_eq!(
            shared.session.source(MAIN),
            Some(eve_means_of_profit::session::Source::Login)
        );
        assert_eq!(
            shared.session.source(ALT),
            Some(eve_means_of_profit::session::Source::Jeve)
        );
        assert_eq!(
            shared.session.source(HAULER),
            Some(eve_means_of_profit::session::Source::Jeve)
        );

        shared
            .session
            .replace(assets_data(ALT, NOW, vec![asset(3, JITA_4_4, 7)]));
        let stock = shared.session.stock_characters();
        assert_eq!(stock.len(), 3);
        assert_eq!(stock[1].assets.len(), 1, "Alt's login assets, not jEve's");
        assert_eq!(
            stock[1].assets[0].quantity, 7,
            "7 from the login, not 60 from jEve"
        );
        assert_eq!(
            shared.session.source(ALT),
            Some(eve_means_of_profit::session::Source::Login)
        );
        assert_eq!(shared.session.revision(ALT), 1);
        assert_eq!(shared.session.revision(MAIN), 0);
    }

    #[test]
    fn place_names_come_from_the_game_data_and_the_saved_names() {
        let env = env();
        saved::save_character(&esi_dir(&env), &main_data()).expect("Main's data saved");
        write_places(&env);
        {
            let mut store = Store::open(&env.data.join("user.sqlite"), NOW)
                .expect("the store opens")
                .store;
            store
                .set_structure_name(HOME_FORT, "Home Fort", NOW)
                .expect("the typed name is saved");
        }
        let mut harness = start(&env, FakeEve::new(vec![character(MAIN, "Main")], None), NOW);
        loaded(&mut harness);

        let shared = harness.state_mut().shared_mut();
        let sde = shared.sde.as_ref();
        let session = &shared.session;
        assert_eq!(
            session.place_label(JITA_4_4, sde),
            "Jita IV - Moon 4 - Caldari Navy Assembly Plant"
        );
        assert_eq!(session.place_label(HOME_FORT, sde), "Home Fort");
        assert_eq!(session.place_label(30_000_142, sde), "Jita");
        assert_eq!(
            session.place_label(REFUSED, sde),
            "Structure 1035466617999 (no access)"
        );
        assert_eq!(session.place_label(9, sde), "Location 9");
    }

    #[test]
    fn nothing_is_fetched_at_start_and_old_data_is_noticed() {
        let env = env();
        saved::save_character(&esi_dir(&env), &main_data()).expect("Main's data saved");
        let eve = FakeEve::new(vec![character(MAIN, "Main")], None);
        let refreshes = eve.refreshes();
        let mut harness = start(&env, eve, 1_100_000);
        loaded(&mut harness);
        for _ in 0..10 {
            harness.run();
        }
        assert_eq!(*refreshes.lock().unwrap_or_else(PoisonError::into_inner), 0);
        assert!(
            notices(&mut harness).contains(&OLD_NOTICE.to_owned()),
            "{:?}",
            notices(&mut harness)
        );
    }

    #[test]
    fn data_of_a_day_or_less_gives_no_notice() {
        let env = env();
        saved::save_character(&esi_dir(&env), &main_data()).expect("Main's data saved");
        let mut harness = start(&env, FakeEve::new(vec![character(MAIN, "Main")], None), NOW);
        loaded(&mut harness);
        assert!(harness.state_mut().shared_mut().notices.is_empty());
    }

    #[test]
    fn without_a_jeve_file_only_the_login_shows_and_there_is_no_notice() {
        let env = env();
        saved::save_character(&esi_dir(&env), &main_data()).expect("Main's data saved");
        let mut harness = start(&env, FakeEve::new(vec![character(MAIN, "Main")], None), NOW);
        loaded(&mut harness);
        let shared = harness.state_mut().shared_mut();
        let ids: Vec<i64> = shared
            .session
            .stock_characters()
            .iter()
            .map(|entry| entry.character_id)
            .collect();
        assert_eq!(ids, [2_112_000_001]);
        assert!(shared.notices.is_empty());
    }

    #[test]
    fn a_jeve_file_that_is_not_a_database_gives_its_notice() {
        let env = env();
        let folder = TempDir::new().expect("a temp folder");
        fs::write(folder.path().join("#Default.db"), "not a database").expect("written");
        let mut harness = start(
            &env,
            FakeEve::new(Vec::new(), Some(folder.path().to_path_buf())),
            NOW,
        );
        loaded(&mut harness);
        let texts = notices(&mut harness);
        assert!(
            texts
                .iter()
                .any(|text| text.starts_with("Could not read jEve's data file: ")),
            "{texts:?}"
        );
    }

    #[test]
    fn a_click_on_a_steps_row_or_a_buy_list_row_picks_that_item() {
        let env = env();
        {
            let mut store = Store::open(&env.data.join("user.sqlite"), NOW)
                .expect("the store opens")
                .store;
            store
                .create_plan(
                    &PlanRecord {
                        id: 0,
                        name: "Drone 10".to_owned(),
                        product: DRONE,
                        quantity: 10,
                        build: BTreeSet::from([DRONE]),
                        me: BTreeMap::new(),
                        manufacturing: None,
                        reactions: None,
                        price_rule: PriceRule::HighestBuy,
                        route: None,
                        service: None,
                        packing: Packing::TopUp,
                        pasted_stock: String::new(),
                        created_at: NOW,
                        updated_at: NOW,
                    },
                    NOW,
                )
                .expect("the plan is saved");
        }
        let mut harness = start(&env, FakeEve::new(Vec::new(), None), NOW);
        loaded(&mut harness);
        rail(&mut harness, "Build");
        harness.get_by_label("Drone 10").click();
        harness.run();
        // Steps comes before Buy list in the Build page, so the first PAU row is the Steps one.
        harness
            .get_all_by_label("Particle Accelerator Unit")
            .next()
            .expect("the step row")
            .click();
        harness.run();
        assert_eq!(harness.state_mut().shared_mut().selection.item, Some(PAU));
        // The Buy list is drawn after Steps, so its Morphite row is the last one.
        harness
            .get_all_by_label("Morphite")
            .last()
            .expect("the buy row")
            .click();
        harness.run();
        assert_eq!(
            harness.state_mut().shared_mut().selection.item,
            Some(MORPHITE)
        );
    }

    #[test]
    fn the_assets_page_matches_its_snapshot() {
        let env = env();
        let mut harness = start(&env, FakeEve::new(Vec::new(), None), NOW);
        loaded(&mut harness);
        rail(&mut harness, "Assets");
        harness.run();
        harness.snapshot("pages-assets");
    }

    /// The six block titles of the Today page, in [`Block::ALL`] order.
    const TODAY_TITLES: [&str; 6] = [
        "Jobs",
        "Plans",
        "Orders",
        "Warnings",
        "Net worth",
        "Characters",
    ];

    /// The left edge of the Today page's content. The rail's Today entry shares its label with
    /// the page heading, which is the rightmost.
    fn today_left(harness: &Harness<'_, App>) -> f32 {
        harness
            .query_all_by_label("Today")
            .map(|node| node.rect().min.x)
            .fold(f32::NEG_INFINITY, f32::max)
    }

    /// The rect of the Today block title `label`, not the rail entry of the same name.
    fn today_title(harness: &Harness<'_, App>, label: &str) -> Rect {
        let left = today_left(harness);
        harness
            .query_all_by_label(label)
            .find(|node| node.rect().min.x >= left - 1.0)
            .unwrap_or_else(|| panic!("no Today block title {label:?}"))
            .rect()
    }

    fn today_titles(harness: &Harness<'_, App>) -> Vec<Rect> {
        TODAY_TITLES
            .iter()
            .map(|label| today_title(harness, label))
            .collect()
    }

    #[test]
    fn a_fresh_app_starts_on_today_and_every_empty_block_says_why() {
        let env = env();
        let mut harness = start(&env, FakeEve::new(Vec::new(), None), NOW);
        loaded(&mut harness);

        assert!(
            shown(&harness, "Warnings"),
            "Warnings shows without a rail click, so Today starts"
        );
        for text in [
            "Jobs to deliver and free slots show here once a character is added.",
            "No plans yet. Each plan shows its next step here.",
            "Market orders someone has beaten show here once a character is added.",
            "None. Price spikes and thin markets in your plans show here.",
            "Starts with your first character.",
            "None yet.",
        ] {
            assert!(shown(&harness, text), "{text:?} shows");
        }

        let titles = today_titles(&harness);
        let left = titles[0].min.x;
        for title in &titles[..4] {
            assert!((title.min.x - left).abs() < 1.0, "the first four are left");
        }
        let right = titles[4].min.x;
        assert!(right > left, "the last two are right of the first four");
        for title in &titles[4..] {
            assert!((title.min.x - right).abs() < 1.0, "the last two are right");
        }
        assert!((titles[4].top() - titles[0].top()).abs() < 0.5);
        for pair in titles.windows(2) {
            assert!(
                pair[1].top() > pair[0].top() || pair[1].min.x > pair[0].min.x,
                "titles are in Block::ALL order"
            );
        }

        for name in [
            "Jobs: Add character",
            "Plans: Pick a product",
            "Orders: Add character",
            "Net worth: Add character",
            "Characters: Add character",
        ] {
            assert_eq!(
                harness.query_all_by_label(name).count(),
                1,
                "{name} is shown once"
            );
        }
        assert_eq!(
            harness
                .query_all_by_label("Warnings: Add character")
                .count(),
            0,
            "Warnings has no button"
        );
        assert_eq!(
            harness
                .query_all_by_label("Warnings: Pick a product")
                .count(),
            0,
            "Warnings has no button"
        );
        assert!(!shown(&harness, "0 ready to deliver"));
        assert!(!shown(&harness, "0 beaten · 0 open"));
    }

    #[test]
    fn the_blocks_keep_their_places_across_pages_and_characters() {
        let env = env();
        let mut harness = start(&env, FakeEve::new(Vec::new(), None), NOW);
        loaded(&mut harness);
        let before = today_titles(&harness);

        rail(&mut harness, "Build");
        rail(&mut harness, "Today");
        let after = today_titles(&harness);
        assert_eq!(after, before, "the block places do not move");

        let mut with_characters =
            start(&env, FakeEve::new(vec![character(MAIN, "Main")], None), NOW);
        loaded(&mut with_characters);
        let character_titles = today_titles(&with_characters);
        for (index, (with, without)) in character_titles.iter().zip(&before).enumerate() {
            // The characters block follows the Net worth block, whose body now has a line, so it
            // moves down; every other title keeps its place.
            if index == 5 {
                continue;
            }
            assert_eq!(with, without, "a character does not move block {index}");
        }
        assert!(
            character_titles[5].top() > character_titles[4].bottom(),
            "the characters block stays below the Net worth block"
        );
        for column in [&before[..4], &before[4..]] {
            let x = column[0].min.x;
            for title in column {
                assert!((title.min.x - x).abs() < 1.0, "one x per column");
            }
        }
    }

    #[test]
    fn today_lists_the_ready_jobs_oldest_first() {
        let env = env();
        saved::save_character(&esi_dir(&env), &data_with(Some(ready_jobs()), None))
            .expect("Main's jobs saved");
        let mut harness = start(&env, FakeEve::new(vec![character(MAIN, "Main")], None), NOW);
        loaded(&mut harness);

        assert!(shown(&harness, "2 ready to deliver"));
        assert!(shown(&harness, "Deliver 5 × Crystalline Carbonide"));
        assert!(shown(&harness, "Home Fort · ready 1h"));
        assert!(shown(&harness, "Deliver 1 × Hobgoblin II Blueprint"));
        assert!(shown(&harness, "Home Fort · ready 10s"));
        assert!(
            crate::ui::top(&harness, "Deliver 5 × Crystalline Carbonide")
                < crate::ui::top(&harness, "Deliver 1 × Hobgoblin II Blueprint"),
            "the job that ended first is above"
        );

        crate::ui::click(&mut harness, "Deliver 5 × Crystalline Carbonide");
        assert!(
            shown(&harness, "4 jobs · 2 ready to deliver"),
            "a job row shows Jobs"
        );
    }

    #[test]
    fn today_lists_the_beaten_orders_and_copies_the_price_to_type() {
        let env = env();
        saved::save_character(&esi_dir(&env), &data_with(None, Some(beaten_orders())))
            .expect("Main's orders saved");
        let mut harness =
            start_with_forge(&env, FakeEve::new(vec![character(MAIN, "Main")], None), NOW);
        loaded(&mut harness);
        hold_forge(&mut harness);

        assert!(shown(&harness, "3 beaten · 4 open"));
        assert!(shown(&harness, "Sell Particle Accelerator Unit beaten"));
        assert!(shown(&harness, "Buy Morphite beaten"));
        assert!(shown(&harness, "70,000.00, best 61,200.00"));
        assert!(shown(&harness, "61,500.00, best 61,200.00"));
        assert!(shown(&harness, "2,100.00, best 2,150.00"));
        assert!(
            crate::ui::top(&harness, "70,000.00, best 61,200.00")
                < crate::ui::top(&harness, "61,500.00, best 61,200.00"),
            "the widest gap is above"
        );
        assert!(
            crate::ui::top(&harness, "61,500.00, best 61,200.00")
                < crate::ui::top(&harness, "2,100.00, best 2,150.00"),
            "the smallest gap is last"
        );

        harness
            .get_all_by_label("Orders: Copy 61,190.00")
            .next()
            .expect("the corp PAU row's copy button")
            .click();
        harness.step();
        assert_eq!(copied_text(&harness).as_deref(), Some("61190.00"));
        assert!(shown(&harness, "Warnings"), "Today stays after a copy");

        crate::ui::click(&mut harness, "Buy Morphite beaten");
        assert!(
            shown(&harness, "4 orders · sell 9,711,000.00 · escrow 21,000.00"),
            "an order row shows Orders"
        );
    }

    #[test]
    fn a_character_with_nothing_to_do_shows_zero_counts_and_no_rows() {
        let env = env();
        let mut harness = start(&env, FakeEve::new(vec![character(MAIN, "Main")], None), NOW);
        loaded(&mut harness);

        assert!(shown(&harness, "0 ready to deliver"));
        assert!(shown(&harness, "0 beaten · 0 open"));
        assert!(!shown(&harness, "Main"), "no rows show");
    }

    #[test]
    fn a_filling_button_goes_to_its_page() {
        let env = env();
        let mut harness = start(&env, FakeEve::new(Vec::new(), None), NOW);
        loaded(&mut harness);

        crate::ui::click(&mut harness, "Plans: Pick a product");
        assert!(shown(&harness, "Steps"), "Pick a product shows Build");

        rail(&mut harness, "Today");
        crate::ui::click(&mut harness, "Jobs: Add character");
        assert!(
            shown(&harness, "jEve data"),
            "Add character shows Characters"
        );
    }

    #[test]
    fn the_today_page_matches_its_snapshot() {
        let env = env();
        let mut harness = start(&env, FakeEve::new(Vec::new(), None), NOW);
        loaded(&mut harness);
        harness.remove_cursor();
        harness.run();
        harness.snapshot("today-empty");
    }

    #[test]
    fn the_today_blocks_match_their_snapshot() {
        let env = env();
        saved::save_character(
            &esi_dir(&env),
            &data_with(Some(ready_jobs()), Some(beaten_orders())),
        )
        .expect("Main's jobs and orders saved");
        let mut harness =
            start_with_forge(&env, FakeEve::new(vec![character(MAIN, "Main")], None), NOW);
        loaded(&mut harness);
        hold_forge(&mut harness);
        harness.remove_cursor();
        harness.run();
        harness.snapshot("today-blocks");
    }

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

    #[test]
    fn the_build_page_matches_its_snapshot() {
        let env = env();
        let mut harness = start(&env, FakeEve::new(Vec::new(), None), NOW);
        loaded(&mut harness);
        rail(&mut harness, "Build");
        harness.remove_cursor();
        harness.run();
        harness.snapshot("pages-build");
    }
}

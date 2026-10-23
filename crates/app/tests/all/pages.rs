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
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Mutex, PoisonError};
    use std::thread;
    use std::time::Duration;

    use egui::vec2;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;
    use eve_means_of_profit::build::prices::Prices;
    use eve_means_of_profit::session::EveAccess;
    use eve_means_of_profit::{App, AppConfig};
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use mop_core::freight::Packing;
    use mop_core::stock::{Asset, CharacterAssets};
    use mop_esi::character_data::{CharacterData, Fetched, Kind, OwnedAssets, Refreshed};
    use mop_esi::characters::{Character, Login};
    use mop_esi::error::EsiError;
    use mop_esi::places::{NamingReport, StructureNames};
    use mop_esi::saved;
    use mop_market::book::PriceRule;
    use mop_market::error::MarketError;
    use mop_market::fuzzwork::{BytesHttp, BytesResponse};
    use mop_sde::{Fetch, FetchError, import};
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
        fs::create_dir_all(&data).expect("the data folder");
        import(&fixture(), &data.join("sde.sqlite")).expect("the fixture imports");
        let mut store = Store::open(&data.join("user.sqlite"), NOW)
            .expect("the store opens")
            .store;
        let mut settings = store.settings().expect("settings");
        settings.last_sde_check = Some(NOW);
        store.save_settings(&settings).expect("settings saved");
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

        fn begin_login(&mut self) -> Result<String, String> {
            Err("Logins are not started in these tests.".to_owned())
        }

        fn finish_login(&mut self, _cancel: &AtomicBool) -> Result<Character, EsiError> {
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
        };
        let mut harness = Harness::builder()
            .with_size(vec2(1280.0, 800.0))
            .build_eframe(move |cc| App::new(&cc.egui_ctx, config).expect("the app starts"));
        harness.state_mut().shared_mut().market = Arc::new(Prices::new(Box::new(EmptyForge)));
        harness
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
        let pages: [(&str, &[&str]); 7] = [
            (
                "Build",
                &["Plans", "Pasted stock", "Plan", "Steps", "Buy list", "Item"],
            ),
            ("Assets", &["Places", "Assets", "Item"]),
            ("Net worth", &["Net worth", "By character", "Right now"]),
            ("Orders", &["Orders"]),
            ("Industry jobs", &["Industry jobs"]),
            ("Characters", &["Characters", "jEve data"]),
            ("Setup", &["Facilities", "Freight services", "Price rule"]),
        ];
        for (page, panels) in pages {
            rail(&mut harness, page);
            for panel in panels {
                // Once each (a tab bar or a second copy would show twice); a title that is also
                // a rail label shows once more, on the rail.
                let on_rail = [
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
        for panel in ["Plans", "Pasted stock", "Plan", "Steps", "Buy list", "Item"] {
            assert_eq!(harness.query_all_by_label(panel).count(), 1, "{panel}");
        }
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

    #[test]
    fn the_build_page_matches_its_snapshot() {
        let env = env();
        let mut harness = start(&env, FakeEve::new(Vec::new(), None), NOW);
        loaded(&mut harness);
        harness.run();
        harness.snapshot("pages-build");
    }
}

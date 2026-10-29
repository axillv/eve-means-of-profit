//! Acceptance tests for the Item panel: the owned rows, the running jobs, the Forge prices with the
//! age of the file, and the saved plans that use the selected item. The game data is the build
//! 3607705 fixture, imported in the test. The login is a fake, and jEve's profile is a made-up file
//! in a temp folder. The clock is fixed at 1,000,000.
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

    use egui::{OutputCommand, vec2};
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;
    use eve_means_of_profit::build::prices::{Failure, Prices, failure_text};
    use eve_means_of_profit::claims::ClaimsState;
    use eve_means_of_profit::item_panel::inspector;
    use eve_means_of_profit::selection::Selection;
    use eve_means_of_profit::session::{EveAccess, PendingLogin, Session};
    use eve_means_of_profit::theme::Style;
    use eve_means_of_profit::worker::Worker;
    use eve_means_of_profit::{App, AppConfig, Shared};
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use mop_core::freight::Packing;
    use mop_core::stock::{Asset, CharacterAssets, Places, StockScope};
    use mop_esi::character_data::{
        CharacterData, Fetched, Job, JobStatus, Kind, OwnedAssets, Refreshed,
    };
    use mop_esi::characters::{Character, Login};
    use mop_esi::error::EsiError;
    use mop_esi::industry::IndustryData;
    use mop_esi::places::{NamingReport, StructureNames};
    use mop_esi::saved;
    use mop_market::book::PriceRule;
    use mop_market::error::MarketError;
    use mop_market::fuzzwork::{BytesHttp, BytesResponse};
    use mop_sde::{Fetch, FetchError, Sde, import};
    use mop_store::reservations::PlanStock;
    use mop_store::{Density, PlanRecord, Store};
    use rusqlite::Connection;
    use tempfile::TempDir;

    const NOW: i64 = 1_000_000;
    const FIXTURE: &str = "../sde/tests/fixtures/sde-3607705";
    const MAIN: u64 = 2_112_000_001;
    const JITA_4_4: i64 = 60_003_760;
    const HOME_FORT: i64 = 1_035_466_617_946;
    const JITA_NAME: &str = "Jita IV - Moon 4 - Caldari Navy Assembly Plant";
    const PAU: u32 = 11_688;
    const MORPHITE: u32 = 11_399;
    const HEADER: &str =
        "what,weightedaverage,maxval,minval,stddev,median,volume,numorders,fivepercent,orderSet";

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

    /// What the fake Fuzzwork file answers with.
    #[derive(Clone)]
    enum Answer {
        /// The file, with its Last-Modified time in Unix seconds.
        File { published: i64, rows: String },
        /// A download that fails.
        Down,
    }

    /// The fake Fuzzwork file. Every request is counted. The answer can be changed by a test.
    struct FakeForge {
        answer: Arc<Mutex<Answer>>,
        calls: Arc<Mutex<usize>>,
    }

    impl BytesHttp for FakeForge {
        fn get_bytes(
            &self,
            _url: &str,
            _if_none_match: Option<&str>,
        ) -> Result<BytesResponse, MarketError> {
            *self.calls.lock().unwrap_or_else(PoisonError::into_inner) += 1;
            let answer = self.answer.lock().unwrap_or_else(PoisonError::into_inner);
            match &*answer {
                Answer::Down => Err(MarketError::Network("the file is down".to_owned())),
                Answer::File { published, rows } => {
                    let text = format!("{HEADER}\n{rows}");
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
                        last_modified: Some(http_date(*published)),
                        retry_after: None,
                        body,
                    })
                }
            }
        }
    }

    /// An HTTP date (`Sun, 06 Nov 1994 08:49:37 GMT`) for Unix seconds.
    fn http_date(secs: i64) -> String {
        let days = secs.div_euclid(86_400);
        let within = secs.rem_euclid(86_400);
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = doy - (153 * mp + 2) / 5 + 1;
        let month = if mp < 10 { mp + 3 } else { mp - 9 };
        let year = yoe + era * 400 + i64::from(month <= 2);
        let weekday = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"]
            [usize::try_from(days.rem_euclid(7)).expect("a day of the week")];
        let months = [
            "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
        ];
        format!(
            "{weekday}, {day:02} {} {year} {:02}:{:02}:{:02} GMT",
            months[usize::try_from(month - 1).expect("a month of the year")],
            within / 3_600,
            within % 3_600 / 60,
            within % 60
        )
    }

    /// One row of the fake file: a side (`true` is buy) with its prices in ISK.
    fn row(type_id: u32, buy: bool, price: &str) -> String {
        format!(
            "10000002|{type_id}|{buy},{price},{price},{price},0.0,{price},10,2,{price},175414\n"
        )
    }

    /// A data folder with the game data imported and the user's store.
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

    fn sde(env: &Env) -> Sde {
        Sde::open(&env.data.join("sde.sqlite")).expect("the game data opens")
    }

    /// The state the inspector reads: the fixture game data and an empty session, with no network.
    fn shared(env: &Env) -> Shared {
        Shared {
            sde: Some(sde(env)),
            store: Store::open(&env.data.join("user.sqlite"), NOW)
                .expect("the store opens")
                .store,
            worker: Worker::spawn(egui::Context::default(), Box::new(NoNetwork)),
            market: Arc::new(Prices::new(Box::new(crate::offline::Offline))),
            // The net worth copy job that would ask for PLEX never runs in these tests, so the
            // client is never used.
            plex_http: Arc::new(mop_market::http::UreqHttp::new()),
            copy_notices: Arc::new(Mutex::new(Vec::new())),
            style: Style::eve_dense(),
            density: Density::Normal,
            rail_icons_only: false,
            data_dir: env.data.clone(),
            notices: Vec::new(),
            clock: Box::new(|| NOW),
            session: Session::new(Box::new(FakeEve {
                characters: Vec::new(),
                jeve: None,
            })),
            selection: Selection::default(),
            industry: IndustryData::default(),
            claims: ClaimsState::default(),
        }
    }

    /// A harness that draws only the inspector over [`shared`].
    fn inspector_harness(env: &Env) -> Harness<'static, Shared> {
        Harness::builder()
            .with_size(vec2(1280.0, 800.0))
            .build_ui_state(
                |ui, cx: &mut Shared| {
                    cx.style.apply(ui.ctx());
                    inspector(ui, cx);
                },
                shared(env),
            )
    }

    /// A login with the characters given, a jEve folder when there is one.
    #[derive(Clone)]
    struct FakeEve {
        characters: Vec<Character>,
        jeve: Option<PathBuf>,
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

    fn asset(
        item_id: i64,
        type_id: u32,
        location_id: i64,
        quantity: u64,
        singleton: bool,
    ) -> Asset {
        Asset {
            item_id,
            type_id,
            location_id,
            flag: Some("Hangar".to_owned()),
            quantity,
            singleton,
        }
    }

    /// Main's saved data at `fetched`: 40 PAU in Jita 4-4, 120 PAU in Home Fort, and one active
    /// manufacturing job of 10 runs of PAU in Home Fort that ends at `now` + 7,500.
    fn main_data(fetched: i64, now: i64) -> CharacterData {
        let stock = CharacterAssets {
            character_id: i64::try_from(MAIN).expect("an id that fits"),
            updated: Some(fetched),
            assets: vec![
                asset(1, PAU, JITA_4_4, 40, false),
                asset(2, PAU, HOME_FORT, 120, false),
            ],
        };
        let job = Job {
            job_id: 1,
            activity_id: 1,
            blueprint_type_id: 1,
            product_type_id: Some(PAU),
            runs: 10,
            status: JobStatus::Active,
            start: now - 100,
            end: now + 7_500,
            facility_id: HOME_FORT,
            station_id: JITA_4_4,
            output_location_id: HOME_FORT,
            installer_id: i64::try_from(MAIN).expect("an id that fits"),
            cost: None,
            licensed_runs: None,
        };
        CharacterData {
            character_id: MAIN,
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
            jobs: Some(Fetched {
                data: vec![job],
                fetched_at: fetched,
                last_modified: None,
                expires: None,
            }),
            wallet: None,
        }
    }

    /// A jEve profile folder with `#Default.db` built from `sql`.
    fn jeve_folder(sql: &str) -> TempDir {
        let folder = TempDir::new().expect("a temp folder");
        let connection = Connection::open(folder.path().join("#Default.db")).expect("database");
        connection.execute_batch(sql).expect("fixture builds");
        drop(connection);
        folder
    }

    /// Alt, jEve only: 60 PAU packaged in Jita 4-4 and 1 PAU as a singleton there.
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
              ('j2', 2112000002, 'Alt', 1, 0, 700000000, 'esi-assets.read_assets.v1');
             INSERT INTO assets (accountid, itemid, typeid, locationid, flagid, flagstring, count, singleton, rawquantity) VALUES
              ('j2', 5001, 11688, 60003760, 4, 'Hangar', 60, 0, NULL),
              ('j2', 5002, 11688, 60003760, 4, 'Hangar', 1, 1, NULL);",
        )
    }

    /// The app with the fake login, jEve's profile and the fake Fuzzwork file. Returns the file's
    /// request count with the app.
    fn start(
        env: &Env,
        eve: FakeEve,
        answer: Answer,
    ) -> (Harness<'static, App>, Arc<Mutex<usize>>) {
        start_with(env, eve, Arc::new(Mutex::new(answer)))
    }

    /// As `start`, with the Fuzzwork answer in `answer` so the test can change it.
    fn start_with(
        env: &Env,
        eve: FakeEve,
        answer: Arc<Mutex<Answer>>,
    ) -> (Harness<'static, App>, Arc<Mutex<usize>>) {
        let config = AppConfig {
            data_dir: env.data.clone(),
            cache_dir: env.cache.clone(),
            fetch: Box::new(NoNetwork),
            clock: Box::new(|| NOW),
            eve: Box::new(eve),
            market: Box::new(crate::offline::Offline),
        };
        let mut harness = Harness::builder()
            .with_size(vec2(1280.0, 800.0))
            .with_max_steps(crate::SETTLE_STEPS)
            .build_eframe(move |cc| App::new(&cc.egui_ctx, config).expect("the app starts"));
        crate::ui::no_images(&mut harness);
        let calls = Arc::new(Mutex::new(0));
        let forge = FakeForge {
            answer,
            calls: Arc::clone(&calls),
        };
        harness.state_mut().shared_mut().market = Arc::new(Prices::new(Box::new(forge)));
        harness
            .state_mut()
            .show_page(eve_means_of_profit::rail::Page::Build);
        (harness, calls)
    }

    fn shown<S>(harness: &Harness<'_, S>, text: &str) -> bool {
        harness.query_all_by_label(text).next().is_some()
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

    /// Selects `type_id` in the picked item and runs a frame.
    fn select(harness: &mut Harness<'_, App>, type_id: u32) {
        harness.state_mut().shared_mut().selection.item = Some(type_id);
        harness.run();
    }

    /// Asks the market for the file and waits until it is held.
    fn fetch_market(harness: &mut Harness<'_, App>) {
        {
            let shared = harness.state_mut().shared_mut();
            let market = Arc::clone(&shared.market);
            market.refresh(&shared.worker, NOW, true);
        }
        for _ in 0..500 {
            harness.run();
            let shared = harness.state_mut().shared_mut();
            if shared.market.held().published_at.is_some() || shared.market.stopped() {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("the market file did not arrive");
    }

    /// Two plans: `Hobgoblin 10` (Hobgoblin II × 10, Particle Accelerator Units bought) and
    /// `Rifter 31` (Rifter × 31), and one whose product is not in the game data.
    fn save_plans(env: &Env) {
        let game = sde(env);
        let hobgoblin = game
            .find_exact("Hobgoblin II")
            .expect("the name searches")
            .expect("Hobgoblin II is in the fixture");
        let rifter = game
            .find_exact("Rifter")
            .expect("the name searches")
            .expect("Rifter is in the fixture");
        let mut store = Store::open(&env.data.join("user.sqlite"), NOW)
            .expect("the store opens")
            .store;
        for (name, product, quantity) in [
            ("Hobgoblin 10", hobgoblin, 10),
            ("Rifter 31", rifter, 31),
            ("Missing", 9_999_999, 1),
        ] {
            store
                .create_plan(
                    &PlanRecord {
                        id: 0,
                        name: name.to_owned(),
                        product,
                        quantity,
                        build: BTreeSet::new(),
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
    }

    /// The fixture's Particle Accelerator Unit and Hobgoblin II, found by name.
    fn hobgoblin_id(env: &Env) -> u32 {
        sde(env)
            .find_exact("Hobgoblin II")
            .expect("the name searches")
            .expect("Hobgoblin II is in the fixture")
    }

    /// The fixture's place names: Home Fort is typed, Jita comes from the game data.
    fn name_home_fort(env: &Env) {
        let mut store = Store::open(&env.data.join("user.sqlite"), NOW)
            .expect("the store opens")
            .store;
        store
            .set_structure_name(HOME_FORT, "Home Fort", NOW)
            .expect("the typed name is saved");
    }

    /// Main's saved data, saved before the app starts; Alt comes from jEve.
    fn with_main(env: &Env) {
        saved::save_character(&esi_dir(env), &main_data(990_000, NOW)).expect("Main's data saved");
    }

    fn fresh_market(published: i64) -> Answer {
        Answer::File {
            published,
            rows: format!(
                "{}{}",
                row(PAU, false, "61200.00"),
                row(PAU, true, "55010.00")
            ),
        }
    }

    #[test]
    fn nothing_selected_shows_no_inspector_on_build_or_assets() {
        let env = env();
        let (mut harness, _) = start(
            &env,
            FakeEve {
                characters: Vec::new(),
                jeve: None,
            },
            Answer::Down,
        );
        loaded(&mut harness);
        assert_no_inspector(&harness, "Build");

        harness
            .get_all_by_label("Assets")
            .next()
            .expect("a rail entry")
            .click();
        harness.run();
        assert_no_inspector(&harness, "Assets");
    }

    /// Neither the window nor its docked place is drawn before an item is picked.
    fn assert_no_inspector(harness: &Harness<'_, App>, page: &str) {
        assert!(
            !shown(harness, "Click an item to see it here."),
            "{page}: no click hint"
        );
        assert_eq!(
            harness.query_all_by_label("Item").count(),
            0,
            "{page}: no Item window or tab"
        );
    }

    #[test]
    fn a_click_on_a_step_row_opens_the_inspector_and_escape_or_close_hides_it() {
        let env = env();
        save_plans(&env);
        let (mut harness, _) = start(
            &env,
            FakeEve {
                characters: Vec::new(),
                jeve: None,
            },
            Answer::Down,
        );
        loaded(&mut harness);

        harness.get_by_label("Hobgoblin 10").click();
        harness.run();
        click_first_step(&mut harness);
        assert_eq!(harness.state_mut().shared_mut().selection.item, Some(PAU));
        assert!(shown(&harness, "None owned."));
        assert!(shown(&harness, "Hobgoblin 10 · need 10"));

        harness.key_press(egui::Key::Escape);
        harness.run();
        assert!(!shown(&harness, "None owned."));
        assert!(!shown(&harness, "Hobgoblin 10 · need 10"));
        assert_eq!(harness.state_mut().shared_mut().selection.item, None);

        click_first_step(&mut harness);
        assert!(shown(&harness, "None owned."));

        harness.get_by_label("Close window").click();
        harness.run();
        assert!(!shown(&harness, "None owned."));
        assert_eq!(harness.state_mut().shared_mut().selection.item, None);
    }

    #[test]
    fn pin_keeps_the_inspector_open_through_escape_and_close() {
        let env = env();
        save_plans(&env);
        let (mut harness, _) = start(
            &env,
            FakeEve {
                characters: Vec::new(),
                jeve: None,
            },
            Answer::Down,
        );
        loaded(&mut harness);

        harness.get_by_label("Hobgoblin 10").click();
        harness.run();
        click_first_step(&mut harness);

        harness.get_by_label("Pin").click();
        harness.run();
        assert!(harness.state_mut().shared_mut().selection.item_pinned);
        harness.key_press(egui::Key::Escape);
        harness.run();
        assert!(shown(&harness, "None owned."));
        assert_eq!(harness.state_mut().shared_mut().selection.item, Some(PAU));

        harness.get_by_label("Close window").click();
        harness.run();
        assert!(shown(&harness, "None owned."));
        assert_eq!(harness.state_mut().shared_mut().selection.item, Some(PAU));

        harness.get_by_label("Pin").click();
        harness.run();
        assert!(!harness.state_mut().shared_mut().selection.item_pinned);
        harness.key_press(egui::Key::Escape);
        harness.run();
        assert!(!shown(&harness, "None owned."));
        assert_eq!(harness.state_mut().shared_mut().selection.item, None);
    }

    /// Clicks the first step row for a Particle Accelerator Unit and runs a frame.
    fn click_first_step(harness: &mut Harness<'_, App>) {
        harness
            .get_all_by_label("Particle Accelerator Unit")
            .next()
            .expect("the step row")
            .click();
        harness.run();
    }

    #[test]
    fn the_owned_table_copies_once_from_the_inspector() {
        let env = env();
        with_main(&env);
        name_home_fort(&env);
        let (mut harness, _) = start(
            &env,
            FakeEve {
                characters: vec![character(MAIN, "Main")],
                jeve: None,
            },
            Answer::Down,
        );
        loaded(&mut harness);
        select(&mut harness, PAU);
        // The copy button appears on the frame after the table first draws.
        harness.run();
        assert_eq!(harness.query_all_by_label("Copy table").count(), 1);

        harness.get_by_label("Copy table").click();
        harness.step();
        let copied = copied_text(&harness).expect("the owned table is copied");
        assert!(
            copied.starts_with("Character\tPlace\tUnits\tCounted\nMain\tHome Fort\t120\t120\n"),
            "{copied}"
        );
        assert!(copied.contains("\nTotal\t\t"), "{copied}");
    }

    #[test]
    fn the_inspector_draws_the_item_window_only_when_an_item_is_picked() {
        let env = env();
        let mut harness = inspector_harness(&env);
        harness.run();
        assert!(!shown(&harness, "Item"));

        harness.state_mut().selection.item = Some(PAU);
        harness.run();
        for text in [
            "Item",
            "Particle Accelerator Unit",
            "None owned.",
            "Plans that use it",
            "No saved plan uses it.",
            "Pin",
        ] {
            assert!(shown(&harness, text), "{text}");
        }
    }

    #[test]
    fn escape_and_close_clear_the_item_unless_pinned() {
        let env = env();
        let mut harness = inspector_harness(&env);
        harness.state_mut().selection.item = Some(PAU);
        harness.run();
        assert!(shown(&harness, "None owned."));

        harness.key_press(egui::Key::Escape);
        harness.run();
        assert!(!shown(&harness, "None owned."));
        assert_eq!(harness.state().selection.item, None);

        {
            let selection = &mut harness.state_mut().selection;
            selection.item = Some(PAU);
            selection.item_pinned = true;
        }
        harness.run();
        assert!(shown(&harness, "None owned."));

        harness.key_press(egui::Key::Escape);
        harness.run();
        assert!(shown(&harness, "None owned."));
        assert_eq!(harness.state().selection.item, Some(PAU));

        harness.get_by_label("Close window").click();
        harness.run();
        assert!(shown(&harness, "None owned."));
        assert_eq!(harness.state().selection.item, Some(PAU));

        harness.get_by_label("Pin").click();
        harness.run();
        harness.key_press(egui::Key::Escape);
        harness.run();
        assert!(!shown(&harness, "None owned."));
        assert_eq!(harness.state().selection.item, None);
    }

    #[test]
    fn owned_rows_and_the_total_for_pau() {
        let env = env();
        with_main(&env);
        name_home_fort(&env);
        let jeve = jeve_profile();
        let eve = FakeEve {
            characters: vec![character(MAIN, "Main")],
            jeve: Some(jeve.path().to_path_buf()),
        };
        let (mut harness, _) = start(&env, eve, fresh_market(NOW - 240));
        loaded(&mut harness);
        select(&mut harness, PAU);

        assert!(shown(&harness, "Home Fort"));
        assert!(shown(&harness, "Alt (jEve)"));
        assert!(shown(&harness, JITA_NAME));
        assert!(!shown(&harness, "None owned."));
        // Rows in order: Main's 120 in Home Fort, Alt's 61 in Jita, Main's 40 in Jita, then the total.
        let home = centre_ys(&harness, "Home Fort");
        let alt = centre_ys(&harness, "Alt (jEve)");
        let jita = centre_ys(&harness, JITA_NAME);
        let total = centre_ys(&harness, "Total");
        assert!(
            on_row(&harness, "Home Fort", &["120", "120"]),
            "Main's row in Home Fort"
        );
        assert!(on_row(&harness, "Alt (jEve)", &["61", "60"]), "Alt's row");
        assert!(
            on_row(&harness, JITA_NAME, &["40", "40"]),
            "Main's row in Jita"
        );
        assert!(on_row(&harness, "Total", &["221", "220"]), "the footer");
        let first_alt = alt.iter().copied().fold(f32::MAX, f32::min);
        let last_jita = jita.iter().copied().fold(f32::MIN, f32::max);
        let first_total = total.iter().copied().fold(f32::MAX, f32::min);
        assert!(
            home.iter().any(|y| *y < first_alt),
            "Home Fort's row is first"
        );
        assert!(first_alt < last_jita, "Alt's row is above Main's Jita row");
        assert!(last_jita < first_total, "the footer is last");
    }

    /// The centre heights of the labels `text`, one per node with that label.
    fn centre_ys(harness: &Harness<'_, App>, text: &str) -> Vec<f32> {
        harness
            .query_all_by_label(text)
            .map(|node| node.rect().center().y)
            .collect()
    }

    /// Whether each of `cells` is drawn on the same line as a label `anchor`.
    fn on_row(harness: &Harness<'_, App>, anchor: &str, cells: &[&str]) -> bool {
        let rows = centre_ys(harness, anchor);
        cells.iter().all(|cell| {
            centre_ys(harness, cell)
                .iter()
                .any(|y| rows.iter().any(|row| (row - y).abs() < 3.0))
        })
    }

    /// Main's saved data with its job ending at `end`.
    fn save_main_with_job_ending(env: &Env, end: i64) {
        let mut data = main_data(990_000, NOW);
        if let Some(jobs) = data.jobs.as_mut() {
            for job in &mut jobs.data {
                job.end = end;
            }
        }
        saved::save_character(&esi_dir(env), &data).expect("Main's data saved");
    }

    #[test]
    fn a_job_that_has_ended_shows_ready() {
        let env = env();
        save_main_with_job_ending(&env, NOW);
        name_home_fort(&env);
        let (mut harness, _) = start(
            &env,
            FakeEve {
                characters: vec![character(MAIN, "Main")],
                jeve: None,
            },
            Answer::Down,
        );
        loaded(&mut harness);
        select(&mut harness, PAU);

        assert!(
            on_row(&harness, "Home Fort", &["ready"]),
            "a job that ends now is ready"
        );
        assert!(!shown(&harness, "in 0s"));
    }

    #[test]
    fn the_counted_column_can_be_hidden_and_the_choice_is_saved() {
        let env = env();
        with_main(&env);
        name_home_fort(&env);
        let (mut harness, _) = start(
            &env,
            FakeEve {
                characters: vec![character(MAIN, "Main")],
                jeve: None,
            },
            Answer::Down,
        );
        loaded(&mut harness);
        select(&mut harness, PAU);
        assert!(shown(&harness, "Counted"));

        harness
            .get_all_by_label("Columns")
            .next()
            .expect("a Columns menu")
            .click();
        harness.run();
        harness
            .get_all_by_label("Counted")
            .last()
            .expect("the Counted tick")
            .click();
        harness.run();
        harness.key_press(egui::Key::Escape);
        harness.run();

        assert!(!shown(&harness, "Counted"), "the Counted column is hidden");
        let saved = harness
            .state_mut()
            .shared_mut()
            .store
            .setting("columns.item_owned")
            .expect("the setting reads");
        assert_eq!(saved.as_deref(), Some("counted"));
    }

    #[test]
    fn running_job_row_for_pau() {
        let env = env();
        with_main(&env);
        name_home_fort(&env);
        let (mut harness, _) = start(
            &env,
            FakeEve {
                characters: vec![character(MAIN, "Main")],
                jeve: None,
            },
            Answer::Down,
        );
        loaded(&mut harness);
        select(&mut harness, PAU);

        assert!(shown(&harness, "Running jobs"));
        assert!(
            on_row(&harness, "Home Fort", &["10", "in 2h"]),
            "Main · Home Fort · 10 · in 2h"
        );
    }

    #[test]
    fn prices_and_the_age_of_the_file() {
        let env = env();
        with_main(&env);
        let (mut harness, calls) = start(
            &env,
            FakeEve {
                characters: vec![character(MAIN, "Main")],
                jeve: None,
            },
            fresh_market(NOW - 240),
        );
        loaded(&mut harness);
        select(&mut harness, PAU);
        fetch_market(&mut harness);
        harness.run();

        assert!(shown(&harness, "Forge sell 61,200.00 · buy 55,010.00"));
        assert!(shown(&harness, "4m"), "the file is 4m old");
        let after_first = *calls.lock().unwrap_or_else(PoisonError::into_inner);
        for _ in 0..10 {
            harness.run();
        }
        select(&mut harness, MORPHITE);
        select(&mut harness, PAU);
        assert_eq!(
            *calls.lock().unwrap_or_else(PoisonError::into_inner),
            after_first,
            "the panel makes no market request"
        );
    }

    #[test]
    fn an_old_file_shows_its_age_in_amber() {
        let env = env();
        with_main(&env);
        let (mut harness, _) = start(
            &env,
            FakeEve {
                characters: vec![character(MAIN, "Main")],
                jeve: None,
            },
            fresh_market(NOW - 700),
        );
        loaded(&mut harness);
        select(&mut harness, PAU);
        fetch_market(&mut harness);
        harness.run();

        assert!(shown(&harness, "11m"), "the file is 11m old");
    }

    #[test]
    fn no_buy_row_shows_a_dash() {
        let env = env();
        with_main(&env);
        let answer = Answer::File {
            published: NOW - 240,
            rows: row(PAU, false, "61200.00"),
        };
        let (mut harness, _) = start(
            &env,
            FakeEve {
                characters: vec![character(MAIN, "Main")],
                jeve: None,
            },
            answer,
        );
        loaded(&mut harness);
        select(&mut harness, PAU);
        fetch_market(&mut harness);
        harness.run();

        assert!(shown(&harness, "Forge sell 61,200.00 · buy —"));
    }

    #[test]
    fn no_file_yet_says_so() {
        let env = env();
        with_main(&env);
        let (mut harness, _) = start(
            &env,
            FakeEve {
                characters: vec![character(MAIN, "Main")],
                jeve: None,
            },
            Answer::Down,
        );
        loaded(&mut harness);
        select(&mut harness, PAU);

        assert!(shown(&harness, "No market data yet"));
    }

    #[test]
    fn a_failed_download_shows_its_error() {
        let env = env();
        with_main(&env);
        let (mut harness, _) = start(
            &env,
            FakeEve {
                characters: vec![character(MAIN, "Main")],
                jeve: None,
            },
            Answer::Down,
        );
        loaded(&mut harness);
        select(&mut harness, PAU);
        let shared = harness.state_mut().shared_mut();
        let market = Arc::clone(&shared.market);
        market.refresh(&shared.worker, NOW, true);
        let expected = failure_text(&Failure::Market(MarketError::Network(
            "the file is down".to_owned(),
        )));
        until(&mut harness, "the failure", |h| shown(h, &expected));
        assert!(shown(&harness, &expected));
    }

    #[test]
    fn a_failed_download_still_shows_after_the_build_page_takes_its_answer() {
        let env = env();
        with_main(&env);
        let (mut harness, _) = start(
            &env,
            FakeEve {
                characters: vec![character(MAIN, "Main")],
                jeve: None,
            },
            Answer::Down,
        );
        loaded(&mut harness);
        select(&mut harness, PAU);
        let shared = harness.state_mut().shared_mut();
        let market = Arc::clone(&shared.market);
        market.refresh(&shared.worker, NOW, true);
        let expected = failure_text(&Failure::Market(MarketError::Network(
            "the file is down".to_owned(),
        )));
        until(&mut harness, "the failure", |h| shown(h, &expected));
        let taken = harness.state_mut().shared_mut().market.take_outcomes();
        assert!(!taken.is_empty());
        harness.run();
        assert!(shown(&harness, &expected));
    }

    #[test]
    fn saved_plans_that_use_pau_are_listed() {
        let env = env();
        with_main(&env);
        save_plans(&env);
        let (mut harness, _) = start(
            &env,
            FakeEve {
                characters: vec![character(MAIN, "Main")],
                jeve: None,
            },
            Answer::Down,
        );
        loaded(&mut harness);
        select(&mut harness, PAU);

        assert!(shown(&harness, "Hobgoblin 10 · need 10"));
        assert!(!shown(&harness, "Rifter 31 · need 31"));
        assert!(!shown(&harness, "Rifter 31 · product × 31"));
        assert!(!shown(&harness, "Rifter 31 · could not work out the plan"));
        assert!(shown(&harness, "Missing · could not work out the plan"));
    }

    #[test]
    fn a_plan_on_character_stock_still_lists_its_need() {
        let env = env();
        with_main(&env);
        save_plans(&env);
        let mut store = Store::open(&env.data.join("user.sqlite"), NOW)
            .expect("the store opens")
            .store;
        let id = store
            .plans()
            .expect("the plans")
            .iter()
            .find(|plan| plan.name == "Hobgoblin 10")
            .expect("the plan")
            .id;
        let mut record = store
            .plan(id)
            .expect("the plan reads")
            .expect("the plan is saved");
        record.quantity = 300;
        record.name = "Hobgoblin 300".to_owned();
        store.save_plan(&record, NOW).expect("the plan is saved");
        store
            .set_plan_stock(
                id,
                &PlanStock::Characters(StockScope {
                    characters: BTreeSet::from([i64::try_from(MAIN).expect("the id fits")]),
                    places: Places::All,
                }),
            )
            .expect("the character stock is saved");
        drop(store);
        let (mut harness, _) = start(
            &env,
            FakeEve {
                characters: vec![character(MAIN, "Main")],
                jeve: None,
            },
            Answer::Down,
        );
        loaded(&mut harness);
        select(&mut harness, PAU);

        assert!(shown(&harness, "Hobgoblin 300 · need 300"));
        assert!(shown(&harness, "Missing · could not work out the plan"));
        assert!(!shown(
            &harness,
            "Hobgoblin 300 · could not work out the plan"
        ));
    }

    #[test]
    fn clicking_a_plan_line_changes_nothing() {
        let env = env();
        with_main(&env);
        save_plans(&env);
        let (mut harness, _) = start(
            &env,
            FakeEve {
                characters: vec![character(MAIN, "Main")],
                jeve: None,
            },
            Answer::Down,
        );
        loaded(&mut harness);
        select(&mut harness, PAU);

        harness.get_by_label("Hobgoblin 10 · need 10").click();
        harness.run();
        assert_eq!(harness.state_mut().shared_mut().selection.item, Some(PAU));
        assert!(shown(&harness, "Hobgoblin 10 · need 10"));
    }

    #[test]
    fn a_working_download_after_a_failure_clears_the_line() {
        let env = env();
        with_main(&env);
        let answer = Arc::new(Mutex::new(Answer::Down));
        let (mut harness, _) = start_with(
            &env,
            FakeEve {
                characters: vec![character(MAIN, "Main")],
                jeve: None,
            },
            Arc::clone(&answer),
        );
        loaded(&mut harness);
        select(&mut harness, PAU);
        let shared = harness.state_mut().shared_mut();
        let market = Arc::clone(&shared.market);
        market.refresh(&shared.worker, NOW, true);
        let failed = failure_text(&Failure::Market(MarketError::Network(
            "the file is down".to_owned(),
        )));
        until(&mut harness, "the failure", |h| shown(h, &failed));

        *answer.lock().unwrap_or_else(PoisonError::into_inner) = Answer::File {
            published: NOW - 240,
            rows: format!(
                "{}{}",
                row(PAU, false, "61200.00"),
                row(PAU, true, "55010.00")
            ),
        };
        {
            // The failure holds the market for a minute, so the retry comes an hour later.
            let shared = harness.state_mut().shared_mut();
            let market = Arc::clone(&shared.market);
            market.refresh(&shared.worker, NOW + 3_600, true);
        }
        until(&mut harness, "the file", |h| {
            shown(h, "Forge sell 61,200.00 · buy 55,010.00")
        });
        assert!(!shown(&harness, &failed));
    }

    #[test]
    fn the_product_of_a_plan_lists_its_quantity() {
        let env = env();
        with_main(&env);
        save_plans(&env);
        let product = hobgoblin_id(&env);
        let (mut harness, _) = start(
            &env,
            FakeEve {
                characters: vec![character(MAIN, "Main")],
                jeve: None,
            },
            Answer::Down,
        );
        loaded(&mut harness);
        select(&mut harness, product);

        assert!(shown(&harness, "Hobgoblin 10 · product × 10"));
    }

    #[test]
    fn an_item_nobody_owns_shows_none_owned_and_no_jobs() {
        let env = env();
        with_main(&env);
        let (mut harness, _) = start(
            &env,
            FakeEve {
                characters: vec![character(MAIN, "Main")],
                jeve: None,
            },
            Answer::Down,
        );
        loaded(&mut harness);
        select(&mut harness, MORPHITE);

        assert!(shown(&harness, "None owned."));
        assert!(!shown(&harness, "Running jobs"));
    }

    #[test]
    fn the_item_panel_matches_its_snapshot() {
        let env = env();
        with_main(&env);
        name_home_fort(&env);
        let jeve = jeve_profile();
        let eve = FakeEve {
            characters: vec![character(MAIN, "Main")],
            jeve: Some(jeve.path().to_path_buf()),
        };
        let (mut harness, _) = start(&env, eve, fresh_market(NOW - 240));
        loaded(&mut harness);
        harness
            .get_all_by_label("Assets")
            .next()
            .expect("a rail entry")
            .click();
        harness.run();
        select(&mut harness, PAU);
        harness.snapshot("item-pau");
    }
}

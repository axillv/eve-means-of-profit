//! Acceptance tests for the Places panel on the Assets page: the tree rows and their totals, the
//! character filter, the clicks that pick a place, the price basis and the footer, the unpriced
//! marker, blueprints, the name dialog of a structure, and the rule that the page sends no request.
//! The game data is the build 3607705 fixture; the login and the market are fakes, and jEve's
//! profile is a made-up file in a temp folder. The clock is fixed at 1,000,000 (day 11).
#[cfg(test)]
mod tests {

    use std::collections::BTreeSet;
    use std::fs;
    use std::io::Write;
    use std::ops::ControlFlow;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex, PoisonError};
    use std::thread;
    use std::time::Duration;

    use egui::{Event, Key, Modifiers, vec2};
    use egui_kittest::Harness;
    use egui_kittest::kittest::{NodeT, Queryable};
    use eve_means_of_profit::build::prices::Prices;
    use eve_means_of_profit::session::{EveAccess, PendingLogin};
    use eve_means_of_profit::{App, AppConfig};
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use mop_core::money::Money;
    use mop_core::net_worth::{ItemPrices, PriceBasis};
    use mop_core::stock::{Asset, CharacterAssets};
    use mop_esi::character_data::{CharacterData, Fetched, Kind, OwnedAssets, Refreshed};
    use mop_esi::characters::{Character, Login};
    use mop_esi::error::EsiError;
    use mop_esi::places::{NamingReport, StructureNames};
    use mop_esi::saved;
    use mop_market::error::MarketError;
    use mop_market::fuzzwork::{BytesHttp, BytesResponse};
    use mop_sde::{Fetch, FetchError, import};
    use mop_store::Store;
    use rusqlite::Connection;
    use tempfile::TempDir;

    const NOW: i64 = 1_000_000;
    /// Today: 1,000,000 seconds is day 11.
    const DAY: i64 = 11;
    const FIXTURE: &str = "../sde/tests/fixtures/sde-3607705";
    const MAIN: u64 = 2_112_000_001;
    const JITA_4_4: i64 = 60_003_760;
    const HOME_FORT: i64 = 1_035_466_617_946;
    const REFUSED: i64 = 1_035_466_617_999;
    const RAM: u32 = 11_481;
    const RIFTER: u32 = 587;
    /// The Rifter Blueprint: a type in the blueprint category (9) of the fixture.
    const BLUEPRINT: u32 = 691;
    const RIFTER_ITEM: i64 = 9_001;
    const RAM_IN_RIFTER: i64 = 9_002;
    const JITA_4_4_NAME: &str = "Jita IV - Moon 4 - Caldari Navy Assembly Plant";
    const REFUSED_NAME: &str = "Structure 1035466617999 (no access)";
    /// The footer's text with the midpoint saved two hours before the clock.
    const MIDPOINT_FOOTER: &str = "Prices: Forge midpoint · saved 13h ago";
    const NO_PRICES: &str = "No saved prices yet. Refresh a character to save today's prices.";
    const DASH: &str = "—";

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

    /// A market file with no rows, counting the requests made to it.
    #[derive(Clone)]
    struct CountingForge {
        requests: Arc<Mutex<usize>>,
    }

    impl BytesHttp for CountingForge {
        fn get_bytes(
            &self,
            _url: &str,
            _if_none_match: Option<&str>,
        ) -> Result<BytesResponse, MarketError> {
            *self.requests.lock().unwrap_or_else(PoisonError::into_inner) += 1;
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

    /// A login with the characters given and a jEve folder, counting the refreshes it is asked for.
    #[derive(Clone)]
    struct FakeEve {
        characters: Vec<Character>,
        jeve: Option<PathBuf>,
        refreshes: Arc<Mutex<usize>>,
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

    /// A data folder with the game data imported, the store's clock check done, and the structure
    /// names EVE gave: Home Fort is Fort Alpha in Jita, the other one is refused.
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
        let mut store = store_of(&data);
        let mut settings = store.settings().expect("settings");
        settings.last_sde_check = Some(NOW);
        store.save_settings(&settings).expect("settings saved");
        store
            .set_structure_name(HOME_FORT, "Home Fort", NOW)
            .expect("the typed name saved");
        let places = format!(
            "{{\"version\":1,\"structures\":{{\
             \"{HOME_FORT}\":{{\"name\":\"Fort Alpha\",\"system_id\":30000142,\"refused\":false,\"checked_at\":990000}},\
             \"{REFUSED}\":{{\"name\":null,\"system_id\":null,\"refused\":true,\"checked_at\":999000}}}}}}"
        );
        fs::write(data.join("esi").join("places.json"), places).expect("places written");
        Env {
            _dir: dir,
            data,
            cache,
        }
    }

    fn store_of(data: &Path) -> Store {
        Store::open(&data.join("user.sqlite"), NOW)
            .expect("the store opens")
            .store
    }

    /// Main's login data: 40 in Jita 4-4, a Rifter in Jita 4-4 holding 10, 120 in Home Fort and 3
    /// in the refused structure. `extra` adds more assets.
    fn main_data(extra: Vec<Asset>) -> CharacterData {
        let mut assets = vec![
            asset(1, RAM, JITA_4_4, "Hangar", 40, false),
            asset(2, RAM, HOME_FORT, "Hangar", 120, false),
            asset(3, RAM, REFUSED, "Hangar", 3, false),
            asset(RIFTER_ITEM, RIFTER, JITA_4_4, "Hangar", 1, true),
            asset(RAM_IN_RIFTER, RAM, RIFTER_ITEM, "Cargo", 10, false),
        ];
        assets.extend(extra);
        let stock = CharacterAssets {
            character_id: i64::try_from(MAIN).expect("an id that fits"),
            updated: Some(990_000),
            assets,
        };
        CharacterData {
            character_id: MAIN,
            assets: Some(Fetched {
                data: OwnedAssets {
                    assets: stock,
                    names: Default::default(),
                },
                fetched_at: 990_000,
                last_modified: None,
                expires: None,
            }),
            orders: None,
            jobs: None,
            wallet: None,
        }
    }

    fn asset(
        item_id: i64,
        type_id: u32,
        location_id: i64,
        flag: &str,
        quantity: u64,
        singleton: bool,
    ) -> Asset {
        Asset {
            item_id,
            type_id,
            location_id,
            flag: Some(flag.to_owned()),
            quantity,
            singleton,
        }
    }

    /// A jEve profile folder: Alt holds 60 in Jita 4-4, Hauler holds 5 there.
    fn jeve_profile() -> TempDir {
        let folder = TempDir::new().expect("a temp folder");
        let connection = Connection::open(folder.path().join("#Default.db")).expect("database");
        connection
            .execute_batch(
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
            .expect("the fixture builds");
        drop(connection);
        folder
    }

    /// Saves the day's prices. Each pair is a type and its lowest sell and highest buy.
    fn save_prices(env: &Env, prices: &[(u32, ItemPrices)]) {
        store_of(&env.data)
            .save_prices(DAY, prices)
            .expect("prices saved");
    }

    /// RAM sells at 478.60 and buys at 450.00; the Rifter sells at 500,000.00 and buys at 400,000.00.
    fn both_prices() -> Vec<(u32, ItemPrices)> {
        vec![(RAM, ram_price()), (RIFTER, rifter_price())]
    }

    fn ram_price() -> ItemPrices {
        ItemPrices {
            lowest_sell: Some(Money::from_ten_thousandths(4_786_000)),
            highest_buy: Some(Money::from_ten_thousandths(4_500_000)),
            average: None,
        }
    }

    fn rifter_price() -> ItemPrices {
        ItemPrices {
            lowest_sell: Some(Money::from_ten_thousandths(5_000_000_000)),
            highest_buy: Some(Money::from_ten_thousandths(4_000_000_000)),
            average: None,
        }
    }

    fn set_basis(env: &Env, basis: PriceBasis) {
        let mut store = store_of(&env.data);
        let mut settings = store.net_worth_settings().expect("net worth settings");
        settings.basis = basis;
        store
            .save_net_worth_settings(&settings)
            .expect("net worth settings saved");
    }

    /// Opens the app on the Assets page with the login and jEve data the tests above describe.
    fn start(
        env: &Env,
        jeve: &TempDir,
        extra: Vec<Asset>,
        forge: CountingForge,
    ) -> (Harness<'static, App>, Arc<Mutex<usize>>) {
        saved::save_character(&env.data.join("esi"), &main_data(extra)).expect("Main saved");
        let refreshes = Arc::new(Mutex::new(0));
        let eve = FakeEve {
            characters: vec![character(MAIN, "Main")],
            jeve: Some(jeve.path().to_path_buf()),
            refreshes: Arc::clone(&refreshes),
        };
        let config = AppConfig {
            data_dir: env.data.clone(),
            cache_dir: env.cache.clone(),
            fetch: Box::new(NoNetwork),
            clock: Box::new(|| NOW),
            eve: Box::new(eve),
        };
        let mut harness = Harness::builder()
            .with_size(vec2(1280.0, 800.0))
            .build_eframe(move |cc| App::new(&cc.egui_ctx, config).expect("the app starts"));
        harness.state_mut().shared_mut().market = Arc::new(Prices::new(Box::new(forge)));
        until(&mut harness, "the saved data", |h| {
            !shown(h, "Loading character data…")
        });
        click(&mut harness, "Assets");
        (harness, refreshes)
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

    /// The node that shows `text`: its label, or its value for the widgets that keep their text there
    /// (the dropdown and the table cells).
    fn find<'h>(harness: &'h Harness<'_, App>, text: &'h str) -> Option<egui_kittest::Node<'h>> {
        harness
            .query_all_by_label(text)
            .next()
            .or_else(|| harness.query_all_by_value(text).next())
    }

    /// Whether some node shows `text`.
    fn shown(harness: &Harness<'_, App>, text: &str) -> bool {
        find(harness, text).is_some()
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

    /// Clicks the node that shows `label`, after scrolling it into view.
    fn click(harness: &mut Harness<'_, App>, label: &str) {
        find(harness, label)
            .unwrap_or_else(|| panic!("no node labelled {label}"))
            .scroll_to_me();
        harness.run_steps(2);
        find(harness, label)
            .unwrap_or_else(|| panic!("no node labelled {label}"))
            .click();
        harness.run_steps(3);
    }

    /// Right-clicks the node that shows `label` and opens its menu.
    fn right_click(harness: &mut Harness<'_, App>, label: &str) {
        find(harness, label)
            .unwrap_or_else(|| panic!("no node labelled {label}"))
            .click_secondary();
        harness.run_steps(3);
    }

    /// The row labelled `label`: its height on the screen, and the texts of the cells to the right of
    /// its label on the same line, left to right. The first cell is the stacks, the second the value.
    fn row(harness: &mut Harness<'_, App>, label: &str) -> (f32, Vec<String>) {
        let anchor = find(harness, label)
            .unwrap_or_else(|| panic!("no row labelled {label}"))
            .rect();
        let mut cells: Vec<(f32, String)> = harness
            .query_all_by(|node| node.label().is_some() || node.value().is_some())
            .filter_map(|node| {
                let rect = node.rect();
                let text = node.accesskit_node().label().or_else(|| node.value())?;
                let same_line = (rect.center().y - anchor.center().y).abs() < 4.0;
                (same_line && rect.min.x > anchor.min.x).then_some((rect.min.x, text))
            })
            .collect();
        cells.sort_by(|a, b| a.0.total_cmp(&b.0));
        (
            anchor.center().y,
            cells.into_iter().map(|(_, text)| text).collect(),
        )
    }

    /// The rows the spec lists, in order: the label, the stacks and the value.
    fn expected_rows() -> Vec<(&'static str, &'static str, &'static str)> {
        vec![
            ("All places", "7", "560,503.40"),
            ("The Forge", "6", "559,110.50"),
            ("Jita", "6", "559,110.50"),
            ("Home Fort", "1", "55,716.00"),
            (JITA_4_4_NAME, "5", "503,394.50"),
            ("Rifter", "1", "4,643.00"),
            ("Unknown region", "1", "1,392.90"),
            ("Unknown system", "1", "1,392.90"),
            (REFUSED_NAME, "1", "1,392.90"),
        ]
    }

    #[test]
    fn the_tree_lists_each_place_with_its_stacks_and_value_in_order() {
        let env = env();
        let jeve = jeve_profile();
        save_prices(&env, &both_prices());
        let forge = CountingForge {
            requests: Arc::new(Mutex::new(0)),
        };
        let (mut harness, _) = start(&env, &jeve, Vec::new(), forge);
        harness.run();

        let mut last_y = f32::MIN;
        for (label, stacks, value) in expected_rows() {
            let (y, cells) = row(&mut harness, label);
            assert!(y > last_y, "{label} is below the row before it");
            last_y = y;
            assert_eq!(cells.first().map(String::as_str), Some(stacks), "{label}");
            assert_eq!(cells.get(1).map(String::as_str), Some(value), "{label}");
        }
        assert!(
            shown(&harness, MIDPOINT_FOOTER),
            "the footer names the basis and age"
        );
    }

    #[test]
    fn unticking_alt_takes_its_assets_out_and_unticking_all_shows_no_tree() {
        let env = env();
        let jeve = jeve_profile();
        save_prices(&env, &both_prices());
        let forge = CountingForge {
            requests: Arc::new(Mutex::new(0)),
        };
        let (mut harness, _) = start(&env, &jeve, Vec::new(), forge);
        harness.run();

        click(&mut harness, "Characters: all");
        click(&mut harness, "Alt (jEve)");
        harness.key_press(Key::Escape);
        harness.run();
        assert!(shown(&harness, "Characters: 2 of 3"), "the dropdown counts");
        let jita = row(&mut harness, JITA_4_4_NAME).1;
        assert_eq!(jita.first().map(String::as_str), Some("4"));
        assert_eq!(jita.get(1).map(String::as_str), Some("475,536.50"));
        let all = row(&mut harness, "All places").1;
        assert_eq!(all.first().map(String::as_str), Some("6"));
        assert_eq!(all.get(1).map(String::as_str), Some("532,645.40"));

        click(&mut harness, "Characters: 2 of 3");
        click(&mut harness, "Hauler (jEve)");
        harness.key_press(Key::Escape);
        harness.run();
        click(&mut harness, "Characters: 1 of 3");
        click(&mut harness, "Main");
        harness.key_press(Key::Escape);
        harness.run();
        assert!(shown(&harness, "No character picked."), "no tree shows");
        assert!(!shown(&harness, JITA_4_4_NAME), "no rows show");
    }

    #[test]
    fn a_click_on_a_row_picks_that_place_and_all_places_picks_none() {
        let env = env();
        let jeve = jeve_profile();
        save_prices(&env, &both_prices());
        let forge = CountingForge {
            requests: Arc::new(Mutex::new(0)),
        };
        let (mut harness, _) = start(&env, &jeve, Vec::new(), forge);
        harness.run();

        for (label, expected) in [
            ("Home Fort", Some(HOME_FORT)),
            ("Jita", Some(30_000_142)),
            ("Rifter", Some(RIFTER_ITEM)),
            ("All places", None),
        ] {
            click(&mut harness, label);
            let picked = harness.state_mut().shared_mut().selection.place;
            assert_eq!(picked, expected, "clicking {label}");
        }
    }

    #[test]
    fn a_saved_basis_changes_the_values_and_the_footer() {
        let env = env();
        let jeve = jeve_profile();
        save_prices(&env, &both_prices());
        set_basis(&env, PriceBasis::HighestBuy);
        let forge = CountingForge {
            requests: Arc::new(Mutex::new(0)),
        };
        let (mut harness, _) = start(&env, &jeve, Vec::new(), forge);
        harness.run();

        assert_eq!(
            row(&mut harness, "Home Fort").1.get(1).map(String::as_str),
            Some("54,000.00")
        );
        assert!(shown(&harness, "Prices: Forge highest buy · saved 13h ago"));
    }

    #[test]
    fn a_type_without_a_price_shows_a_plus_after_its_value() {
        let env = env();
        let jeve = jeve_profile();
        save_prices(&env, &[(RAM, ram_price())]);
        let forge = CountingForge {
            requests: Arc::new(Mutex::new(0)),
        };
        let (mut harness, _) = start(&env, &jeve, Vec::new(), forge);
        harness.run();

        let (_, cells) = row(&mut harness, JITA_4_4_NAME);
        assert_eq!(cells.get(1).map(String::as_str), Some("53,394.50"));
        assert_eq!(
            cells.get(2).map(String::as_str),
            Some(" +"),
            "the marker follows the value"
        );
        assert!(shown(&harness, MIDPOINT_FOOTER));
    }

    #[test]
    fn with_no_saved_prices_every_value_is_a_dash_and_the_footer_says_so() {
        let env = env();
        let jeve = jeve_profile();
        let forge = CountingForge {
            requests: Arc::new(Mutex::new(0)),
        };
        let (mut harness, _) = start(&env, &jeve, Vec::new(), forge);
        harness.run();

        for (label, _, _) in expected_rows() {
            let (_, cells) = row(&mut harness, label);
            assert_eq!(cells.get(1).map(String::as_str), Some(DASH), "{label}");
        }
        assert!(shown(&harness, NO_PRICES));
    }

    #[test]
    fn a_blueprint_is_counted_but_not_valued_or_marked() {
        let env = env();
        let jeve = jeve_profile();
        save_prices(&env, &both_prices());
        let forge = CountingForge {
            requests: Arc::new(Mutex::new(0)),
        };
        let blueprint = asset(9_100, BLUEPRINT, JITA_4_4, "Hangar", 1, true);
        let (mut harness, _) = start(&env, &jeve, vec![blueprint], forge);
        harness.run();

        let (_, cells) = row(&mut harness, JITA_4_4_NAME);
        assert_eq!(cells.first().map(String::as_str), Some("6"));
        assert_eq!(cells.get(1).map(String::as_str), Some("503,394.50"));
        assert!(!shown(&harness, " +"), "no type is unpriced");
    }

    #[test]
    fn a_structure_is_named_and_cleared_from_its_menu() {
        let env = env();
        let jeve = jeve_profile();
        save_prices(&env, &both_prices());
        let forge = CountingForge {
            requests: Arc::new(Mutex::new(0)),
        };
        let (mut harness, _) = start(&env, &jeve, Vec::new(), forge);
        harness.run();

        right_click(&mut harness, "Jita");
        assert!(
            !shown(&harness, "Name this structure…"),
            "a system has no menu"
        );
        harness.key_press(Key::Escape);
        harness.run();

        right_click(&mut harness, "Home Fort");
        click(&mut harness, "Name this structure…");
        assert!(shown(&harness, "Name this structure"), "the window opens");
        assert!(shown(&harness, "EVE's name: Fort Alpha"));
        let field = harness
            .query_all_by(|node| {
                node.role() == egui::accesskit::Role::TextInput
                    && node.value().as_deref() == Some("Home Fort")
            })
            .count();
        assert_eq!(field, 1, "the field holds the typed name");

        harness
            .get_by(|node| node.role() == egui::accesskit::Role::TextInput)
            .click();
        harness.key_press_modifiers(Modifiers::COMMAND, Key::A);
        harness.event(Event::Text("Fort Bravo".to_owned()));
        harness.run_steps(2);
        click(&mut harness, "Save");
        assert_eq!(
            store_of(&env.data).structure_name(HOME_FORT).expect("read"),
            Some("Fort Bravo".to_owned())
        );
        assert!(shown(&harness, "Fort Bravo"), "the row reads the new name");

        right_click(&mut harness, "Fort Bravo");
        click(&mut harness, "Name this structure…");
        harness
            .get_by(|node| node.role() == egui::accesskit::Role::TextInput)
            .click();
        harness.key_press_modifiers(Modifiers::COMMAND, Key::A);
        harness.key_press(Key::Backspace);
        harness.run_steps(2);
        click(&mut harness, "Save");
        assert_eq!(
            store_of(&env.data).structure_name(HOME_FORT).expect("read"),
            None
        );
        assert!(shown(&harness, "Fort Alpha"), "EVE's name comes back");
    }

    #[test]
    fn a_name_of_a_hundred_and_one_characters_is_refused_and_the_window_stays() {
        let env = env();
        let jeve = jeve_profile();
        save_prices(&env, &both_prices());
        let forge = CountingForge {
            requests: Arc::new(Mutex::new(0)),
        };
        let (mut harness, _) = start(&env, &jeve, Vec::new(), forge);
        harness.run();

        let long = "a".repeat(101);
        let expected_error = store_of(&env.data)
            .set_structure_name(HOME_FORT, &long, NOW)
            .expect_err("a long name is refused")
            .to_string();

        right_click(&mut harness, "Home Fort");
        click(&mut harness, "Name this structure…");
        harness
            .get_by(|node| node.role() == egui::accesskit::Role::TextInput)
            .click();
        harness.key_press_modifiers(Modifiers::COMMAND, Key::A);
        harness.event(Event::Text(long));
        harness.run_steps(2);
        click(&mut harness, "Save");

        assert_eq!(
            store_of(&env.data).structure_name(HOME_FORT).expect("read"),
            Some("Home Fort".to_owned()),
            "the typed name stays"
        );
        assert!(
            shown(&harness, "Name this structure"),
            "the window stays open"
        );
        assert!(
            shown(&harness, &expected_error),
            "the error shows under the field"
        );
    }

    #[test]
    fn the_menu_is_not_offered_on_a_station_row() {
        let env = env();
        let jeve = jeve_profile();
        save_prices(&env, &both_prices());
        let forge = CountingForge {
            requests: Arc::new(Mutex::new(0)),
        };
        let (mut harness, _) = start(&env, &jeve, Vec::new(), forge);
        harness.run();

        right_click(&mut harness, JITA_4_4_NAME);
        assert!(!shown(&harness, "Name this structure…"));
    }

    #[test]
    fn the_page_sends_no_request_while_it_is_shown() {
        let env = env();
        let jeve = jeve_profile();
        save_prices(&env, &both_prices());
        let forge = CountingForge {
            requests: Arc::new(Mutex::new(0)),
        };
        let counted = Arc::clone(&forge.requests);
        let (mut harness, refreshes) = start(&env, &jeve, Vec::new(), forge);
        for _ in 0..10 {
            harness.run();
        }

        assert_eq!(*refreshes.lock().unwrap_or_else(PoisonError::into_inner), 0);
        assert_eq!(*counted.lock().unwrap_or_else(PoisonError::into_inner), 0);
    }

    #[test]
    fn the_assets_tree_matches_its_snapshot() {
        let env = env();
        let jeve = jeve_profile();
        save_prices(&env, &both_prices());
        let forge = CountingForge {
            requests: Arc::new(Mutex::new(0)),
        };
        let (mut harness, _) = start(&env, &jeve, Vec::new(), forge);
        harness.run();
        click(&mut harness, JITA_4_4_NAME);
        harness.run();
        harness.snapshot("assets-tree");
    }
}

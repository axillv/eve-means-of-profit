//! Acceptance tests for the Assets table on the Assets page: grouping, the optional Forge price
//! columns and their saved choices, the age of the oldest data, row clicks, and the app-wide value
//! price seen by the table and the tree in the same frame. The game data is the build 3607705
//! fixture; the login and the market are fakes, and jEve's profile is a made-up file in a temp
//! folder. The clock is fixed at 1,000,000, and the held market file was published at 992,800, two
//! hours before it.
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

    use crate::ui::{click, shown, snapshot, until};
    use egui::{Key, Rect, vec2};
    use egui_kittest::Harness;
    use egui_kittest::kittest::{NodeT, Queryable};
    use eve_means_of_profit::build::prices::Prices;
    use eve_means_of_profit::session::{EveAccess, PendingLogin};
    use eve_means_of_profit::{App, AppConfig};
    use flate2::Compression;
    use flate2::write::GzEncoder;
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
    use mop_store::Store;
    use rusqlite::Connection;
    use tempfile::TempDir;

    const NOW: i64 = 1_000_000;
    /// When the held market file was published: two hours before the clock.
    const PUBLISHED_AT: i64 = 992_800;
    const FIXTURE: &str = "../sde/tests/fixtures/sde-3607705";
    const MAIN: u64 = 2_112_000_001;
    const JITA_4_4: i64 = 60_003_760;
    const HOME_FORT: i64 = 1_035_466_617_946;
    const REFUSED: i64 = 1_035_466_617_999;
    const RAM: u32 = 11_481;
    const RIFTER: u32 = 587;
    const RIFTER_ITEM: i64 = 9_001;
    const RAM_IN_RIFTER: i64 = 9_002;
    const JITA_4_4_NAME: &str = "Jita IV - Moon 4 - Caldari Navy Assembly Plant";
    const DASH: &str = "—";

    /// The acceptance file's rows. R.A.M. sells at 480.00 and buys at 450.00; the Rifter sells at
    /// 505,000.00 and has no buy row.
    const RAM_SELL: &str = "10000002|11481|false,479.2,520,478.6,0,481,9000,12,480,1";
    const RAM_BUY: &str = "10000002|11481|true,448,450,440,0,447,4000,6,449,1";
    const RIFTER_SELL: &str = "10000002|587|false,503000,600000,500000,0,502000,20,4,505000,1";

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

    /// A market file with the given rows, counting the requests made to it.
    #[derive(Clone)]
    struct CountingForge {
        requests: Arc<Mutex<usize>>,
        body: Arc<String>,
    }

    impl CountingForge {
        fn with_rows(rows: &[&str]) -> Self {
            let mut body = String::from(
                "what,weightedaverage,maxval,minval,stddev,median,volume,numorders,fivepercent,orderSet\n",
            );
            for row in rows {
                body.push_str(row);
                body.push('\n');
            }
            Self {
                requests: Arc::new(Mutex::new(0)),
                body: Arc::new(body),
            }
        }

        fn all_rows() -> Self {
            Self::with_rows(&[RAM_SELL, RAM_BUY, RIFTER_SELL])
        }
    }

    impl BytesHttp for CountingForge {
        fn get_bytes(
            &self,
            _url: &str,
            _if_none_match: Option<&str>,
        ) -> Result<BytesResponse, MarketError> {
            *self.requests.lock().unwrap_or_else(PoisonError::into_inner) += 1;
            let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
            encoder
                .write_all(self.body.as_bytes())
                .map_err(|error| MarketError::Network(error.to_string()))?;
            let body = encoder
                .finish()
                .map_err(|error| MarketError::Network(error.to_string()))?;
            Ok(BytesResponse {
                status: 200,
                etag: Some("\"0\"".to_owned()),
                last_modified: Some("Mon, 12 Jan 1970 11:46:40 GMT".to_owned()),
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

    fn test_env() -> Env {
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

    /// A jEve profile folder: Alt holds 60 in Jita 4-4, Hauler holds 5 there and has no update time.
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

    /// Saves the app-wide `Value items at` price.
    fn set_value_price(env: &Env, rule: PriceRule) {
        store_of(&env.data)
            .set_value_price(rule)
            .expect("the value price saved");
    }

    /// Asks the market for the file and waits until the fake's file is held.
    fn fetch_market(harness: &mut Harness<'static, App>) {
        {
            let shared = harness.state_mut().shared_mut();
            let market = Arc::clone(&shared.market);
            market.refresh(&shared.worker, NOW, true);
        }
        for _ in 0..500 {
            harness.run();
            if harness.state_mut().shared_mut().market.held().published_at == Some(PUBLISHED_AT) {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("the market file did not arrive");
    }

    /// Opens the app on the Assets page with the login and jEve data the tests above describe. The
    /// window is wide enough for the table's base columns to show without sideways scrolling.
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
            market: Box::new(crate::offline::Offline),
        };
        let mut harness = Harness::builder()
            .with_size(vec2(2400.0, 900.0))
            .with_max_steps(crate::SETTLE_STEPS)
            .build_eframe(move |cc| App::new(&cc.egui_ctx, config).expect("the app starts"));
        crate::ui::no_images(&mut harness);
        harness.state_mut().shared_mut().market = Arc::new(Prices::new(Box::new(forge)));
        until(&mut harness, "the saved data", |h| {
            !shown(h, "Loading character data…")
        });
        fetch_market(&mut harness);
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

    /// The left and right edges of the Assets panel: Places is a quarter, Assets the middle half,
    /// Item the last quarter.
    fn panel_bounds(harness: &Harness<'_, App>) -> (f32, f32) {
        let width = harness.ctx.viewport_rect().width();
        (width * 0.25, width * 0.75)
    }

    /// The row labelled `label` whose anchor sits between `left` and `right`: its rectangle and
    /// the non-empty texts to its right on the same line, left to right.
    fn row_between(
        harness: &Harness<'_, App>,
        label: &str,
        left: f32,
        right: f32,
    ) -> Option<(Rect, Vec<String>)> {
        let anchor = harness
            .query_all_by_label(label)
            .map(|node| node.rect())
            .find(|rect| rect.center().x > left && rect.center().x < right)?;
        let mut cells: Vec<(f32, String)> = harness
            .query_all_by(|node| node.label().is_some() || node.value().is_some())
            .filter_map(|node| {
                let rect = node.rect();
                let text = node.accesskit_node().label().or_else(|| node.value())?;
                if text.is_empty() {
                    return None;
                }
                let same_line = (rect.center().y - anchor.center().y).abs() < 4.0;
                (same_line && rect.min.x > anchor.min.x && rect.min.x < right)
                    .then_some((rect.min.x, text))
            })
            .collect();
        cells.sort_by(|a, b| a.0.total_cmp(&b.0));
        Some((anchor, cells.into_iter().map(|(_, text)| text).collect()))
    }

    /// The first table row labelled `label`.
    fn table_row(harness: &Harness<'_, App>, label: &str) -> (Rect, Vec<String>) {
        let (left, right) = panel_bounds(harness);
        row_between(harness, label, left, right)
            .unwrap_or_else(|| panic!("no table row labelled {label}"))
    }

    /// Every table row labelled `label`, in tree order.
    fn table_rows(harness: &Harness<'_, App>, label: &str) -> Vec<(Rect, Vec<String>)> {
        let (left, right) = panel_bounds(harness);
        let anchors: Vec<Rect> = harness
            .query_all_by_label(label)
            .map(|node| node.rect())
            .filter(|rect| rect.center().x > left && rect.center().x < right)
            .collect();
        anchors
            .into_iter()
            .map(|anchor| {
                let mut cells: Vec<(f32, String)> = harness
                    .query_all_by(|node| node.label().is_some() || node.value().is_some())
                    .filter_map(|node| {
                        let rect = node.rect();
                        let text = node.accesskit_node().label().or_else(|| node.value())?;
                        if text.is_empty() {
                            return None;
                        }
                        let same_line = (rect.center().y - anchor.center().y).abs() < 4.0;
                        (same_line && rect.min.x > anchor.min.x && rect.min.x < right)
                            .then_some((rect.min.x, text))
                    })
                    .collect();
                cells.sort_by(|a, b| a.0.total_cmp(&b.0));
                (anchor, cells.into_iter().map(|(_, text)| text).collect())
            })
            .collect()
    }

    /// Clicks the `pick`th table node labelled `label`.
    fn click_table(harness: &mut Harness<'_, App>, label: &str, pick: usize) {
        let (left, right) = panel_bounds(harness);
        {
            let node = harness
                .query_all_by_label(label)
                .filter(|node| {
                    let rect = node.rect();
                    rect.center().x > left && rect.center().x < right
                })
                .nth(pick)
                .unwrap_or_else(|| panic!("no table node labelled {label}"));
            node.scroll_to_me();
        }
        harness.run_steps(2);
        {
            let node = harness
                .query_all_by_label(label)
                .filter(|node| {
                    let rect = node.rect();
                    rect.center().x > left && rect.center().x < right
                })
                .nth(pick)
                .unwrap_or_else(|| panic!("no table node labelled {label}"));
            node.hover();
        }
        harness.run_steps(2);
        {
            let node = harness
                .query_all_by_label(label)
                .filter(|node| {
                    let rect = node.rect();
                    rect.center().x > left && rect.center().x < right
                })
                .nth(pick)
                .unwrap_or_else(|| panic!("no table node labelled {label}"));
            node.click();
        }
        harness.run_steps(3);
    }

    /// Clicks the dropdown option labelled `label` inside the Assets panel. A table header or cell
    /// can share the name, so the option is the button rather than a plain label.
    fn click_option(harness: &mut Harness<'_, App>, label: &str) {
        let (left, right) = panel_bounds(harness);
        let rect = harness
            .query_all_by_label(label)
            .find(|node| {
                let rect = node.rect();
                node.accesskit_node().role() == egui::accesskit::Role::Button
                    && rect.center().x > left
                    && rect.center().x < right
            })
            .map(|node| node.rect())
            .unwrap_or_else(|| panic!("no dropdown option labelled {label}"));
        {
            let node = harness
                .query_all_by_label(label)
                .find(|node| node.rect() == rect)
                .unwrap_or_else(|| panic!("no node labelled {label}"));
            node.hover();
        }
        harness.run_steps(2);
        {
            let node = harness
                .query_all_by_label(label)
                .find(|node| node.rect() == rect)
                .unwrap_or_else(|| panic!("no node labelled {label}"));
            node.click();
        }
        harness.run_steps(3);
    }

    fn choose_group(harness: &mut Harness<'_, App>, option: &str) {
        click(harness, "Group by: Nothing");
        click_option(harness, option);
        harness.run();
    }

    /// Opens the Columns menu, clicks the checkbox labelled `label`, and closes the menu.
    fn tick_column(harness: &mut Harness<'_, App>, label: &str) {
        click(harness, "Columns");
        let node = harness
            .query_all_by_label(label)
            .find(|node| node.accesskit_node().toggled().is_some())
            .unwrap_or_else(|| panic!("no checkbox labelled {label}"));
        node.click();
        harness.run_steps(3);
        harness.key_press(Key::Escape);
        harness.run();
    }

    #[test]
    fn grouping_by_character_lists_the_owners_and_their_values_in_name_order() {
        let env = test_env();
        let jeve = jeve_profile();
        let (mut harness, _) = start(&env, &jeve, Vec::new(), CountingForge::all_rows());
        harness.run();
        choose_group(&mut harness, "Character");

        let mut last = f32::MIN;
        for (label, volume, value) in [
            ("Alt (jEve)", "2.40", "28,800.00"),
            ("Hauler (jEve)", "0.20", "2,400.00"),
            ("Main", "27,295.92", "588,040.00"),
        ] {
            let (rect, cells) = table_row(&harness, label);
            assert!(
                rect.center().y > last,
                "{label} is below the group before it"
            );
            last = rect.center().y;
            assert_eq!(cells.last().map(String::as_str), Some(value), "{label}");
            assert_eq!(
                cells.get(cells.len().saturating_sub(2)).map(String::as_str),
                Some(volume),
                "{label} sums its volume"
            );
        }

        // Each group is open on its stacks: Alt's 60 R.A.M. is the first stack shown.
        let (_, alt_stack) = table_row(&harness, "R.A.M.- Robotics");
        assert_eq!(alt_stack.first().map(String::as_str), Some("Alt (jEve)"));
        assert_eq!(alt_stack.get(3).map(String::as_str), Some("60"));
        assert_eq!(alt_stack.get(5).map(String::as_str), Some("28,800.00"));
    }

    #[test]
    fn grouping_by_item_sums_units_and_value_and_a_missing_price_shows_a_dash() {
        let env = test_env();
        let jeve = jeve_profile();
        let (mut harness, _) = start(&env, &jeve, Vec::new(), CountingForge::all_rows());
        harness.run();
        choose_group(&mut harness, "Item");

        let (ram_rect, ram) = table_row(&harness, "R.A.M.- Robotics");
        assert_eq!(
            ram,
            vec!["238", "9.52", "114,240.00"],
            "the item group sums units, volume and value"
        );
        let (rifter_rect, rifter) = table_row(&harness, "Rifter");
        assert!(rifter_rect.center().y > ram_rect.center().y);
        assert_eq!(
            rifter,
            vec!["1", "27,289.00", "505,000.00"],
            "the Rifter group holds its single stack"
        );

        let second = test_env();
        let second_jeve = jeve_profile();
        let (mut harness, _) = start(
            &second,
            &second_jeve,
            Vec::new(),
            CountingForge::with_rows(&[RAM_SELL, RAM_BUY]),
        );
        harness.run();
        choose_group(&mut harness, "Item");
        let (_, ram) = table_row(&harness, "R.A.M.- Robotics");
        assert_eq!(ram, vec!["238", "9.52", "114,240.00"]);
        let (_, rifter) = table_row(&harness, "Rifter");
        assert_eq!(rifter.first().map(String::as_str), Some("1"));
        assert_eq!(rifter.get(1).map(String::as_str), Some("27,289.00"));
        assert!(
            rifter.iter().any(|cell| cell == DASH),
            "a group with nothing priced shows a dash: {rifter:?}"
        );
    }

    #[test]
    fn optional_price_columns_tick_save_reload_and_clear() {
        let env = test_env();
        let jeve = jeve_profile();
        let (mut harness, _) = start(&env, &jeve, Vec::new(), CountingForge::all_rows());
        harness.run();
        assert!(!shown(&harness, "Forge sell"), "no price column at start");

        for label in ["Forge sell", "Forge buy", "Sell average"] {
            tick_column(&mut harness, label);
        }
        assert_eq!(
            store_of(&env.data)
                .setting("assets.columns")
                .expect("the setting reads"),
            Some("forge_sell,forge_buy,sell_average".to_owned())
        );

        harness.set_size(vec2(3200.0, 900.0));
        harness.run();
        let rows = table_rows(&harness, "R.A.M.- Robotics");
        let (_, ram) = rows
            .iter()
            .find(|(_, cells)| cells.iter().any(|cell| cell == "120"))
            .expect("the 120 R.A.M. row");
        assert_eq!(
            ram,
            &vec![
                "Main".to_owned(),
                "Home Fort".to_owned(),
                "Hangar".to_owned(),
                "120".to_owned(),
                "4.80".to_owned(),
                "57,600.00".to_owned(),
                "478.60".to_owned(),
                "450.00".to_owned(),
                "479.20".to_owned(),
            ]
        );
        let (_, rifter) = table_row(&harness, "Rifter");
        assert_eq!(
            rifter,
            vec![
                "Main",
                JITA_4_4_NAME,
                "Hangar",
                "1",
                "27,289.00",
                "505,000.00",
                "500,000.00",
                DASH,
                "503,000.00",
            ]
        );

        // A new app on the same data shows the saved columns.
        drop(harness);
        let (mut harness, _) = start(&env, &jeve, Vec::new(), CountingForge::all_rows());
        harness.run();
        for label in ["Forge sell", "Forge buy", "Sell average"] {
            assert!(shown(&harness, label), "{label} is shown again");
        }

        // Unticking all clears the setting.
        for label in ["Forge sell", "Forge buy", "Sell average"] {
            tick_column(&mut harness, label);
        }
        assert_eq!(
            store_of(&env.data)
                .setting("assets.columns")
                .expect("the setting reads"),
            None
        );
    }

    #[test]
    fn choosing_character_saves_the_group_and_the_footer_says_never() {
        let env = test_env();
        let jeve = jeve_profile();
        let (mut harness, _) = start(&env, &jeve, Vec::new(), CountingForge::all_rows());
        harness.run();
        choose_group(&mut harness, "Character");

        assert_eq!(
            store_of(&env.data)
                .setting("assets.group")
                .expect("the group reads"),
            Some("character".to_owned())
        );
        assert!(shown(&harness, "Oldest data: never"), "Hauler has no time");
    }

    #[test]
    fn a_click_on_the_rifter_stack_picks_the_rifter() {
        let env = test_env();
        let jeve = jeve_profile();
        let (mut harness, _) = start(&env, &jeve, Vec::new(), CountingForge::all_rows());
        harness.run();

        click_table(&mut harness, "Rifter", 0);
        assert_eq!(
            harness.state_mut().shared_mut().selection.item,
            Some(RIFTER)
        );
    }

    #[test]
    fn the_highest_buy_price_changes_the_table_and_the_tree_in_the_same_frame() {
        let env = test_env();
        let jeve = jeve_profile();
        set_value_price(&env, PriceRule::HighestBuy);
        let (mut harness, _) = start(&env, &jeve, Vec::new(), CountingForge::all_rows());
        harness.run();

        let rows = table_rows(&harness, "R.A.M.- Robotics");
        let (_, home) = rows
            .iter()
            .find(|(_, cells)| cells.iter().any(|cell| cell == "Home Fort"))
            .expect("the Home Fort R.A.M. row");
        assert_eq!(home.last().map(String::as_str), Some("54,000.00"));

        let width = harness.ctx.viewport_rect().width();
        let (rect, tree) =
            row_between(&harness, "Home Fort", 0.0, width).expect("the tree's Home Fort row");
        assert!(
            rect.center().x < width * 0.30,
            "the row is in the Places panel"
        );
        assert!(
            tree.iter().any(|cell| cell == "54,000.00"),
            "the tree's Home Fort row shows the new value: {tree:?}"
        );
    }

    #[test]
    fn the_assets_table_matches_its_snapshot() {
        let env = test_env();
        let jeve = jeve_profile();
        let (mut harness, _) = start(&env, &jeve, Vec::new(), CountingForge::all_rows());
        harness.run();
        choose_group(&mut harness, "Character");
        click_table(&mut harness, "R.A.M.- Robotics", 0);
        harness.run();
        snapshot(&mut harness, "assets-table");
    }
}

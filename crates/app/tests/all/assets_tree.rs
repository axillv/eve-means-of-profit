//! Acceptance tests for the Places panel on the Assets page: the tree rows and their totals, the
//! character filter, the clicks that pick a place, the app-wide value price and the footer, the
//! unpriced marker, blueprints, the name dialog of a structure, and the rule that the page sends
//! no request. The game data is the build 3607705 fixture; the login and the market are fakes, and
//! jEve's profile is a made-up file in a temp folder. The clock is fixed at 1,000,000, and the
//! held market file was published at 992,800, two hours before it.
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

    use crate::ui::{click, find, hover, shown, snapshot, top, until};
    use egui::{Event, Key, Modifiers, PointerButton, pos2, vec2};
    use egui_kittest::Harness;
    use egui_kittest::kittest::{NodeT, Queryable};
    use eve_means_of_profit::build::prices::Prices;
    use eve_means_of_profit::session::{EveAccess, PendingLogin};
    use eve_means_of_profit::{App, AppConfig};
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use mop_core::money::Money;
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
    use mop_store::settings::{RuleTarget, RuleValue};
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
    /// The Rifter Blueprint: a type in the blueprint category (9) of the fixture.
    const BLUEPRINT: u32 = 691;
    const RIFTER_ITEM: i64 = 9_001;
    const RAM_IN_RIFTER: i64 = 9_002;
    const JITA_4_4_NAME: &str = "Jita IV - Moon 4 - Caldari Navy Assembly Plant";
    const REFUSED_NAME: &str = "Structure 1035466617999 (no access)";
    /// The footer's text under the default value price.
    const FOOTER: &str = "Prices: Forge sell, 5% price · file made 2h ago";
    const NO_PRICES: &str = "No market prices yet.";
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

        fn no_rows() -> Self {
            Self::with_rows(&[])
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

    /// Saves the app-wide `Value items at` price.
    fn set_value_price(env: &Env, rule: PriceRule) {
        store_of(&env.data)
            .set_value_price(rule)
            .expect("the value price saved");
    }

    /// Saves a price rule in force at the clock.
    fn set_price_rule(env: &Env, target: RuleTarget, value: RuleValue) {
        store_of(&env.data)
            .set_price_rule(target, Some(value), NOW)
            .expect("the price rule saved");
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
            market: Box::new(crate::offline::Offline),
        };
        let mut harness = Harness::builder()
            .with_size(vec2(1280.0, 800.0))
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

    /// A primary click at `pos`: the pointer moves there, then presses and releases.
    fn click_at(harness: &mut Harness<'_, App>, pos: egui::Pos2) {
        harness.hover_at(pos);
        for pressed in [true, false] {
            harness.event(Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            });
        }
        harness.run();
    }

    /// Opens the row labelled `label` by clicking its fold arrow, just left of its text.
    fn open_row(harness: &mut Harness<'_, App>, label: &str) {
        let rect = find(harness, label)
            .unwrap_or_else(|| panic!("no row labelled {label}"))
            .rect();
        click_at(harness, pos2(rect.min.x - 7.0, rect.center().y));
        harness.run();
    }

    /// Whether the Places panel (left of the Assets panel) shows `text`. The Assets panel repeats
    /// some place names, so a global check would find rows the tree has folded away.
    fn shown_in_places(harness: &Harness<'_, App>, text: &str) -> bool {
        let split = harness
            .query_all_by_label("Assets")
            .map(|node| node.rect().min.x)
            .fold(0.0, f32::max);
        harness
            .query_all_by_label(text)
            .any(|node| node.rect().center().x < split)
    }

    /// Opens the whole sample tree: the region, the system, the station and the unknown rows.
    fn open_all(harness: &mut Harness<'_, App>) {
        for label in [
            "The Forge",
            "Jita",
            JITA_4_4_NAME,
            "Unknown region",
            "Unknown system",
        ] {
            open_row(harness, label);
        }
    }

    /// Opens the rows above the Jita station: the region and the system.
    fn open_jita(harness: &mut Harness<'_, App>) {
        open_row(harness, "The Forge");
        open_row(harness, "Jita");
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

    /// The rows the spec lists, in tree order sorted by value: the label, the item count and the value.
    fn expected_rows() -> Vec<(&'static str, &'static str, &'static str)> {
        vec![
            ("All places", "239", "619,240.00"),
            ("The Forge", "236", "617,800.00"),
            ("Jita", "236", "617,800.00"),
            (JITA_4_4_NAME, "116", "560,200.00"),
            ("Rifter", "10", "4,800.00"),
            ("Home Fort", "120", "57,600.00"),
            ("Unknown region", "3", "1,440.00"),
            ("Unknown system", "3", "1,440.00"),
            (REFUSED_NAME, "3", "1,440.00"),
        ]
    }

    #[test]
    fn the_tree_lists_each_place_with_its_stacks_and_value_in_order() {
        let env = env();
        let jeve = jeve_profile();
        let forge = CountingForge::all_rows();
        let (mut harness, _) = start(&env, &jeve, Vec::new(), forge);
        harness.run();
        open_all(&mut harness);

        let mut last_y = f32::MIN;
        for (label, stacks, value) in expected_rows() {
            let (y, cells) = row(&mut harness, label);
            assert!(y > last_y, "{label} is below the row before it");
            last_y = y;
            assert_eq!(cells.first().map(String::as_str), Some(stacks), "{label}");
            assert_eq!(cells.get(1).map(String::as_str), Some(value), "{label}");
        }
        assert!(
            shown(&harness, FOOTER),
            "the footer names the value price and the file's age"
        );
    }

    #[test]
    fn unticking_alt_takes_its_assets_out_and_unticking_all_shows_no_tree() {
        let env = env();
        let jeve = jeve_profile();
        let forge = CountingForge::all_rows();
        let (mut harness, _) = start(&env, &jeve, Vec::new(), forge);
        harness.run();
        open_jita(&mut harness);

        click(&mut harness, "Characters: all");
        click(&mut harness, "Alt (jEve)");
        harness.key_press(Key::Escape);
        harness.run();
        assert!(shown(&harness, "Characters: 2 of 3"), "the dropdown counts");
        let jita = row(&mut harness, JITA_4_4_NAME).1;
        assert_eq!(jita.first().map(String::as_str), Some("56"));
        assert_eq!(jita.get(1).map(String::as_str), Some("531,400.00"));
        let all = row(&mut harness, "All places").1;
        assert_eq!(all.first().map(String::as_str), Some("179"));
        assert_eq!(all.get(1).map(String::as_str), Some("590,440.00"));

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
        let forge = CountingForge::all_rows();
        let (mut harness, _) = start(&env, &jeve, Vec::new(), forge);
        harness.run();
        open_jita(&mut harness);
        open_row(&mut harness, JITA_4_4_NAME);

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
    fn a_value_price_changes_the_values_and_the_footer() {
        let env = env();
        let jeve = jeve_profile();
        set_value_price(&env, PriceRule::HighestBuy);
        let forge = CountingForge::all_rows();
        let (mut harness, _) = start(&env, &jeve, Vec::new(), forge);
        harness.run();
        open_jita(&mut harness);

        assert_eq!(
            row(&mut harness, "Home Fort").1.get(1).map(String::as_str),
            Some("54,000.00")
        );
        let (_, cells) = row(&mut harness, JITA_4_4_NAME);
        assert_eq!(cells.get(1).map(String::as_str), Some("51,750.00"));
        assert_eq!(
            cells.get(2).map(String::as_str),
            Some(" +"),
            "the Rifter has no buy row"
        );
        // The Places panel is narrow at the test's window size, which clips the Value column; a
        // wider window shows the marker so its tooltip can be read.
        harness.set_size(vec2(3200.0, 800.0));
        harness.run();
        let tip = hover(&mut harness, " +");
        assert!(
            tip.iter().any(|text| text == "1 item type not priced"),
            "the marker names the unpriced type: {tip:?}"
        );
        assert!(shown(
            &harness,
            "Prices: Forge buy, highest · file made 2h ago"
        ));
    }

    #[test]
    fn a_type_without_a_row_shows_a_plus_after_its_value() {
        let env = env();
        let jeve = jeve_profile();
        let forge = CountingForge::with_rows(&[RAM_SELL, RAM_BUY]);
        let (mut harness, _) = start(&env, &jeve, Vec::new(), forge);
        harness.run();
        open_jita(&mut harness);

        let (_, cells) = row(&mut harness, JITA_4_4_NAME);
        assert_eq!(cells.get(1).map(String::as_str), Some("55,200.00"));
        assert_eq!(
            cells.get(2).map(String::as_str),
            Some(" +"),
            "the marker follows the value"
        );
        assert!(shown(&harness, FOOTER));
    }

    #[test]
    fn an_item_rule_prices_the_rifter_and_leaves_its_stack_alone() {
        let env = env();
        let jeve = jeve_profile();
        set_price_rule(
            &env,
            RuleTarget::Item(RIFTER),
            RuleValue::Fixed(Money::parse_isk("400,000.00").expect("a test price")),
        );
        let forge = CountingForge::all_rows();
        let (mut harness, _) = start(&env, &jeve, Vec::new(), forge);
        harness.run();
        open_jita(&mut harness);
        open_row(&mut harness, JITA_4_4_NAME);

        assert_eq!(
            row(&mut harness, JITA_4_4_NAME)
                .1
                .get(1)
                .map(String::as_str),
            Some("455,200.00")
        );
        assert_eq!(
            row(&mut harness, "Rifter").1.get(1).map(String::as_str),
            Some("4,800.00")
        );
    }

    #[test]
    fn with_no_market_rows_every_value_is_a_dash_and_the_footer_says_so() {
        let env = env();
        let jeve = jeve_profile();
        let forge = CountingForge::no_rows();
        let (mut harness, _) = start(&env, &jeve, Vec::new(), forge);
        harness.run();
        open_all(&mut harness);

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
        let forge = CountingForge::all_rows();
        let blueprint = asset(9_100, BLUEPRINT, JITA_4_4, "Hangar", 1, true);
        let (mut harness, _) = start(&env, &jeve, vec![blueprint], forge);
        harness.run();
        open_jita(&mut harness);

        let (_, cells) = row(&mut harness, JITA_4_4_NAME);
        assert_eq!(cells.first().map(String::as_str), Some("117"));
        assert_eq!(cells.get(1).map(String::as_str), Some("560,200.00"));
        assert!(!shown(&harness, " +"), "no type is unpriced");
    }

    #[test]
    fn a_structure_is_named_and_cleared_from_its_menu() {
        let env = env();
        let jeve = jeve_profile();
        let forge = CountingForge::all_rows();
        let (mut harness, _) = start(&env, &jeve, Vec::new(), forge);
        harness.run();
        open_jita(&mut harness);

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
            .get_by(|node| {
                node.role() == egui::accesskit::Role::TextInput
                    && node.value().as_deref() == Some("Home Fort")
            })
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
            .get_by(|node| {
                node.role() == egui::accesskit::Role::TextInput
                    && node.value().as_deref() == Some("Fort Bravo")
            })
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
        let forge = CountingForge::all_rows();
        let (mut harness, _) = start(&env, &jeve, Vec::new(), forge);
        harness.run();
        open_jita(&mut harness);

        let long = "a".repeat(101);
        let expected_error = store_of(&env.data)
            .set_structure_name(HOME_FORT, &long, NOW)
            .expect_err("a long name is refused")
            .to_string();

        right_click(&mut harness, "Home Fort");
        click(&mut harness, "Name this structure…");
        harness
            .get_by(|node| {
                node.role() == egui::accesskit::Role::TextInput
                    && node.value().as_deref() == Some("Home Fort")
            })
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
        let forge = CountingForge::all_rows();
        let (mut harness, _) = start(&env, &jeve, Vec::new(), forge);
        harness.run();
        open_jita(&mut harness);

        right_click(&mut harness, JITA_4_4_NAME);
        assert!(!shown(&harness, "Name this structure…"));
    }

    #[test]
    fn the_page_sends_no_request_while_it_is_shown() {
        let env = env();
        let jeve = jeve_profile();
        let forge = CountingForge::all_rows();
        let counted = Arc::clone(&forge.requests);
        let (mut harness, refreshes) = start(&env, &jeve, Vec::new(), forge);
        for _ in 0..10 {
            harness.run();
        }
        let started = *refreshes.lock().unwrap_or_else(PoisonError::into_inner);
        let counted_at_start = *counted.lock().unwrap_or_else(PoisonError::into_inner);
        // The start refreshed Main once and fetched the file once.
        assert_eq!(started, 1);
        assert_eq!(counted_at_start, 1);

        // The tree must not ask for another refresh or another file while it is shown.
        for _ in 0..10 {
            harness.run();
        }
        assert_eq!(
            *refreshes.lock().unwrap_or_else(PoisonError::into_inner),
            started
        );
        assert_eq!(
            *counted.lock().unwrap_or_else(PoisonError::into_inner),
            counted_at_start
        );
    }

    #[test]
    fn with_stock_and_game_data_the_panels_draw_no_empty_state() {
        let env = env();
        let jeve = jeve_profile();
        let forge = CountingForge::all_rows();
        let (mut harness, _) = start(&env, &jeve, Vec::new(), forge);
        harness.run();
        open_jita(&mut harness);

        assert!(shown(&harness, JITA_4_4_NAME), "the places tree shows");
        for text in [
            "No characters yet.",
            "No assets yet.",
            "The game data is not in yet.",
            "No character picked.",
            "Add character",
            "Refresh a character",
        ] {
            assert!(!shown(&harness, text), "{text:?} is not drawn");
        }
    }

    #[test]
    fn the_tree_starts_at_its_top_level_and_the_arrows_open_a_level() {
        let env = env();
        let jeve = jeve_profile();
        let forge = CountingForge::all_rows();
        let (mut harness, _) = start(&env, &jeve, Vec::new(), forge);
        harness.run();

        assert!(shown(&harness, "Place"));
        assert!(shown(&harness, "Count"));
        assert!(shown(&harness, "Value"));
        for label in ["All places", "The Forge", "Unknown region"] {
            assert!(shown_in_places(&harness, label), "{label} shows at the top");
        }
        assert!(
            !shown_in_places(&harness, "Jita"),
            "the region starts folded"
        );
        assert!(
            !shown_in_places(&harness, "Home Fort"),
            "the system starts folded"
        );
        assert!(top(&harness, "All places") < top(&harness, "The Forge"));
        assert!(top(&harness, "The Forge") < top(&harness, "Unknown region"));
        let all = row(&mut harness, "All places").1;
        assert_eq!(all.first().map(String::as_str), Some("239"));
        assert_eq!(all.get(1).map(String::as_str), Some("619,240.00"));

        // A click 13 points left of the label lands on The Forge's fold arrow.
        let forge = find(&harness, "The Forge").expect("the region row").rect();
        click_at(&mut harness, pos2(forge.min.x - 13.0, forge.center().y));
        harness.run();
        assert!(shown_in_places(&harness, "Jita"), "the region opens");
        assert!(
            !shown_in_places(&harness, JITA_4_4_NAME),
            "the system stays folded"
        );

        let jita = find(&harness, "Jita").expect("the system row").rect();
        click_at(&mut harness, pos2(jita.min.x - 13.0, jita.center().y));
        harness.run();
        assert!(shown_in_places(&harness, JITA_4_4_NAME));
        assert!(shown_in_places(&harness, "Home Fort"));
        assert!(
            top(&harness, JITA_4_4_NAME) < top(&harness, "Home Fort"),
            "the station has the larger value"
        );
        let station = row(&mut harness, JITA_4_4_NAME).1;
        assert_eq!(station.first().map(String::as_str), Some("116"));
        assert_eq!(station.get(1).map(String::as_str), Some("560,200.00"));
        let home = row(&mut harness, "Home Fort").1;
        assert_eq!(home.first().map(String::as_str), Some("120"));
        assert_eq!(home.get(1).map(String::as_str), Some("57,600.00"));
    }

    #[test]
    fn searching_places_keeps_ancestors_and_clearing_restores_the_folds() {
        let env = env();
        let jeve = jeve_profile();
        let forge = CountingForge::all_rows();
        let (mut harness, _) = start(&env, &jeve, Vec::new(), forge);
        harness.run();

        harness
            .get_by(|node| node.role() == egui::accesskit::Role::TextInput)
            .click();
        harness.event(Event::Text("rifter".to_owned()));
        harness.run();
        for label in ["All places", "The Forge", "Jita", JITA_4_4_NAME, "Rifter"] {
            assert!(shown_in_places(&harness, label), "{label} is kept");
        }
        assert!(
            !shown_in_places(&harness, "Home Fort"),
            "the sibling does not match"
        );
        let forge = row(&mut harness, "The Forge").1;
        assert_eq!(forge.first().map(String::as_str), Some("236"));
        assert_eq!(forge.get(1).map(String::as_str), Some("617,800.00"));

        harness.key_press_modifiers(Modifiers::COMMAND, Key::A);
        harness.key_press(Key::Backspace);
        harness.run();
        assert!(
            !shown_in_places(&harness, "Jita"),
            "clearing folds the region again"
        );
        assert!(
            !shown_in_places(&harness, JITA_4_4_NAME),
            "clearing folds the system again"
        );

        harness.event(Event::Text("zzz".to_owned()));
        harness.run();
        assert!(shown(&harness, "No places match."));
        assert!(
            !shown_in_places(&harness, "All places"),
            "no table is drawn"
        );
    }

    #[test]
    fn the_assets_tree_matches_its_snapshot() {
        let env = env();
        let jeve = jeve_profile();
        let forge = CountingForge::all_rows();
        let (mut harness, _) = start(&env, &jeve, Vec::new(), forge);
        harness.run();
        open_jita(&mut harness);
        click(&mut harness, JITA_4_4_NAME);
        harness.run();
        snapshot(&mut harness, "assets-tree");
    }
}

//! Acceptance tests for the Build page's stock from characters: the Stock row, the ticks for
//! characters and places, what they save, and the pasted stock panel. The login is a fake, and
//! jEve's profile is a made-up file in a temp folder. The clock is fixed at 1,000,000.
#[cfg(test)]
mod tests {

    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;
    use std::io::Write;
    use std::ops::ControlFlow;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use egui::accesskit::{Role, Toggled};
    use egui::{Event, Key, Modifiers, PointerButton, pos2, vec2};
    use egui_kittest::kittest::{NodeT, Queryable};
    use egui_kittest::{Harness, SnapshotResults};
    use eve_means_of_profit::build::prices::Prices;
    use eve_means_of_profit::session::{EveAccess, PendingLogin};
    use eve_means_of_profit::{App, AppConfig};
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use mop_core::stock::{Asset, CharacterAssets, Places, StockScope};
    use mop_esi::character_data::{CharacterData, Fetched, Kind, OwnedAssets, Refreshed};
    use mop_esi::characters::{Character, Login};
    use mop_esi::error::EsiError;
    use mop_esi::places::{NamingReport, StructureNames};
    use mop_esi::saved;
    use mop_market::error::MarketError;
    use mop_market::fuzzwork::{BytesHttp, BytesResponse};
    use mop_sde::{Fetch, FetchError, import};
    use mop_store::Store;
    use mop_store::reservations::PlanStock;
    use rusqlite::Connection;
    use tempfile::TempDir;

    const NOW: i64 = 1_000_000;
    const FIXTURE: &str = "../sde/tests/fixtures/sde-3607705";
    const MAIN: i64 = 2_112_000_001;
    const ALT: i64 = 2_112_000_002;
    const HAULER: i64 = 2_112_000_003;
    const UNKNOWN: i64 = 2_112_000_099;
    const JITA_4_4: i64 = 60_003_760;
    const HOME_FORT: i64 = 1_035_466_617_946;
    const RAM: u32 = 11_481;
    const JITA_LABEL: &str = "Jita IV - Moon 4 - Caldari Navy Assembly Plant";
    const NO_CHARACTERS: &str =
        "No character data yet. Log in or read jEve's file on the Characters page.";

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

    /// A data folder with the game data imported and the user's store.
    struct Env {
        _dir: TempDir,
        data: PathBuf,
        cache: PathBuf,
    }

    fn make_env() -> Env {
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
        // The background price refresh stays off: the test asks for each download itself.
        let mut history = store.history_settings().expect("history settings");
        history.auto_refresh = false;
        store
            .save_history_settings(&history)
            .expect("history settings saved");
        store
            .set_structure_name(HOME_FORT, "Home Fort", NOW)
            .expect("the name is saved");
        Env {
            _dir: dir,
            data,
            cache,
        }
    }

    fn esi_dir(env: &Env) -> PathBuf {
        env.data.join("esi")
    }

    /// The login with the characters given, and a jEve folder when there is one.
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

    fn character(id: i64, name: &str) -> Character {
        Character {
            id: u64::try_from(id).expect("an id that fits"),
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

    /// Main's login data: 40 in Jita 4-4 and 120 in Home Fort.
    fn save_main(env: &Env) {
        let stock = CharacterAssets {
            character_id: MAIN,
            updated: Some(990_000),
            assets: vec![asset(1, JITA_4_4, 40), asset(2, HOME_FORT, 120)],
        };
        let data = CharacterData {
            character_id: u64::try_from(MAIN).expect("an id that fits"),
            assets: Some(Fetched {
                data: OwnedAssets {
                    assets: stock,
                    names: BTreeMap::new(),
                },
                fetched_at: 990_000,
                last_modified: None,
                expires: None,
            }),
            orders: None,
            jobs: None,
            wallet: None,
        };
        saved::save_character(&esi_dir(env), &data).expect("Main's data saved");
    }

    /// A jEve profile folder: Alt with 60 in Jita 4-4 and Hauler with 5 there.
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
            .expect("the profile builds");
        drop(connection);
        folder
    }

    fn start(env: &Env, eve: FakeEve) -> Harness<'static, App> {
        start_sized(env, eve, 800.0)
    }

    /// `start` in a window `height` points tall: the Plan panel needs the room at 14 px text.
    fn start_sized(env: &Env, eve: FakeEve, height: f32) -> Harness<'static, App> {
        let config = AppConfig {
            data_dir: env.data.clone(),
            cache_dir: env.cache.clone(),
            fetch: Box::new(NoNetwork),
            clock: Box::new(|| NOW),
            eve: Box::new(eve),
            market: Box::new(crate::offline::Offline),
        };
        let mut harness = Harness::builder()
            .with_size(vec2(1900.0, height))
            .with_max_steps(crate::SETTLE_STEPS)
            .build_eframe(move |cc| App::new(&cc.egui_ctx, config).expect("the app starts"));
        harness.state_mut().shared_mut().market = Arc::new(Prices::new(Box::new(EmptyForge)));
        harness
            .state_mut()
            .show_page(eve_means_of_profit::rail::Page::Build);
        harness.run();
        harness
    }

    fn shown(harness: &Harness<'_, App>, text: &str) -> bool {
        harness.query_all_by_label(text).next().is_some()
    }

    /// Whether some node shows `text` as its value: a box's text or a dropdown's chosen item.
    fn has_value(harness: &Harness<'_, App>, text: &str) -> bool {
        harness
            .query_all(
                egui_kittest::kittest::by()
                    .predicate(|node| node.value().is_some_and(|value| value == text)),
            )
            .next()
            .is_some()
    }

    /// Clicks the last node labelled `label` (a dialog's button when several are drawn).
    fn click(harness: &mut Harness<'_, App>, label: &str) {
        let count = harness.get_all_by_label(label).count();
        assert!(count > 0, "no node labelled {label:?}");
        harness
            .get_all_by_label(label)
            .nth(count - 1)
            .expect("the button")
            .click();
        harness.run();
    }

    /// Clicks the tick or radio button of `role` labelled `label`, near its left edge: a long
    /// label can run under the Steps table, which takes a click at its middle.
    fn click_box(harness: &mut Harness<'_, App>, role: Role, label: &str) {
        let rect = harness
            .get_all_by_role(role)
            .find(|node| node.accesskit_node().label().as_deref() == Some(label))
            .unwrap_or_else(|| panic!("no {label:?} box"))
            .rect();
        let pos = pos2(rect.min.x + 6.0, rect.center().y);
        harness.event(Event::PointerMoved(pos));
        harness.run();
        for pressed in [true, false] {
            harness.event(Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            });
            harness.run();
        }
    }

    /// Whether the tick labelled `label` is on.
    fn ticked(harness: &Harness<'_, App>, label: &str) -> bool {
        harness
            .get_all_by_role(Role::CheckBox)
            .find(|node| node.accesskit_node().label().as_deref() == Some(label))
            .unwrap_or_else(|| panic!("no {label:?} tick"))
            .accesskit_node()
            .toggled()
            == Some(Toggled::True)
    }

    /// Opens the dropdown that shows `current` and picks `option` from it.
    fn pick(harness: &mut Harness<'_, App>, current: &str, option: &str) {
        harness
            .get_by(|node| node.value().is_some_and(|value| value == current))
            .click();
        harness.run();
        click(harness, option);
    }

    /// Opens the pasted stock window from the Stock row, then pastes `text` into its box.
    fn paste(harness: &mut Harness<'_, App>, text: &str) {
        click(harness, "Pasted stock…");
        harness
            .get_by(|node| node.role() == Role::MultilineTextInput)
            .click();
        harness.event(Event::Paste(text.to_owned()));
        harness.run();
        harness.get_by_label("Close window").click();
        harness.run();
    }

    /// The left edge of the Buy list panel, which sits right of the Steps panel.
    fn steps_edge(harness: &Harness<'_, App>) -> f32 {
        harness.get_by_label("Buy list").rect().min.x
    }

    /// The height of the Steps row labelled `row`.
    fn steps_y(harness: &Harness<'_, App>, row: &str) -> f32 {
        let edge = steps_edge(harness);
        harness
            .query_all_by_label(row)
            .find(|node| node.rect().min.x < edge)
            .expect("the row in Steps")
            .rect()
            .center()
            .y
    }

    /// The non-empty cell texts of the Steps row labelled `row`, left to right.
    fn cells(harness: &Harness<'_, App>, row: &str) -> Vec<String> {
        let edge = steps_edge(harness);
        let y = steps_y(harness, row);
        let nodes: Vec<(f32, f32, Option<String>)> = harness
            .query_all(egui_kittest::kittest::by().predicate(|node| node.bounding_box().is_some()))
            .map(|node| (node.rect().min.x, node.rect().center().y, node.value()))
            .collect();
        let start = nodes
            .iter()
            .position(|(x, _, value)| *x < edge && value.as_deref() == Some(row))
            .expect("the row in Steps");
        let mut found: Vec<String> = Vec::new();
        let mut last = f32::MIN;
        for (x, node_y, value) in &nodes[start..] {
            if (node_y - y).abs() >= 4.0 {
                continue;
            }
            if *x < last {
                break;
            }
            last = *x;
            if let Some(value) = value
                && !value.is_empty()
            {
                found.push(value.clone());
            }
        }
        found
    }

    /// The Own and Buy cells of the R.A.M.- Robotics row: the need is 300, Own follows it and
    /// Buy is the third cell after it.
    fn own_and_buy(harness: &Harness<'_, App>) -> (String, String) {
        let row = cells(harness, "R.A.M.- Robotics");
        let need = row
            .iter()
            .position(|value| value == "300")
            .expect("the need of 300");
        (row[need + 1].clone(), row[need + 3].clone())
    }

    /// The labels of the place ticks drawn, in the order they are drawn: the places of the
    /// Steps panel's `Only these:` list.
    fn place_ticks_in_order(harness: &Harness<'_, App>) -> Vec<String> {
        let places = [JITA_LABEL, "Home Fort"];
        harness
            .get_all_by_role(Role::CheckBox)
            .filter_map(|node| node.accesskit_node().label())
            .filter(|label| places.contains(&label.as_str()))
            .collect()
    }

    fn assert_own_buy(harness: &Harness<'_, App>, own: &str, buy: &str) {
        assert_eq!(own_and_buy(harness), (own.to_owned(), buy.to_owned()));
    }

    /// Makes the plan Hobgoblin II × 300 at the NPC station, pasted with R.A.M.- Robotics × 3, and
    /// saves it as `Hobgoblin 300`.
    fn make_plan(harness: &mut Harness<'_, App>) {
        click(harness, "New");
        harness
            .get_by(|node| node.role() == Role::TextInput && node.value().as_deref() == Some(""))
            .click();
        harness.event(Event::Text("hobgoblin".to_owned()));
        harness.run();
        click(harness, "Hobgoblin II");
        harness
            .get_by(|node| node.role() == Role::TextInput && node.value().as_deref() == Some("1"))
            .click();
        harness.key_press_modifiers(Modifiers::COMMAND, Key::A);
        harness.event(Event::Text("300".to_owned()));
        harness.run();
        paste(harness, "R.A.M.- Robotics\t3");
        click(harness, "Rename");
        harness
            .get_by(|node| {
                node.role() == Role::TextInput && node.value().as_deref() == Some("New plan")
            })
            .click();
        harness.key_press_modifiers(Modifiers::COMMAND, Key::A);
        harness.event(Event::Text("Hobgoblin 300".to_owned()));
        harness.run();
        click(harness, "Rename");
        click(harness, "Save");
    }

    /// The stock the store holds for the plan `Hobgoblin 300`.
    fn stored_stock(env: &Env) -> PlanStock {
        let mut store = Store::open(&env.data.join("user.sqlite"), NOW)
            .expect("the store opens")
            .store;
        let id = store
            .plans()
            .expect("the plans")
            .iter()
            .find(|plan| plan.name == "Hobgoblin 300")
            .expect("the plan")
            .id;
        store.plan_stock(id).expect("the stock")
    }

    fn scope_of(stock: &PlanStock) -> &StockScope {
        match stock {
            PlanStock::Characters(scope) => scope,
            PlanStock::Pasted => panic!("the plan holds pasted stock"),
        }
    }

    #[test]
    fn pasted_stock_counts_the_paste() {
        let env = make_env();
        let mut harness = start(
            &env,
            FakeEve {
                characters: vec![],
                jeve: None,
            },
        );
        make_plan(&mut harness);
        assert_own_buy(&harness, "3", "297");
    }

    #[test]
    fn the_pasted_stock_window_opens_from_the_stock_row_and_closes() {
        let env = make_env();
        let mut harness = start(
            &env,
            FakeEve {
                characters: vec![],
                jeve: None,
            },
        );
        make_plan(&mut harness);
        // The paste helper closes the window, so the fresh page has only the Stock row button.
        assert!(!shown(&harness, "Pasted stock"), "the window is closed");
        assert_eq!(
            harness
                .query_all(
                    egui_kittest::kittest::by()
                        .predicate(|node| { node.role() == Role::MultilineTextInput })
                )
                .count(),
            0,
            "no pasted stock box is drawn"
        );
        assert!(
            shown(&harness, "Pasted stock…"),
            "the Stock row keeps the button"
        );

        click(&mut harness, "Pasted stock…");
        assert_eq!(
            harness.query_all_by_label("Pasted stock").count(),
            1,
            "one window is drawn"
        );
        assert_eq!(
            harness
                .query_all(
                    egui_kittest::kittest::by()
                        .predicate(|node| { node.role() == Role::MultilineTextInput })
                )
                .count(),
            1,
            "the window holds one box"
        );

        harness.get_by_label("Close window").click();
        harness.run();
        assert!(
            !shown(&harness, "Pasted stock"),
            "the title goes with the window"
        );
        assert_eq!(
            harness
                .query_all(
                    egui_kittest::kittest::by()
                        .predicate(|node| { node.role() == Role::MultilineTextInput })
                )
                .count(),
            0,
            "the box goes with the window"
        );
    }

    #[test]
    fn from_characters_counts_the_ticks_and_saves_each_change() {
        let env = make_env();
        save_main(&env);
        let jeve = jeve_profile();
        let eve = FakeEve {
            characters: vec![character(MAIN, "Main"), character(ALT, "Alt")],
            jeve: Some(jeve.path().to_path_buf()),
        };
        let mut harness = start_sized(&env, eve, 1100.0);
        make_plan(&mut harness);

        pick(&mut harness, "Pasted", "From characters");
        assert!(ticked(&harness, "Main"));
        assert!(ticked(&harness, "Alt (jEve)"));
        assert!(ticked(&harness, "Hauler (jEve)"));
        assert_own_buy(&harness, "225", "75");
        assert!(shown(&harness, "PLAN  Hobgoblin II × 300"));
        let stock = stored_stock(&env);
        assert_eq!(
            scope_of(&stock).characters,
            BTreeSet::from([MAIN, ALT, HAULER])
        );
        assert!(matches!(scope_of(&stock).places, Places::All));

        click_box(&mut harness, Role::CheckBox, "Hauler (jEve)");
        assert_own_buy(&harness, "220", "80");

        click(&mut harness, "Only these:");
        assert!(shown(&harness, "Home Fort"));
        assert!(shown(&harness, JITA_LABEL));
        assert_eq!(
            place_ticks_in_order(&harness),
            vec!["Home Fort".to_owned(), JITA_LABEL.to_owned()]
        );
        assert_own_buy(&harness, "220", "80");

        click_box(&mut harness, Role::CheckBox, "Home Fort");
        assert_own_buy(&harness, "100", "200");
        let stock = stored_stock(&env);
        assert_eq!(
            scope_of(&stock).places,
            Places::Only(BTreeSet::from([JITA_4_4]))
        );

        click_box(&mut harness, Role::CheckBox, "Home Fort");
        click_box(&mut harness, Role::CheckBox, JITA_LABEL);
        assert_own_buy(&harness, "120", "180");

        click_box(&mut harness, Role::CheckBox, "Home Fort");
        assert!(shown(&harness, "Pick at least one place, or all places."));
        assert!(ticked(&harness, "Home Fort"));
        assert_own_buy(&harness, "120", "180");
        assert_eq!(
            scope_of(&stored_stock(&env)).places,
            Places::Only(BTreeSet::from([HOME_FORT]))
        );

        click_box(&mut harness, Role::CheckBox, "Main");
        click_box(&mut harness, Role::CheckBox, "Alt (jEve)");
        assert!(shown(&harness, "Pick at least one character."));
        assert!(ticked(&harness, "Alt (jEve)"));
        assert_eq!(
            scope_of(&stored_stock(&env)).characters,
            BTreeSet::from([ALT])
        );
    }

    #[test]
    fn the_choice_comes_back_after_a_restart() {
        let env = make_env();
        save_main(&env);
        let jeve = jeve_profile();
        let eve = FakeEve {
            characters: vec![character(MAIN, "Main"), character(ALT, "Alt")],
            jeve: Some(jeve.path().to_path_buf()),
        };
        let mut harness = start_sized(&env, eve.clone(), 1100.0);
        make_plan(&mut harness);
        pick(&mut harness, "Pasted", "From characters");
        click_box(&mut harness, Role::CheckBox, "Hauler (jEve)");
        click(&mut harness, "Only these:");
        click_box(&mut harness, Role::CheckBox, JITA_LABEL);
        drop(harness);

        let mut restarted = start_sized(&env, eve, 1100.0);
        click(&mut restarted, "Hobgoblin 300");
        assert!(ticked(&restarted, "Main"));
        assert!(ticked(&restarted, "Alt (jEve)"));
        assert!(!ticked(&restarted, "Hauler (jEve)"));
        assert!(ticked(&restarted, "Home Fort"));
        assert!(!ticked(&restarted, JITA_LABEL));
        assert_own_buy(&restarted, "120", "180");
    }

    #[test]
    fn a_scope_character_with_no_data_is_ticked_and_counts_nothing() {
        let env = make_env();
        save_main(&env);
        let jeve = jeve_profile();
        let eve = FakeEve {
            characters: vec![character(MAIN, "Main"), character(ALT, "Alt")],
            jeve: Some(jeve.path().to_path_buf()),
        };
        let mut harness = start(&env, eve.clone());
        make_plan(&mut harness);
        drop(harness);
        let mut store = Store::open(&env.data.join("user.sqlite"), NOW)
            .expect("the store opens")
            .store;
        let id = store
            .plans()
            .expect("the plans")
            .iter()
            .find(|plan| plan.name == "Hobgoblin 300")
            .expect("the plan")
            .id;
        let scope = StockScope {
            characters: BTreeSet::from([MAIN, ALT, HAULER, UNKNOWN]),
            places: Places::All,
        };
        store
            .set_plan_stock(id, &PlanStock::Characters(scope))
            .expect("the stock saved");
        drop(store);

        let mut harness = start(&env, eve);
        click(&mut harness, "Hobgoblin 300");
        assert!(ticked(&harness, "2112000099 (no data)"));
        assert_own_buy(&harness, "225", "75");
    }

    #[test]
    fn back_to_pasted_forgets_the_scope() {
        let env = make_env();
        save_main(&env);
        let jeve = jeve_profile();
        let eve = FakeEve {
            characters: vec![character(MAIN, "Main"), character(ALT, "Alt")],
            jeve: Some(jeve.path().to_path_buf()),
        };
        let mut harness = start(&env, eve);
        make_plan(&mut harness);
        pick(&mut harness, "Pasted", "From characters");
        click_box(&mut harness, Role::CheckBox, "Hauler (jEve)");
        pick(&mut harness, "From characters", "Pasted");
        assert_own_buy(&harness, "3", "297");
        assert!(matches!(stored_stock(&env), PlanStock::Pasted));

        pick(&mut harness, "Pasted", "From characters");
        assert!(ticked(&harness, "Hauler (jEve)"));
        assert!(ticked(&harness, "Main"));
        assert_own_buy(&harness, "225", "75");
    }

    #[test]
    fn the_pasted_panel_says_the_paste_is_not_used() {
        let env = make_env();
        save_main(&env);
        let eve = FakeEve {
            characters: vec![character(MAIN, "Main")],
            jeve: None,
        };
        let mut harness = start(&env, eve);
        make_plan(&mut harness);
        pick(&mut harness, "Pasted", "From characters");
        click(&mut harness, "Pasted stock…");
        assert!(shown(
            &harness,
            "This plan uses stock from characters. The pasted text is kept but not used."
        ));
        harness
            .get_by(|node| node.role() == Role::MultilineTextInput)
            .click();
        harness.event(Event::Paste("Tritanium\t9".to_owned()));
        harness.run();
        assert!(!has_value(&harness, "Tritanium\t9"));
        assert!(has_value(&harness, "R.A.M.- Robotics\t3"));
    }

    #[test]
    fn without_character_data_from_characters_is_disabled_with_its_hover() {
        let env = make_env();
        let eve = FakeEve {
            characters: vec![],
            jeve: None,
        };
        let mut harness = start(&env, eve);
        make_plan(&mut harness);
        harness
            .get_by(|node| node.value().is_some_and(|value| value == "Pasted"))
            .click();
        harness.run();
        let option = harness
            .get_all_by_label("From characters")
            .last()
            .expect("the option");
        assert!(option.accesskit_node().is_disabled());
        option.hover();
        for _ in 0..60 {
            harness.run();
        }
        assert!(shown(&harness, NO_CHARACTERS));
    }

    #[test]
    fn the_stock_characters_snapshot_is_taken() {
        let mut results = SnapshotResults::new();
        let env = make_env();
        save_main(&env);
        let jeve = jeve_profile();
        let eve = FakeEve {
            characters: vec![character(MAIN, "Main"), character(ALT, "Alt")],
            jeve: Some(jeve.path().to_path_buf()),
        };
        let mut harness = start(&env, eve);
        make_plan(&mut harness);
        pick(&mut harness, "Pasted", "From characters");
        click(&mut harness, "Only these:");
        harness.snapshot("build-stock-characters");
        results.extend_harness(&mut harness);
        results.unwrap();
    }
}

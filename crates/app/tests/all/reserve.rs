//! Acceptance tests for plans taking their stock after the claims: a plan counts the hangar units
//! the plans served before it left, plus the running jobs assigned to it. The store, the login data
//! and the jobs are seeded before the app starts. The clock is fixed at 1,000,000.
#[cfg(test)]
mod tests {

    use std::collections::BTreeMap;
    use std::fs;
    use std::io::Write;
    use std::ops::ControlFlow;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    use egui::accesskit::{Role, Toggled};
    use egui::{Event, Key, Modifiers, PointerButton, pos2, vec2};
    use egui_kittest::Harness;
    use egui_kittest::kittest::{NodeT, Queryable, by};
    use eve_means_of_profit::build::prices::Prices;
    use eve_means_of_profit::session::{EveAccess, PendingLogin};
    use eve_means_of_profit::{App, AppConfig};
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use mop_core::freight::Packing;
    use mop_core::stock::{Asset, CharacterAssets, Places, StockScope};
    use mop_esi::character_data::{
        CharacterData, Fetched, Job, JobStatus, Kind, OwnedAssets, Refreshed,
    };
    use mop_esi::characters::{Character, Login};
    use mop_esi::error::EsiError;
    use mop_esi::places::{NamingReport, StructureNames};
    use mop_esi::saved;
    use mop_market::book::PriceRule;
    use mop_market::error::MarketError;
    use mop_market::fuzzwork::{BytesHttp, BytesResponse};
    use mop_sde::{Fetch, FetchError, import};
    use mop_store::reservations::PlanStock;
    use mop_store::{PlanRecord, Store};
    use tempfile::TempDir;

    use crate::ui::{click, hover, shown, snapshot, top};

    const NOW: i64 = 1_000_000;
    const SCOPE_HOVER: &str =
        "Plans that reserve take stock in list order; later plans only see what is left.";
    const NEEDS_STOCK: &str = "Reserve needs stock from characters.";
    const FIXTURE: &str = "../sde/tests/fixtures/sde-3607705";
    const MAIN: i64 = 2_112_000_001;
    const ALT: i64 = 2_112_000_002;
    const JITA_4_4: i64 = 60_003_760;
    const HOME_FORT: i64 = 1_035_466_617_946;
    const HOBGOBLIN_I: u32 = 2_454;
    const HOBGOBLIN_II: u32 = 2_456;
    const HOBGOBLIN_I_BLUEPRINT: u32 = 2_457;
    const TRITANIUM: u32 = 34;

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

    /// A data folder with the game data imported, the user's store, and Home Fort named.
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

    /// The login with the characters given. Nothing here refreshes or logs in.
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

    fn character(id: i64, name: &str) -> Character {
        Character {
            id: u64::try_from(id).expect("an id that fits"),
            name: name.to_owned(),
            scopes: std::collections::BTreeSet::new(),
            added_at: NOW,
            login: Login::Ok,
        }
    }

    fn login() -> FakeEve {
        FakeEve {
            characters: vec![character(MAIN, "Main"), character(ALT, "Alt")],
        }
    }

    fn asset(item_id: i64, type_id: u32, location_id: i64, quantity: u64, flag: &str) -> Asset {
        Asset {
            item_id,
            type_id,
            location_id,
            flag: Some(flag.to_owned()),
            quantity,
            singleton: false,
        }
    }

    fn fetched<T>(data: T) -> Fetched<T> {
        Fetched {
            data,
            fetched_at: 990_000,
            last_modified: None,
            expires: None,
        }
    }

    /// A running manufacturing job of Hobgoblin I: `runs` of them, delivered to `at`.
    fn job(job_id: i64, runs: u32, at: i64) -> Job {
        Job {
            job_id,
            activity_id: 1,
            blueprint_type_id: HOBGOBLIN_I_BLUEPRINT,
            product_type_id: Some(HOBGOBLIN_I),
            runs,
            status: JobStatus::Active,
            start: 0,
            end: NOW + 3_600,
            facility_id: 0,
            station_id: 0,
            output_location_id: at,
            installer_id: MAIN,
            cost: None,
            licensed_runs: None,
        }
    }

    fn save_character(env: &Env, data: &CharacterData) {
        saved::save_character(&esi_dir(env), data).expect("the character's data is saved");
    }

    /// Main's assets: Hobgoblin I × 4 in Jita (and × 9 fitted, which never counts), Tritanium
    /// × 1,500 in Jita and × 2,000 in Home Fort. Its jobs: 501 (1 run, output Home Fort) and 503
    /// (5 runs, output Jita).
    fn save_main(env: &Env) {
        let assets = CharacterAssets {
            character_id: MAIN,
            updated: Some(990_000),
            assets: vec![
                asset(1, HOBGOBLIN_I, JITA_4_4, 4, "Hangar"),
                asset(3, TRITANIUM, JITA_4_4, 1_500, "Hangar"),
                asset(4, TRITANIUM, HOME_FORT, 2_000, "Hangar"),
                asset(5, HOBGOBLIN_I, JITA_4_4, 9, "HiSlot0"),
            ],
        };
        let data = CharacterData {
            character_id: u64::try_from(MAIN).expect("an id that fits"),
            assets: Some(fetched(OwnedAssets {
                assets,
                names: BTreeMap::new(),
            })),
            orders: None,
            jobs: Some(fetched(vec![job(501, 1, HOME_FORT), job(503, 5, JITA_4_4)])),
            wallet: None,
        };
        save_character(env, &data);
    }

    /// Alt's assets: Hobgoblin I × 3 in Jita, and no jobs.
    fn save_alt(env: &Env) {
        let assets = CharacterAssets {
            character_id: ALT,
            updated: Some(990_000),
            assets: vec![asset(2, HOBGOBLIN_I, JITA_4_4, 3, "Hangar")],
        };
        let data = CharacterData {
            character_id: u64::try_from(ALT).expect("an id that fits"),
            assets: Some(fetched(OwnedAssets {
                assets,
                names: BTreeMap::new(),
            })),
            orders: None,
            jobs: None,
            wallet: None,
        };
        save_character(env, &data);
    }

    fn stock_environment() -> Env {
        let env = make_env();
        save_main(&env);
        save_alt(&env);
        env
    }

    fn store(env: &Env) -> Store {
        Store::open(&env.data.join("user.sqlite"), NOW)
            .expect("the store opens")
            .store
    }

    /// A Hobgoblin II plan at the NPC station, with the default ME values.
    fn make_plan(env: &Env, name: &str, quantity: u64, build: &[u32], pasted: &str) -> i64 {
        let record = PlanRecord {
            id: 0,
            name: name.to_owned(),
            product: HOBGOBLIN_II,
            quantity,
            build: build.iter().copied().collect(),
            me: BTreeMap::new(),
            manufacturing: None,
            reactions: None,
            price_rule: PriceRule::WalkSell,
            route: None,
            service: None,
            packing: Packing::default(),
            pasted_stock: pasted.to_owned(),
            created_at: NOW,
            updated_at: NOW,
        };
        store(env)
            .create_plan(&record, NOW)
            .expect("the plan is saved")
    }

    fn only(places: &[i64]) -> Places {
        Places::Only(places.iter().copied().collect())
    }

    fn use_stock(env: &Env, id: i64, characters: &[i64], places: Places) {
        let scope = StockScope {
            characters: characters.iter().copied().collect(),
            places,
        };
        store(env)
            .set_plan_stock(id, &PlanStock::Characters(scope))
            .expect("the plan's stock is saved");
    }

    fn reserve(env: &Env, id: i64) {
        store(env).set_reserve(id, true).expect("the plan reserves");
    }

    fn assign(env: &Env, job_id: i64, plan_id: i64) {
        store(env)
            .assign_job(job_id, plan_id, NOW)
            .expect("the job is assigned");
    }

    fn set_order(env: &Env, ids: &[i64]) {
        store(env).set_plan_order(ids).expect("the order is saved");
    }

    /// The plans as the store lists them.
    fn plan_order(env: &Env) -> Vec<i64> {
        store(env)
            .plans()
            .expect("the plans")
            .iter()
            .map(|plan| plan.id)
            .collect()
    }

    /// Right-clicks the row labelled `label`, opening its menu.
    fn right_click(harness: &mut Harness<'_, App>, label: &str) {
        harness.get_by_label(label).scroll_to_me();
        harness.run_steps(2);
        harness.get_by_label(label).hover();
        harness.run_steps(2);
        harness.get_by_label(label).click_secondary();
        harness.run_steps(3);
    }

    /// Whether the menu entry labelled `label` is disabled.
    fn menu_disabled(harness: &Harness<'_, App>, label: &str) -> bool {
        harness.get_by_label(label).accesskit_node().is_disabled()
    }

    /// Clicks the last node labelled `label`, which is a dialog's button when the panel has one of
    /// the same name.
    fn click_last(harness: &mut Harness<'_, App>, label: &str) {
        let count = harness.get_all_by_label(label).count();
        assert!(count > 0, "no node labelled {label:?}");
        harness
            .get_all_by_label(label)
            .nth(count - 1)
            .expect("the button")
            .click();
        harness.run_steps(3);
    }

    /// Drags the row labelled `from` onto the row labelled `to`, moving in steps so egui sees the
    /// pointer travel.
    fn drag(harness: &mut Harness<'_, App>, from: &str, to: &str) {
        let source = harness.get_by_label(from).rect().center();
        let target = harness.get_by_label(to).rect().center();
        harness.hover_at(source);
        harness.run_steps(2);
        harness.drag_at(source);
        harness.run_steps(2);
        let middle = pos2(source.x, source.y + (target.y - source.y) / 2.0);
        harness.hover_at(middle);
        harness.run_steps(2);
        harness.hover_at(target);
        harness.run_steps(2);
        harness.drop_at(target);
        harness.run_steps(4);
    }

    /// Clicks the tick labelled `label`, near its left edge: a long label can run under a panel
    /// that takes a click at its middle.
    fn click_box(harness: &mut Harness<'_, App>, label: &str) {
        let rect = harness
            .get_all_by_role(Role::CheckBox)
            .find(|node| node.accesskit_node().label().as_deref() == Some(label))
            .unwrap_or_else(|| panic!("no {label:?} tick"))
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

    /// Whether the tick labelled `label` is disabled.
    fn disabled(harness: &Harness<'_, App>, label: &str) -> bool {
        harness
            .get_all_by_role(Role::CheckBox)
            .find(|node| node.accesskit_node().label().as_deref() == Some(label))
            .unwrap_or_else(|| panic!("no {label:?} tick"))
            .accesskit_node()
            .is_disabled()
    }

    /// Opens the dropdown that shows `current` and picks `option` from it.
    fn pick(harness: &mut Harness<'_, App>, current: &str, option: &str) {
        harness
            .get_by(|node| node.value().is_some_and(|value| value == current))
            .click();
        harness.run();
        click(harness, option);
    }

    /// The claimed line in the Plan panel, or `None` when it shows none.
    fn claimed(harness: &Harness<'_, App>) -> Option<String> {
        harness
            .query_all(by().predicate(|node| node.bounding_box().is_some()))
            .filter_map(|node| node.value())
            .find(|value| value.starts_with("Claimed by other plans:"))
    }

    fn start(env: &Env, eve: FakeEve) -> Harness<'static, App> {
        let config = AppConfig {
            data_dir: env.data.clone(),
            cache_dir: env.cache.clone(),
            fetch: Box::new(NoNetwork),
            clock: Box::new(|| NOW),
            eve: Box::new(eve),
            market: Box::new(crate::offline::Offline),
        };
        let mut harness = Harness::builder()
            .with_size(vec2(1900.0, 800.0))
            .with_max_steps(crate::SETTLE_STEPS)
            .build_eframe(move |cc| App::new(&cc.egui_ctx, config).expect("the app starts"));
        harness.state_mut().shared_mut().market = Arc::new(Prices::new(Box::new(EmptyForge)));
        // The saved login and its data arrive on the worker thread.
        for _ in 0..500 {
            harness.run();
            if harness.state_mut().shared_mut().session.loaded() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(
            harness.state_mut().shared_mut().session.loaded(),
            "the start-up read never arrived"
        );
        harness
    }

    /// The left edge of the Buy list panel, which sits right of the Steps panel.
    fn steps_edge(harness: &Harness<'_, App>) -> f32 {
        harness.get_by_label("Buy list").rect().min.x
    }

    /// The y of the Steps row labelled `row`.
    fn steps_y(harness: &Harness<'_, App>, row: &str) -> f32 {
        let edge = steps_edge(harness);
        harness
            .query_all_by_label(row)
            .find(|node| node.rect().min.x < edge)
            .unwrap_or_else(|| panic!("no {row:?} row"))
            .rect()
            .center()
            .y
    }

    /// Scrolls the Steps table sideways by `delta` points (negative moves it left), and runs the
    /// frames the animated scroll takes.
    fn scroll_steps(harness: &mut Harness<'_, App>, delta: f32) {
        let y = steps_y(harness, "Hobgoblin I");
        let x = harness.get_by_label("Steps").rect().min.x + 100.0;
        harness.hover_at(pos2(x, y));
        harness.event(Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: vec2(delta, 0.0),
            phase: egui::TouchPhase::Move,
            modifiers: Modifiers::NONE,
        });
        for _ in 0..30 {
            harness.step();
        }
    }

    /// The nodes of the Steps row labelled `row`, left to right: position and text, empty cells
    /// left out. The table is wider than its panel, so cells reach past the Buy list's edge; the
    /// row ends where the x-position falls back. Empty when Steps shows no such row.
    fn row_nodes(harness: &Harness<'_, App>, row: &str) -> Vec<(egui::Rect, String)> {
        let edge = steps_edge(harness);
        let nodes: Vec<(egui::Rect, Option<String>)> = harness
            .query_all(by().predicate(|node| node.bounding_box().is_some()))
            .map(|node| (node.rect(), node.value()))
            .collect();
        let Some(start) = nodes
            .iter()
            .position(|(rect, value)| rect.min.x < edge && value.as_deref() == Some(row))
        else {
            return Vec::new();
        };
        let y = nodes[start].0.center().y;
        let mut found = Vec::new();
        let mut last = f32::MIN;
        for (rect, value) in &nodes[start..] {
            if (rect.center().y - y).abs() >= 4.0 {
                continue;
            }
            if rect.min.x < last {
                break;
            }
            last = rect.min.x;
            if let Some(value) = value
                && !value.is_empty()
            {
                found.push((*rect, value.clone()));
            }
        }
        found
    }

    /// The cell of the Steps row labelled `row` in the column headed `header`: its rectangle and
    /// text. Empty cells are left out of `row_nodes`, so an empty cell reads as `None`.
    fn cell(harness: &Harness<'_, App>, row: &str, header: &str) -> Option<(egui::Rect, String)> {
        const HEADERS: [&str; 13] = [
            "Item",
            "Build",
            "ME",
            "Need",
            "Own",
            "Running",
            "Make",
            "Buy",
            "Runs",
            "Jobs",
            "Spare",
            "Job cost",
            "Make or buy",
        ];
        // The Steps header row: the row of the one "Make or buy" header.
        let y = harness.get_by_label("Make or buy").rect().center().y;
        let centres: Vec<(&str, f32)> = HEADERS
            .iter()
            .filter_map(|name| {
                harness
                    .query_all_by_label(name)
                    .find(|node| (node.rect().center().y - y).abs() < 4.0)
                    .map(|node| (*name, node.rect().center().x))
            })
            .collect();
        row_nodes(harness, row).into_iter().find(|(rect, _)| {
            let x = rect.center().x;
            centres
                .iter()
                .min_by(|a, b| (a.1 - x).abs().total_cmp(&(b.1 - x).abs()))
                .is_some_and(|(name, _)| *name == header)
        })
    }

    /// The text of the row's cell in the column headed `header`.
    fn cell_under(harness: &Harness<'_, App>, row: &str, header: &str) -> Option<String> {
        cell(harness, row, header).map(|(_, text)| text)
    }

    /// Hovers the cell of the Steps row labelled `row` in the column headed `header`, scrolling the
    /// table sideways first when the cell is under the Buy list, and returns the hangar split
    /// hovers it shows.
    fn hover_cell(harness: &mut Harness<'_, App>, row: &str, header: &str) -> Vec<String> {
        for _ in 0..3 {
            let edge = steps_edge(harness);
            let (rect, _) = cell(harness, row, header)
                .unwrap_or_else(|| panic!("no {row:?} cell under {header:?}"));
            if rect.center().x < edge - 40.0 {
                harness.hover_at(rect.center());
                for _ in 0..60 {
                    harness.run();
                }
                return harness
                    .query_all(by().predicate(|node| node.bounding_box().is_some()))
                    .filter_map(|node| {
                        let node = node.accesskit_node();
                        node.label().or_else(|| node.value())
                    })
                    .filter(|text| text.contains(" in hangars, "))
                    .collect();
            }
            scroll_steps(harness, -(rect.center().x - edge + 120.0));
        }
        panic!("the cell {row:?} {header:?} cannot be brought into view");
    }

    /// Asserts the Steps headers `left`, `middle` and `right` appear in that order, left to right.
    fn assert_header_order(harness: &Harness<'_, App>, left: &str, middle: &str, right: &str) {
        let y = harness.get_by_label("Make or buy").rect().center().y;
        let centre = |name: &str| {
            harness
                .query_all_by_label(name)
                .find(|node| (node.rect().center().y - y).abs() < 4.0)
                .unwrap_or_else(|| panic!("no {name:?} header"))
                .rect()
                .center()
                .x
        };
        assert!(
            centre(left) < centre(middle) && centre(middle) < centre(right),
            "{left:?}, {middle:?} and {right:?} are not in that order"
        );
    }

    /// Asserts the Steps row `row` shows `text` under each header.
    fn assert_cells(harness: &Harness<'_, App>, row: &str, expected: &[(&str, &str)]) {
        for (header, text) in expected {
            assert_eq!(
                cell_under(harness, row, header).as_deref(),
                Some(*text),
                "{row} {header}: {:?}",
                row_nodes(harness, row)
            );
        }
    }

    /// Asserts the Steps table shows no `row`.
    fn assert_no_row(harness: &Harness<'_, App>, row: &str) {
        let found = row_nodes(harness, row);
        assert!(found.is_empty(), "{row} is shown: {found:?}");
    }

    /// The seed of these tests: `Hobgoblin A` × 10 taking Main and Alt at Jita, `Hobgoblin B` × 2
    /// taking Main everywhere, both reserving in that order, with job 501 assigned to B.
    fn reserving_pair(env: &Env) -> (i64, i64) {
        let a = make_plan(env, "Hobgoblin A", 10, &[HOBGOBLIN_I], "");
        use_stock(env, a, &[MAIN, ALT], only(&[JITA_4_4]));
        reserve(env, a);
        let b = make_plan(env, "Hobgoblin B", 2, &[HOBGOBLIN_I], "");
        use_stock(env, b, &[MAIN], Places::All);
        reserve(env, b);
        assign(env, 501, b);
        (a, b)
    }

    #[test]
    fn the_claimed_line_names_the_plans_before_this_one() {
        let env = stock_environment();
        reserving_pair(&env);

        let mut harness = start(&env, login());
        click(&mut harness, "Hobgoblin B");
        assert_eq!(
            claimed(&harness).as_deref(),
            Some(
                "Claimed by other plans: Hobgoblin I 4 (Hobgoblin A), Tritanium 1,500 (Hobgoblin A)"
            )
        );
        assert!(ticked(&harness, "Reserve"));
        let tip = hover(&mut harness, "Reserve");
        assert!(
            tip.iter().any(|line| line == SCOPE_HOVER),
            "the Reserve hover: {tip:?}"
        );

        click(&mut harness, "Hobgoblin A");
        assert!(claimed(&harness).is_none(), "{:?}", claimed(&harness));
        assert!(ticked(&harness, "Reserve"));
        let tip = hover(&mut harness, "Reserve");
        assert!(
            tip.iter().any(|line| line == SCOPE_HOVER),
            "the Reserve hover: {tip:?}"
        );
    }

    #[test]
    fn an_unticked_reserve_leaves_the_plan_out_of_the_order() {
        let env = stock_environment();
        let (a, b) = reserving_pair(&env);

        let mut harness = start(&env, login());
        click(&mut harness, "Hobgoblin A");
        click_box(&mut harness, "Reserve");
        assert!(!ticked(&harness, "Reserve"));
        assert_eq!(store(&env).reserving_plans().expect("reserving"), vec![b]);
        // Out of the order, A is served after B and so sees what B claimed.
        assert!(claimed(&harness).is_some(), "A: {:?}", claimed(&harness));

        click(&mut harness, "Hobgoblin B");
        assert!(claimed(&harness).is_none(), "{:?}", claimed(&harness));
        assert_cells(
            &harness,
            "Hobgoblin I",
            &[("Need", "2"), ("Own", "2"), ("Make", "0")],
        );

        click(&mut harness, "Hobgoblin A");
        click_box(&mut harness, "Reserve");
        assert!(ticked(&harness, "Reserve"));
        assert_eq!(
            store(&env).reserving_plans().expect("reserving"),
            vec![a, b]
        );
        // Back at the head of the order, A has nothing claimed from it.
        assert!(claimed(&harness).is_none(), "A: {:?}", claimed(&harness));

        click(&mut harness, "Hobgoblin B");
        assert_eq!(
            claimed(&harness).as_deref(),
            Some(
                "Claimed by other plans: Hobgoblin I 4 (Hobgoblin A), Tritanium 1,500 (Hobgoblin A)"
            )
        );
        assert_cells(
            &harness,
            "Hobgoblin I",
            &[("Need", "2"), ("Own", "1"), ("Make", "1")],
        );
    }

    #[test]
    fn pasted_stock_cannot_reserve() {
        let env = stock_environment();
        let (_, b) = reserving_pair(&env);

        let mut harness = start(&env, login());
        click(&mut harness, "Hobgoblin A");
        assert!(ticked(&harness, "Reserve"));
        pick(&mut harness, "From characters", "Pasted");

        assert!(!ticked(&harness, "Reserve"));
        assert!(disabled(&harness, "Reserve"));
        let tick = harness
            .get_all_by_role(Role::CheckBox)
            .find(|node| node.accesskit_node().label().as_deref() == Some("Reserve"))
            .expect("the Reserve tick");
        tick.hover();
        for _ in 0..60 {
            harness.run();
        }
        assert!(shown(&harness, NEEDS_STOCK), "{:?}", claimed(&harness));
        assert_eq!(store(&env).reserving_plans().expect("reserving"), vec![b]);

        // A no longer reserves, so B sees the whole shared stock again.
        click(&mut harness, "Hobgoblin B");
        assert!(claimed(&harness).is_none(), "{:?}", claimed(&harness));
        assert_cells(
            &harness,
            "Hobgoblin I",
            &[("Need", "2"), ("Own", "2"), ("Make", "0")],
        );
    }

    #[test]
    fn the_reserve_snapshot_is_taken() {
        let env = stock_environment();
        reserving_pair(&env);
        let mut harness = start(&env, login());
        click(&mut harness, "Hobgoblin B");
        snapshot(&mut harness, "build_reserve");
    }

    #[test]
    fn the_plans_order_snapshot_is_taken() {
        let env = stock_environment();
        reserving_pair(&env);
        make_plan(&env, "Hobgoblin C", 2, &[HOBGOBLIN_I], "Hobgoblin I\t1");
        let mut harness = start(&env, login());
        click(&mut harness, "Hobgoblin A");
        snapshot(&mut harness, "plans_order");
    }

    #[test]
    fn claims_are_taken_in_the_plan_order_a_then_b() {
        let env = stock_environment();
        let a = make_plan(&env, "Hobgoblin A", 10, &[HOBGOBLIN_I], "");
        use_stock(&env, a, &[MAIN, ALT], only(&[JITA_4_4]));
        reserve(&env, a);
        let b = make_plan(&env, "Hobgoblin B", 2, &[HOBGOBLIN_I], "");
        use_stock(&env, b, &[MAIN], Places::All);
        reserve(&env, b);
        assign(&env, 501, b);

        let mut harness = start(&env, login());
        click(&mut harness, "Hobgoblin A");
        assert_cells(
            &harness,
            "Hobgoblin I",
            &[("Need", "10"), ("Own", "7"), ("Make", "3")],
        );
        assert_cells(
            &harness,
            "Tritanium",
            &[("Need", "1,527"), ("Own", "1,500"), ("Buy", "27")],
        );

        click(&mut harness, "Hobgoblin B");
        assert_cells(
            &harness,
            "Hobgoblin I",
            &[("Need", "2"), ("Own", "1"), ("Make", "1")],
        );
        assert_cells(
            &harness,
            "Tritanium",
            &[("Need", "509"), ("Own", "509"), ("Buy", "0")],
        );
    }

    #[test]
    fn the_plan_served_first_takes_the_shared_units() {
        let env = stock_environment();
        let a = make_plan(&env, "Hobgoblin A", 10, &[HOBGOBLIN_I], "");
        use_stock(&env, a, &[MAIN, ALT], only(&[JITA_4_4]));
        reserve(&env, a);
        let b = make_plan(&env, "Hobgoblin B", 2, &[HOBGOBLIN_I], "");
        use_stock(&env, b, &[MAIN], Places::All);
        reserve(&env, b);
        assign(&env, 501, b);
        set_order(&env, &[b, a]);

        let mut harness = start(&env, login());
        click(&mut harness, "Hobgoblin B");
        assert_cells(
            &harness,
            "Hobgoblin I",
            &[("Need", "2"), ("Own", "2"), ("Make", "0")],
        );
        assert_no_row(&harness, "Tritanium");

        click(&mut harness, "Hobgoblin A");
        assert_cells(
            &harness,
            "Hobgoblin I",
            &[("Need", "10"), ("Own", "6"), ("Make", "4")],
        );
        assert_cells(
            &harness,
            "Tritanium",
            &[("Need", "2,036"), ("Own", "1,500"), ("Buy", "536")],
        );
    }

    #[test]
    fn a_plan_that_does_not_reserve_sees_every_claim() {
        let env = stock_environment();
        let a = make_plan(&env, "Hobgoblin A", 10, &[HOBGOBLIN_I], "");
        use_stock(&env, a, &[MAIN, ALT], only(&[JITA_4_4]));
        let b = make_plan(&env, "Hobgoblin B", 2, &[HOBGOBLIN_I], "");
        use_stock(&env, b, &[MAIN], Places::All);
        reserve(&env, b);
        assign(&env, 501, b);
        set_order(&env, &[b, a]);

        let mut harness = start(&env, login());
        click(&mut harness, "Hobgoblin A");
        assert_cells(
            &harness,
            "Hobgoblin I",
            &[("Need", "10"), ("Own", "6"), ("Make", "4")],
        );
        assert_cells(
            &harness,
            "Tritanium",
            &[("Need", "2,036"), ("Own", "1,500"), ("Buy", "536")],
        );
    }

    #[test]
    fn a_saved_edit_changes_what_the_next_plan_gets() {
        let env = stock_environment();
        let a = make_plan(&env, "Hobgoblin A", 10, &[HOBGOBLIN_I], "");
        use_stock(&env, a, &[MAIN, ALT], only(&[JITA_4_4]));
        reserve(&env, a);
        let b = make_plan(&env, "Hobgoblin B", 2, &[HOBGOBLIN_I], "");
        use_stock(&env, b, &[MAIN], Places::All);
        reserve(&env, b);
        assign(&env, 501, b);

        let mut harness = start(&env, login());
        click(&mut harness, "Hobgoblin A");
        harness
            .get_by(|node| node.role() == Role::TextInput && node.value().as_deref() == Some("10"))
            .click();
        harness.key_press_modifiers(Modifiers::COMMAND, Key::A);
        harness.event(Event::Text("3".to_owned()));
        harness.run();
        click(&mut harness, "Save");

        click(&mut harness, "Hobgoblin B");
        assert_cells(
            &harness,
            "Hobgoblin I",
            &[("Need", "2"), ("Own", "2"), ("Make", "0")],
        );
        assert_no_row(&harness, "Tritanium");
    }

    #[test]
    fn the_menu_moves_a_plan_down_and_the_claims_follow_it() {
        let env = stock_environment();
        let (a, b) = reserving_pair(&env);

        let mut harness = start(&env, login());
        right_click(&mut harness, "Hobgoblin A");
        assert!(!menu_disabled(&harness, "Move down"));
        click(&mut harness, "Move down");

        assert_eq!(plan_order(&env), vec![b, a]);
        assert!(top(&harness, "Hobgoblin B") < top(&harness, "Hobgoblin A"));

        click(&mut harness, "Hobgoblin B");
        assert_cells(
            &harness,
            "Hobgoblin I",
            &[("Need", "2"), ("Own", "2"), ("Make", "0")],
        );
        click(&mut harness, "Hobgoblin A");
        assert_cells(
            &harness,
            "Hobgoblin I",
            &[("Need", "10"), ("Own", "6"), ("Make", "4")],
        );
        assert_cells(
            &harness,
            "Tritanium",
            &[("Need", "2,036"), ("Own", "1,500"), ("Buy", "536")],
        );
        drop(harness);

        let harness = start(&env, login());
        assert!(top(&harness, "Hobgoblin B") < top(&harness, "Hobgoblin A"));
    }

    #[test]
    fn the_fresh_list_ends_cannot_move_further() {
        let env = stock_environment();
        reserving_pair(&env);

        let mut harness = start(&env, login());
        right_click(&mut harness, "Hobgoblin A");
        assert!(menu_disabled(&harness, "Move up"));
        assert!(!menu_disabled(&harness, "Move down"));
        drop(harness);

        let mut harness = start(&env, login());
        right_click(&mut harness, "Hobgoblin B");
        assert!(menu_disabled(&harness, "Move down"));
        assert!(!menu_disabled(&harness, "Move up"));
    }

    #[test]
    fn dragging_a_plan_onto_another_reorders_the_list() {
        let env = stock_environment();
        let (a, b) = reserving_pair(&env);
        let c = make_plan(&env, "Hobgoblin C", 2, &[HOBGOBLIN_I], "Hobgoblin I\t1");

        let mut harness = start(&env, login());
        drag(&mut harness, "Hobgoblin C", "Hobgoblin A");
        assert_eq!(plan_order(&env), vec![c, a, b]);
        assert!(top(&harness, "Hobgoblin C") < top(&harness, "Hobgoblin A"));
        assert!(top(&harness, "Hobgoblin A") < top(&harness, "Hobgoblin B"));

        drag(&mut harness, "Hobgoblin A", "Hobgoblin B");
        assert_eq!(plan_order(&env), vec![c, b, a]);
        assert!(top(&harness, "Hobgoblin C") < top(&harness, "Hobgoblin B"));
        assert!(top(&harness, "Hobgoblin B") < top(&harness, "Hobgoblin A"));
    }

    #[test]
    fn only_reserving_plans_show_the_reserves_label() {
        let env = stock_environment();
        reserving_pair(&env);
        make_plan(&env, "Hobgoblin C", 2, &[HOBGOBLIN_I], "Hobgoblin I\t1");

        let harness = start(&env, login());
        let reserves: Vec<f32> = harness
            .query_all(by().role(Role::Label).predicate(|node| {
                node.label()
                    .or_else(|| node.value())
                    .is_some_and(|text| text.contains("reserves"))
            }))
            .map(|node| node.rect().center().y)
            .collect();
        assert_eq!(reserves.len(), 2, "{reserves:?}");
        for name in ["Hobgoblin A", "Hobgoblin B"] {
            let row = harness.get_by_label(name).rect().center().y;
            assert!(
                reserves.iter().any(|reserve| (reserve - row).abs() <= 3.0),
                "{name} has no reserves label"
            );
        }
        let c = harness.get_by_label("Hobgoblin C").rect().center().y;
        assert!(
            !reserves.iter().any(|reserve| (reserve - c).abs() <= 3.0),
            "Hobgoblin C has a reserves label"
        );
    }

    #[test]
    fn deleting_a_reserving_plan_releases_its_claims() {
        let env = stock_environment();
        let (_, b) = reserving_pair(&env);

        let mut harness = start(&env, login());
        click(&mut harness, "Hobgoblin A");
        click(&mut harness, "Delete");
        assert!(shown(&harness, "Delete plan Hobgoblin A?"));
        click_last(&mut harness, "Delete");
        assert_eq!(plan_order(&env), vec![b]);

        click(&mut harness, "Hobgoblin B");
        assert_cells(
            &harness,
            "Hobgoblin I",
            &[("Need", "2"), ("Own", "2"), ("Make", "0")],
        );
    }

    #[test]
    fn an_assigned_job_adds_to_pasted_stock() {
        let env = stock_environment();
        let c = make_plan(&env, "Hobgoblin C", 2, &[HOBGOBLIN_I], "Hobgoblin I\t1");
        assign(&env, 503, c);

        let mut harness = start(&env, login());
        click(&mut harness, "Hobgoblin C");
        assert_cells(
            &harness,
            "Hobgoblin I",
            &[("Need", "2"), ("Own", "2"), ("Make", "0")],
        );
        drop(harness);

        store(&env)
            .unassign_job(503)
            .expect("the job is unassigned");
        let mut harness = start(&env, login());
        click(&mut harness, "Hobgoblin C");
        assert_cells(
            &harness,
            "Hobgoblin I",
            &[("Need", "2"), ("Own", "1"), ("Make", "1")],
        );
    }

    #[test]
    fn the_running_column_sits_after_own_and_shows_the_assigned_jobs() {
        let env = stock_environment();
        reserving_pair(&env);

        let mut harness = start(&env, login());
        click(&mut harness, "Hobgoblin B");
        assert_header_order(&harness, "Own", "Running", "Make");
        assert_cells(
            &harness,
            "Hobgoblin I",
            &[("Need", "2"), ("Own", "1"), ("Running", "1"), ("Make", "1")],
        );
        let tip = hover_cell(&mut harness, "Hobgoblin I", "Own");
        assert!(
            tip.iter()
                .any(|line| line == "0 in hangars, 1 from running jobs"),
            "the Own hover: {tip:?}"
        );

        click(&mut harness, "Hobgoblin A");
        for row in ["Hobgoblin II", "Hobgoblin I", "Tritanium"] {
            assert_eq!(
                cell_under(&harness, row, "Running"),
                None,
                "{row} shows Running"
            );
        }
        let tip = hover_cell(&mut harness, "Hobgoblin I", "Own");
        assert!(
            !tip.iter().any(|line| line.contains("from running jobs")),
            "the Own hover: {tip:?}"
        );
    }

    #[test]
    fn running_is_used_before_the_hangar_units() {
        let env = stock_environment();
        let (a, b) = reserving_pair(&env);
        set_order(&env, &[b, a]);

        let mut harness = start(&env, login());
        click(&mut harness, "Hobgoblin B");
        assert_cells(
            &harness,
            "Hobgoblin I",
            &[("Own", "2"), ("Running", "1"), ("Make", "0")],
        );
        let tip = hover_cell(&mut harness, "Hobgoblin I", "Own");
        assert!(
            tip.iter()
                .any(|line| line == "1 in hangars, 1 from running jobs"),
            "the Own hover: {tip:?}"
        );
    }

    #[test]
    fn running_follows_the_job_assigned_to_a_pasted_plan() {
        let env = stock_environment();
        reserving_pair(&env);
        let c = make_plan(&env, "Hobgoblin C", 2, &[HOBGOBLIN_I], "Hobgoblin I\t1");
        assign(&env, 503, c);

        let mut harness = start(&env, login());
        click(&mut harness, "Hobgoblin C");
        assert_cells(
            &harness,
            "Hobgoblin I",
            &[("Own", "2"), ("Running", "2"), ("Make", "0")],
        );
        let tip = hover_cell(&mut harness, "Hobgoblin I", "Own");
        assert!(
            tip.iter()
                .any(|line| line == "0 in hangars, 2 from running jobs"),
            "the Own hover: {tip:?}"
        );
        for row in ["Hobgoblin II", "Tritanium"] {
            assert_eq!(
                cell_under(&harness, row, "Running"),
                None,
                "{row} shows Running"
            );
        }
    }

    #[test]
    fn the_running_snapshot_is_taken() {
        let env = stock_environment();
        reserving_pair(&env);
        let mut harness = start(&env, login());
        click(&mut harness, "Hobgoblin B");
        snapshot(&mut harness, "build_running");
    }
}

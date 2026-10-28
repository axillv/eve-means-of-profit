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

    use crate::ui::{click, hover, shown, snapshot};

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

    /// The text of the row's cell in the column headed `header`: the cell whose centre is
    /// nearest to that header's centre among all the Steps headers.
    fn cell_under(harness: &Harness<'_, App>, row: &str, header: &str) -> Option<String> {
        const HEADERS: [&str; 12] = [
            "Item",
            "Build",
            "ME",
            "Need",
            "Own",
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
        row_nodes(harness, row)
            .into_iter()
            .find(|(rect, _)| {
                let x = rect.center().x;
                centres
                    .iter()
                    .min_by(|a, b| (a.1 - x).abs().total_cmp(&(b.1 - x).abs()))
                    .is_some_and(|(name, _)| *name == header)
            })
            .map(|(_, text)| text)
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
}

//! Acceptance tests for the job cost and make-or-buy columns of the Steps table, the total line,
//! and the EVE industry data the app loads and refreshes. The game data is the build 3607705
//! fixture. Prices come from a fake of Fuzzwork's summary file, the industry data from a saved
//! `industry.json` or a fake `EveAccess`. The clock is fixed at 1,000,000.
#[cfg(test)]
mod tests {

    use std::collections::BTreeMap;
    use std::fs;
    use std::io::Write;
    use std::ops::ControlFlow;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
    use std::thread;
    use std::time::Duration;

    use egui::vec2;
    use egui_kittest::kittest::{Queryable, by};
    use egui_kittest::{Harness, SnapshotResults};
    use eve_means_of_profit::build::prices::Prices;
    use eve_means_of_profit::session::eve::save_fetched;
    use eve_means_of_profit::session::{EveAccess, NoEve, PendingLogin};
    use eve_means_of_profit::{App, AppConfig};
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use mop_core::money::{Money, Rate};
    use mop_esi::character_data::{CharacterData, Fetched, Kind, Refreshed};
    use mop_esi::characters::Character;
    use mop_esi::error::EsiError;
    use mop_esi::industry::{
        CostIndices, IndustryData, IndustryKind, MarketPrice, load_industry, save_industry,
    };
    use mop_esi::places::{NamingReport, StructureNames};
    use mop_market::book::PriceRule;
    use mop_market::error::MarketError;
    use mop_market::fuzzwork::{BytesHttp, BytesResponse};
    use mop_sde::{Facility, Fetch, FetchError, Security, import};
    use mop_store::{FacilityCosts, PlanRecord, Store};
    use tempfile::TempDir;

    const NOW: i64 = 1_000_000;
    const FIXTURE: &str = "../sde/tests/fixtures/sde-3607705";
    const JITA: u32 = 30_000_142;
    const HEADER: &str =
        "what,weightedaverage,maxval,minval,stddev,median,volume,numorders,fivepercent,orderSet";
    const HOBGOBLIN_II: u32 = 2456;
    const RAM_ROBOTICS: u32 = 11_481;

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

    /// The sell side prices of the fake market file, one row per type.
    const SELL_PRICES: [(u32, &str); 10] = [
        (2454, "27500.00"),
        (9834, "10450.00"),
        (9848, "39990.00"),
        (11399, "2150.00"),
        (11481, "478.60"),
        (11688, "30900.00"),
        (34, "4.00"),
        (35, "10.00"),
        (37, "50.00"),
        (38, "800.00"),
    ];

    /// The fake of Fuzzwork's summary file of The Forge.
    struct FakeForge;

    impl BytesHttp for FakeForge {
        fn get_bytes(
            &self,
            _url: &str,
            _if_none_match: Option<&str>,
        ) -> Result<BytesResponse, MarketError> {
            let mut text = format!("{HEADER}\n");
            for (type_id, price) in SELL_PRICES {
                text.push_str(&format!(
                    "10000002|{type_id}|false,{price},{price},{price},0.0,{price},1000000,30,{price},175414\n"
                ));
            }
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

    fn isk(amount: i64) -> Money {
        Money::from_ten_thousandths(amount * 10_000)
    }

    /// The adjusted prices of the spec, as the fake industry answers hold them.
    const ADJUSTED: [(u32, i64); 10] = [
        (11399, 17_000),
        (11688, 30_000),
        (9834, 10_000),
        (2454, 30_000),
        (9848, 40_000),
        (11481, 500),
        (34, 4),
        (35, 10),
        (37, 50),
        (38, 800),
    ];

    /// Industry data fetched at the test clock, good for an hour more: Jita with its manufacturing
    /// and reaction indices, and the adjusted prices, except those of the types in `without`.
    fn industry(without: &[u32], expires: i64) -> IndustryData {
        let mut systems = BTreeMap::new();
        systems.insert(
            JITA,
            CostIndices {
                manufacturing: Some(Rate::from_ppm(50_000)),
                reaction: Some(Rate::from_ppm(1_400)),
                ..CostIndices::default()
            },
        );
        let prices: BTreeMap<u32, MarketPrice> = ADJUSTED
            .iter()
            .filter(|(type_id, _)| !without.contains(type_id))
            .map(|(type_id, price)| {
                (
                    *type_id,
                    MarketPrice {
                        adjusted: Some(isk(*price)),
                        average: None,
                    },
                )
            })
            .collect();
        IndustryData {
            systems: Some(Fetched {
                data: systems,
                fetched_at: NOW,
                last_modified: None,
                expires: Some(expires),
            }),
            prices: Some(Fetched {
                data: prices,
                fetched_at: NOW,
                last_modified: None,
                expires: Some(expires),
            }),
        }
    }

    /// A data folder with the game data installed, the facility `Raitaru Jita`, and the plan.
    struct Env {
        _dir: TempDir,
        data: PathBuf,
        cache: PathBuf,
    }

    /// How a test sets up the folder.
    struct Setup {
        costs: FacilityCosts,
        /// Plan Manufacturing: the saved facility, or the built-in NPC station.
        saved_facility: bool,
        /// The saved industry data, if any.
        industry: Option<IndustryData>,
    }

    impl Setup {
        fn standard() -> Self {
            Self {
                costs: FacilityCosts {
                    system: Some(JITA),
                    tax: Some(Rate::from_ppm(10_000)),
                },
                saved_facility: true,
                industry: Some(industry(&[], NOW + 3_600)),
            }
        }
    }

    fn make_env(setup: &Setup) -> Env {
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
        let facility = store
            .add_facility(
                "Raitaru Jita",
                &Facility {
                    structure: Some(35825),
                    security: Security::High,
                    rigs: Vec::new(),
                },
            )
            .expect("the facility is saved");
        store
            .set_facility_costs(facility, &setup.costs)
            .expect("the costs are saved");
        store
            .create_plan(
                &PlanRecord {
                    id: 0,
                    name: "Costs".to_owned(),
                    product: HOBGOBLIN_II,
                    quantity: 10,
                    build: std::collections::BTreeSet::new(),
                    me: BTreeMap::new(),
                    manufacturing: setup.saved_facility.then_some(facility),
                    reactions: None,
                    price_rule: PriceRule::SellFivePercent,
                    route: None,
                    service: None,
                    packing: mop_core::freight::Packing::TopUp,
                    pasted_stock: String::new(),
                    created_at: NOW,
                    updated_at: NOW,
                },
                NOW,
            )
            .expect("the plan is saved");
        drop(store);
        if let Some(data_to_save) = &setup.industry {
            save_industry(&data.join("esi"), data_to_save).expect("industry data saved");
        }
        Env {
            _dir: dir,
            data,
            cache,
        }
    }

    fn lock<T>(state: &Mutex<T>) -> MutexGuard<'_, T> {
        state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// What the fake `EveAccess` does when the app asks for industry data.
    struct IndustryState {
        /// The kinds asked for, in order.
        calls: Vec<IndustryKind>,
        /// Requests block until this is false.
        hold: bool,
        /// The error every request answers instead of data.
        fault: Option<EsiError>,
        /// The data a request fetches.
        answer: IndustryData,
        /// Where fetched data is saved.
        dir: PathBuf,
    }

    /// A login-less `EveAccess` whose industry answers come from the test.
    struct FakeEve {
        state: Arc<Mutex<IndustryState>>,
    }

    impl EveAccess for FakeEve {
        fn characters(&self) -> Vec<Character> {
            NoEve.characters()
        }

        fn begin_login(&mut self) -> Result<PendingLogin, String> {
            NoEve.begin_login()
        }

        fn finish_login(&mut self, code: &str) -> Result<Character, EsiError> {
            NoEve.finish_login(code)
        }

        fn refresh_all(
            &mut self,
            _data: &mut CharacterData,
            _now: i64,
        ) -> Vec<(Kind, Result<Refreshed, EsiError>)> {
            Vec::new()
        }

        fn refresh_industry(
            &mut self,
            data: &mut IndustryData,
            kind: IndustryKind,
            _now: i64,
        ) -> Result<Refreshed, EsiError> {
            loop {
                if !lock(&self.state).hold {
                    break;
                }
                thread::sleep(Duration::from_millis(5));
            }
            let mut state = lock(&self.state);
            state.calls.push(kind);
            if let Some(EsiError::RateLimited {
                retry_after_seconds,
            }) = &state.fault
            {
                return Err(EsiError::RateLimited {
                    retry_after_seconds: *retry_after_seconds,
                });
            }
            match kind {
                IndustryKind::Systems => data.systems.clone_from(&state.answer.systems),
                IndustryKind::Prices => data.prices.clone_from(&state.answer.prices),
            }
            // Saved the way the live method saves.
            save_fetched(&state.dir, data, kind)?;
            Ok(Refreshed::Fetched)
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

    fn fake_state(env: &Env) -> Arc<Mutex<IndustryState>> {
        Arc::new(Mutex::new(IndustryState {
            calls: Vec::new(),
            hold: false,
            fault: None,
            answer: industry(&[], NOW + 3_600),
            dir: env.data.join("esi"),
        }))
    }

    fn start(env: &Env) -> Harness<'static, App> {
        start_with(env, Box::new(NoEve), vec2(3600.0, 800.0))
    }

    fn start_with(env: &Env, eve: Box<dyn EveAccess>, size: egui::Vec2) -> Harness<'static, App> {
        let config = AppConfig {
            data_dir: env.data.clone(),
            cache_dir: env.cache.clone(),
            fetch: Box::new(NoNetwork),
            clock: Box::new(|| NOW),
            eve,
            market: Box::new(crate::offline::Offline),
        };
        let mut harness = Harness::builder()
            .with_size(size)
            .with_max_steps(crate::SETTLE_STEPS)
            .build_eframe(move |cc| App::new(&cc.egui_ctx, config).expect("the app starts"));
        harness.state_mut().shared_mut().market = Arc::new(Prices::new(Box::new(FakeForge)));
        harness
    }

    /// Opens the plan and waits until the prices and the industry data have arrived.
    fn open_plan(harness: &mut Harness<'_, App>) {
        harness.run();
        click(harness, "Costs");
        settle(harness, "Hobgoblin I");
    }

    /// Runs frames until `done` holds, since the data arrives from the worker thread.
    fn wait_for(
        harness: &mut Harness<'_, App>,
        what: &str,
        done: impl Fn(&Harness<'_, App>) -> bool,
    ) {
        for _ in 0..600 {
            harness.run();
            if done(harness) {
                return;
            }
            thread::sleep(Duration::from_millis(5));
        }
        panic!("{what} never happened");
    }

    fn settle(harness: &mut Harness<'_, App>, label: &str) {
        wait_for(harness, label, |harness| shown(harness, label));
    }

    fn shown(harness: &Harness<'_, App>, text: &str) -> bool {
        harness.query_all_by_label(text).next().is_some()
    }

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

    /// The left edge of the Buy list panel; the Steps rows are looked up left of it.
    fn steps_edge(harness: &Harness<'_, App>) -> f32 {
        harness.get_by_label("Buy list").rect().min.x
    }

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

    /// The nodes of the Steps row labelled `row`, left to right: position and text, empty cells
    /// left out. The table is wider than its panel, so cells reach past the Buy list's edge; the
    /// row ends where the x-position falls back. Nodes without text (the row's own click area and
    /// the cells' frames) do not count as a fallback.
    fn row_nodes(harness: &Harness<'_, App>, row: &str) -> Vec<(egui::Rect, String)> {
        let edge = steps_edge(harness);
        let y = steps_y(harness, row);
        let nodes: Vec<(egui::Rect, Option<String>)> = harness
            .query_all(by().predicate(|node| node.bounding_box().is_some()))
            .map(|node| (node.rect(), node.value()))
            .collect();
        let start = nodes
            .iter()
            .position(|(rect, value)| rect.min.x < edge && value.as_deref() == Some(row))
            .expect("the row in Steps");
        let mut found = Vec::new();
        let mut last = f32::MIN;
        for (rect, value) in &nodes[start..] {
            let Some(value) = value else {
                continue;
            };
            if value.is_empty() {
                continue;
            }
            if (rect.center().y - y).abs() >= 0.5 {
                continue;
            }
            if rect.min.x < last {
                break;
            }
            last = rect.min.x;
            found.push((*rect, value.clone()));
        }
        found
    }

    fn cells(harness: &Harness<'_, App>, row: &str) -> Vec<String> {
        row_nodes(harness, row)
            .into_iter()
            .map(|(_, text)| text)
            .collect()
    }

    /// Scrolls the Steps table sideways by `delta` points (negative moves it left), and runs the
    /// frames the animated scroll takes.
    fn scroll_steps(harness: &mut Harness<'_, App>, delta: f32) {
        let y = steps_y(harness, "Hobgoblin II");
        let x = harness.get_by_label("Steps").rect().min.x + 100.0;
        harness.hover_at(egui::pos2(x, y));
        harness.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: vec2(delta, 0.0),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
        });
        for _ in 0..30 {
            harness.step();
        }
    }

    const II_COST: &str = "125,587.00";
    // The Raitaru's 1% material bonus lowers Tritanium to 5,040 (the spec says 5,090, 42,907.00).
    const I_BUILD: &str = "build: 42,707.00 vs buy 275,000.00";

    fn has_cell(harness: &Harness<'_, App>, row: &str, text: &str) -> bool {
        cells(harness, row).iter().any(|cell| cell == text)
    }

    /// Runs frames until the row has a cell with this text.
    fn settle_cell(harness: &mut Harness<'_, App>, row: &str, text: &str) {
        wait_for(harness, text, |harness| has_cell(harness, row, text));
    }

    /// Opens the plan and waits for the Hobgoblin II row to show `text`.
    fn open_with(harness: &mut Harness<'_, App>, text: &str) {
        open_plan(harness);
        settle_cell(harness, "Hobgoblin II", text);
    }

    /// Hovers the cell of the row with this text, scrolling the table sideways first when the
    /// cell is under the Buy list, and runs the frames the tooltip needs.
    fn hover_cell(harness: &mut Harness<'_, App>, row: &str, text: &str) {
        for _ in 0..3 {
            let edge = steps_edge(harness);
            let (rect, _) = row_nodes(harness, row)
                .into_iter()
                .find(|(_, cell)| cell == text)
                .expect("the cell in the row");
            if rect.center().x < edge - 40.0 {
                harness.hover_at(rect.center());
                for _ in 0..60 {
                    harness.run();
                }
                return;
            }
            scroll_steps(harness, -(rect.center().x - edge + 120.0));
        }
        panic!("the cell {text:?} cannot be brought into view");
    }

    fn label_ends_with(harness: &Harness<'_, App>, end: &str) -> bool {
        harness
            .query_all_by_label_contains(end)
            .any(|node| node.value().is_some_and(|value| value.ends_with(end)))
    }

    #[test]
    fn a_built_row_shows_its_job_cost_and_the_breakdown_on_hover() {
        let env = make_env(&Setup::standard());
        let mut harness = start(&env);
        open_with(&mut harness, II_COST);
        // Make or buy is the last column; empty, so the Job cost is the row's last text.
        let row = cells(&harness, "Hobgoblin II");
        assert_eq!(row.last().map(String::as_str), Some(II_COST), "{row:?}");
        hover_cell(&mut harness, "Hobgoblin II", II_COST);
        assert!(shown(
            &harness,
            "item value 1,275,000.00 · system cost 63,750.00 · structure bonus -1,913.00 · facility tax 12,750.00 · SCC 51,000.00"
        ));
    }

    #[test]
    fn a_bought_part_suggests_building_it_and_the_click_ticks_it() {
        let env = make_env(&Setup::standard());
        let mut harness = start(&env);
        open_with(&mut harness, II_COST);
        settle_cell(&mut harness, "Hobgoblin I", I_BUILD);
        assert!(!shown_containing(&harness, "Job cost 129,434.00"));
        hover_cell(&mut harness, "Hobgoblin I", I_BUILD);
        assert!(shown(&harness, "Click to build this part"));
        // The suggestion the row does not follow is amber.
        let area = cell_area(&harness, "Hobgoblin I", I_BUILD);
        assert!(coloured(&mut harness, area), "the verdict is not amber");
        click(&mut harness, I_BUILD);
        wait_for(&mut harness, "the tick", |harness| {
            shown(harness, "Particle Accelerator Unit")
                || has_cell(harness, "Hobgoblin I", "3,847.00")
        });
        assert!(label_ends_with(&harness, " (unsaved)"));
        let row = cells(&harness, "Hobgoblin I");
        assert_eq!(row.last().map(String::as_str), Some(I_BUILD), "{row:?}");
        assert!(row.iter().any(|cell| cell == "3,847.00"), "{row:?}");
        assert_eq!(
            cell_under(&harness, "Hobgoblin I", "Make").as_deref(),
            Some("10"),
            "{row:?}"
        );
        // The row now follows the verdict, so it is in the text colour.
        let area = cell_area(&harness, "Hobgoblin I", I_BUILD);
        assert!(!coloured(&mut harness, area), "the verdict is still amber");
        for (name, need) in [
            ("Tritanium", "5,040"),
            ("Pyerite", "70"),
            ("Isogen", "40"),
            ("Nocxium", "20"),
        ] {
            assert!(
                has_cell(&harness, name, need),
                "{name}: {:?}",
                cells(&harness, name)
            );
        }
        assert!(
            total_line(&harness).is_some_and(|line| line.starts_with("Job cost 129,434.00 · "))
        );
    }

    /// The part of the row's cell with this text that is inside the Steps panel.
    fn cell_area(harness: &Harness<'_, App>, row: &str, text: &str) -> egui::Rect {
        let edge = steps_edge(harness);
        let (rect, _) = row_nodes(harness, row)
            .into_iter()
            .find(|(_, cell)| cell == text)
            .expect("the cell in the row");
        egui::Rect::from_min_max(rect.min, egui::pos2(rect.max.x.min(edge - 1.0), rect.max.y))
    }

    /// Whether a pixel inside `area` is coloured rather than grey: the amber of a verdict the
    /// row does not follow is, the text colour is not.
    fn coloured(harness: &mut Harness<'_, App>, area: egui::Rect) -> bool {
        let image = harness.render().expect("the frame draws");
        image.enumerate_pixels().any(|(x, y, pixel)| {
            // Pixel centres, in points: the frame is drawn at one pixel per point.
            let x = f32::from(u16::try_from(x).unwrap_or(u16::MAX)) + 0.5;
            let y = f32::from(u16::try_from(y).unwrap_or(u16::MAX)) + 0.5;
            let [r, g, b, _] = pixel.0;
            area.contains(egui::pos2(x, y)) && r.max(g).max(b) - r.min(g).min(b) > 60
        })
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

    /// The text of the total line under the Steps table.
    fn total_line(harness: &Harness<'_, App>) -> Option<String> {
        harness
            .query_all_by_label_contains("Job cost ")
            .filter_map(|node| node.value())
            .find(|value| value.contains(" · cost indices "))
    }

    fn shown_containing(harness: &Harness<'_, App>, text: &str) -> bool {
        harness.query_all_by_label_contains(text).next().is_some()
    }

    #[test]
    fn a_missing_adjusted_price_marks_the_cost_incomplete() {
        let env = make_env(&Setup {
            industry: Some(industry(&[RAM_ROBOTICS], NOW + 3_600)),
            ..Setup::standard()
        });
        let mut harness = start(&env);
        open_with(&mut harness, "125,095.00 incomplete");
        hover_cell(&mut harness, "Hobgoblin II", "125,095.00 incomplete");
        assert!(label_ends_with(
            &harness,
            "No adjusted price: R.A.M.- Robotics"
        ));
        let line = total_line(&harness).expect("the total line");
        assert!(
            line.starts_with("Job cost 125,095.00 · cost indices "),
            "{line}"
        );
        assert!(line.ends_with(" incomplete"), "{line}");
    }

    #[test]
    fn a_facility_without_a_system_or_tax_says_so() {
        for (costs, message) in [
            (
                FacilityCosts {
                    system: None,
                    tax: Some(Rate::from_ppm(10_000)),
                },
                "no system on Raitaru Jita",
            ),
            (
                FacilityCosts {
                    system: Some(JITA),
                    tax: None,
                },
                "no tax on Raitaru Jita",
            ),
        ] {
            let env = make_env(&Setup {
                costs,
                ..Setup::standard()
            });
            let mut harness = start(&env);
            open_with(&mut harness, message);
            settle_cell(&mut harness, "Hobgoblin I", message);
            wait_for(&mut harness, message, |harness| {
                cell_under(harness, "Hobgoblin II", "Job cost").as_deref() == Some(message)
            });
        }
    }

    #[test]
    fn an_npc_station_asks_for_a_saved_facility() {
        let env = make_env(&Setup {
            saved_facility: false,
            ..Setup::standard()
        });
        let mut harness = start(&env);
        let message = "pick a saved facility for job costs";
        open_plan(&mut harness);
        settle_cell(&mut harness, "Hobgoblin I", message);
    }

    #[test]
    fn the_app_refreshes_industry_data_once_and_saves_it_for_the_next_start() {
        let env = make_env(&Setup {
            industry: None,
            ..Setup::standard()
        });
        let state = fake_state(&env);
        let mut harness = start_with(
            &env,
            Box::new(FakeEve {
                state: Arc::clone(&state),
            }),
            vec2(3600.0, 800.0),
        );
        open_with(&mut harness, II_COST);
        for _ in 0..10 {
            harness.run();
        }
        let calls = lock(&state).calls.clone();
        assert_eq!(calls.len(), 2, "{calls:?}");
        assert!(calls.contains(&IndustryKind::Systems));
        assert!(calls.contains(&IndustryKind::Prices));
        drop(harness);
        let saved = load_industry(&env.data.join("esi")).expect("the saved industry data");
        assert!(saved.systems.is_some() && saved.prices.is_some());
        // The second app has no EVE access, so the cost can only come from the saved file. The
        // `open_with` call below waits until the cost shows, which is when the worker has read it.
        let mut second = start(&env);
        open_with(&mut second, II_COST);
    }

    #[test]
    fn a_corrupt_saved_file_is_replaced_by_the_fetched_data() {
        let env = make_env(&Setup {
            industry: None,
            ..Setup::standard()
        });
        let file = env.data.join("esi").join("industry.json");
        fs::write(&file, "not json").expect("the corrupt file is written");
        let state = fake_state(&env);
        let mut harness = start_with(
            &env,
            Box::new(FakeEve {
                state: Arc::clone(&state),
            }),
            vec2(3600.0, 800.0),
        );
        open_with(&mut harness, II_COST);
        wait_for(&mut harness, "both kinds saved", |_| {
            load_industry(&env.data.join("esi"))
                .is_ok_and(|saved| saved.systems.is_some() && saved.prices.is_some())
        });
        // The status line shows the first notice (the start-up read error) and queues this one.
        wait_for(&mut harness, "the queued notice", |harness| {
            shown_containing(harness, "(1 of 2)")
        });
        click(&mut harness, "Dismiss");
        wait_for(&mut harness, "the replacement notice", |harness| {
            shown_containing(
                harness,
                "could not be read and was replaced with fresh data",
            )
        });
        let calls = lock(&state).calls.clone();
        assert_eq!(calls.len(), 2, "{calls:?}");
    }

    #[test]
    fn fresh_industry_data_is_not_fetched_again() {
        let env = make_env(&Setup::standard());
        let state = fake_state(&env);
        let mut harness = start_with(
            &env,
            Box::new(FakeEve {
                state: Arc::clone(&state),
            }),
            vec2(3600.0, 800.0),
        );
        open_with(&mut harness, II_COST);
        for _ in 0..40 {
            harness.run();
            thread::sleep(Duration::from_millis(5));
        }
        assert!(lock(&state).calls.is_empty());
    }

    #[test]
    fn a_rate_limited_refresh_is_reported_and_keeps_the_costs() {
        let env = make_env(&Setup {
            industry: Some(industry(&[], NOW - 1)),
            ..Setup::standard()
        });
        let state = fake_state(&env);
        lock(&state).fault = Some(EsiError::RateLimited {
            retry_after_seconds: 30,
        });
        let mut harness = start_with(
            &env,
            Box::new(FakeEve {
                state: Arc::clone(&state),
            }),
            vec2(3600.0, 800.0),
        );
        open_with(&mut harness, II_COST);
        wait_for(&mut harness, "the rate limit message", |harness| {
            shown_containing(
                harness,
                "EVE industry data paused for 30 s (ESI rate limit).",
            )
        });
        assert!(has_cell(&harness, "Hobgoblin II", II_COST));
    }

    #[test]
    fn a_slow_refresh_does_not_block_the_page() {
        let env = make_env(&Setup {
            industry: None,
            ..Setup::standard()
        });
        let state = fake_state(&env);
        lock(&state).hold = true;
        let mut harness = start_with(
            &env,
            Box::new(FakeEve {
                state: Arc::clone(&state),
            }),
            vec2(3600.0, 800.0),
        );
        open_plan(&mut harness);
        wait_for(&mut harness, "the waiting cell", |harness| {
            has_cell(harness, "Hobgoblin II", "…")
        });
        for _ in 0..20 {
            harness.run();
            thread::sleep(Duration::from_millis(5));
        }
        assert!(has_cell(&harness, "Hobgoblin II", "…"));
        assert!(lock(&state).calls.is_empty());
        lock(&state).hold = false;
        settle_cell(&mut harness, "Hobgoblin II", II_COST);
    }

    #[test]
    fn the_steps_table_with_both_new_columns_matches_its_snapshot() {
        let mut results = SnapshotResults::new();
        let env = make_env(&Setup::standard());
        let mut harness = start(&env);
        open_with(&mut harness, II_COST);
        settle_cell(&mut harness, "Hobgoblin I", I_BUILD);
        scroll_steps(&mut harness, -600.0);
        harness.snapshot("costs-steps");
        results.extend_harness(&mut harness);
        results.unwrap();
    }
}

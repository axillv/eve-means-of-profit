//! Acceptance tests for the invention columns of the Steps table: the chance, copies, attempts,
//! fee, cost per copy, carried cost, spare runs and the words that say why there are none. The
//! game data is the build 3607705 fixture. Prices come from a fake of Fuzzwork's summary file, the
//! industry data from a saved `industry.json`. The clock is fixed at 1,000,000.
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

    use egui::vec2;
    use egui_kittest::kittest::{Queryable, by};
    use egui_kittest::{Harness, SnapshotResults};
    use eve_means_of_profit::build::prices::Prices;
    use eve_means_of_profit::session::NoEve;
    use eve_means_of_profit::{App, AppConfig};
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use mop_core::money::{Money, Rate};
    use mop_esi::industry::{CostIndices, IndustryData, MarketPrice, save_industry};
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
    const HOBGOBLIN_I: u32 = 2454;
    const HOBGOBLIN_II: u32 = 2456;
    const PLAN: &str = "Invention";
    const HINT: &str =
        "Invention skills not set: type them in Settings or read them from a character.";
    /// The contract of the acceptance: the three invention skills, saved at 4.
    const SKILLS: [(u32, u8); 3] = [(11_453, 4), (11_446, 4), (23_121, 4)];

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

    /// The sell side prices of the fake market file, one row per type. The datacores are the
    /// prices the acceptance names.
    fn sell_prices() -> Vec<(u32, &'static str)> {
        vec![
            (HOBGOBLIN_I, "27500.00"),
            (9834, "10450.00"),
            (9848, "39990.00"),
            (11399, "2150.00"),
            (11481, "478.60"),
            (11688, "30900.00"),
            (34, "4.00"),
            (35, "10.00"),
            (37, "50.00"),
            (38, "800.00"),
            (20_418, "50000.00"),
            (20_419, "43750.00"),
        ]
    }

    /// The fake of Fuzzwork's summary file of The Forge.
    struct FakeForge {
        lines: Vec<(u32, &'static str)>,
    }

    impl BytesHttp for FakeForge {
        fn get_bytes(
            &self,
            _url: &str,
            _if_none_match: Option<&str>,
        ) -> Result<BytesResponse, MarketError> {
            let mut text = format!("{HEADER}\n");
            for (type_id, price) in &self.lines {
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

    /// The adjusted prices of the costs tests, except Robotics at the acceptance's 14,286.00.
    fn adjusted() -> [(u32, i64); 10] {
        [
            (11_399, 17_000),
            (11_688, 30_000),
            (9834, 10_000),
            (HOBGOBLIN_I, 30_000),
            (9848, 14_286),
            (11_481, 500),
            (34, 4),
            (35, 10),
            (37, 50),
            (38, 800),
        ]
    }

    /// Industry data fetched at the test clock, good for an hour more: Jita with its manufacturing
    /// and invention indices, and the adjusted prices, except those of the types in `without`.
    fn industry(without: &[u32], expires: i64) -> IndustryData {
        let mut systems = BTreeMap::new();
        systems.insert(
            JITA,
            CostIndices {
                manufacturing: Some(Rate::from_ppm(50_000)),
                reaction: Some(Rate::from_ppm(1_400)),
                invention: Some(Rate::from_ppm(1_400)),
                ..CostIndices::default()
            },
        );
        let prices: BTreeMap<u32, MarketPrice> = adjusted()
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
            systems: Some(mop_esi::character_data::Fetched {
                data: systems,
                fetched_at: NOW,
                last_modified: None,
                expires: Some(expires),
            }),
            prices: Some(mop_esi::character_data::Fetched {
                data: prices,
                fetched_at: NOW,
                last_modified: None,
                expires: Some(expires),
            }),
        }
    }

    /// A data folder with the game data installed and the saved facility `NPC Jita`.
    struct Env {
        _dir: TempDir,
        data: PathBuf,
        cache: PathBuf,
        sell: Vec<(u32, &'static str)>,
    }

    /// How a test sets up the folder.
    struct Setup {
        /// The plan's product and quantity.
        product: u32,
        quantity: u64,
        costs: FacilityCosts,
        /// The saved industry data, if any.
        industry: Option<IndustryData>,
        /// Save the invention skills of the acceptance.
        skills: bool,
        /// Remove these types from the fake market file.
        no_sell: Vec<u32>,
    }

    impl Setup {
        fn standard() -> Self {
            Self {
                product: HOBGOBLIN_II,
                quantity: 8,
                costs: FacilityCosts {
                    system: Some(JITA),
                    tax: Some(Rate::from_ppm(2_500)),
                },
                industry: Some(industry(&[], NOW + 3_600)),
                skills: true,
                no_sell: Vec::new(),
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
                "NPC Jita",
                &Facility {
                    structure: None,
                    security: Security::High,
                    rigs: Vec::new(),
                },
            )
            .expect("the facility is saved");
        store
            .set_facility_costs(facility, &setup.costs)
            .expect("the costs are saved");
        if setup.skills {
            for (type_id, level) in SKILLS {
                store
                    .set_invention_skill(type_id, level)
                    .expect("the skill is saved");
            }
        }
        store
            .create_plan(
                &PlanRecord {
                    id: 0,
                    name: PLAN.to_owned(),
                    product: setup.product,
                    quantity: setup.quantity,
                    build: std::collections::BTreeSet::new(),
                    me: BTreeMap::new(),
                    manufacturing: Some(facility),
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
        let sell = sell_prices()
            .into_iter()
            .filter(|(type_id, _)| !setup.no_sell.contains(type_id))
            .collect();
        Env {
            _dir: dir,
            data,
            cache,
            sell,
        }
    }

    fn start(env: &Env) -> Harness<'static, App> {
        let config = AppConfig {
            data_dir: env.data.clone(),
            cache_dir: env.cache.clone(),
            fetch: Box::new(NoNetwork),
            clock: Box::new(|| NOW),
            eve: Box::new(NoEve),
            market: Box::new(crate::offline::Offline),
        };
        let mut harness = Harness::builder()
            .with_size(vec2(3600.0, 800.0))
            .with_max_steps(crate::SETTLE_STEPS)
            .build_eframe(move |cc| App::new(&cc.egui_ctx, config).expect("the app starts"));
        harness.state_mut().shared_mut().market = Arc::new(Prices::new(Box::new(FakeForge {
            lines: env.sell.clone(),
        })));
        harness
            .state_mut()
            .show_page(eve_means_of_profit::rail::Page::Build);
        harness
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

    /// Opens the plan, scrolls the Steps table to the invention columns, and waits until the
    /// Hobgoblin II row shows `chance`.
    fn open(harness: &mut Harness<'_, App>, chance: &str) {
        harness.run();
        click(harness, PLAN);
        wait_for(harness, "the plan rows", |harness| {
            shown(harness, "Hobgoblin II")
        });
        scroll_to_invention(harness, "Hobgoblin II");
        wait_for(harness, "the invention cells", |harness| {
            invention_cells(harness, "Hobgoblin II")
                .iter()
                .any(|cell| cell == chance)
        });
    }

    /// Opens a plan whose rows have no invention and scrolls the table to the invention columns.
    fn open_plain(harness: &mut Harness<'_, App>, row: &str) {
        harness.run();
        click(harness, PLAN);
        wait_for(harness, "the plan rows", |harness| shown(harness, row));
        scroll_to_invention(harness, row);
    }

    /// The left edge of the Buy list panel; the Steps rows are looked up left of it.
    fn steps_edge(harness: &Harness<'_, App>) -> f32 {
        harness.get_by_label("Buy list").rect().min.x
    }

    /// The left edge of the Steps panel.
    fn steps_left(harness: &Harness<'_, App>) -> f32 {
        harness.get_by_label("Steps").rect().min.x
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

    /// Scrolls the Steps table to its right edge, where the invention columns are, and runs the
    /// frames the animated scroll takes.
    fn scroll_to_invention(harness: &mut Harness<'_, App>, row: &str) {
        for _ in 0..3 {
            let y = steps_y(harness, row);
            let x = steps_left(harness) + 100.0;
            harness.hover_at(egui::pos2(x, y));
            harness.event(egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: vec2(-2_000.0, 0.0),
                phase: egui::TouchPhase::Move,
                modifiers: egui::Modifiers::NONE,
            });
            for _ in 0..30 {
                harness.step();
            }
        }
    }

    /// The visible texts of the seven invention cells of `row`, left to right, once the table is
    /// scrolled to them. Empty cells are empty strings.
    fn invention_cells(harness: &Harness<'_, App>, row: &str) -> Vec<String> {
        let y = steps_y(harness, row);
        let mut found: Vec<(f32, String)> = Vec::new();
        let edge = steps_edge(harness) - 20.0;
        let left = steps_left(harness);
        for node in harness.query_all(by().predicate(|node| node.bounding_box().is_some())) {
            let rect = node.rect();
            if (rect.center().y - y).abs() >= 0.5 {
                continue;
            }
            if rect.min.x < left || rect.min.x >= edge {
                continue;
            }
            found.push((rect.min.x, node.value().unwrap_or_default()));
        }
        found.sort_by(|a, b| a.0.total_cmp(&b.0));
        let cells: Vec<String> = found.into_iter().map(|(_, text)| text).collect();
        assert!(
            cells.len() >= 7,
            "the seven invention cells of {row} are not in view: {cells:?}"
        );
        cells[cells.len() - 7..].to_vec()
    }

    #[test]
    fn a_built_t2_part_shows_its_invention_numbers() {
        let env = make_env(&Setup::standard());
        let mut harness = start(&env);
        open(&mut harness, "46.47%");
        assert_eq!(
            invention_cells(&harness, "Hobgoblin II"),
            [
                "46.47%",
                "1 × 10 runs",
                "3",
                "89.00",
                "201,949.07",
                "161,559.25",
                "2 runs left on the copy",
            ]
        );
        assert!(!shown(&harness, HINT));
    }

    #[test]
    fn without_saved_skills_the_hint_shows_and_the_chance_counts_them_as_zero() {
        let env = make_env(&Setup {
            skills: false,
            ..Setup::standard()
        });
        let mut harness = start(&env);
        open(&mut harness, "34.00%");
        assert!(shown(&harness, HINT));
        let cells = invention_cells(&harness, "Hobgoblin II");
        for expected in ["34.00%", "3", "275,997.06", "220,797.65"] {
            assert!(cells.iter().any(|cell| cell == expected), "{cells:?}");
        }
    }

    #[test]
    fn a_fee_that_cannot_be_worked_out_is_shown_with_its_reason() {
        let env = make_env(&Setup {
            costs: FacilityCosts {
                system: None,
                tax: Some(Rate::from_ppm(2_500)),
            },
            ..Setup::standard()
        });
        let mut harness = start(&env);
        open(&mut harness, "46.47%");
        let cells = invention_cells(&harness, "Hobgoblin II");
        assert!(cells.iter().any(|cell| cell == "0.00"), "{cells:?}");
        assert!(cells.iter().any(|cell| cell == "201,757.53"), "{cells:?}");
        assert!(
            cells
                .iter()
                .any(|cell| cell
                    == "fee not counted: no system on NPC Jita · 2 runs left on the copy"),
            "{cells:?}"
        );
    }

    #[test]
    fn a_datacore_without_a_price_says_so_instead_of_the_numbers() {
        let env = make_env(&Setup {
            no_sell: vec![20_419],
            ..Setup::standard()
        });
        let mut harness = start(&env);
        open(
            &mut harness,
            "Invention: no price for Datacore - Graviton Physics.",
        );
        let cells = invention_cells(&harness, "Hobgoblin II");
        assert_eq!(
            cells,
            [
                "—",
                "—",
                "—",
                "—",
                "—",
                "—",
                "Invention: no price for Datacore - Graviton Physics.",
            ]
        );
    }

    #[test]
    fn a_t1_plan_has_no_invention_cells_and_no_hint() {
        let env = make_env(&Setup {
            product: HOBGOBLIN_I,
            ..Setup::standard()
        });
        let mut harness = start(&env);
        open_plain(&mut harness, "Tritanium");
        assert!(!shown(&harness, HINT));
        for row in ["Hobgoblin I", "Tritanium"] {
            let cells = invention_cells(&harness, row);
            assert!(cells.iter().all(String::is_empty), "{row}: {cells:?}");
        }
    }

    #[test]
    fn the_invention_columns_match_their_snapshot() {
        let mut results = SnapshotResults::new();
        let env = make_env(&Setup::standard());
        let mut harness = start(&env);
        open(&mut harness, "46.47%");
        harness.snapshot("steps-invention");
        results.extend_harness(&mut harness);
        results.unwrap();
    }
}

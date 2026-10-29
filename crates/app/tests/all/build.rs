//! Acceptance tests for the Build page: the plans, the plan inputs, the pasted stock and the steps.
//! The game data is the build 3607705 fixture, imported in the test. The user data is a temp store
//! with the facilities the plans use. The clock is fixed at 1,000,000.
#[cfg(test)]
mod tests {

    use std::fs;
    use std::io::Write;
    use std::ops::ControlFlow;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use egui::accesskit::Role;
    use egui::{Event, Key, Modifiers, vec2};
    use egui_kittest::kittest::{NodeT, Queryable, by};
    use egui_kittest::{Harness, SnapshotResults};
    use eve_means_of_profit::build::prices::Prices;
    use eve_means_of_profit::session::NoEve;
    use eve_means_of_profit::{App, AppConfig};
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use mop_market::book::PriceRule;
    use mop_market::error::MarketError;
    use mop_market::fuzzwork::{BytesHttp, BytesResponse};
    use mop_sde::{Facility, Fetch, FetchError, Security, import};
    use mop_store::Store;
    use rusqlite::Connection;
    use tempfile::TempDir;

    const NOW: i64 = 1_000_000;
    const FIXTURE: &str = "../sde/tests/fixtures/sde-3607705";

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

    /// A data folder with the game data installed and the given facilities saved.
    struct Env {
        _dir: TempDir,
        data: PathBuf,
        cache: PathBuf,
    }

    fn facility(structure: u32, rigs: &[u32]) -> Facility {
        Facility {
            structure: Some(structure),
            security: Security::Null,
            rigs: rigs.to_vec(),
        }
    }

    /// `Raitaru null` and `Athanor null`, as the plans expect them.
    fn standard_facilities() -> Vec<(&'static str, Facility)> {
        vec![
            ("Raitaru null", facility(35825, &[37157])),
            ("Athanor null", facility(35835, &[46487])),
        ]
    }

    fn make_env(facilities: &[(&str, Facility)]) -> Env {
        let dir = TempDir::new().expect("a temp folder");
        let data = dir.path().join("data");
        let cache = dir.path().join("cache");
        fs::create_dir_all(&data).expect("the data folder");
        import(&fixture(), &data.join("sde.sqlite")).expect("the fixture imports");
        history_off(&data.join("user.sqlite"));
        let mut store = Store::open(&data.join("user.sqlite"), NOW)
            .expect("the store opens")
            .store;
        let mut settings = store.settings().expect("settings");
        settings.last_sde_check = Some(NOW);
        store.save_settings(&settings).expect("settings saved");
        for (name, facility) in facilities {
            store.add_facility(name, facility).expect("facility saved");
        }
        Env {
            _dir: dir,
            data,
            cache,
        }
    }

    /// Saves an empty plan named `name` in the env's store.
    fn seed_plan(env: &Env, name: &str) {
        let mut store = Store::open(&env.data.join("user.sqlite"), NOW)
            .expect("the store opens")
            .store;
        store
            .create_plan(
                &mop_store::PlanRecord {
                    id: 0,
                    name: name.to_owned(),
                    product: 0,
                    quantity: 1,
                    build: std::collections::BTreeSet::new(),
                    me: std::collections::BTreeMap::new(),
                    manufacturing: None,
                    reactions: None,
                    price_rule: PriceRule::SellFivePercent,
                    route: None,
                    service: None,
                    packing: mop_core::freight::Packing::default(),
                    pasted_stock: String::new(),
                    created_at: NOW,
                    updated_at: NOW,
                },
                NOW,
            )
            .expect("the plan is saved");
    }

    /// The number of plans in the running app's store.
    fn plan_count(harness: &mut Harness<'_, App>) -> usize {
        harness
            .state_mut()
            .shared_mut()
            .store
            .plans()
            .expect("the plans")
            .len()
    }

    /// Clicks the `New plan` empty-state button of the panel titled `panel`, out of the up to four
    /// such buttons a Build with no plan on screen draws.
    fn click_new_plan_in(harness: &mut Harness<'_, App>, panel: &str) {
        let plan_left = harness.get_by_label("Plan").rect().min.x;
        let buy_left = harness.get_by_label("Buy list").rect().min.x;
        let steps_top = harness.get_by_label("Steps").rect().min.y;
        let node = harness
            .get_all_by_label("New plan")
            .find(|node| match panel {
                "Plans" => node.rect().min.x < plan_left,
                "Buy list" => node.rect().min.x >= buy_left,
                "Steps" => node.rect().min.x < buy_left && node.rect().min.y >= steps_top,
                _ => node.rect().min.x < buy_left && node.rect().min.y < steps_top,
            })
            .unwrap_or_else(|| panic!("the New plan button of {panel}"));
        node.click();
        harness.run_steps(6);
    }

    fn stored_rule(env: &Env, name: &str) -> PriceRule {
        let mut store = Store::open(&env.data.join("user.sqlite"), NOW)
            .expect("the store opens")
            .store;
        let id = store
            .plans()
            .expect("plans")
            .into_iter()
            .find(|plan| plan.name == name)
            .expect("the plan is saved")
            .id;
        store
            .plan(id)
            .expect("plan")
            .expect("plan exists")
            .price_rule
    }

    /// The background price refresh stays off here: the test asks for each download itself.
    fn history_off(db: &Path) {
        let mut store = Store::open(db, NOW).expect("the store opens").store;
        let mut settings = store.history_settings().expect("history settings");
        settings.auto_refresh = false;
        store
            .save_history_settings(&settings)
            .expect("history settings saved");
    }

    #[test]
    fn a_plan_shows_its_rule_and_a_pick_is_unsaved_until_it_is_saved() {
        let env = make_env(&standard_facilities());
        let mut store = Store::open(&env.data.join("user.sqlite"), NOW)
            .expect("the store opens")
            .store;
        store
            .create_plan(
                &mop_store::PlanRecord {
                    id: 0,
                    name: "Buy rule".to_owned(),
                    product: 2456,
                    quantity: 10,
                    build: std::collections::BTreeSet::new(),
                    me: std::collections::BTreeMap::new(),
                    manufacturing: None,
                    reactions: None,
                    price_rule: PriceRule::HighestBuy,
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
        let mut harness = start(&env);
        harness.run();
        click(&mut harness, "Buy rule");
        assert!(has_value(&harness, "Forge buy, highest"));

        pick(
            &mut harness,
            "Forge buy, highest",
            "Forge sell, weighted average",
        );
        assert!(
            harness
                .query_all_by_label_contains("(unsaved)")
                .next()
                .is_some(),
            "the plan is marked unsaved"
        );
        click(&mut harness, "Save");
        assert_eq!(
            stored_rule(&env, "Buy rule"),
            PriceRule::SellWeightedAverage
        );
    }

    #[test]
    fn a_new_plan_takes_the_default_rule_and_an_older_plan_keeps_its_own() {
        let env = make_env(&standard_facilities());
        let mut store = Store::open(&env.data.join("user.sqlite"), NOW)
            .expect("the store opens")
            .store;
        let mut settings = store.settings().expect("settings");
        settings.default_price_rule = PriceRule::BuyMedian;
        store.save_settings(&settings).expect("settings saved");
        for (name, rule) in [
            ("Old buy", PriceRule::HighestBuy),
            ("Old walk", PriceRule::WalkSell),
        ] {
            store
                .create_plan(
                    &mop_store::PlanRecord {
                        id: 0,
                        name: name.to_owned(),
                        product: 2456,
                        quantity: 10,
                        build: std::collections::BTreeSet::new(),
                        me: std::collections::BTreeMap::new(),
                        manufacturing: None,
                        reactions: None,
                        price_rule: rule,
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
        }
        drop(store);
        let mut harness = start(&env);
        harness.run();

        click(&mut harness, "New");
        assert!(has_value(&harness, "Forge buy, middle"));

        click(&mut harness, "Old buy");
        assert!(has_value(&harness, "Forge buy, highest"));

        click(&mut harness, "Old walk");
        assert!(has_value(&harness, "Forge sell, 5% price"));
    }

    /// A market file with no rows at all, so the Buy list never reaches the network in these tests.
    /// It is dated the test clock.
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

    fn start(env: &Env) -> Harness<'static, App> {
        start_sized(env, vec2(1900.0, 800.0))
    }

    fn start_sized(env: &Env, size: egui::Vec2) -> Harness<'static, App> {
        let config = AppConfig {
            data_dir: env.data.clone(),
            cache_dir: env.cache.clone(),
            fetch: Box::new(NoNetwork),
            clock: Box::new(|| NOW),
            eve: Box::new(NoEve),
            market: Box::new(crate::offline::Offline),
        };
        let mut harness = Harness::builder()
            .with_size(size)
            .with_max_steps(crate::SETTLE_STEPS)
            .build_eframe(move |cc| App::new(&cc.egui_ctx, config).expect("the app starts"));
        crate::ui::no_images(&mut harness);
        harness.state_mut().shared_mut().market = Arc::new(Prices::new(Box::new(EmptyForge)));
        harness
            .state_mut()
            .show_page(eve_means_of_profit::rail::Page::Build);
        harness
    }

    fn shown(harness: &Harness<'_, App>, text: &str) -> bool {
        harness.query_all_by_label(text).next().is_some()
    }

    /// Clicks the button labelled `label`; when several are, the last one drawn (a dialog's button).
    fn click(harness: &mut Harness<'_, App>, label: &str) {
        let count = harness.get_all_by_label(label).count();
        assert!(count > 0, "no node labelled {label:?}");
        let last = count - 1;
        harness
            .get_all_by_label(label)
            .nth(last)
            .expect("the button")
            .click();
        // Steps, not `run`: the background industry job can keep the page repainting.
        harness.run_steps(6);
    }

    /// Opens the dropdown that shows `current` and picks `option` from it.
    fn pick(harness: &mut Harness<'_, App>, current: &str, option: &str) {
        harness
            .get_by(|node| node.value().is_some_and(|value| value == current))
            .click();
        harness.run();
        click(harness, option);
    }

    /// Clicks the single-line box that holds `value`, selects its text and types `text` over it.
    fn type_into(harness: &mut Harness<'_, App>, value: &str, text: &str) {
        harness
            .get_by(|node| node.role() == Role::TextInput && node.value().as_deref() == Some(value))
            .click();
        harness.key_press_modifiers(Modifiers::COMMAND, Key::A);
        harness.event(Event::Text(text.to_owned()));
        harness.run();
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

    /// Whether some node shows `text` as its value: a box's text or a dropdown's chosen item.
    fn has_value(harness: &Harness<'_, App>, text: &str) -> bool {
        harness
            .query_all(by().predicate(|node| node.value().is_some_and(|value| value == text)))
            .next()
            .is_some()
    }

    /// The left edge of the Buy list panel, which sits right of the Steps panel. The Buy list shows
    /// the same item names, so the Steps rows are looked up left of this edge.
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

    /// The cell texts of the row labelled `row`, left to right: the item name, then each column
    /// that has text (the Build box has none). Spare 0 is an empty text.
    fn cells(harness: &Harness<'_, App>, row: &str) -> Vec<String> {
        let edge = steps_edge(harness);
        let y = steps_y(harness, row);
        let nodes: Vec<(f32, f32, Option<String>)> = harness
            .query_all(by().predicate(|node| node.bounding_box().is_some()))
            .map(|node| (node.rect().min.x, node.rect().center().y, node.value()))
            .collect();
        // The Steps table is wider than its panel and scrolls sideways, so its cells can reach past
        // the Buy list's edge. Nodes come in drawing order, left to right within a row; the row ends
        // where the x-position falls back, which is where the Buy list's first node starts.
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

    /// The top edge of the first node labelled `label`, to check the order of rows.
    fn top_of(harness: &Harness<'_, App>, label: &str) -> f32 {
        let edge = steps_edge(harness);
        harness
            .query_all_by_label(label)
            .find(|node| node.rect().min.x < edge)
            .expect("the row in Steps")
            .rect()
            .min
            .y
    }

    /// Ticks the Build box on the row labelled `row`, or the ME box of that row.
    fn tick_row(harness: &mut Harness<'_, App>, row: &str) {
        let edge = steps_edge(harness);
        let y = steps_y(harness, row);
        harness
            .get_all_by_role(Role::CheckBox)
            .find(|node| node.rect().min.x < edge && (node.rect().center().y - y).abs() < 4.0)
            .expect("a Build box on the row")
            .click();
        harness.run();
    }

    /// Whether the Build box on the row labelled `row` is disabled.
    fn tick_disabled(harness: &Harness<'_, App>, row: &str) -> bool {
        let edge = steps_edge(harness);
        let y = steps_y(harness, row);
        harness
            .get_all_by_role(Role::CheckBox)
            .find(|node| node.rect().min.x < edge && (node.rect().center().y - y).abs() < 4.0)
            .expect("a Build box on the row")
            .accesskit_node()
            .is_disabled()
    }

    /// Hovers the Build box on the row labelled `row`.
    fn hover_tick(harness: &mut Harness<'_, App>, row: &str) {
        let edge = steps_edge(harness);
        let y = steps_y(harness, row);
        harness
            .get_all_by_role(Role::CheckBox)
            .find(|node| node.rect().min.x < edge && (node.rect().center().y - y).abs() < 4.0)
            .expect("a Build box on the row")
            .hover();
        harness.run();
    }

    /// Sets the ME box of the row labelled `row` to `me`.
    fn set_me(harness: &mut Harness<'_, App>, row: &str, me: &str) {
        let edge = steps_edge(harness);
        let y = steps_y(harness, row);
        harness
            .get_all_by_role(Role::SpinButton)
            .find(|node| node.rect().min.x < edge && (node.rect().center().y - y).abs() < 4.0)
            .expect("an ME box on the row")
            .click();
        harness.key_press_modifiers(Modifiers::COMMAND, Key::A);
        harness.event(Event::Text(me.to_owned()));
        harness.key_press(Key::Enter);
        harness.run();
    }

    /// Makes a plan (named `New plan`) for Hobgoblin II, quantity 10, built at the NPC station.
    fn hobgoblin_ii_plan(harness: &mut Harness<'_, App>) {
        click(harness, "New plan");
        type_into(harness, "", "hobgoblin");
        click(harness, "Hobgoblin II");
        type_into(harness, "1", "10");
    }

    /// Hobgoblin II built at Raitaru null, with Hobgoblin I and Particle Accelerator Unit built at
    /// ME 10, and the paste from the spec.
    fn p3_plan(harness: &mut Harness<'_, App>) {
        p3_plan_with(harness, "Particle Accelerator Unit\t4\nTritanium\t1,000");
    }

    /// The P3 plan with `pasted` in the stock box.
    fn p3_plan_with(harness: &mut Harness<'_, App>, pasted: &str) {
        hobgoblin_ii_plan(harness);
        pick(
            harness,
            "NPC station · high sec",
            "Raitaru null · Raitaru · null sec · 1 rigs",
        );
        tick_row(harness, "Hobgoblin I");
        tick_row(harness, "Particle Accelerator Unit");
        set_me(harness, "Hobgoblin I", "10");
        set_me(harness, "Particle Accelerator Unit", "10");
        paste(harness, pasted);
    }

    #[test]
    fn plans_are_made_copied_renamed_and_deleted() {
        let env = make_env(&[]);
        let mut harness = start(&env);
        harness.run();
        click(&mut harness, "New plan");
        click(&mut harness, "New");
        assert!(shown(&harness, "New plan"));
        assert!(shown(&harness, "New plan 2"));

        click(&mut harness, "New plan");
        click(&mut harness, "Duplicate");
        assert!(shown(&harness, "New plan (copy)"));

        click(&mut harness, "New plan 2");
        click(&mut harness, "Delete");
        assert!(shown(&harness, "Delete plan New plan 2?"));
        click(&mut harness, "Cancel");
        assert!(shown(&harness, "New plan 2"));

        // The list is in the user's order, not by name: the renamed plan keeps its place between
        // its neighbours.
        let (first, copy) = (
            top_of(&harness, "New plan"),
            top_of(&harness, "New plan (copy)"),
        );
        assert!(first < top_of(&harness, "New plan 2") && top_of(&harness, "New plan 2") < copy);
        click(&mut harness, "Rename");
        type_into(&mut harness, "New plan 2", "Hobgoblin 10");
        click(&mut harness, "Rename");
        assert!(shown(&harness, "Hobgoblin 10"));
        let renamed = top_of(&harness, "Hobgoblin 10");
        assert!(first < renamed && renamed < copy);
    }

    #[test]
    fn product_search_lists_only_types_with_a_recipe() {
        let env = make_env(&standard_facilities());
        let mut harness = start(&env);
        harness.run();
        click(&mut harness, "New plan");
        type_into(&mut harness, "", "hobgoblin");
        assert!(shown(&harness, "Hobgoblin I"));
        assert!(shown(&harness, "Hobgoblin II"));
        assert!(!shown(&harness, "Hobgoblin II Blueprint"));
    }

    #[test]
    fn hobgoblin_ii_shows_its_six_parts_in_recipe_order() {
        let env = make_env(&standard_facilities());
        let mut harness = start(&env);
        harness.run();
        hobgoblin_ii_plan(&mut harness);
        assert!(shown(&harness, "PLAN  Hobgoblin II × 10 (unsaved)"));
        let parts = [
            "Morphite",
            "Particle Accelerator Unit",
            "Guidance Systems",
            "Hobgoblin I",
            "Robotics",
            "R.A.M.- Robotics",
        ];
        let tops: Vec<f32> = parts.iter().map(|part| top_of(&harness, part)).collect();
        assert!(tops.windows(2).all(|pair| pair[0] < pair[1]));
        assert_eq!(
            cells(&harness, "Hobgoblin II"),
            ["Hobgoblin II", "2", "10", "0", "10", "—", "10", "1", "…"]
        );
        for part in parts {
            assert_eq!(cells(&harness, part), [part, "—", "10", "0", "—", "10"]);
        }
    }

    #[test]
    fn p3_steps_and_stock_numbers() {
        let env = make_env(&standard_facilities());
        let mut harness = start(&env);
        harness.run();
        p3_plan(&mut harness);
        assert_eq!(
            cells(&harness, "Hobgoblin II"),
            ["Hobgoblin II", "2", "10", "0", "10", "—", "10", "1", "…"]
        );
        assert_eq!(
            cells(&harness, "Hobgoblin I"),
            ["Hobgoblin I", "10", "10", "0", "10", "—", "10", "1", "…"]
        );
        assert_eq!(
            cells(&harness, "Particle Accelerator Unit"),
            [
                "Particle Accelerator Unit",
                "10",
                "10",
                "4",
                "6",
                "—",
                "6",
                "1",
                "…"
            ]
        );
        assert_eq!(
            cells(&harness, "Tritanium"),
            ["Tritanium", "—", "4,307", "1,000", "—", "3,307"]
        );
        assert_eq!(
            cells(&harness, "Pyerite"),
            ["Pyerite", "—", "60", "0", "—", "60"]
        );
        assert_eq!(
            cells(&harness, "Isogen"),
            ["Isogen", "—", "34", "0", "—", "34"]
        );
        assert_eq!(
            cells(&harness, "Nocxium"),
            ["Nocxium", "—", "17", "0", "—", "17"]
        );
        assert_eq!(
            cells(&harness, "Crystalline Carbonide"),
            ["Crystalline Carbonide", "—", "166", "0", "—", "166"]
        );
        assert_eq!(
            cells(&harness, "Fullerides"),
            ["Fullerides", "—", "59", "0", "—", "59"]
        );
        assert_eq!(
            cells(&harness, "Hypersynaptic Fibers"),
            ["Hypersynaptic Fibers", "—", "6", "0", "—", "6"]
        );
        // The materials of Hobgoblin I sit between it and the next part; those of the unit sit
        // between the unit and the part after it.
        assert!(top_of(&harness, "Hobgoblin I") < top_of(&harness, "Tritanium"));
        assert!(top_of(&harness, "Nocxium") < top_of(&harness, "Robotics"));
        assert!(
            top_of(&harness, "Particle Accelerator Unit")
                < top_of(&harness, "Crystalline Carbonide")
        );
        assert!(top_of(&harness, "Hypersynaptic Fibers") < top_of(&harness, "Guidance Systems"));
        assert!(!shown(&harness, "Left out, could not be read:"));
        assert!(!shown(&harness, "does not use"));
    }

    #[test]
    fn paste_problems_are_counted_and_listed() {
        let env = make_env(&standard_facilities());
        let mut harness = start(&env);
        harness.run();
        p3_plan_with(
            &mut harness,
            "Particle Accelerator Unit\t4\nTritanium\t1,000\nNo Such Thing\t4\nRifter\t2",
        );
        click(&mut harness, "Pasted stock…");
        assert!(shown(&harness, "Left out, could not be read:"));
        assert!(shown(
            &harness,
            "Line 3: \"No Such Thing\t4\" (unknown item)"
        ));
        assert!(shown(&harness, "Pasted, but this plan does not use:"));
        assert!(shown(&harness, "Rifter 2"));
    }

    #[test]
    fn quantity_errors_keep_the_last_good_plan() {
        let env = make_env(&standard_facilities());
        let mut harness = start(&env);
        harness.run();
        hobgoblin_ii_plan(&mut harness);
        let last_good = cells(&harness, "Hobgoblin II");
        let mut shown_now = "10";
        for bad in ["0", "abc"] {
            type_into(&mut harness, shown_now, bad);
            shown_now = bad;
            assert!(shown(
                &harness,
                "Quantity must be a whole number of 1 or more"
            ));
            assert!(shown(&harness, "PLAN  Hobgoblin II × 10 (unsaved)"));
            assert_eq!(cells(&harness, "Hobgoblin II"), last_good);
        }
    }

    #[test]
    fn a_facility_with_the_wrong_rig_shows_the_plan_error() {
        let env = make_env(&[("Raitaru bad", facility(35825, &[37157]))]);
        let mut harness = start(&env);
        harness.run();
        hobgoblin_ii_plan(&mut harness);
        pick(
            &mut harness,
            "NPC station · high sec",
            "Raitaru bad · Raitaru · null sec · 1 rigs",
        );
        click(&mut harness, "Save");
        drop(harness);

        let mut store = Store::open(&env.data.join("user.sqlite"), NOW)
            .expect("the store opens")
            .store;
        let row = store.facilities().expect("facilities").remove(0);
        store
            .update_facility(row.id, &row.name, &facility(35825, &[46487]))
            .expect("facility saved");
        drop(store);

        let mut restarted = start(&env);
        restarted.run();
        restarted
            .get_all_by_label("New plan")
            .next()
            .expect("the saved plan's row")
            .click();
        restarted.run_steps(6);
        assert!(shown(
            &restarted,
            "Could not work out the plan: rig 46487 does not fit structure 35825"
        ));
    }

    #[test]
    fn reactions_tick_waits_for_a_reactions_facility() {
        let env = make_env(&standard_facilities());
        let mut harness = start(&env);
        harness.run();
        p3_plan(&mut harness);
        assert!(tick_disabled(&harness, "Crystalline Carbonide"));
        hover_tick(&mut harness, "Crystalline Carbonide");
        assert!(shown(&harness, "Pick a reactions facility first"));
        pick(
            &mut harness,
            "None",
            "Athanor null · Athanor · null sec · 1 rigs",
        );
        tick_row(&mut harness, "Crystalline Carbonide");
        assert_eq!(
            cells(&harness, "Crystalline Carbonide"),
            [
                "Crystalline Carbonide",
                "—",
                "166",
                "0",
                "166",
                "—",
                "1",
                "1",
                "9,834",
                "…"
            ]
        );
        assert_eq!(
            cells(&harness, "Helium Fuel Block"),
            ["Helium Fuel Block", "—", "5", "0", "—", "5"]
        );
        assert_eq!(
            cells(&harness, "Crystallite Alloy"),
            ["Crystallite Alloy", "—", "98", "0", "—", "98"]
        );
        assert_eq!(
            cells(&harness, "Carbon Polymers"),
            ["Carbon Polymers", "—", "98", "0", "—", "98"]
        );
    }

    #[test]
    fn saved_plan_comes_back_the_same_after_a_restart() {
        let env = make_env(&standard_facilities());
        let mut harness = start(&env);
        harness.run();
        p3_plan(&mut harness);
        click(&mut harness, "Rename");
        type_into(&mut harness, "New plan", "Hobgoblin 10");
        click(&mut harness, "Rename");
        click(&mut harness, "Save");
        assert!(!shown(&harness, "PLAN  Hobgoblin II × 10 (unsaved)"));
        click(&mut harness, "New");
        drop(harness);

        let mut restarted = start(&env);
        restarted.run();
        assert!(!shown(&restarted, "PLAN  Hobgoblin II × 10"));
        click(&mut restarted, "Hobgoblin 10");
        assert!(shown(&restarted, "PLAN  Hobgoblin II × 10"));
        assert_eq!(
            cells(&restarted, "Hobgoblin I"),
            ["Hobgoblin I", "10", "10", "0", "10", "—", "10", "1", "…"]
        );
        assert_eq!(
            cells(&restarted, "Particle Accelerator Unit"),
            [
                "Particle Accelerator Unit",
                "10",
                "10",
                "4",
                "6",
                "—",
                "6",
                "1",
                "…"
            ]
        );
        assert!(has_value(
            &restarted,
            "Raitaru null · Raitaru · null sec · 1 rigs"
        ));
        assert!(has_value(&restarted, "Forge sell, 5% price"));
        assert_eq!(
            stored_rule(&env, "Hobgoblin 10"),
            PriceRule::SellFivePercent
        );
        click(&mut restarted, "Pasted stock…");
        assert!(has_value(
            &restarted,
            "Particle Accelerator Unit\t4\nTritanium\t1,000"
        ));
        restarted.get_by_label("Close window").click();
        restarted.run();

        type_into(&mut restarted, "10", "11");
        assert!(shown(&restarted, "PLAN  Hobgoblin II × 11 (unsaved)"));
        click(&mut restarted, "New plan");
        assert!(shown(&restarted, "Save changes to Hobgoblin 10?"));
        click(&mut restarted, "Discard");
        click(&mut restarted, "Hobgoblin 10");
        assert!(shown(&restarted, "PLAN  Hobgoblin II × 10"));
        assert!(!shown(&restarted, "PLAN  Hobgoblin II × 11 (unsaved)"));
    }

    #[test]
    fn leaving_an_edited_plan_asks_before_it_goes() {
        let env = make_env(&standard_facilities());
        let mut harness = start(&env);
        harness.run();
        hobgoblin_ii_plan(&mut harness);
        click(&mut harness, "New");
        assert!(shown(&harness, "Save changes to New plan?"));
        click(&mut harness, "Cancel");
        assert!(shown(&harness, "PLAN  Hobgoblin II × 10 (unsaved)"));
    }

    #[test]
    fn the_plan_strip_at_1280_by_720_reads_across_and_matches_its_snapshot() {
        let env = make_env(&standard_facilities());
        let mut harness = start_sized(&env, vec2(1280.0, 720.0));
        harness.run();
        hobgoblin_ii_plan(&mut harness);

        let centre_y = |label: &str| harness.get_by_label(label).rect().center().y;
        let (product, quantity, manufacturing) = (
            centre_y("Product"),
            centre_y("Quantity"),
            centre_y("Manufacturing"),
        );
        assert!(
            (product - quantity).abs() <= 2.0 && (quantity - manufacturing).abs() <= 2.0,
            "Product, Quantity and Manufacturing share a line"
        );
        assert!(
            harness.get_by_label("Manufacturing").rect().min.x
                > harness.get_by_label("Quantity").rect().min.x,
            "Manufacturing is right of Quantity"
        );

        harness.remove_cursor();
        harness.run();
        harness.snapshot("build-1280x720");
    }

    #[test]
    fn a_fresh_build_says_why_each_panel_is_empty_and_new_plan_makes_one() {
        let env = make_env(&standard_facilities());
        let mut harness = start(&env);
        harness.run();

        assert!(shown(&harness, "No plans yet."));
        assert!(shown(
            &harness,
            "A plan starts with a product and a quantity, then gives you a buy list to paste into EVE."
        ));
        assert_eq!(harness.get_all_by_label("No plan on screen.").count(), 3);
        for what in [
            "Pick one in the Plans list, or start a new plan with a product and a quantity.",
            "The steps, and what to build or buy, show when you pick a plan.",
            "What to buy, and the list to paste into EVE, shows when you pick a plan.",
        ] {
            assert!(shown(&harness, what), "{what:?} shows");
        }
        assert!(!shown(&harness, "No plan selected."));
        assert_eq!(harness.get_all_by_label("New plan").count(), 4);
        // None of the panels' own chrome, tables or footers shows.
        for text in ["Columns", "Only these:", "Save", "Pasted stock…"] {
            assert!(!shown(&harness, text), "{text:?} is not drawn");
        }

        click_new_plan_in(&mut harness, "Plans");
        assert!(!shown(&harness, "No plans yet."));
        assert!(!shown(&harness, "No plan on screen."));
        assert_eq!(plan_count(&mut harness), 1);
        assert!(shown(&harness, "PLAN  no product yet × 1"));
    }

    #[test]
    fn a_saved_plan_with_none_selected_says_why_in_three_panels_and_any_new_plan_opens_one() {
        for panel in ["Plan", "Steps", "Buy list"] {
            let env = make_env(&standard_facilities());
            seed_plan(&env, "Old plan");
            let mut harness = start(&env);
            harness.run();

            assert!(!shown(&harness, "No plans yet."));
            assert_eq!(harness.get_all_by_label("No plan on screen.").count(), 3);
            assert!(shown(
                &harness,
                "Pick one in the Plans list, or start a new plan with a product and a quantity."
            ));
            assert!(shown(
                &harness,
                "The steps, and what to build or buy, show when you pick a plan."
            ));
            assert!(shown(
                &harness,
                "What to buy, and the list to paste into EVE, shows when you pick a plan."
            ));
            assert!(!shown(&harness, "No plan selected."));
            // The Plans panel keeps its list and toolbar.
            assert!(shown(&harness, "Old plan"));
            assert!(shown(&harness, "New"));

            click_new_plan_in(&mut harness, panel);
            assert!(
                !shown(&harness, "No plan on screen."),
                "a click in {panel} opens a plan"
            );
            assert_eq!(plan_count(&mut harness), 2);
            assert!(shown(&harness, "PLAN  no product yet × 1"));
        }
    }

    #[test]
    fn a_store_error_in_the_plans_panel_is_not_empty() {
        let env = make_env(&[]);
        let mut harness = start(&env);
        harness.run();
        Connection::open(env.data.join("user.sqlite"))
            .expect("the database opens")
            .execute_batch("PRAGMA foreign_keys = OFF; DROP TABLE plans;")
            .expect("the plans table drops");
        harness.run();

        assert!(!shown(&harness, "No plans yet."));
        assert!(
            harness
                .query_all_by_label_contains("Could not read the plans")
                .next()
                .is_some(),
            "the panel keeps its own error"
        );
    }

    // Record: the empty states re-record the Build snapshots build-new-plan (taken below),
    // pages-build and shell-build-empty (taken in pages.rs and shell.rs).
    #[test]
    fn the_snapshots_for_the_owner_are_taken() {
        let mut results = SnapshotResults::new();

        let env = make_env(&standard_facilities());
        let mut harness = start(&env);
        harness.run();
        click(&mut harness, "New plan");
        harness.remove_cursor();
        harness.run();
        harness.snapshot("build-new-plan");
        results.extend_harness(&mut harness);

        let env = make_env(&standard_facilities());
        let mut harness = start(&env);
        harness.run();
        p3_plan(&mut harness);
        harness.snapshot("build-p3");
        results.extend_harness(&mut harness);

        let env = make_env(&standard_facilities());
        let mut harness = start(&env);
        harness.run();
        hobgoblin_ii_plan(&mut harness);
        paste(
            &mut harness,
            "Particle Accelerator Unit\t4\nNo Such Thing\t4\nRifter\t2",
        );
        harness.snapshot("build-paste-problems");
        results.extend_harness(&mut harness);

        let env = make_env(&standard_facilities());
        let mut harness = start(&env);
        harness.run();
        hobgoblin_ii_plan(&mut harness);
        harness
            .get_by(|node| {
                node.value()
                    .is_some_and(|value| value == "Forge sell, 5% price")
            })
            .click();
        harness.run();
        harness.snapshot("build-price-rule-open");
        results.extend_harness(&mut harness);

        results.unwrap();
    }
}

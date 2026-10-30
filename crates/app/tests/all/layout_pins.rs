//! Acceptance tests for adding a panel to a layout from the app and for panels pinned to one plan
//! or stockpile. The game data is the build 3607705 fixture, the clock is fixed at 1,000,000, and
//! the window is 1280 by 800.
#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::Write;
    use std::ops::ControlFlow;
    use std::path::{Path, PathBuf};

    use egui::accesskit::Role;
    use egui::{Key, Modifiers, PointerButton, Pos2, vec2};
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;
    use eve_means_of_profit::session::NoEve;
    use eve_means_of_profit::{App, AppConfig, rail::Page};
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use mop_market::book::PriceRule;
    use mop_market::error::MarketError;
    use mop_market::fuzzwork::{BytesHttp, BytesResponse};
    use mop_sde::{Facility, Fetch, FetchError, Security, import};
    use mop_store::Store;
    use tempfile::TempDir;

    use crate::ui;

    const NOW: i64 = 1_000_000;
    const FIXTURE: &str = "../sde/tests/fixtures/sde-3607705";
    const HOBGOBLIN_II: u32 = 2456;
    const RIFTER: u32 = 587;
    /// A base layout the stockpile test adds to.
    const DAILY: &str = "EVE Means of Profit layout 1\nname: Daily industry\nBuy list\n";
    const PINNED_PLAN: &str = "split right 50%\n  Buy list\n  Steps [pinned plan 1]\n";

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

    /// A market file with no rows at all, dated the test clock, so the Buy list never reaches the
    /// network and no price notice reaches the status line.
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

    /// A data folder with the game data installed and the facilities the plans use.
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
        let mut store = store_of(&data);
        let mut settings = store.settings().expect("settings");
        settings.last_sde_check = Some(NOW);
        store.save_settings(&settings).expect("settings saved");
        store
            .add_facility(
                "Raitaru null",
                &Facility {
                    structure: Some(35825),
                    security: Security::Null,
                    rigs: vec![37157],
                },
            )
            .expect("facility saved");
        store
            .add_facility(
                "Athanor null",
                &Facility {
                    structure: Some(35835),
                    security: Security::Null,
                    rigs: vec![46487],
                },
            )
            .expect("facility saved");
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

    fn fixture() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE)
    }

    /// Saves a plan named `name` for `product`, quantity `quantity`, at the NPC station.
    fn seed_plan(env: &Env, name: &str, product: u32, quantity: u64) -> i64 {
        let mut store = store_of(&env.data);
        store
            .create_plan(
                &mop_store::PlanRecord {
                    id: 0,
                    name: name.to_owned(),
                    product,
                    quantity,
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
            .expect("the plan is saved")
    }

    /// Makes a stockpile with no targets or filters, and returns its id.
    fn seed_stockpile(env: &Env, name: &str) -> i64 {
        store_of(&env.data)
            .create_stockpile(name, None)
            .expect("the stockpile is made")
    }

    fn write_layouts(env: &Env, text: &str) {
        fs::write(env.data.join("layouts.txt"), text).expect("the layouts file is written");
    }

    fn layouts_text(env: &Env) -> String {
        fs::read_to_string(env.data.join("layouts.txt")).expect("the layouts file")
    }

    fn start(env: &Env) -> Harness<'static, App> {
        let config = AppConfig {
            data_dir: env.data.clone(),
            cache_dir: env.cache.clone(),
            fetch: Box::new(NoNetwork),
            clock: Box::new(|| NOW),
            eve: Box::new(NoEve),
            market: Box::new(EmptyForge),
        };
        let mut harness = Harness::builder()
            .with_size(vec2(1280.0, 800.0))
            .with_max_steps(crate::SETTLE_STEPS)
            .build_eframe(move |cc| App::new(&cc.egui_ctx, config).expect("the app starts"));
        ui::no_images(&mut harness);
        harness.state_mut().show_page(Page::Build);
        harness
    }

    fn click_at(harness: &mut Harness<'_, App>, pos: Pos2) {
        harness.event(egui::Event::PointerMoved(pos));
        for pressed in [true, false] {
            harness.event(egui::Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            });
        }
        harness.run_steps(3);
    }

    /// Clicks the `Layout` button on the header row of the panel titled `title`.
    fn click_layout(harness: &mut Harness<'_, App>, title: &str) {
        let title = harness.get_by_label(title).rect();
        // Panels side by side share a header row, so the button is the one nearest the title's right
        // edge on that row.
        let at = harness
            .get_all_by_label("Layout")
            .map(|node| node.rect())
            .filter(|rect| (rect.center().y - title.center().y).abs() < 6.0)
            .min_by(|a, b| {
                (a.left() - title.right())
                    .abs()
                    .total_cmp(&(b.left() - title.right()).abs())
            })
            .map(|rect| rect.center())
            .expect("the panel's Layout button");
        click_at(harness, at);
    }

    /// Clicks the `Add to layout…` item of the open Layout menu.
    fn click_add_to_layout(harness: &mut Harness<'_, App>, title: &str) {
        click_layout(harness, title);
        ui::click(harness, "Add to layout…");
    }

    /// Clicks the radio of the existing layout `name` in the `Add to layout…` window.
    fn click_existing_layout(harness: &mut Harness<'_, App>, name: &str) {
        let new = harness.get_by_label("New layout").rect();
        let at = harness
            .get_all_by_label(name)
            .map(|node| node.rect())
            .find(|rect| (rect.left() - new.left()).abs() < 6.0)
            .map(|rect| rect.center())
            .expect("the layout's radio");
        click_at(harness, at);
    }

    /// Replaces the name in the `Add to layout…` window's Name field.
    fn type_name(harness: &mut Harness<'_, App>, text: &str) {
        let label = harness.get_by_label("Name").rect();
        let field = harness
            .get_all_by(|node| node.role() == Role::TextInput)
            .find(|node| (node.rect().center().y - label.center().y).abs() < 8.0)
            .expect("the name field");
        field.click();
        harness.run_steps(1);
        harness.key_press_modifiers(Modifiers::COMMAND, Key::A);
        harness.run_steps(1);
        harness.event(egui::Event::Text(text.to_owned()));
        harness.run_steps(2);
    }

    /// The layout part of a one-layout file: everything after its first two lines.
    fn layout_part(text: &str) -> String {
        text.splitn(3, '\n').nth(2).unwrap_or_default().to_owned()
    }

    #[test]
    fn add_to_layout_puts_the_panel_in_a_new_layout_and_keeps_the_build_page() {
        let env = make_env();
        seed_plan(&env, "Hobgoblin 10", HOBGOBLIN_II, 10);
        seed_plan(&env, "Rifter 31", RIFTER, 31);
        let mut harness = start(&env);
        harness.run();

        ui::click(&mut harness, "Hobgoblin 10");
        click_add_to_layout(&mut harness, "Buy list");
        assert!(
            ui::shown(&harness, "Add Buy list to a layout"),
            "the window is open for the Buy list"
        );
        type_name(&mut harness, "Daily industry");
        ui::snapshot(&mut harness, "layout-add-to");
        ui::click(&mut harness, "Add");

        assert_eq!(
            layouts_text(&env),
            "EVE Means of Profit layout 1\nname: Daily industry\nBuy list\n",
            "the new layout is written with the Buy list"
        );
        let pages_bottom = harness.get_by_label("Settings").rect().bottom();
        let daily = harness.get_by_label("Daily industry").rect();
        assert!(
            daily.top() > pages_bottom,
            "the layout is listed below the pages"
        );
        assert!(
            ui::shown(&harness, "Added Buy list to \"Daily industry\"."),
            "the notice says what was added"
        );
        assert!(ui::shown(&harness, "Plans"), "the Build page stays on show");
    }

    #[test]
    fn a_pinned_plan_keeps_its_plan_while_the_build_page_moves_on() {
        let env = make_env();
        write_layouts(&env, DAILY);
        let hobgoblin = seed_plan(&env, "Hobgoblin 10", HOBGOBLIN_II, 10);
        seed_plan(&env, "Rifter 31", RIFTER, 31);
        let mut harness = start(&env);
        harness.run();

        ui::click(&mut harness, "Hobgoblin 10");
        click_add_to_layout(&mut harness, "Steps");
        click_existing_layout(&mut harness, "Daily industry");
        ui::click(&mut harness, "Pin to \"Hobgoblin 10\"");
        ui::click(&mut harness, "Add");

        assert_eq!(
            layout_part(&layouts_text(&env)),
            PINNED_PLAN,
            "the Steps is pinned to the selected plan"
        );

        ui::click(&mut harness, "Daily industry");
        assert!(
            ui::shown(&harness, "pinned: Hobgoblin 10"),
            "the Steps header names its plan"
        );
        assert!(
            ui::shown(&harness, "follows plan"),
            "the Buy list follows the selection"
        );
        assert!(
            ui::shown(&harness, "Hobgoblin II"),
            "the pinned Steps shows the Hobgoblin chain"
        );
        assert!(
            !ui::shown(&harness, "Mexallon"),
            "the Hobgoblin chain needs no Mexallon"
        );

        ui::click(&mut harness, "Build");
        ui::click(&mut harness, "Rifter 31");
        ui::click(&mut harness, "Daily industry");
        assert!(
            ui::shown(&harness, "Mexallon"),
            "the layout's Buy list moved to the Rifter's lines"
        );
        assert!(
            ui::shown(&harness, "Hobgoblin II"),
            "the pinned Steps still shows the Hobgoblin chain"
        );

        store_of(&env.data)
            .delete_plan(hobgoblin)
            .expect("the plan is deleted");
        harness.run_steps(3);
        assert!(
            ui::shown(
                &harness,
                "The plan this panel was pinned to no longer exists."
            ),
            "a pin to a deleted plan says so"
        );

        ui::click(&mut harness, "Follow the selected plan");
        assert_eq!(
            layout_part(&layouts_text(&env)),
            "split right 50%\n  Buy list\n  Steps\n",
            "following writes the plain Steps line"
        );
    }

    #[test]
    fn a_pinned_stockpile_keeps_its_stockpile_while_the_stockpiles_page_moves_on() {
        let env = make_env();
        write_layouts(&env, DAILY);
        // Two stockpiles take ids 1 and 2, so Kit is id 3 as the layout text names it.
        seed_stockpile(&env, "Spare");
        seed_stockpile(&env, "Parts");
        let kit = seed_stockpile(&env, "Kit");
        assert_eq!(kit, 3, "Kit has the id the file names");
        seed_stockpile(&env, "Weekly");
        let mut harness = start(&env);
        harness.state_mut().show_page(Page::Stockpiles);
        harness.run();

        ui::click(&mut harness, "Kit");
        click_add_to_layout(&mut harness, "Contents");
        click_existing_layout(&mut harness, "Daily industry");
        ui::click(&mut harness, "Pin to \"Kit\"");
        ui::click(&mut harness, "Add");
        assert!(
            layouts_text(&env).contains("  Contents [pinned stockpile 3]\n"),
            "the Contents is pinned to Kit"
        );

        ui::click(&mut harness, "Daily industry");
        assert!(
            ui::shown(&harness, "CONTENTS  Kit"),
            "the pinned Contents shows Kit"
        );

        ui::click(&mut harness, "Stockpiles");
        ui::click(&mut harness, "Weekly");
        ui::click(&mut harness, "Daily industry");
        assert!(
            ui::shown(&harness, "CONTENTS  Kit"),
            "the Stockpiles page moved on but the pin did not"
        );

        store_of(&env.data)
            .delete_stockpile(kit)
            .expect("the stockpile is deleted");
        harness.run_steps(3);
        assert!(
            ui::shown(
                &harness,
                "The stockpile this panel was pinned to no longer exists."
            ),
            "a pin to a deleted stockpile says so"
        );
    }
}

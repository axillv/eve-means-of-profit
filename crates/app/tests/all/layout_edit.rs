//! Acceptance tests for the player's layouts in the app: the rail lists them below the pages, a
//! click shows one with every panel following the selected plan, a tab closed is written back to
//! `layouts.txt`, and a file that cannot be read is set aside. The game data is the build 3607705
//! fixture, the clock is fixed at 1,000,000, and the window is 1280 by 800.
#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::Write;
    use std::ops::ControlFlow;
    use std::path::{Path, PathBuf};

    use egui::{Modifiers, PointerButton, Pos2, Rect, pos2, vec2};
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;
    use eve_means_of_profit::session::NoEve;
    use eve_means_of_profit::{App, AppConfig};
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

    /// The file the tests start with: two layouts, one split in two panels, one empty.
    const DAILY: &str = "EVE Means of Profit layout 1\nname: Daily industry\nsplit right 50%\n  Buy list\n  Steps\n\nEVE Means of Profit layout 1\nname: Ideas\nempty\n";
    /// The same file after the Buy list tab is closed in `Daily industry`.
    const DAILY_WITHOUT_BUY_LIST: &str = "EVE Means of Profit layout 1\nname: Daily industry\nSteps\n\nEVE Means of Profit layout 1\nname: Ideas\nempty\n";
    /// What a user layout with no panel at all says.
    const EMPTY_TEXT: &str =
        "This layout is empty. Add panels with \"Add to layout…\" in any panel's Layout menu.";

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
        let mut store = Store::open(&data.join("user.sqlite"), NOW)
            .expect("the store opens")
            .store;
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

    fn fixture() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE)
    }

    /// Turns on the saved icons-only rail.
    fn set_rail_icons_only(env: &Env) {
        let mut store = Store::open(&env.data.join("user.sqlite"), NOW)
            .expect("the store opens")
            .store;
        let mut settings = store.settings().expect("settings");
        settings.rail_icons_only = true;
        store.save_settings(&settings).expect("settings saved");
    }

    /// Saves a plan named `name` for `product`, quantity `quantity`, at the NPC station.
    fn seed_plan(env: &Env, name: &str, product: u32, quantity: u64) {
        let mut store = Store::open(&env.data.join("user.sqlite"), NOW)
            .expect("the store opens")
            .store;
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
            .expect("the plan is saved");
    }

    fn write_layouts(env: &Env, text: &str) {
        fs::write(env.data.join("layouts.txt"), text).expect("the layouts file is written");
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
        harness
            .state_mut()
            .show_page(eve_means_of_profit::rail::Page::Build);
        harness
    }

    /// The rect of the topmost painted text equal to `text`. Tab titles are painted, not widgets.
    fn text_rect(harness: &Harness<'_, App>, text: &str) -> Rect {
        fn collect(shape: &egui::Shape, text: &str, found: &mut Vec<Rect>) {
            match shape {
                egui::Shape::Text(painted) if painted.galley.text() == text => {
                    found.push(Rect::from_min_size(painted.pos, painted.galley.size()));
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        collect(shape, text, found);
                    }
                }
                egui::Shape::Noop
                | egui::Shape::Circle(_)
                | egui::Shape::Ellipse(_)
                | egui::Shape::LineSegment { .. }
                | egui::Shape::Path(_)
                | egui::Shape::Rect(_)
                | egui::Shape::Text(_)
                | egui::Shape::Mesh(_)
                | egui::Shape::QuadraticBezier(_)
                | egui::Shape::CubicBezier(_)
                | egui::Shape::Callback(_) => {}
            }
        }
        let mut found = Vec::new();
        for clipped in &harness.output().shapes {
            collect(&clipped.shape, text, &mut found);
        }
        found
            .into_iter()
            .min_by(|a, b| a.top().total_cmp(&b.top()))
            .unwrap_or_else(|| panic!("\"{text}\" is not painted"))
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

    /// Clicks the close button at the right of the tab titled `name`. The close button is 24 points
    /// wide with 8 points of padding before the title, so its centre sits 20 points right of the
    /// painted title.
    fn close_tab(harness: &mut Harness<'_, App>, name: &str) {
        let title = text_rect(harness, name);
        click_at(harness, pos2(title.right() + 20.0, title.center().y));
    }

    #[test]
    fn the_rail_lists_the_layouts_below_the_pages_and_a_click_shows_one() {
        let env = make_env();
        write_layouts(&env, DAILY);
        let mut harness = start(&env);
        harness.run();

        let pages_bottom = harness.get_by_label("Settings").rect().bottom();
        let daily = harness.get_by_label("Daily industry").rect();
        let ideas = harness.get_by_label("Ideas").rect();
        assert!(
            daily.top() > pages_bottom,
            "the layouts come below the pages"
        );
        assert!(
            ideas.top() >= daily.bottom(),
            "the layouts keep their file order"
        );

        ui::click(&mut harness, "Daily industry");
        assert!(ui::shown(&harness, "Buy list"), "the layout's Buy list");
        assert!(ui::shown(&harness, "Steps"), "the layout's Steps");
        assert_eq!(
            harness.get_all_by_label("follows plan").count(),
            2,
            "both panels follow the selection"
        );
        assert!(
            !ui::shown(&harness, "Plans"),
            "the layout has no Plans panel"
        );

        ui::click(&mut harness, "Build");
        assert!(
            ui::shown(&harness, "Plans"),
            "Build shows its own page again"
        );
        let follows = harness.query_all_by_label("follows plan").count();
        assert_eq!(follows, 0, "only a user layout writes its own headers");
    }

    #[test]
    fn the_layouts_stay_in_the_rail_with_the_icons_only_setting() {
        let env = make_env();
        write_layouts(&env, DAILY);
        set_rail_icons_only(&env);
        let mut harness = start(&env);
        harness.run();

        assert!(
            ui::shown(&harness, "Daily industry"),
            "the icons-only rail still lists the layouts"
        );
        assert!(
            harness.get_by_label("Build").rect().width() <= 48.0,
            "the rail keeps its narrow look with layouts present"
        );
        assert!(
            harness.get_by_label("Daily industry").rect().width() <= 48.0,
            "a layout entry is an icon alone"
        );
        assert!(
            ui::hover(&mut harness, "Daily industry")
                .iter()
                .any(|tip| tip == "Daily industry"),
            "the layout's name shows on hover"
        );
        ui::click(&mut harness, "Daily industry");
        assert!(ui::shown(&harness, "Buy list"), "the layout's Buy list");
        assert_eq!(
            harness.query_all_by_label("follows plan").count(),
            2,
            "every panel of the layout on show is drawn"
        );
    }

    #[test]
    fn every_panel_follows_the_plan_selected_on_the_build_page() {
        let env = make_env();
        write_layouts(&env, DAILY);
        seed_plan(&env, "Hobgoblin 10", HOBGOBLIN_II, 10);
        seed_plan(&env, "Rifter 31", RIFTER, 31);
        let mut harness = start(&env);
        harness.run();

        ui::click(&mut harness, "Hobgoblin 10");
        ui::click(&mut harness, "Daily industry");
        assert!(
            ui::shown(&harness, "Hobgoblin II"),
            "the layout's Steps shows the selected plan's product"
        );
        assert!(!ui::shown(&harness, "Rifter"));

        ui::click(&mut harness, "Build");
        ui::click(&mut harness, "Rifter 31");
        ui::click(&mut harness, "Daily industry");
        assert!(
            ui::shown(&harness, "Rifter"),
            "the Steps now shows the second plan's product"
        );
    }

    #[test]
    fn closing_a_tab_in_a_layout_writes_the_file_back_and_an_empty_layout_says_so() {
        let env = make_env();
        write_layouts(&env, DAILY);
        let mut harness = start(&env);
        harness.run();

        ui::click(&mut harness, "Daily industry");
        harness.run();
        close_tab(&mut harness, "Buy list");
        let path = env.data.join("layouts.txt");
        assert_eq!(
            fs::read_to_string(&path).expect("the layouts file"),
            DAILY_WITHOUT_BUY_LIST,
            "closing a tab is written back at once"
        );

        ui::click(&mut harness, "Ideas");
        assert!(ui::shown(&harness, EMPTY_TEXT));
    }

    #[test]
    fn an_unreadable_layouts_file_is_set_aside_and_reported() {
        let env = make_env();
        write_layouts(&env, "nonsense");
        let mut harness = start(&env);
        harness.run();

        assert!(!ui::shown(&harness, "Daily industry"));
        let path = env.data.join("layouts.txt");
        assert!(!path.exists(), "the bad file is moved aside");
        assert!(
            env.data.join("layouts.txt.unreadable-1000000").exists(),
            "the bad file is kept under the new name"
        );

        let full = "Your saved layouts could not be read (No layout found: the text has no line \"EVE Means of Profit layout 1\".). The file was kept as layouts.txt.unreadable-1000000.";
        let notice = harness
            .state_mut()
            .shared_mut()
            .notices
            .iter()
            .find(|notice| {
                notice
                    .text
                    .starts_with("Your saved layouts could not be read")
            })
            .expect("the notice is kept");
        assert_eq!(notice.text, full);

        // The status line cuts long notices at 120 characters and shows the whole text on hover.
        let cut: String = full.chars().take(120).chain(std::iter::once('…')).collect();
        assert!(
            ui::shown(&harness, &cut),
            "the status line shows the notice"
        );
        assert!(
            ui::hover(&mut harness, &cut).iter().any(|tip| tip == full),
            "the whole notice shows on hover"
        );
    }

    #[test]
    fn the_screenshots_of_the_layouts_are_taken() {
        let env = make_env();
        write_layouts(&env, DAILY);
        let mut harness = start(&env);
        harness.run();

        ui::click(&mut harness, "Daily industry");
        ui::snapshot(&mut harness, "layout-user");

        ui::click(&mut harness, "Ideas");
        ui::snapshot(&mut harness, "layout-empty");
    }
}

//! Acceptance tests for the Buy list panel: the rows and totals, the freight services, thin and
//! missing prices, old and rate-limited prices, the multibuy copy, and the screenshots. The game
//! data is the build 3607705 fixture. Prices come from a fake market, and the clock is fixed at
//! 2,000,000.
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

    use egui::accesskit::Role;
    use egui::{Event, Key, Modifiers, OutputCommand, vec2};
    use egui_kittest::kittest::{NodeT, Queryable, by};
    use egui_kittest::{Harness, SnapshotResults};
    use eve_means_of_profit::build::prices::Prices;
    use eve_means_of_profit::{App, AppConfig};
    use profit_core::freight::RateCard;
    use profit_core::money::{Money, Rate};
    use profit_core::volume::Volume;
    use profit_market::cache::{Http, HttpResponse};
    use profit_market::error::MarketError;
    use profit_sde::{Fetch, FetchError, RouteKind, import};
    use profit_store::Store;
    use tempfile::TempDir;

    const NOW: i64 = 2_000_000;
    const FIXTURE: &str = "../sde/tests/fixtures/sde-3607705";
    const JITA_ID: u64 = 60_003_760;
    const JITA_SYSTEM: u32 = 30_000_142;
    const KIAINTI_SYSTEM: u32 = 30_000_189;

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

    /// Locks the fake market's state. A lock poisoned by a failed frame is still read.
    fn lock(state: &Mutex<Jita>) -> MutexGuard<'_, Jita> {
        state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// What the fake market serves. `ago` is how long before the clock the books last changed.
    struct Jita {
        /// Sell orders by type: each price text and the volume on the market. No entry, or an
        /// empty list, is a book with no sell orders.
        sells: BTreeMap<u64, Vec<(&'static str, u64)>>,
        ago: i64,
        rate_limit: bool,
        hold: bool,
    }

    impl Jita {
        fn full() -> Self {
            let mut sells = BTreeMap::new();
            sells.insert(2454, vec![("27500.00", 1_000_000)]);
            sells.insert(9834, vec![("10450.00", 1_000_000)]);
            sells.insert(9848, vec![("39990.00", 1_000_000)]);
            sells.insert(11399, vec![("2150.00", 1_000_000)]);
            sells.insert(11481, vec![("478.60", 1_000_000)]);
            sells.insert(11688, vec![("30900.00", 1_000_000)]);
            Self {
                sells,
                ago: 240,
                rate_limit: false,
                hold: false,
            }
        }
    }

    /// The fake market: the books in [`Jita`], read through the market's own cache.
    struct FakeHttp {
        state: Arc<Mutex<Jita>>,
    }

    impl Http for FakeHttp {
        fn get(
            &self,
            url: &str,
            _if_none_match: Option<&str>,
        ) -> Result<HttpResponse, MarketError> {
            loop {
                let state = lock(&self.state);
                if !state.hold {
                    break;
                }
                drop(state);
                thread::sleep(Duration::from_millis(5));
            }
            let state = lock(&self.state);
            if state.rate_limit {
                return Ok(HttpResponse {
                    status: 420,
                    retry_after: Some(30),
                    ..HttpResponse::default()
                });
            }
            let type_id: u64 = url
                .split("type_id=")
                .nth(1)
                .and_then(|rest| rest.split('&').next())
                .and_then(|text| text.parse().ok())
                .ok_or_else(|| MarketError::Json(url.to_owned()))?;
            let orders: Vec<String> = state
                .sells
                .get(&type_id)
                .into_iter()
                .flatten()
                .enumerate()
                .map(|(index, (price, volume))| {
                    format!(
                        "{{\"order_id\":{},\"type_id\":{type_id},\"location_id\":{JITA_ID},\"is_buy_order\":false,\"price\":{price},\"volume_remain\":{volume}}}",
                        type_id * 10 + index as u64
                    )
                })
                .collect();
            let changed = http_date(NOW - state.ago);
            Ok(HttpResponse {
                status: 200,
                etag: Some(format!("\"{type_id}\"")),
                last_modified: Some(changed),
                expires: Some(http_date(NOW)),
                pages: None,
                retry_after: None,
                body: format!("[{}]", orders.join(",")),
            })
        }
    }

    /// An HTTP date (`Sun, 06 Nov 1994 08:49:37 GMT`) for Unix seconds.
    fn http_date(secs: i64) -> String {
        let days = secs.div_euclid(86_400);
        let within = secs.rem_euclid(86_400);
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = doy - (153 * mp + 2) / 5 + 1;
        let month = if mp < 10 { mp + 3 } else { mp - 9 };
        let year = yoe + era * 400 + i64::from(month <= 2);
        let weekday = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"]
            [usize::try_from(days.rem_euclid(7)).unwrap_or(0)];
        let months = [
            "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
        ];
        format!(
            "{weekday}, {day:02} {} {year} {:02}:{:02}:{:02} GMT",
            months[usize::try_from(month - 1).unwrap_or(0)],
            within / 3_600,
            within % 3_600 / 60,
            within % 60
        )
    }

    /// A data folder with the game data installed, the route Jita → Kiainti and the given services.
    struct Env {
        _dir: TempDir,
        data: PathBuf,
        cache: PathBuf,
    }

    fn per_m3(isk: i64) -> Money {
        Money::from_ten_thousandths(isk * 10_000)
    }

    /// Alliance (per m³, 0.5 %) and Haulers (per jump, 1 %, high sec only), as the plans expect.
    fn standard_services() -> Vec<(&'static str, RateCard, RouteKind, Option<u32>)> {
        vec![
            (
                "Alliance",
                RateCard {
                    per_m3: Some(per_m3(850)),
                    collateral: Some(Rate::from_ppm(5_000)),
                    ..RateCard::default()
                },
                RouteKind::Shortest,
                None,
            ),
            (
                "Haulers",
                RateCard {
                    per_jump: Some(per_m3(250_000)),
                    collateral: Some(Rate::from_ppm(10_000)),
                    ..RateCard::default()
                },
                RouteKind::HighSecOnly,
                None,
            ),
        ]
    }

    fn make_env(services: &[(&'static str, RateCard, RouteKind, Option<u32>)]) -> Env {
        let dir = TempDir::new().expect("a temp folder");
        let data = dir.path().join("data");
        let cache = dir.path().join("cache");
        fs::create_dir_all(&data).expect("the data folder");
        import(&fixture(), &data.join("sde.sqlite")).expect("the fixture imports");
        let mut store = Store::open(&data.join("user.sqlite"), NOW).expect("the store opens");
        let mut settings = store.settings().expect("settings");
        settings.last_sde_check = Some(NOW);
        store.save_settings(&settings).expect("settings saved");
        let route = store
            .add_route(JITA_SYSTEM, KIAINTI_SYSTEM)
            .expect("the route is saved");
        for (name, card, kind, jumps) in services {
            store
                .add_service(route, name, card, *kind, *jumps)
                .expect("the service is saved");
        }
        Env {
            _dir: dir,
            data,
            cache,
        }
    }

    /// Starts the app with the fake market in place of the real one.
    fn start(env: &Env, jita: &Arc<Mutex<Jita>>) -> Harness<'static, App> {
        let config = AppConfig {
            data_dir: env.data.clone(),
            cache_dir: env.cache.clone(),
            fetch: Box::new(NoNetwork),
            clock: Box::new(|| NOW),
        };
        let market = Arc::new(Prices::new(Box::new(FakeHttp {
            state: Arc::clone(jita),
        })));
        let mut harness = Harness::builder()
            .with_size(vec2(1280.0, 800.0))
            .build_eframe(move |cc| App::new(&cc.egui_ctx, config).expect("the app starts"));
        harness.state_mut().shared_mut().market = market;
        harness
    }

    fn shown(harness: &Harness<'_, App>, text: &str) -> bool {
        harness.query_all_by_label(text).next().is_some()
    }

    fn has_value(harness: &Harness<'_, App>, text: &str) -> bool {
        harness
            .query_all(by().predicate(|node| node.value().is_some_and(|value| value == text)))
            .next()
            .is_some()
    }

    /// Clicks the button labelled `label`; when several are, the last one drawn.
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

    /// Runs frames until a label or a dropdown's chosen text shows, since the prices arrive from the
    /// worker thread.
    fn settle(harness: &mut Harness<'_, App>, label: &str) {
        for _ in 0..500 {
            harness.run();
            if shown(harness, label) || has_value(harness, label) {
                return;
            }
            thread::sleep(Duration::from_millis(5));
        }
        panic!("{label:?} never shown");
    }

    /// The cell texts of the Buy list row labelled `row`, left to right, empty cells left out. The
    /// Steps panel has a row of the same name; the Buy list's row is the one with an age cell.
    fn cells(harness: &Harness<'_, App>, row: &str) -> Vec<String> {
        // The Buy list is the panel to the right of the Steps panel, so its label is the rightmost.
        let anchor = harness
            .get_all_by_label(row)
            .map(|node| (node.rect().min.x, node.rect().center().y))
            .max_by(|a, b| a.0.total_cmp(&b.0))
            .expect("the row");
        let mut found: Vec<(f32, String)> = harness
            .query_all(by().predicate(|node| node.bounding_box().is_some()))
            .filter(|node| (node.rect().center().y - anchor.1).abs() < 4.0)
            .filter(|node| node.rect().min.x >= anchor.0 - 1.0)
            .filter_map(|node| node.value().map(|value| (node.rect().min.x, value)))
            .filter(|(_, value)| !value.is_empty())
            .collect();
        found.sort_by(|a, b| a.0.total_cmp(&b.0));
        found.into_iter().map(|(_, value)| value).collect()
    }

    /// Checks that the Buy list row labelled `row` shows each of the `expected` cell texts.
    fn assert_row(harness: &Harness<'_, App>, row: &str, expected: &[&str]) {
        let got = cells(harness, row);
        for text in expected {
            assert!(
                got.iter().any(|cell| cell == text),
                "{row}: no cell {text:?} in {got:?}"
            );
        }
    }

    /// Points at a label and runs enough frames for its tooltip to appear.
    fn hover(harness: &mut Harness<'_, App>, label: &str) {
        harness.get_by_label(label).hover();
        for _ in 0..60 {
            harness.run();
        }
    }

    /// Makes a plan for Hobgoblin II, quantity 10, freight on Jita → Kiainti, and opens its Buy list.
    fn buy_list_for(harness: &mut Harness<'_, App>) {
        click(harness, "New");
        type_into(harness, "", "hobgoblin");
        click(harness, "Hobgoblin II");
        type_into(harness, "1", "10");
        // The Buy list's own freight line also reads "No freight", so take the dropdown.
        harness
            .query_all(
                by().predicate(|node| node.value().is_some_and(|value| value == "No freight")),
            )
            .find(|node| node.accesskit_node().role() == Role::ComboBox)
            .expect("the freight dropdown")
            .click();
        harness.run();
        click(harness, "Jita → Kiainti");
    }

    /// The Buy list rows of the complete plan, once the prices are in.
    fn complete_list(
        jita: &Arc<Mutex<Jita>>,
        services: &[(&'static str, RateCard, RouteKind, Option<u32>)],
    ) -> Harness<'static, App> {
        let env = make_env(services);
        let mut harness = start(&env, jita);
        harness.run();
        buy_list_for(&mut harness);
        settle(&mut harness, "Oldest: 4m");
        harness
    }

    #[test]
    fn the_buy_list_rows_and_the_footer_are_complete() {
        let jita = Arc::new(Mutex::new(Jita::full()));
        let harness = complete_list(&jita, &standard_services());
        assert_row(
            &harness,
            "Hobgoblin I",
            &[
                "Hobgoblin I",
                "10",
                "10",
                "50.00",
                "27,500.00",
                "4m",
                "275,000.00",
            ],
        );
        assert_row(
            &harness,
            "Guidance Systems",
            &[
                "Guidance Systems",
                "10",
                "30.00",
                "10,450.00",
                "4m",
                "104,500.00",
            ],
        );
        assert_row(
            &harness,
            "Robotics",
            &["Robotics", "10", "30.00", "39,990.00", "4m", "399,900.00"],
        );
        assert_row(
            &harness,
            "Morphite",
            &["Morphite", "10", "0.10", "2,150.00", "4m", "21,500.00"],
        );
        assert_row(
            &harness,
            "R.A.M.- Robotics",
            &["R.A.M.- Robotics", "10", "0.40", "478.60", "4m", "4,786.00"],
        );
        assert_row(
            &harness,
            "Particle Accelerator Unit",
            &[
                "Particle Accelerator Unit",
                "10",
                "10.00",
                "30,900.00",
                "4m",
                "309,000.00",
            ],
        );
        assert!(shown(&harness, "m³ 120.50 · value 1,114,686.00 · freight "));
        assert!(shown(
            &harness,
            " · collateral 1,114,686.00 · delivered 1,222,684.43"
        ));
        assert!(!shown(&harness, " incomplete"));
        assert!(shown(&harness, "Oldest: 4m"));
        assert!(has_value(&harness, "Cheapest (Alliance)"));
        assert!(shown(&harness, "1 contract"));
    }

    #[test]
    fn the_freight_hover_lists_every_service() {
        let jita = Arc::new(Mutex::new(Jita::full()));
        let mut harness = complete_list(&jita, &standard_services());
        hover(&mut harness, "107,998.43");
        assert!(shown(&harness, "Alliance 107,998.43"));
        assert!(shown(&harness, "Haulers 3,011,146.86"));
    }

    #[test]
    fn choosing_haulers_changes_the_freight_and_saves_the_service() {
        let jita = Arc::new(Mutex::new(Jita::full()));
        let mut harness = complete_list(&jita, &standard_services());
        pick(&mut harness, "Cheapest (Alliance)", "Haulers");
        settle(&mut harness, "3,011,146.86");
        assert!(shown(
            &harness,
            " · collateral 1,114,686.00 · delivered 4,125,832.86"
        ));
        assert!(has_value(&harness, "Haulers"));
    }

    #[test]
    fn a_thin_book_and_an_empty_book_are_marked_and_left_out_of_the_value() {
        let jita = Arc::new(Mutex::new(Jita::full()));
        {
            let mut state = lock(&jita);
            state.sells.insert(11688, vec![("30900.00", 4)]);
            state.sells.insert(9848, Vec::new());
        }
        let env = make_env(&standard_services());
        let mut harness = start(&env, &jita);
        harness.run();
        buy_list_for(&mut harness);
        settle(&mut harness, "only 4 on market");
        assert!(shown(&harness, " incomplete"));
        assert!(shown(&harness, "m³ 120.50 · value 405,786.00 · freight "));
    }

    #[test]
    fn old_prices_show_their_age_and_the_status_line_names_the_oldest() {
        let jita = Arc::new(Mutex::new(Jita::full()));
        lock(&jita).ago = 700;
        let env = make_env(&standard_services());
        let mut harness = start(&env, &jita);
        harness.run();
        buy_list_for(&mut harness);
        settle(&mut harness, "Oldest: 11m");
        assert!(shown(&harness, "Market data: oldest 11m"));
    }

    #[test]
    fn a_rate_limit_keeps_the_cached_prices_and_names_the_wait() {
        let jita = Arc::new(Mutex::new(Jita::full()));
        let mut harness = complete_list(&jita, &standard_services());
        lock(&jita).rate_limit = true;
        click(&mut harness, "Refresh prices");
        harness.run();
        settle(&mut harness, "ERROR");
        assert!(shown(
            &harness,
            "Jita prices paused for 30 s (ESI rate limit)."
        ));
        assert!(shown(&harness, "27,500.00"));
    }

    #[test]
    fn no_freight_is_an_empty_quote() {
        let jita = Arc::new(Mutex::new(Jita::full()));
        let mut harness = complete_list(&jita, &standard_services());
        pick(&mut harness, "Jita → Kiainti", "No freight");
        settle(&mut harness, "No freight");
        assert!(shown(&harness, "m³ 120.50 · value 1,114,686.00 · freight "));
        assert!(shown(
            &harness,
            " · collateral 0.00 · delivered 1,114,686.00"
        ));
        assert!(!shown(&harness, " incomplete"));
    }

    #[test]
    fn a_service_with_a_minimum_charge_packs_two_contracts() {
        let jita = Arc::new(Mutex::new(Jita::full()));
        let bulk = (
            "Bulk",
            RateCard {
                per_m3: Some(per_m3(1_000)),
                minimum: Some(per_m3(10_000_000)),
                max_m3: Some(Volume::from_ten_thousandths(100 * 10_000)),
                ..RateCard::default()
            },
            RouteKind::Shortest,
            None,
        );
        let mut harness = complete_list(&jita, std::slice::from_ref(&bulk));
        settle(&mut harness, "2 contracts");
        assert!(shown(&harness, "20,000,000.00"));
        click(&mut harness, "Top up the last contract");
        settle(&mut harness, "20,000,000.00");
    }

    #[test]
    fn copy_multibuy_puts_the_lines_on_the_clipboard() {
        let jita = Arc::new(Mutex::new(Jita::full()));
        let mut harness = complete_list(&jita, &standard_services());
        harness
            .get_all_by_label("Copy multibuy")
            .last()
            .expect("the copy button")
            .click();
        // The copy is made in the frame of the click, and `run` would leave only the last frame's output.
        harness.step();
        let copied = harness
            .output()
            .platform_output
            .commands
            .iter()
            .find_map(|command| {
                if let OutputCommand::CopyText(text) = command {
                    Some(text.clone())
                } else {
                    None
                }
            });
        assert_eq!(
            copied.as_deref(),
            Some(
                "Hobgoblin I\t10\nGuidance Systems\t10\nRobotics\t10\nMorphite\t10\nR.A.M.- Robotics\t10\nParticle Accelerator Unit\t10"
            )
        );
        // The notice shows in the frame after the click: the status line is drawn before the page.
        harness.run();
        assert!(shown(&harness, "Copied 6 lines for multibuy."));
    }

    #[test]
    fn a_request_that_blocks_does_not_stop_the_frames() {
        let jita = Arc::new(Mutex::new(Jita::full()));
        lock(&jita).hold = true;
        let env = make_env(&standard_services());
        let mut harness = start(&env, &jita);
        harness.run();
        buy_list_for(&mut harness);
        for _ in 0..20 {
            harness.run();
        }
        assert!(shown(&harness, "Morphite"));
        assert!(shown(&harness, "…"));
        lock(&jita).hold = false;
        settle(&mut harness, "27,500.00");
    }

    /// Pastes `text` into the pasted stock box.
    fn paste(harness: &mut Harness<'_, App>, text: &str) {
        harness
            .get_by(|node| node.role() == Role::MultilineTextInput)
            .click();
        harness.event(Event::Paste(text.to_owned()));
        harness.run();
    }

    #[test]
    fn haulers_is_saved_and_falls_back_to_the_cheapest_once_deleted() {
        let jita = Arc::new(Mutex::new(Jita::full()));
        let env = make_env(&standard_services());
        let mut harness = start(&env, &jita);
        harness.run();
        buy_list_for(&mut harness);
        settle(&mut harness, "Oldest: 4m");
        pick(&mut harness, "Cheapest (Alliance)", "Haulers");
        settle(&mut harness, "3,011,146.86");
        click(&mut harness, "Save");

        let mut store = Store::open(&env.data.join("user.sqlite"), NOW).expect("the store opens");
        let plan = store.plans().expect("the plans")[0].id;
        let route = store.routes().expect("the routes")[0].id;
        let services = store.services(route).expect("the services");
        let haulers = services
            .iter()
            .find(|service| service.name == "Haulers")
            .expect("Haulers is saved")
            .id;
        let saved = store.plan(plan).expect("the plan").expect("a saved plan");
        assert_eq!(saved.service, Some(haulers));

        store.delete_service(haulers).expect("Haulers is deleted");
        // A new app opens the plan from the store; clicking the plan already on screen would not reload it.
        drop(harness);
        let mut harness = start(&env, &jita);
        harness.run();
        click(&mut harness, "New plan");
        settle(&mut harness, "Cheapest (Alliance)");
        assert!(shown(&harness, "107,998.43"));
    }

    #[test]
    fn copy_multibuy_is_disabled_when_every_line_is_in_stock() {
        let jita = Arc::new(Mutex::new(Jita::full()));
        let mut harness = complete_list(&jita, &standard_services());
        paste(
            &mut harness,
            "Morphite\t10\nParticle Accelerator Unit\t10\nGuidance Systems\t10\nHobgoblin I\t10\nRobotics\t10\nR.A.M.- Robotics\t10",
        );
        harness.run();
        let copy = harness
            .get_all_by_label("Copy multibuy")
            .last()
            .expect("the copy button");
        assert!(copy.accesskit_node().is_disabled());
    }

    #[test]
    fn a_cheaper_second_service_is_the_cheapest() {
        let jita = Arc::new(Mutex::new(Jita::full()));
        let cheap = (
            "Cheap",
            RateCard {
                per_m3: Some(per_m3(100)),
                ..RateCard::default()
            },
            RouteKind::Shortest,
            None,
        );
        let mut services = standard_services();
        services.push(cheap);
        let mut harness = complete_list(&jita, &services);
        settle(&mut harness, "Cheapest (Cheap)");
        settle(&mut harness, "12,050.00");
    }

    #[test]
    fn the_buy_list_screenshots_are_taken() {
        let mut results = SnapshotResults::new();

        let jita = Arc::new(Mutex::new(Jita::full()));
        let mut harness = complete_list(&jita, &standard_services());
        harness.snapshot("buylist-complete");
        results.extend_harness(&mut harness);

        let jita = Arc::new(Mutex::new(Jita::full()));
        {
            let mut state = lock(&jita);
            state.sells.insert(11688, vec![("30900.00", 4)]);
            state.sells.insert(9848, Vec::new());
        }
        let env = make_env(&standard_services());
        let mut harness = start(&env, &jita);
        harness.run();
        buy_list_for(&mut harness);
        settle(&mut harness, "only 4 on market");
        harness.snapshot("buylist-incomplete");
        results.extend_harness(&mut harness);

        let jita = Arc::new(Mutex::new(Jita::full()));
        let mut harness = complete_list(&jita, &standard_services());
        hover(&mut harness, "107,998.43");
        harness.snapshot("buylist-freight-hover");
        results.extend_harness(&mut harness);

        results.unwrap();
    }
}

//! Acceptance tests for the Buy list panel: the rows and totals, the freight services, thin and
//! missing prices, the typed prices, the refresh timer, the multibuy copy, and the screenshots. The
//! game data is the build 3607705 fixture. Prices come from a fake of Fuzzwork's summary file of
//! The Forge, and the clock is a value the tests move.
#[cfg(test)]
mod tests {

    use std::fs;
    use std::io::Write;
    use std::ops::ControlFlow;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicI64, Ordering};
    use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
    use std::thread;
    use std::time::Duration;

    use egui::accesskit::Role;
    use egui::{Event, Id, Key, Modifiers, MouseWheelUnit, OutputCommand, TouchPhase, Vec2, vec2};
    use egui_kittest::kittest::{NodeT, Queryable, by};
    use egui_kittest::{Harness, SnapshotResults};
    use eve_means_of_profit::build::prices::Prices;
    use eve_means_of_profit::rail::Page;
    use eve_means_of_profit::session::NoEve;
    use eve_means_of_profit::table::{copy_request, has_table};
    use eve_means_of_profit::{App, AppConfig};
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use mop_core::freight::RateCard;
    use mop_core::money::{Money, Rate};
    use mop_core::volume::Volume;
    use mop_esi::character_data::Fetched;
    use mop_esi::industry::{CostIndices, IndustryData, MarketPrice, save_industry};
    use mop_market::book::PriceRule;
    use mop_market::error::MarketError;
    use mop_market::fuzzwork::{BytesHttp, BytesResponse};
    use mop_sde::{Fetch, FetchError, RouteKind, import};
    use mop_store::Store;
    use std::collections::BTreeMap;
    use tempfile::TempDir;

    const NOW: i64 = 2_000_000;
    const FIXTURE: &str = "../sde/tests/fixtures/sde-3607705";
    const JITA_SYSTEM: u32 = 30_000_142;
    const KIAINTI_SYSTEM: u32 = 30_000_189;
    const HEADER: &str =
        "what,weightedaverage,maxval,minval,stddev,median,volume,numorders,fivepercent,orderSet";

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

    /// Locks the fake file's state. A lock poisoned by a failed frame is still read.
    fn lock(state: &Mutex<Forge>) -> MutexGuard<'_, Forge> {
        state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// One row of the summary file: an item's summary on one side of The Forge.
    #[derive(Clone)]
    struct Line {
        type_id: u64,
        buy: bool,
        min: &'static str,
        max: &'static str,
        weighted: &'static str,
        median: &'static str,
        five: &'static str,
        volume: u64,
        orders: u32,
    }

    impl Line {
        /// A sell side with a million units on 30 orders, as the test plans expect.
        fn sell(
            type_id: u64,
            min: &'static str,
            weighted: &'static str,
            median: &'static str,
            five: &'static str,
        ) -> Self {
            Self {
                type_id,
                buy: false,
                min,
                max: weighted,
                weighted,
                median,
                five,
                volume: 1_000_000,
                orders: 30,
            }
        }
    }

    /// The rows the tests start with: the six items of the plan, and Robotics' buy side for R.A.M.
    fn standard_lines() -> Vec<Line> {
        vec![
            Line::sell(2454, "27000.00", "28100.00", "27900.00", "27500.00"),
            Line::sell(9834, "10300.00", "10600.00", "10500.00", "10450.00"),
            Line::sell(9848, "39500.00", "40500.00", "40100.00", "39990.00"),
            Line::sell(11399, "2100.00", "2200.00", "2180.00", "2150.00"),
            Line::sell(11481, "470.00", "480.00", "479.00", "478.60"),
            Line::sell(11688, "30500.00", "31500.00", "31000.00", "30900.00"),
            Line::sell(20418, "50000.00", "50000.00", "50000.00", "50000.00"),
            Line::sell(20419, "43750.00", "43750.00", "43750.00", "43750.00"),
            Line {
                type_id: 11481,
                buy: true,
                min: "400.00",
                max: "431.60",
                weighted: "425.00",
                median: "428.00",
                five: "430.00",
                volume: 353_968,
                orders: 20,
            },
        ]
    }

    /// The file the fake serves. `published` is when Fuzzwork made it; `version` changes with each
    /// new file, and the answer to a request with the current version is "not modified".
    struct Forge {
        lines: Vec<Line>,
        published: i64,
        version: u64,
        /// The `If-None-Match` of each request, in order.
        calls: Vec<Option<String>>,
        /// What the next requests answer instead of the file.
        fault: Fault,
        /// Requests block until this is false.
        hold: bool,
    }

    /// A failure the fake file gives in place of its answer.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Fault {
        None,
        /// 429 with `retry-after: 30`.
        RateLimit,
        /// 503.
        Unavailable,
        /// The download crashes, as a panic would.
        Crash,
    }

    fn crash() -> ! {
        panic!("the download crashed")
    }

    impl Forge {
        fn published(published: i64) -> Self {
            Self {
                lines: standard_lines(),
                published,
                version: 1,
                calls: Vec::new(),
                fault: Fault::None,
                hold: false,
            }
        }

        /// Replaces the file with a new one, made at `published`.
        fn publish(&mut self, lines: Vec<Line>, published: i64) {
            self.lines = lines;
            self.published = published;
            self.version += 1;
        }

        fn line_mut(&mut self, type_id: u64, buy: bool) -> &mut Line {
            self.lines
                .iter_mut()
                .find(|line| line.type_id == type_id && line.buy == buy)
                .expect("the line is in the file")
        }
    }

    /// The fake file, read through the market's own download and copy.
    struct FakeForge {
        state: Arc<Mutex<Forge>>,
    }

    impl BytesHttp for FakeForge {
        fn get_bytes(
            &self,
            _url: &str,
            if_none_match: Option<&str>,
        ) -> Result<BytesResponse, MarketError> {
            loop {
                let state = lock(&self.state);
                if !state.hold {
                    break;
                }
                drop(state);
                thread::sleep(Duration::from_millis(5));
            }
            let mut state = lock(&self.state);
            state.calls.push(if_none_match.map(str::to_owned));
            match state.fault {
                Fault::None => {}
                Fault::RateLimit => return Ok(answer(429, None, Some(30))),
                Fault::Unavailable => return Ok(answer(503, None, None)),
                Fault::Crash => {
                    drop(state);
                    crash();
                }
            }
            let etag = format!("\"{}\"", state.version);
            if if_none_match == Some(etag.as_str()) {
                return Ok(answer(304, Some(etag), None));
            }
            Ok(BytesResponse {
                status: 200,
                etag: Some(etag),
                last_modified: Some(http_date(state.published)),
                retry_after: None,
                body: gzip(csv(&state.lines).as_bytes()),
            })
        }
    }

    fn answer(status: u16, etag: Option<String>, retry_after: Option<u64>) -> BytesResponse {
        BytesResponse {
            status,
            etag,
            last_modified: None,
            retry_after,
            body: Vec::new(),
        }
    }

    /// The file as text: the header, then one row per item and side.
    fn csv(lines: &[Line]) -> String {
        let mut text = format!("{HEADER}\n");
        for line in lines {
            text.push_str(&format!(
                "10000002|{}|{},{},{},{},0.0,{},{},{},{},175414\n",
                line.type_id,
                line.buy,
                line.weighted,
                line.max,
                line.min,
                line.median,
                line.volume,
                line.orders,
                line.five,
            ));
        }
        text
    }

    fn gzip(text: &[u8]) -> Vec<u8> {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(text).expect("the text is written");
        encoder.finish().expect("the file is compressed")
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

    /// The fake file and the clock every test in this file runs with.
    #[derive(Clone)]
    struct World {
        forge: Arc<Mutex<Forge>>,
        now: Arc<AtomicI64>,
        /// The data folders of the lists that `complete_list` made. The app writes to them while
        /// the test runs, so they stay until the test ends.
        folders: Arc<Mutex<Vec<Env>>>,
    }

    impl World {
        /// The file as Fuzzwork made it 240 s before the first refresh of a test.
        fn new() -> Self {
            Self {
                forge: Arc::new(Mutex::new(Forge::published(NOW - 240))),
                now: Arc::new(AtomicI64::new(NOW)),
                folders: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn at(&self, now: i64) {
            self.now.store(now, Ordering::SeqCst);
        }

        /// How many requests the fake has had.
        fn requests(&self) -> usize {
            lock(&self.forge).calls.len()
        }
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

    /// Industry data fetched at the test clock and good for an hour: Jita's indices and no adjusted
    /// prices. The Buy list needs it for the invention's attempts; the fee is left out.
    fn industry() -> IndustryData {
        let mut systems = BTreeMap::new();
        systems.insert(
            JITA_SYSTEM,
            CostIndices {
                manufacturing: Some(Rate::from_ppm(50_000)),
                reaction: Some(Rate::from_ppm(1_400)),
                ..CostIndices::default()
            },
        );
        IndustryData {
            systems: Some(Fetched {
                data: systems,
                fetched_at: NOW,
                last_modified: None,
                expires: Some(NOW + 3_600),
            }),
            prices: Some(Fetched {
                data: BTreeMap::<u32, MarketPrice>::new(),
                fetched_at: NOW,
                last_modified: None,
                expires: Some(NOW + 3_600),
            }),
        }
    }

    fn make_env(services: &[(&'static str, RateCard, RouteKind, Option<u32>)]) -> Env {
        let dir = TempDir::new().expect("a temp folder");
        let data = dir.path().join("data");
        let cache = dir.path().join("cache");
        fs::create_dir_all(&data).expect("the data folder");
        import(&fixture(), &data.join("sde.sqlite")).expect("the fixture imports");
        save_industry(&data.join("esi"), &industry()).expect("industry data saved");
        let mut store = Store::open(&data.join("user.sqlite"), NOW)
            .expect("the store opens")
            .store;
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

    /// Starts the app with the fake file in place of the real market, on the world's clock.
    fn start(env: &Env, world: &World) -> Harness<'static, App> {
        start_sized(env, world, 1280.0)
    }

    /// `start` in a window `width` points wide.
    fn start_sized(env: &Env, world: &World, width: f32) -> Harness<'static, App> {
        start_window(env, world, vec2(width, 800.0))
    }

    /// `start` in a window of `size`: the Buy list needs the room at 14 px text.
    fn start_window(env: &Env, world: &World, size: Vec2) -> Harness<'static, App> {
        let clock = Arc::clone(&world.now);
        let config = AppConfig {
            data_dir: env.data.clone(),
            cache_dir: env.cache.clone(),
            fetch: Box::new(NoNetwork),
            clock: Box::new(move || clock.load(Ordering::SeqCst)),
            eve: Box::new(NoEve),
            market: Box::new(crate::offline::Offline),
        };
        let market = Arc::new(Prices::new(Box::new(FakeForge {
            state: Arc::clone(&world.forge),
        })));
        let mut harness = Harness::builder()
            .with_size(size)
            .with_max_steps(crate::SETTLE_STEPS)
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

    /// Types `text` into the price box of the Buy list row labelled `row`, and presses Enter, which
    /// commits it.
    fn type_price(harness: &mut Harness<'_, App>, row: &str, text: &str) {
        let (left, y) = row_anchor(harness, row);
        // The Buy list is narrow at 1280 px, so its price column sits past the right edge until the
        // table is scrolled sideways. The table goes back to where it was after the price is typed.
        scroll_table(harness, left, y, -400.0);
        // The price box is the rightmost text box on the row's line.
        let price_box = harness
            .query_all(by().predicate(|node| node.role() == Role::TextInput))
            .filter(|node| (node.rect().center().y - y).abs() < 4.0)
            .max_by(|a, b| a.rect().min.x.total_cmp(&b.rect().min.x))
            .expect("the price box of the row");
        price_box.click();
        harness.run();
        harness.key_press_modifiers(Modifiers::COMMAND, Key::A);
        if text.is_empty() {
            harness.key_press(Key::Delete);
        } else {
            harness.event(Event::Text(text.to_owned()));
        }
        harness.key_press(Key::Enter);
        harness.run();
        scroll_table(harness, left, y, 400.0);
    }

    /// Scrolls the table that holds the point (`left`, `y`) sideways by `delta` points, and runs the
    /// frames the animated scroll takes.
    fn scroll_table(harness: &mut Harness<'_, App>, left: f32, y: f32, delta: f32) {
        harness.hover_at(egui::pos2(left + 20.0, y));
        harness.event(Event::MouseWheel {
            unit: MouseWheelUnit::Point,
            delta: vec2(delta, 0.0),
            phase: TouchPhase::Move,
            modifiers: Modifiers::NONE,
        });
        for _ in 0..30 {
            harness.step();
        }
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
    /// Steps panel has a row of the same name; the Buy list's row is the one to the right of it.
    fn cells(harness: &Harness<'_, App>, row: &str) -> Vec<String> {
        let anchor = row_anchor(harness, row);
        let mut found: Vec<(f32, String)> = harness
            .query_all(by().predicate(|node| node.bounding_box().is_some()))
            .filter(|node| (node.rect().center().y - anchor.1).abs() < 4.0)
            .filter(|node| node.rect().min.x >= anchor.0 - 1.0)
            // A text box's own text is a child node of the same value; the box is the one to count.
            .filter(|node| node.accesskit_node().role() != Role::TextRun)
            .filter_map(|node| node.value().map(|value| (node.rect().min.x, value)))
            .filter(|(_, value)| !value.is_empty())
            .collect();
        found.sort_by(|a, b| a.0.total_cmp(&b.0));
        found.into_iter().map(|(_, value)| value).collect()
    }

    /// Where the Buy list's row labelled `row` starts: the rightmost label of that name, since the
    /// Buy list is the panel to the right of the Steps panel.
    fn row_anchor(harness: &Harness<'_, App>, row: &str) -> (f32, f32) {
        harness
            .get_all_by_label(row)
            .map(|node| (node.rect().min.x, node.rect().center().y))
            .max_by(|a, b| a.0.total_cmp(&b.0))
            .expect("the row")
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

    /// Scrolls a label into view, points at it and runs enough frames for its tooltip to appear.
    /// The datacore lines make the list taller, so the freight amount sits at the window's lower
    /// edge, where a tooltip opened without scrolling never appears.
    fn hover(harness: &mut Harness<'_, App>, label: &str) {
        crate::ui::hover(harness, label);
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

    /// The Buy list of the complete plan, once the prices are in.
    fn complete_list(
        world: &World,
        services: &[(&'static str, RateCard, RouteKind, Option<u32>)],
    ) -> Harness<'static, App> {
        complete_list_sized(world, services, 1280.0)
    }

    /// `complete_list` in a window `width` points wide. The Item panel narrows the Buy list, and the
    /// freight amount needs the room to be hovered.
    fn complete_list_sized(
        world: &World,
        services: &[(&'static str, RateCard, RouteKind, Option<u32>)],
        width: f32,
    ) -> Harness<'static, App> {
        let env = make_env(services);
        let mut harness = start_sized(&env, world, width);
        harness.run();
        buy_list_for(&mut harness);
        settle(&mut harness, "Oldest: 4m");
        lock_folders(world).push(env);
        harness
    }

    fn lock_folders(world: &World) -> MutexGuard<'_, Vec<Env>> {
        world.folders.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The Buy list once the prices are in, from a file made `seconds` before the clock's time. The
    /// frame it returns waits for the age cells, which all read `age`.
    fn aged_list(world: &World, seconds: i64, age: &str) -> Harness<'static, App> {
        lock(&world.forge).published = NOW - seconds;
        let env = make_env(&standard_services());
        let mut harness = start(&env, world);
        harness.run();
        buy_list_for(&mut harness);
        settle(&mut harness, age);
        lock_folders(world).push(env);
        harness
    }

    /// Pastes `text` into the pasted stock box.
    fn paste(harness: &mut Harness<'_, App>, text: &str) {
        harness
            .get_by(|node| node.role() == Role::MultilineTextInput)
            .click();
        harness.event(Event::Paste(text.to_owned()));
        harness.run();
    }

    /// The six rows of the complete plan, at the prices of the standard file.
    fn assert_complete_rows(harness: &Harness<'_, App>) {
        assert_row(
            harness,
            "Hobgoblin I",
            &[
                "Hobgoblin I",
                "10",
                "50.00",
                "27,500.00",
                "27,500.00",
                "4m",
                "275,000.00",
            ],
        );
        assert_row(
            harness,
            "Guidance Systems",
            &[
                "Guidance Systems",
                "10",
                "30.00",
                "10,450.00",
                "10,450.00",
                "4m",
                "104,500.00",
            ],
        );
        assert_row(
            harness,
            "Robotics",
            &[
                "Robotics",
                "10",
                "30.00",
                "39,990.00",
                "39,990.00",
                "4m",
                "399,900.00",
            ],
        );
        assert_row(
            harness,
            "Morphite",
            &[
                "Morphite",
                "10",
                "0.10",
                "2,150.00",
                "2,150.00",
                "4m",
                "21,500.00",
            ],
        );
        assert_row(
            harness,
            "R.A.M.- Robotics",
            &[
                "R.A.M.- Robotics",
                "10",
                "0.40",
                "478.60",
                "478.60",
                "4m",
                "4,786.00",
            ],
        );
        assert_row(
            harness,
            "Particle Accelerator Unit",
            &[
                "Particle Accelerator Unit",
                "10",
                "10.00",
                "30,900.00",
                "30,900.00",
                "4m",
                "309,000.00",
            ],
        );
    }

    /// The two datacores the invention buys for the whole attempts of Hobgoblin II.
    fn assert_datacore_rows(harness: &Harness<'_, App>) {
        assert_row(
            harness,
            "Datacore - Electronic Engineering",
            &[
                "Datacore - Electronic Engineering",
                "3",
                "0.30",
                "50,000.00",
                "50,000.00",
                "4m",
                "150,000.00",
            ],
        );
        assert_row(
            harness,
            "Datacore - Graviton Physics",
            &[
                "Datacore - Graviton Physics",
                "3",
                "0.30",
                "43,750.00",
                "43,750.00",
                "4m",
                "131,250.00",
            ],
        );
    }

    /// The price boxes of the Buy list rows labelled `rows`, with what each holds. The table is
    /// scrolled sideways so the prices are in view. The rows are found before the scroll, since the
    /// scroll moves their names out of view.
    fn price_boxes(
        harness: &mut Harness<'_, App>,
        rows: &[&str],
    ) -> Vec<(egui::Rect, Option<String>)> {
        let lines: Vec<f32> = rows.iter().map(|row| row_anchor(harness, row).1).collect();
        let (left, y) = row_anchor(harness, rows[0]);
        scroll_table(harness, left, y, -400.0);
        lines
            .into_iter()
            .map(|y| price_box_at(harness, y))
            .collect()
    }

    /// The rightmost text box on the line at `y`, which is a Buy list row's price box.
    fn price_box_at(harness: &Harness<'_, App>, y: f32) -> (egui::Rect, Option<String>) {
        harness
            .query_all(by().predicate(|node| node.role() == Role::TextInput))
            .filter(|node| (node.rect().center().y - y).abs() < 4.0)
            .max_by(|a, b| a.rect().min.x.total_cmp(&b.rect().min.x))
            .map(|node| (node.rect(), node.value()))
            .expect("the price box of the row")
    }

    /// Points at `at` and runs enough frames for the tooltip there to appear.
    fn hover_at_point(harness: &mut Harness<'_, App>, at: egui::Pos2) {
        harness.hover_at(at);
        for _ in 0..60 {
            harness.run();
        }
    }

    /// Whether a pixel inside `area` is coloured rather than grey. The amber of an old price and the
    /// colour of a typed price are coloured; the grey text is not.
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

    /// The Age cell of the row labelled `row`, which reads `text`, with the table scrolled sideways
    /// so the age column is in view.
    fn age_cell(harness: &mut Harness<'_, App>, row: &str, text: &str) -> egui::Rect {
        let (left, y) = row_anchor(harness, row);
        scroll_table(harness, left, y, -700.0);
        harness
            .get_all_by_label(text)
            .map(|node| node.rect())
            .find(|rect| (rect.center().y - y).abs() < 4.0)
            .expect("the age cell of the row")
    }

    /// The text put on the clipboard by the frame just drawn, if any.
    fn copied_text(harness: &Harness<'_, App>) -> Option<String> {
        harness
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
            })
    }

    #[test]
    fn the_buy_list_rows_and_the_footer_are_complete() {
        let world = World::new();
        let harness = complete_list(&world, &standard_services());
        assert!(shown(&harness, "Price rule: Forge sell, 5% price"));
        assert_complete_rows(&harness);
        assert_datacore_rows(&harness);
        assert!(shown(&harness, "m³ 121.10 · value 1,395,936.00 · freight "));
        assert!(shown(
            &harness,
            " · collateral 1,395,936.00 · delivered 1,505,850.68"
        ));
        assert!(!shown(&harness, " incomplete"));
        assert!(shown(&harness, "Oldest: 4m"));
        assert!(shown(&harness, "Next refresh in 15m"));
        assert!(has_value(&harness, "Cheapest (Alliance)"));
        assert!(shown(&harness, "1 contract"));
    }

    #[test]
    fn the_freight_hover_lists_every_service() {
        let world = World::new();
        let mut harness = complete_list_sized(&world, &standard_services(), 1600.0);
        hover(&mut harness, "109,914.68");
        assert!(shown(&harness, "Alliance 109,914.68"));
        assert!(shown(&harness, "Haulers 3,013,959.36"));
    }

    #[test]
    fn choosing_haulers_changes_the_freight_and_saves_the_service() {
        let world = World::new();
        let mut harness = complete_list(&world, &standard_services());
        pick(&mut harness, "Cheapest (Alliance)", "Haulers");
        settle(&mut harness, "3,013,959.36");
        assert!(shown(
            &harness,
            " · collateral 1,395,936.00 · delivered 4,409,895.36"
        ));
        assert!(has_value(&harness, "Haulers"));
    }

    /// Sets the price rule that new plans start with, in the store the app opens.
    fn default_rule(env: &Env, rule: PriceRule) {
        let mut store = Store::open(&env.data.join("user.sqlite"), NOW)
            .expect("the store opens")
            .store;
        let mut settings = store.settings().expect("settings");
        settings.default_price_rule = rule;
        store.save_settings(&settings).expect("settings saved");
    }

    /// The Buy list of a new plan under `rule`, once its prices are in.
    fn list_under(world: &World, rule: PriceRule) -> (Env, Harness<'static, App>) {
        let env = make_env(&standard_services());
        default_rule(&env, rule);
        let mut harness = start(&env, world);
        harness.run();
        buy_list_for(&mut harness);
        settle(&mut harness, "Oldest: 4m");
        (env, harness)
    }

    #[test]
    fn the_median_rule_prices_each_line_at_its_median() {
        let (_env, harness) = list_under(&World::new(), PriceRule::SellMedian);
        assert_row(&harness, "Robotics", &["40,100.00", "401,000.00"]);
    }

    #[test]
    fn the_lowest_sell_rule_prices_each_line_at_its_lowest_sell() {
        let (_env, harness) = list_under(&World::new(), PriceRule::LowestSell);
        assert_row(&harness, "Robotics", &["39,500.00", "395,000.00"]);
    }

    #[test]
    fn the_weighted_average_rule_prices_each_line_at_its_average() {
        let (_env, harness) = list_under(&World::new(), PriceRule::SellWeightedAverage);
        assert_row(&harness, "Robotics", &["40,500.00", "405,000.00"]);
    }

    #[test]
    fn an_old_walk_the_book_plan_reads_the_five_percent_price() {
        let (_env, harness) = list_under(&World::new(), PriceRule::WalkSell);
        assert!(shown(&harness, "Price rule: Forge sell, 5% price"));
        assert_row(&harness, "Robotics", &["39,990.00", "399,900.00"]);
    }

    #[test]
    fn highest_buy_prices_only_the_line_with_buy_orders() {
        let (_env, mut harness) = list_under(&World::new(), PriceRule::HighestBuy);
        assert_row(&harness, "R.A.M.- Robotics", &["431.60", "4,316.00"]);
        let rows = [
            "Hobgoblin I",
            "Guidance Systems",
            "Robotics",
            "Morphite",
            "R.A.M.- Robotics",
            "Particle Accelerator Unit",
        ];
        let boxes = price_boxes(&mut harness, &rows);
        for (row, (_, value)) in rows.iter().zip(&boxes) {
            if *row != "R.A.M.- Robotics" {
                assert_eq!(value.as_deref(), Some("—"), "{row}");
            }
        }
        let robotics = boxes[2].0;
        hover_at_point(&mut harness, robotics.center());
        assert!(shown(&harness, "no Forge buy orders"));
        assert!(shown(&harness, " incomplete"));
    }

    #[test]
    fn a_thin_side_and_a_missing_side_are_marked_and_left_out_of_the_value() {
        let world = World::new();
        {
            let mut forge = lock(&world.forge);
            forge.line_mut(11688, false).volume = 4;
            forge.lines.retain(|line| line.type_id != 9848);
        }
        let env = make_env(&standard_services());
        let mut harness = start_window(&env, &world, vec2(1280.0, 1000.0));
        harness.run();
        buy_list_for(&mut harness);
        settle(&mut harness, "only 4 on market");
        // A thin line keeps the summary price; only its value is cut to the units on the market.
        assert_row(
            &harness,
            "Particle Accelerator Unit",
            &["30,900.00", "only 4 on market"],
        );
        assert_row(&harness, "Robotics", &["—"]);
        assert!(shown(&harness, " incomplete"));
        assert!(shown(&harness, "m³ 121.10 · value 687,036.00 · freight "));
        hover(&mut harness, " incomplete");
        assert!(shown(&harness, "Robotics\nParticle Accelerator Unit"));
    }

    #[test]
    fn a_missing_price_shows_a_dash_and_says_why_on_hover() {
        let world = World::new();
        lock(&world.forge).lines.retain(|line| line.type_id != 9848);
        let env = make_env(&standard_services());
        let mut harness = start(&env, &world);
        harness.run();
        buy_list_for(&mut harness);
        settle(&mut harness, "Oldest: 4m");
        let boxes = price_boxes(&mut harness, &["Robotics"]);
        let (at, value) = &boxes[0];
        assert_eq!(value.as_deref(), Some("—"));
        hover_at_point(&mut harness, at.center());
        assert!(shown(&harness, "no Forge sell orders"));
    }

    #[test]
    fn old_prices_show_their_age_and_the_status_line_names_the_oldest() {
        let world = World::new();
        lock(&world.forge).published = NOW - 21_601;
        let env = make_env(&standard_services());
        let mut harness = start_window(&env, &world, vec2(1280.0, 1000.0));
        harness.run();
        buy_list_for(&mut harness);
        settle(&mut harness, "Oldest: 6h");
        assert!(shown(&harness, "Market data: oldest 6h"));
        // The age of the oldest price is in the stale colour.
        let old = harness.get_by_label("Oldest: 6h").rect();
        assert!(coloured(&mut harness, old));

        // A file made 4 minutes ago is not stale: its age is in the text colour.
        world.at(NOW + 900);
        lock(&world.forge).publish(standard_lines(), NOW + 660);
        click(&mut harness, "Refresh now");
        settle(&mut harness, "Oldest: 4m");
        let fresh = harness.get_by_label("Oldest: 4m").rect();
        assert!(!coloured(&mut harness, fresh));
    }

    #[test]
    fn a_price_exactly_six_hours_old_keeps_the_text_colour() {
        let world = World::new();
        lock(&world.forge).published = NOW - 21_600;
        let env = make_env(&standard_services());
        let mut harness = start(&env, &world);
        harness.run();
        buy_list_for(&mut harness);
        settle(&mut harness, "Oldest: 6h");
        assert!(shown(&harness, "Market data: oldest 6h"));
        // Six hours is the limit itself, not past it: the age is in the text colour.
        let limit = harness.get_by_label("Oldest: 6h").rect();
        assert!(!coloured(&mut harness, limit));
    }

    #[test]
    fn a_price_exactly_six_hours_old_keeps_the_age_cell_in_the_text_colour() {
        let world = World::new();
        let mut harness = aged_list(&world, 21_600, "6h");
        assert_row(&harness, "Robotics", &["6h"]);
        // Six hours is the limit itself, not past it: the cell is in the text colour.
        let cell = age_cell(&mut harness, "Robotics", "6h");
        assert!(!coloured(&mut harness, cell));
    }

    #[test]
    fn an_age_past_a_day_says_old_and_warns_before_buying() {
        // A second past six hours: the cell is amber.
        let world = World::new();
        let mut harness = aged_list(&world, 21_601, "6h");
        let cell = age_cell(&mut harness, "Robotics", "6h");
        assert!(coloured(&mut harness, cell));

        // Exactly a day is still `1d`, in amber.
        let world = World::new();
        let mut harness = aged_list(&world, 86_400, "1d");
        assert_row(&harness, "Robotics", &["1d"]);
        let cell = age_cell(&mut harness, "Robotics", "1d");
        assert!(coloured(&mut harness, cell));

        // A second past a day: the cell says `old`, in amber, and warns on hover.
        let world = World::new();
        let mut harness = aged_list(&world, 86_401, "1d old");
        assert_row(&harness, "Robotics", &["1d old"]);
        let cell = age_cell(&mut harness, "Robotics", "1d old");
        assert!(coloured(&mut harness, cell));
        hover_at_point(&mut harness, cell.center());
        assert!(shown(
            &harness,
            "Over a day old. Refresh before you rely on it."
        ));
        // The copied list says `old` too, not only the drawn cell.
        harness
            .ctx
            .data_mut(|data| data.insert_temp(copy_request(Id::new("Buy list")), true));
        harness.step();
        let copied = copied_text(&harness).expect("the list is copied");
        assert!(
            copied.contains("\t1d old\t"),
            "the copied age reads old: {copied}"
        );
    }

    #[test]
    fn the_oldest_line_asks_for_a_refresh_once_the_prices_are_over_a_day_old() {
        let world = World::new();
        let mut harness = aged_list(&world, 172_800, "2d old");
        let text = "Oldest: 2d · Prices over a day old: refresh before you buy.";
        settle(&mut harness, text);
        // The line sits below an 800 px window at the 14 px metrics; the colour is only on screen
        // once it is scrolled into view.
        harness.get_by_label(text).scroll_to_me();
        harness.run();
        let old = harness.get_by_label(text).rect();
        assert!(coloured(&mut harness, old));

        // An hour old is not stale: the line gives no refresh advice and is in the text colour.
        let world = World::new();
        let mut harness = aged_list(&world, 3_600, "1h");
        settle(&mut harness, "Oldest: 1h");
        let fresh = harness.get_by_label("Oldest: 1h").rect();
        assert!(!coloured(&mut harness, fresh));
    }

    #[test]
    fn a_rate_limit_keeps_the_prices_and_names_the_wait() {
        let world = World::new();
        let mut harness = complete_list(&world, &standard_services());
        world.at(NOW + 900);
        lock(&world.forge).fault = Fault::RateLimit;
        click(&mut harness, "Refresh now");
        settle(&mut harness, "ERROR");
        assert!(shown(
            &harness,
            "Market prices paused for 30 s (rate limited)."
        ));
        assert!(shown(&harness, "27,500.00"));
    }

    #[test]
    fn an_unavailable_file_keeps_the_prices_and_names_the_error() {
        let world = World::new();
        let mut harness = complete_list(&world, &standard_services());
        world.at(NOW + 900);
        lock(&world.forge).fault = Fault::Unavailable;
        click(&mut harness, "Refresh now");
        settle(&mut harness, "ERROR");
        assert!(shown(
            &harness,
            "Could not get market prices: ESI answered with status 503"
        ));
        assert!(shown(&harness, "27,500.00"));
    }

    #[test]
    fn no_freight_is_an_empty_quote() {
        let world = World::new();
        let mut harness = complete_list(&world, &standard_services());
        pick(&mut harness, "Jita → Kiainti", "No freight");
        settle(&mut harness, "No freight");
        assert!(shown(&harness, "m³ 121.10 · value 1,395,936.00 · freight "));
        // The freight amount itself is 0.00, as is the collateral.
        assert!(shown(&harness, "0.00"));
        assert!(shown(
            &harness,
            " · collateral 0.00 · delivered 1,395,936.00"
        ));
        assert!(!shown(&harness, " incomplete"));
    }

    #[test]
    fn a_minimum_charge_packs_two_contracts_at_twenty_million_either_way() {
        let world = World::new();
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
        let mut harness = complete_list(&world, std::slice::from_ref(&bulk));
        settle(&mut harness, "2 contracts");
        assert!(shown(&harness, "20,000,000.00"));
        click(&mut harness, "Top up the last contract");
        settle(&mut harness, "2 contracts");
        assert!(shown(&harness, "20,000,000.00"));
    }

    #[test]
    fn top_up_moves_units_into_the_last_contract_and_turning_it_off_packs_them_in_order() {
        let world = World::new();
        // Each contract pays at least 60,000 ISK. With top-up on, the last contract is filled to 62 m³
        // and the first, at 58.5 m³, pays its minimum: 122,000.00. Off, 100 m³ and 20.5 m³ pay 160,000.00.
        let minimum = (
            "Minimum",
            RateCard {
                per_m3: Some(per_m3(1_000)),
                minimum: Some(per_m3(60_000)),
                max_m3: Some(Volume::from_ten_thousandths(100 * 10_000)),
                ..RateCard::default()
            },
            RouteKind::Shortest,
            None,
        );
        let mut harness = complete_list(&world, std::slice::from_ref(&minimum));
        // Top-up is on by default.
        settle(&mut harness, "122,000.00");
        click(&mut harness, "Top up the last contract");
        settle(&mut harness, "160,000.00");
        assert!(shown(&harness, "2 contracts"));
    }

    #[test]
    fn a_service_that_cannot_carry_an_item_says_why_in_the_freight_hover() {
        let world = World::new();
        // A contract holds 1 m³; one Hobgoblin I is 5 m³.
        let tiny = (
            "Tiny",
            RateCard {
                per_m3: Some(per_m3(850)),
                max_m3: Some(Volume::from_ten_thousandths(10_000)),
                ..RateCard::default()
            },
            RouteKind::Shortest,
            None,
        );
        let mut services = standard_services();
        services.push(tiny);
        let mut harness = complete_list_sized(&world, &services, 1600.0);
        hover(&mut harness, "109,914.68");
        assert!(shown(&harness, "Tiny: cannot carry Hobgoblin I"));
        assert!(shown(&harness, "Alliance 109,914.68"));
    }

    #[test]
    fn the_earlier_service_wins_a_tie_for_the_cheapest() {
        let world = World::new();
        let mut services = standard_services();
        // The same rate card and route as Alliance, saved after it.
        services.push((
            "Twin",
            RateCard {
                per_m3: Some(per_m3(850)),
                collateral: Some(Rate::from_ppm(5_000)),
                ..RateCard::default()
            },
            RouteKind::Shortest,
            None,
        ));
        let mut harness = complete_list(&world, &services);
        settle(&mut harness, "Cheapest (Alliance)");
    }

    #[test]
    fn copy_multibuy_puts_the_lines_on_the_clipboard() {
        let world = World::new();
        let mut harness = complete_list(&world, &standard_services());
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
                "Hobgoblin I\t10\nGuidance Systems\t10\nRobotics\t10\nMorphite\t10\nR.A.M.- Robotics\t10\nParticle Accelerator Unit\t10\nDatacore - Electronic Engineering\t3\nDatacore - Graviton Physics\t3"
            )
        );
        // The notice shows in the frame after the click: the status line is drawn before the page.
        harness.run();
        assert!(shown(&harness, "Copied 8 lines for multibuy."));
    }

    #[test]
    fn a_request_that_blocks_does_not_stop_the_frames() {
        let world = World::new();
        lock(&world.forge).hold = true;
        let env = make_env(&standard_services());
        let mut harness = start(&env, &world);
        harness.run();
        buy_list_for(&mut harness);
        for _ in 0..20 {
            harness.run();
        }
        assert!(shown(&harness, "Morphite"));
        assert!(shown(&harness, "…"));
        lock(&world.forge).hold = false;
        settle(&mut harness, "27,500.00");
    }

    #[test]
    fn haulers_is_saved_and_falls_back_to_the_cheapest_once_deleted() {
        let world = World::new();
        let env = make_env(&standard_services());
        let mut harness = start(&env, &world);
        harness.run();
        buy_list_for(&mut harness);
        settle(&mut harness, "Oldest: 4m");
        pick(&mut harness, "Cheapest (Alliance)", "Haulers");
        settle(&mut harness, "3,013,959.36");
        click(&mut harness, "Save");

        let mut store = Store::open(&env.data.join("user.sqlite"), NOW)
            .expect("the store opens")
            .store;
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
        let mut harness = start(&env, &world);
        harness.run();
        click(&mut harness, "New plan");
        // The new app downloads the file at its start, so the price can arrive after the label.
        settle(&mut harness, "109,914.68");
        settle(&mut harness, "Cheapest (Alliance)");
        assert!(shown(&harness, "109,914.68"));
    }

    #[test]
    fn copy_multibuy_is_disabled_when_every_line_is_in_stock() {
        let world = World::new();
        let mut harness = complete_list(&world, &standard_services());
        paste(
            &mut harness,
            "Morphite\t10\nParticle Accelerator Unit\t10\nGuidance Systems\t10\nHobgoblin I\t10\nRobotics\t10\nR.A.M.- Robotics\t10\nDatacore - Electronic Engineering\t3\nDatacore - Graviton Physics\t3",
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
        let world = World::new();
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
        let mut harness = complete_list(&world, &services);
        settle(&mut harness, "Cheapest (Cheap)");
        settle(&mut harness, "12,110.00");
    }

    #[test]
    fn a_typed_price_freezes_the_line_until_it_is_cleared() {
        let world = World::new();
        let mut harness = complete_list(&world, &standard_services());
        type_price(&mut harness, "Robotics", "1,000,000,000.00");
        assert_row(
            &harness,
            "Robotics",
            &[
                "1,000,000,000.00",
                "39,990.00",
                "+2,500,525%",
                "manual",
                "10,000,000,000.00",
            ],
        );
        // The typed price is in the accent colour; the line on the live price is not.
        let (left, y) = row_anchor(&harness, "Robotics");
        let boxes = price_boxes(&mut harness, &["Robotics", "Hobgoblin I"]);
        assert!(coloured(&mut harness, boxes[0].0));
        assert!(!coloured(&mut harness, boxes[1].0));
        scroll_table(&mut harness, left, y, 400.0);
        world.at(NOW + 600);
        lock(&world.forge).publish(
            {
                let mut lines = standard_lines();
                lines
                    .iter_mut()
                    .find(|line| line.type_id == 9848)
                    .expect("Robotics")
                    .five = "41000.00";
                lines
            },
            NOW + 600,
        );
        world.at(NOW + 900);
        click(&mut harness, "Refresh now");
        settle(&mut harness, "41,000.00");
        assert_row(
            &harness,
            "Robotics",
            &["1,000,000,000.00", "41,000.00", "manual"],
        );
        type_price(&mut harness, "Robotics", "");
        assert_row(&harness, "Robotics", &["41,000.00", "41,000.00", "5m"]);
    }

    #[test]
    fn a_price_that_is_not_a_number_keeps_the_old_one() {
        let world = World::new();
        let mut harness = complete_list(&world, &standard_services());
        type_price(&mut harness, "Robotics", "abc");
        assert!(shown(&harness, "Not a price"));
        assert_row(&harness, "Robotics", &["39,990.00"]);
        assert!(!shown(&harness, "manual"));
    }

    #[test]
    fn a_negative_price_is_not_a_price() {
        let world = World::new();
        let mut harness = complete_list(&world, &standard_services());
        type_price(&mut harness, "Robotics", "-1");
        assert!(shown(&harness, "Not a price"));
        assert_row(&harness, "Robotics", &["39,990.00"]);
    }

    #[test]
    fn a_typed_price_for_a_missing_side_prices_the_line() {
        let world = World::new();
        lock(&world.forge).lines.retain(|line| line.type_id != 9848);
        let env = make_env(&standard_services());
        let mut harness = start(&env, &world);
        harness.run();
        buy_list_for(&mut harness);
        settle(&mut harness, "Oldest: 4m");
        type_price(&mut harness, "Robotics", "39,990.00");
        assert_row(&harness, "Robotics", &["39,990.00", "399,900.00"]);
        // With no market row there is no live price: Live is `—`, and the age reads `manual`.
        assert_eq!(
            cells(&harness, "Robotics"),
            [
                "Robotics",
                "10",
                "0",
                "10",
                "30.00",
                "39,990.00",
                "—",
                "manual",
                "399,900.00",
            ]
        );
        assert!(!shown(&harness, " incomplete"));
    }

    #[test]
    fn hidden_columns_leave_the_table() {
        let world = World::new();
        let env = make_env(&standard_services());
        let mut harness = start(&env, &world);
        harness.run();
        buy_list_for(&mut harness);
        settle(&mut harness, "Oldest: 4m");
        for column in ["Live", "Diff", "Age"] {
            toggle_column(&mut harness, column);
        }
        assert!(!shown(&harness, "Live"));
        assert!(!shown(&harness, "Diff"));
        assert!(!shown(&harness, "Age"));
        assert!(shown(&harness, "m³ 121.10 · value 1,395,936.00 · freight "));
    }

    /// Ticks or unticks the column labelled `label` through the Columns menu.
    fn toggle_column(harness: &mut Harness<'_, App>, label: &str) {
        click(harness, "Columns");
        click(harness, label);
        harness.key_press(Key::Escape);
        harness.run();
    }

    /// Whether the checkbox labelled `label` is ticked.
    fn checked(harness: &Harness<'_, App>, label: &str) -> bool {
        let node = harness
            .query_all_by_label(label)
            .find(|node| node.accesskit_node().toggled().is_some())
            .unwrap_or_else(|| panic!("no checkbox labelled {label}"));
        node.accesskit_node().toggled() == Some(egui::accesskit::Toggled::True)
    }

    /// The hidden columns saved for the buy list, as the store holds them.
    fn saved_columns(env: &Env) -> Option<String> {
        let store = Store::open(&env.data.join("user.sqlite"), NOW)
            .expect("the store opens")
            .store;
        store
            .setting("columns.build_buy_list")
            .expect("the setting")
    }

    /// Saves `text` as the buy list's hidden columns, as an earlier session would have.
    fn save_columns(env: &Env, text: &str) {
        let mut store = Store::open(&env.data.join("user.sqlite"), NOW)
            .expect("the store opens")
            .store;
        store
            .set_setting("columns.build_buy_list", text)
            .expect("the setting is saved");
    }

    #[test]
    fn hidden_column_text_is_split_and_joined_on_commas() {
        use eve_means_of_profit::table::{hidden_from_text, hidden_to_text};

        assert_eq!(hidden_from_text("volume,share"), ["volume", "share"]);
        assert_eq!(hidden_from_text(""), Vec::<String>::new());
        assert_eq!(hidden_from_text(",volume,,"), ["volume"]);
        assert_eq!(
            hidden_to_text(&["volume".to_owned(), "share".to_owned()]),
            "volume,share"
        );
        assert_eq!(hidden_to_text(&[]), "");
    }

    #[test]
    fn hidden_columns_are_saved_and_restored_by_a_new_app() {
        let world = World::new();
        let env = make_env(&standard_services());
        let mut harness = start(&env, &world);
        harness.run();
        buy_list_for(&mut harness);
        settle(&mut harness, "Oldest: 4m");

        toggle_column(&mut harness, "m³");
        assert!(!shown(&harness, "m³"));
        assert_eq!(saved_columns(&env).as_deref(), Some("volume"));

        drop(harness);
        let mut harness = start(&env, &world);
        harness.run();
        buy_list_for(&mut harness);
        settle(&mut harness, "Oldest: 4m");
        assert!(!shown(&harness, "m³"));
        click(&mut harness, "Columns");
        assert!(!checked(&harness, "m³"));
    }

    #[test]
    fn showing_a_hidden_column_again_clears_the_saved_key() {
        let world = World::new();
        let env = make_env(&standard_services());
        save_columns(&env, "volume");
        let mut harness = start(&env, &world);
        harness.run();
        buy_list_for(&mut harness);
        settle(&mut harness, "Oldest: 4m");
        assert!(!shown(&harness, "m³"));

        toggle_column(&mut harness, "m³");
        assert!(shown(&harness, "m³"));
        assert_eq!(saved_columns(&env), None);
    }

    #[test]
    fn an_unknown_hidden_id_is_ignored_and_dropped_at_the_next_save() {
        let world = World::new();
        let env = make_env(&standard_services());
        save_columns(&env, "nope");
        let mut harness = start(&env, &world);
        harness.run();
        buy_list_for(&mut harness);
        settle(&mut harness, "Oldest: 4m");
        for column in ["m³", "Live", "Diff", "Age"] {
            assert!(shown(&harness, column), "{column} is not drawn");
        }

        toggle_column(&mut harness, "m³");
        assert!(!shown(&harness, "m³"));
        assert_eq!(saved_columns(&env).as_deref(), Some("volume"));
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_save_is_reported_and_the_column_stays_hidden() {
        use std::os::unix::fs::PermissionsExt;

        let world = World::new();
        let env = make_env(&standard_services());
        let mut harness = start(&env, &world);
        harness.run();
        buy_list_for(&mut harness);
        settle(&mut harness, "Oldest: 4m");
        // A data folder that cannot be written to makes the journal, and so the save, fail.
        let writable = fs::metadata(&env.data)
            .expect("the data folder")
            .permissions();
        let mut read_only = writable.clone();
        read_only.set_mode(0o555);
        fs::set_permissions(&env.data, read_only).expect("the data folder is read-only");

        toggle_column(&mut harness, "m³");
        fs::set_permissions(&env.data, writable).expect("the data folder is writable again");

        assert!(
            harness
                .query_all(by().label_contains("Could not save the settings:"))
                .next()
                .is_some()
        );
        assert!(!shown(&harness, "m³"));
        assert_eq!(saved_columns(&env), None);
    }

    #[test]
    fn the_timer_asks_for_the_file_when_the_refresh_is_due_and_takes_a_new_one() {
        let world = World::new();
        let mut harness = complete_list(&world, &standard_services());
        assert_eq!(world.requests(), 1);
        world.at(NOW + 899);
        harness.run();
        assert_eq!(world.requests(), 1);
        assert!(shown(&harness, "Next refresh in 1s"));
        lock(&world.forge).publish(
            {
                let mut lines = standard_lines();
                lines
                    .iter_mut()
                    .find(|line| line.type_id == 2454)
                    .expect("Hobgoblin I")
                    .five = "27600.00";
                lines
            },
            NOW + 600,
        );
        world.at(NOW + 900);
        settle(&mut harness, "27,600.00");
        assert_eq!(world.requests(), 2);
        assert_eq!(
            lock(&world.forge).calls.last(),
            Some(&Some("\"1\"".to_owned()))
        );
    }

    #[test]
    fn with_the_timer_off_nothing_is_asked_until_the_player_asks() {
        let world = World::new();
        let mut harness = complete_list(&world, &standard_services());
        click(&mut harness, "Refresh automatically");
        assert!(shown(&harness, "Automatic refresh is off"));
        world.at(NOW + 5_000);
        harness.run();
        assert_eq!(world.requests(), 1);
        world.at(NOW + 300);
        click(&mut harness, "Refresh now");
        settle(&mut harness, "Prices are 9m old; next refresh in 10m");
        assert_eq!(world.requests(), 1);
        world.at(NOW + 900);
        click(&mut harness, "Refresh now");
        harness.run();
        assert_eq!(world.requests(), 2);
    }

    #[test]
    fn opening_the_list_with_no_prices_asks_once_even_with_the_timer_off() {
        let world = World::new();
        let env = make_env(&standard_services());
        let mut harness = start(&env, &world);
        harness.run();
        buy_list_for(&mut harness);
        click(&mut harness, "Refresh automatically");
        settle(&mut harness, "Oldest: 4m");
        assert_eq!(world.requests(), 1);
    }

    #[test]
    fn the_rows_follow_the_plan_order() {
        let world = World::new();
        let harness = complete_list(&world, &standard_services());
        let names = [
            "Hobgoblin I",
            "Guidance Systems",
            "Robotics",
            "Morphite",
            "R.A.M.- Robotics",
            "Particle Accelerator Unit",
            "Datacore - Electronic Engineering",
            "Datacore - Graviton Physics",
        ];
        let tops: Vec<f32> = names
            .iter()
            .map(|name| row_anchor(&harness, name).1)
            .collect();
        assert!(
            tops.windows(2).all(|pair| pair[0] < pair[1]),
            "the rows are out of order: {tops:?}"
        );
    }

    #[test]
    fn the_buy_list_says_it_has_a_table() {
        let world = World::new();
        let harness = complete_list(&world, &standard_services());
        assert_eq!(
            harness
                .ctx
                .data(|data| data.get_temp::<bool>(has_table(Id::new("Buy list")))),
            Some(true)
        );
    }

    #[test]
    fn copy_as_tsv_leaves_out_the_hidden_columns_and_keeps_the_footer() {
        let world = World::new();
        let env = make_env(&standard_services());
        let mut harness = start(&env, &world);
        harness.run();
        buy_list_for(&mut harness);
        settle(&mut harness, "Oldest: 4m");
        for column in ["Live", "Diff", "Age"] {
            click(&mut harness, "Columns");
            click(&mut harness, column);
            harness.key_press(Key::Escape);
            harness.run();
        }
        // The copy is asked for in memory, as the panel's menu does, and the table makes it in the
        // next frame it draws.
        harness
            .ctx
            .data_mut(|data| data.insert_temp(copy_request(Id::new("Buy list")), true));
        harness.step();
        assert_eq!(
            copied_text(&harness).as_deref(),
            Some(
                "Item\tNeed\tOwn\tBuy\tm³\tPrice\tValue\n\
                 Hobgoblin I\t10\t0\t10\t50\t27500\t275000\n\
                 Guidance Systems\t10\t0\t10\t30\t10450\t104500\n\
                 Robotics\t10\t0\t10\t30\t39990\t399900\n\
                 Morphite\t10\t0\t10\t0.1\t2150\t21500\n\
                 R.A.M.- Robotics\t10\t0\t10\t0.4\t478.6\t4786\n\
                 Particle Accelerator Unit\t10\t0\t10\t10\t30900\t309000\n\
                 Datacore - Electronic Engineering\t3\t0\t3\t0.3\t50000\t150000\n\
                 Datacore - Graviton Physics\t3\t0\t3\t0.3\t43750\t131250\n\
                 Total\t\t\t\t121.1\t\t1395936"
            )
        );
    }

    #[test]
    fn refresh_now_is_disabled_while_a_request_runs() {
        let world = World::new();
        let mut harness = complete_list(&world, &standard_services());
        world.at(NOW + 900);
        lock(&world.forge).hold = true;
        // The timer asks for the file; the request waits on the fake.
        harness.run();
        harness.run();
        assert!(shown(&harness, "Refreshing…"));
        assert!(
            harness
                .get_by_label("Refresh now")
                .accesskit_node()
                .is_disabled()
        );
        assert_eq!(world.requests(), 1);
        lock(&world.forge).hold = false;
        settle(&mut harness, "Next refresh in 15m");
        assert_eq!(world.requests(), 2);
    }

    #[test]
    fn a_first_download_that_fails_is_not_asked_again_on_its_own() {
        let world = World::new();
        lock(&world.forge).fault = Fault::Unavailable;
        let env = make_env(&standard_services());
        let mut harness = start(&env, &world);
        harness.run();
        buy_list_for(&mut harness);
        click(&mut harness, "Refresh automatically");
        settle(&mut harness, "ERROR");
        for _ in 0..50 {
            harness.run();
        }
        assert_eq!(world.requests(), 1);
    }

    #[test]
    fn a_crashed_download_is_reported_and_the_timer_tries_again_later() {
        let world = World::new();
        let mut harness = complete_list(&world, &standard_services());
        lock(&world.forge).fault = Fault::Crash;
        world.at(NOW + 900);
        settle(
            &mut harness,
            "Could not get market prices: the download stopped unexpectedly.",
        );
        assert_row(&harness, "Robotics", &["39,990.00"]);
        assert!(
            !harness
                .get_by_label("Refresh now")
                .accesskit_node()
                .is_disabled()
        );
        // The failed download is not asked again on every frame.
        let asked = world.requests();
        for _ in 0..10 {
            harness.run();
        }
        assert_eq!(world.requests(), asked);
        // Once the download works again, the timer asks at the time the failed one held back to.
        lock(&world.forge).fault = Fault::None;
        world.at(NOW + 1_800);
        settle(&mut harness, "Next refresh in 15m");
        assert_eq!(world.requests(), asked + 1);
    }

    #[test]
    fn a_stopped_worker_says_the_prices_cannot_refresh_until_restart() {
        let world = World::new();
        let mut harness = complete_list(&world, &standard_services());
        // A job that crashes stops the worker's thread for the rest of the session.
        harness
            .state_mut()
            .shared_mut()
            .worker
            .run(Box::new(|| panic!("a job crashed")));
        world.at(NOW + 900);
        settle(
            &mut harness,
            "Prices cannot refresh until the app restarts.",
        );
        assert!(
            harness
                .get_by_label("Refresh now")
                .accesskit_node()
                .is_disabled()
        );
        assert_row(&harness, "Robotics", &["39,990.00"]);
    }

    #[test]
    fn with_another_page_open_the_timer_request_still_runs() {
        let world = World::new();
        let mut harness = complete_list(&world, &standard_services());
        click(&mut harness, Page::Setup.label());
        world.at(NOW + 900);
        for _ in 0..5 {
            harness.run();
        }
        // The background refresh asks whatever page is open, so the timer's request runs on Setup.
        assert_eq!(world.requests(), 2);
        // Back on the Build page the list is drawn again, with the next refresh already set.
        click(&mut harness, "Build");
        settle(&mut harness, "Next refresh in 15m");
        assert_eq!(world.requests(), 2);
    }

    #[test]
    fn a_typed_price_on_a_saved_plan_is_kept_across_a_restart_until_it_is_cleared() {
        let world = World::new();
        let env = make_env(&standard_services());
        let mut harness = start(&env, &world);
        harness.run();
        buy_list_for(&mut harness);
        settle(&mut harness, "Oldest: 4m");
        click(&mut harness, "Save");
        type_price(&mut harness, "Robotics", "1,000,000,000.00");
        drop(harness);

        let mut harness = start(&env, &world);
        harness.run();
        click(&mut harness, "New plan");
        settle(&mut harness, "Oldest: 4m");
        assert_row(&harness, "Robotics", &["1,000,000,000.00", "manual"]);
        let (left, y) = row_anchor(&harness, "Robotics");
        let boxes = price_boxes(&mut harness, &["Robotics", "Hobgoblin I"]);
        assert!(coloured(&mut harness, boxes[0].0));
        assert!(!coloured(&mut harness, boxes[1].0));
        scroll_table(&mut harness, left, y, 400.0);
        type_price(&mut harness, "Robotics", "");
        drop(harness);

        let mut harness = start(&env, &world);
        harness.run();
        click(&mut harness, "New plan");
        settle(&mut harness, "Oldest: 4m");
        assert_row(&harness, "Robotics", &["39,990.00", "4m"]);
        assert!(!shown(&harness, "manual"));
    }

    #[test]
    fn a_typed_price_that_cannot_be_saved_is_reported_and_stays_on_screen() {
        use std::os::unix::fs::PermissionsExt;

        let world = World::new();
        let mut harness = complete_list(&world, &standard_services());
        let data = lock_folders(&world)[0].data.clone();
        // A data folder that cannot be written to makes the journal, and so the save, fail.
        let writable = fs::metadata(&data).expect("the data folder").permissions();
        let mut read_only = writable.clone();
        read_only.set_mode(0o555);
        fs::set_permissions(&data, read_only).expect("the data folder is read-only");

        type_price(&mut harness, "Robotics", "1,000,000,000.00");
        fs::set_permissions(&data, writable).expect("the data folder is writable again");

        assert!(
            harness
                .query_all(by().label_contains("Could not save the price:"))
                .next()
                .is_some()
        );
        assert_row(&harness, "Robotics", &["1,000,000,000.00", "manual"]);
    }

    #[test]
    fn a_typed_price_is_kept_when_the_plan_is_left_and_opened_again() {
        let world = World::new();
        let env = make_env(&standard_services());
        let mut harness = start(&env, &world);
        harness.run();
        buy_list_for(&mut harness);
        settle(&mut harness, "Oldest: 4m");
        click(&mut harness, "Save");
        type_price(&mut harness, "Robotics", "1,000,000,000.00");
        click(&mut harness, "New");
        harness.run();
        click(&mut harness, "New plan");
        settle(&mut harness, "manual");
        assert_row(&harness, "Robotics", &["1,000,000,000.00", "manual"]);
    }

    #[test]
    fn the_screenshots_of_the_buy_list_are_taken() {
        let mut results = SnapshotResults::new();

        let world = World::new();
        let mut harness = complete_list(&world, &standard_services());
        harness.snapshot("buylist-complete");
        results.extend_harness(&mut harness);

        let world = World::new();
        {
            let mut forge = lock(&world.forge);
            forge.line_mut(11688, false).volume = 4;
            forge.lines.retain(|line| line.type_id != 9848);
        }
        let env = make_env(&standard_services());
        let mut harness = start(&env, &world);
        harness.run();
        buy_list_for(&mut harness);
        settle(&mut harness, "m³ 121.10 · value 687,036.00 · freight ");
        harness.snapshot("buylist-incomplete");
        results.extend_harness(&mut harness);

        let world = World::new();
        let mut harness = complete_list_sized(&world, &standard_services(), 1600.0);
        hover(&mut harness, "109,914.68");
        harness.snapshot("buylist-freight-hover");
        results.extend_harness(&mut harness);

        let world = World::new();
        let mut harness = complete_list(&world, &standard_services());
        type_price(&mut harness, "Robotics", "1,000,000,000.00");
        // The price columns are past the right edge at 1280 px, so the picture scrolls to them.
        let (left, y) = row_anchor(&harness, "Robotics");
        scroll_table(&mut harness, left, y, -540.0);
        // The pointer rests over the Plans panel, so no tooltip covers the row.
        harness.hover_at(egui::pos2(300.0, 600.0));
        harness.run();
        harness.snapshot("buylist-manual-price");
        results.extend_harness(&mut harness);

        results.unwrap();
    }
}

#![cfg(test)]

//! Acceptance tests for the Buy list panel: the rows and totals, the freight services, thin and
//! missing prices, the typed prices, the refresh button, the multibuy copy, and the screenshots. The
//! game data is the build 3607705 fixture. Prices come from a fake of Fuzzwork's summary file of
//! The Forge, and the clock is a value the tests move.

mod columns;
mod copy;
mod freight;
mod plans;
mod prices;
mod refresh;
mod rows;
mod snapshots;
mod typed_price;

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
use egui_kittest::{Harness, Node, SnapshotResults};
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
use mop_store::{InventionChoice, Store};
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
        Line::sell(
            34202,
            "1200000.00",
            "1200000.00",
            "1200000.00",
            "1200000.00",
        ),
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
        market: Box::new(FakeForge {
            state: Arc::clone(&world.forge),
        }),
    };
    let mut harness = Harness::builder()
        .with_size(size)
        .with_max_steps(crate::SETTLE_STEPS)
        .build_eframe(move |cc| App::new(&cc.egui_ctx, config).expect("the app starts"));
    crate::ui::no_images(&mut harness);
    harness.state_mut().show_page(Page::Build);
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
/// commits it. The Price column is in view at 1280 px, so the table is not scrolled.
fn type_price(harness: &mut Harness<'_, App>, row: &str, text: &str) {
    let (_, y) = row_anchor(harness, row);
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

/// The Buy list panel that holds `anchor`.
fn buy_list_panel<'a>(harness: &'a Harness<'_, App>, anchor: Node<'a>) -> Node<'a> {
    let title_id = harness.get_by_label("Buy list").accesskit_node().id();
    let mut panel = anchor;
    while !panel
        .children_recursive()
        .any(|node| node.accesskit_node().id() == title_id)
    {
        panel = panel.parent().expect("the Buy list tab above the line");
    }
    panel
}

/// The rightmost node labelled `label`: the Buy list is to the right of the Steps panel, which
/// gives some of its columns the same labels.
fn rightmost<'h>(harness: &'h Harness<'_, App>, label: &'h str) -> Node<'h> {
    harness
        .get_all_by_label(label)
        .max_by(|a, b| a.rect().min.x.total_cmp(&b.rect().min.x))
        .unwrap_or_else(|| panic!("no node labelled {label:?}"))
}

/// The text and box of each cell on the line that `anchor` is on, left to right, empty cells
/// left out. The Steps panel has a row of the same name and its table can reach under the Buy
/// list, so only nodes under the Buy list's own tab are counted.
fn line_cells(harness: &Harness<'_, App>, anchor: Node<'_>) -> Vec<(egui::Rect, String)> {
    let anchor_rect = anchor.rect();
    let panel = buy_list_panel(harness, anchor);
    let mut found: Vec<(egui::Rect, String)> = panel
        .children_recursive()
        .filter(|node| node.accesskit_node().bounding_box().is_some())
        .filter(|node| (node.rect().center().y - anchor_rect.center().y).abs() < 4.0)
        .filter(|node| node.rect().min.x >= anchor_rect.min.x - 1.0)
        // A text box's own text is a child node of the same value; the box is the one to count.
        .filter(|node| node.accesskit_node().role() != Role::TextRun)
        .filter_map(|node| node.value().map(|value| (node.rect(), value)))
        .filter(|(_, value)| !value.is_empty())
        .collect();
    found.sort_by(|a, b| a.0.min.x.total_cmp(&b.0.min.x));
    found
}

/// The cell texts of the Buy list row labelled `row`, left to right, empty cells left out.
fn cells(harness: &Harness<'_, App>, row: &str) -> Vec<String> {
    let anchor = harness
        .get_all_by_label(row)
        .max_by(|a, b| a.rect().min.x.total_cmp(&b.rect().min.x))
        .expect("the row");
    line_cells(harness, anchor)
        .into_iter()
        .map(|(_, value)| value)
        .collect()
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
    buy_list_for_quantity(harness, 10);
}

/// Makes a plan for Hobgoblin II, quantity `quantity`, freight on Jita → Kiainti, and opens its
/// Buy list.
fn buy_list_for_quantity(harness: &mut Harness<'_, App>, quantity: u32) {
    click(harness, "New plan");
    type_into(harness, "", "hobgoblin");
    click(harness, "Hobgoblin II");
    type_into(harness, "1", &quantity.to_string());
    // The Buy list's own freight line also reads "No freight", so take the dropdown.
    harness
        .query_all(by().predicate(|node| node.value().is_some_and(|value| value == "No freight")))
        .find(|node| node.accesskit_node().role() == Role::ComboBox)
        .expect("the freight dropdown")
        .click();
    harness.run();
    click(harness, "Jita → Kiainti");
}

/// Picks the saved plan named `name` in the Plans list. The other panels' empty states also
/// offer a `New plan` button, so the first node with the label is the plan's row.
fn open_saved_plan(harness: &mut Harness<'_, App>, name: &str) {
    harness
        .get_all_by_label(name)
        .next()
        .unwrap_or_else(|| panic!("the plan {name:?} in the Plans list"))
        .click();
    harness.run();
}

/// The Buy list of the complete plan, once the prices are in.
fn complete_list(
    world: &World,
    services: &[(&'static str, RateCard, RouteKind, Option<u32>)],
) -> Harness<'static, App> {
    complete_list_sized(world, services, 1280.0)
}

/// `complete_list` in a window `width` points wide. The Buy list takes half the Build page's
/// lower row, and the freight amount needs the room to be hovered.
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

/// A Hobgoblin II × 8 plan saved with the invention skills at 4 and `choice` stored for 2456,
/// then opened again by a new app with the prices already in. The window is taller than the
/// other tests' so the decryptor row, below the two datacores, is in view.
fn invention_list(world: &World, choice: InventionChoice) -> Harness<'static, App> {
    let size = vec2(1280.0, 1000.0);
    let env = make_env(&standard_services());
    let mut harness = start_window(&env, world, size);
    harness.run();
    buy_list_for_quantity(&mut harness, 8);
    settle(&mut harness, "Oldest: 4m");
    click(&mut harness, "Save");
    {
        let mut store = Store::open(&env.data.join("user.sqlite"), NOW)
            .expect("the store opens")
            .store;
        let plan = store.plans().expect("the plans")[0].id;
        for skill in [11453, 11446, 23121] {
            store
                .set_invention_skill(skill, 4)
                .expect("the skill is saved");
        }
        store
            .set_plan_invention(plan, 2456, choice)
            .expect("the choice is saved");
    }
    drop(harness);
    let mut harness = start_window(&env, world, size);
    harness.run();
    open_saved_plan(&mut harness, "New plan");
    settle(&mut harness, "Oldest: 4m");
    lock_folders(world).push(env);
    harness
}

fn isk(text: &str) -> Money {
    Money::parse_isk(text).expect("a price")
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

/// The price boxes of the Buy list rows labelled `rows`, with what each holds. The Price column
/// is in view at 1280 px, so the table is not scrolled.
fn price_boxes(harness: &mut Harness<'_, App>, rows: &[&str]) -> Vec<(egui::Rect, Option<String>)> {
    rows.iter()
        .map(|row| price_box_at(harness, row_anchor(harness, row).1))
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

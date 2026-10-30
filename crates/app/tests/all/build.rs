#![cfg(test)]

//! Acceptance tests for the Build page: the plans, the plan inputs, the pasted stock and the steps.
//! The game data is the build 3607705 fixture, imported in the test. The user data is a temp store
//! with the facilities the plans use. The clock is fixed at 1,000,000.

use std::fs;
use std::io::Write;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use egui::accesskit::Role;
use egui::{Event, Key, Modifiers, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable, by};
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
use tempfile::TempDir;

mod plans;
mod rules;
mod shots;
mod steps;
mod strip;

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

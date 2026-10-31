#![cfg(test)]

//! Acceptance tests for the Characters page: the table of login characters, refresh, login,
//! remove, the keychain warning and the jEve panel. The login is a fake that records its calls;
//! jEve's profile is a made-up file in a temp folder. The clock is fixed at 1,000,000.

use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use crate::ui::hover;
use egui::accesskit::Role;
use egui::{Key, Modifiers, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable, by};
use eve_means_of_profit::characters::list::{
    date_text, kind_view, login_view, parse_date, plex_text, portrait_size,
};
use eve_means_of_profit::characters::{forget_state, set_client_id_present, set_opener};
use eve_means_of_profit::images::{self, Picture};
use eve_means_of_profit::session::{EveAccess, PendingLogin, lock_eve};
use eve_means_of_profit::status::Level;
use eve_means_of_profit::theme::Style;
use eve_means_of_profit::{App, AppConfig};
use mop_core::money::Money;
use mop_core::stock::{Asset, CharacterAssets};
use mop_esi::character_data::{
    CharacterData, Fetched, Job, JobStatus, Kind, Order, OwnedAssets, Refreshed,
};
use mop_esi::characters::{Character, Login};
use mop_esi::error::EsiError;
use mop_esi::places::{NamingReport, StructureEntry, StructureNames};
use mop_esi::saved;
use mop_market::error::MarketError;
use mop_market::fuzzwork::{BytesHttp, BytesResponse};
use mop_sde::{Fetch, FetchError, import};
use mop_store::net_worth::Account;
use mop_store::{Density, Store};
use rusqlite::Connection;
use tempfile::TempDir;

use fakes::*;

const NOW: i64 = 1_000_000;
const FIXTURE: &str = "../sde/tests/fixtures/sde-3607705";
const MAIN: u64 = 2_112_000_001;
const ALT: u64 = 2_112_000_002;
const GAMMA: u64 = 2_112_000_004;
const HOME_FORT: i64 = 1_035_466_617_946;

const RAM: u32 = 11_481;
const KEYCHAIN: &str = "No system keychain found (no backend). Tokens are kept in a file.";
/// The steps any wait in this file may run before it gives up. The app's jobs arrive in
/// frames, and a machine that also runs other work draws those frames slowly; a generous
/// budget keeps a slow machine from failing a test whose work still happens, while work
/// that never happens still ends in the wait's own panic.
const WAIT_STEPS: usize = 1_000;

fn held<T>(data: T, at: i64, expires: Option<i64>) -> Fetched<T> {
    Fetched {
        data,
        fetched_at: at,
        last_modified: None,
        expires,
    }
}

fn owned(character_id: u64, assets: Vec<Asset>) -> OwnedAssets {
    OwnedAssets {
        assets: CharacterAssets {
            character_id: i64::try_from(character_id).expect("an id that fits"),
            updated: Some(NOW),
            assets,
        },
        names: std::collections::BTreeMap::new(),
    }
}

/// Main: assets fetched 4,000 s ago (EVE allowed a refresh 400 s ago), orders 300 s ago
/// (allowed in 900 s), jobs never, wallet 30 s ago (allowed in 90 s).
fn main_data() -> CharacterData {
    CharacterData {
        character_id: MAIN,
        assets: Some(held(owned(MAIN, Vec::new()), NOW - 4_000, Some(NOW - 400))),
        orders: Some(held(Vec::new(), NOW - 300, Some(NOW + 900))),
        jobs: None,
        wallet: Some(held(Money::ZERO, NOW - 30, Some(NOW + 90))),
    }
}

/// Assets and jobs fetched (one asset in a structure), orders and wallet not yet due.
fn partial(data: &mut CharacterData) -> Results {
    let asset = Asset {
        item_id: 7,
        type_id: RAM,
        location_id: HOME_FORT,
        flag: Some("Hangar".to_owned()),
        quantity: 3,
        singleton: false,
    };
    data.assets = Some(held(owned(data.character_id, vec![asset]), NOW, None));
    data.jobs = Some(held(Vec::new(), NOW, None));
    vec![
        (Kind::Assets, Ok(Refreshed::Fetched)),
        (Kind::Orders, Ok(Refreshed::NotYet { until: NOW + 900 })),
        (Kind::Jobs, Ok(Refreshed::Fetched)),
        (Kind::Wallet, Ok(Refreshed::NotYet { until: NOW + 90 })),
    ]
}

fn all_not_yet(_data: &mut CharacterData) -> Results {
    Kind::ALL
        .into_iter()
        .map(|kind| (kind, Ok(Refreshed::NotYet { until: NOW + 90 })))
        .collect()
}

fn expired(_data: &mut CharacterData) -> Results {
    vec![(Kind::Assets, Err(EsiError::LoginExpired))]
}

fn fill_jeve(folder: &Path) {
    let connection = Connection::open(folder.join("#Default.db")).expect("database");
    connection
        .execute_batch(
            "CREATE TABLE owners (accountid TEXT, ownerid INTEGER, name TEXT, corp TEXT, show NUMERIC, \
             invalid NUMERIC, assetslastupdate INTEGER, assetsnextupdate INTEGER, scopes TEXT, \
             UNIQUE(accountid));
             CREATE TABLE assets (accountid TEXT, count INTEGER, flagid INTEGER, flagstring TEXT, \
             itemid INTEGER, typeid INTEGER, locationid INTEGER, singleton NUMERIC, rawquantity INTEGER, \
             UNIQUE(accountid, itemid));
             CREATE TABLE blueprints (accountid TEXT, itemid INTEGER, locationid INTEGER, typeid INTEGER, \
             flagid INTEGER, flagstring TEXT, quantity INTEGER, timeefficiency INTEGER, \
             materialefficiency INTEGER, runs NUMERIC, UNIQUE(accountid, itemid));
             INSERT INTO owners (accountid, ownerid, name, show, invalid, assetslastupdate, scopes) VALUES
              ('j2', 2112000002, 'Alt', 1, 0, 700000000, 'esi-assets.read_assets.v1'),
              ('j3', 2112000003, 'Hauler', 1, 0, NULL, 'esi-assets.read_assets.v1');
             INSERT INTO assets (accountid, itemid, typeid, locationid, flagid, flagstring, count, singleton, rawquantity) VALUES
              ('j2', 5001, 11481, 60003760, 4, 'Hangar', 60, 0, NULL),
              ('j3', 6001, 11481, 60003760, 4, 'Hangar', 5, 0, NULL);",
        )
        .expect("fixture builds");
    drop(connection);
}

/// Opens the app on the Characters page. Main's data is saved first, so the app loads it. The
/// start-up refresh is left to settle and the fake's log is cleared, so a test sees only what
/// it caused.
fn open(env: &Env, eve: &FakeEve) -> Harness<'static, App> {
    let mut harness = open_raw(env, eve);
    drop(settle_start(&mut harness));
    clear_log(&mut harness, eve);
    harness
}

/// Like [`open`], without waiting for the start-up refresh or clearing its log.
fn open_raw(env: &Env, eve: &FakeEve) -> Harness<'static, App> {
    open_with(env, eve, true)
}

/// Runs frames until the start-up character refresh and its price request are both over.
/// Returns the `explicit` mark of every price answer that arrived.
fn settle_start(harness: &mut Harness<'_, App>) -> Vec<bool> {
    let mut prices = Vec::new();
    let mut loaded = false;
    let mut busy = true;
    let mut running = true;
    for _ in 0..WAIT_STEPS {
        harness.step();
        prices.extend(price_requests(harness));
        loaded = harness.state_mut().shared_mut().session.loaded();
        busy = disabled(harness, "Refresh all");
        running = harness.state_mut().shared_mut().market.running();
        if loaded && !prices.is_empty() && !busy && !running {
            return prices;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!(
        "the start-up refresh did not finish: loaded {loaded}, prices {prices:?}, busy {busy}, running {running}"
    );
}

/// Clears what the fake and the market have recorded, so a test sees only its own work.
fn clear_log(harness: &mut Harness<'_, App>, eve: &FakeEve) {
    eve.write(|log| {
        log.refreshed.clear();
        log.named.clear();
        log.removed.clear();
        log.opened.clear();
        log.begun = 0;
    });
    drop(harness.state_mut().shared_mut().market.take_outcomes());
}

/// The `explicit` marks of the price requests that have finished since the last call.
fn price_requests(harness: &mut Harness<'_, App>) -> Vec<bool> {
    harness
        .state_mut()
        .shared_mut()
        .market
        .take_outcomes()
        .into_iter()
        .map(|outcome| outcome.explicit)
        .collect()
}

/// Like [`open`]; with `hooks` false the page uses its own client id check and browser opener.
fn open_with(env: &Env, eve: &FakeEve, hooks: bool) -> Harness<'static, App> {
    saved::save_character(&esi_dir(env), &main_data()).expect("Main's data saved");
    open_app(env, eve, hooks)
}

/// Opens the app with whatever the fake's login and the saved files already hold.
fn open_app(env: &Env, eve: &FakeEve, hooks: bool) -> Harness<'static, App> {
    let config = AppConfig {
        data_dir: env.data.clone(),
        cache_dir: env.cache.clone(),
        fetch: Box::new(NoNetwork),
        clock: Box::new(|| NOW),
        eve: Box::new(eve.clone()),
        market: Box::new(NoForge),
    };
    let mut harness = Harness::builder()
        .with_size(vec2(1280.0, 800.0))
        .with_max_steps(crate::SETTLE_STEPS)
        .build_eframe(move |cc| App::new(&cc.egui_ctx, config).expect("the app starts"));
    if hooks {
        set_client_id_present(&harness.ctx, true);
        let log = Arc::clone(&eve.log);
        set_opener(
            &harness.ctx,
            Arc::new(move |url| {
                log.lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .opened
                    .push(url.to_owned());
                Ok(())
            }),
        );
    }
    until(&mut harness, "the saved data", |h| {
        h.query_all_by_label("Loading character data…")
            .next()
            .is_none()
    });
    rail(&mut harness, "Characters");
    harness
}

/// Three frames: a job's repaint request may arrive at any time, so `run` could give up.
fn frames(harness: &mut Harness<'_, App>) {
    for _ in 0..3 {
        harness.step();
    }
}

fn until(harness: &mut Harness<'_, App>, what: &str, done: impl Fn(&Harness<'_, App>) -> bool) {
    for _ in 0..WAIT_STEPS {
        harness.step();
        if done(harness) {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("{what} did not happen");
}

fn rail(harness: &mut Harness<'_, App>, label: &str) {
    harness
        .get_all_by_label(label)
        .next()
        .expect("a rail entry")
        .click();
    frames(harness);
}

/// Picks `label` from the Characters table's Columns menu.
fn choose_level(harness: &mut Harness<'_, App>, label: &str) {
    harness.get_by_label("Columns").click();
    frames(harness);
    harness.get_by_label(label).click();
    frames(harness);
}

fn shown(harness: &Harness<'_, App>, text: &str) -> bool {
    harness.query_all_by_label(text).next().is_some()
}

fn count(harness: &Harness<'_, App>, text: &str) -> usize {
    harness.query_all_by_label(text).count()
}

fn notices(harness: &mut Harness<'_, App>) -> Vec<String> {
    harness
        .state_mut()
        .shared_mut()
        .notices
        .iter()
        .map(|notice| notice.text.clone())
        .collect()
}

/// Runs frames until the app holds a notice.
fn until_notice(harness: &mut Harness<'_, App>) {
    for _ in 0..WAIT_STEPS {
        harness.step();
        if !notices(harness).is_empty() {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("no notice came");
}

/// The buttons labelled `Refresh all`, from the top of the window down: the page's own first,
/// the status line's last.
fn refresh_buttons<'a>(harness: &'a Harness<'a, App>) -> Vec<egui_kittest::Node<'a>> {
    let mut buttons: Vec<egui_kittest::Node<'a>> = harness
        .get_all_by_label("Refresh all")
        .filter(|node| node.accesskit_node().role() == Role::Button)
        .collect();
    buttons.sort_by(|a, b| a.rect().center().y.total_cmp(&b.rect().center().y));
    buttons
}

/// Whether the page's button labelled `label` is disabled. The status line also draws a
/// `Refresh all`, so that one is picked out by its place above the status line.
fn disabled(harness: &Harness<'_, App>, label: &str) -> bool {
    if label == "Refresh all" {
        return refresh_buttons(harness)
            .first()
            .expect("a button")
            .accesskit_node()
            .is_disabled();
    }
    harness
        .get_all_by_label(label)
        .find(|node| node.accesskit_node().role() == Role::Button)
        .expect("a button")
        .accesskit_node()
        .is_disabled()
}

/// Clicks the `nth` button labelled `label` and waits until the job it started is over.
fn press(harness: &mut Harness<'_, App>, label: &str, nth: usize) {
    if label == "Refresh all" {
        refresh_buttons(harness)
            .into_iter()
            .nth(nth)
            .expect("a button")
            .click();
    } else {
        harness
            .get_all_by_label(label)
            .nth(nth)
            .expect("a button")
            .click();
    }
    frames(harness);
    until(harness, "the job", |h| !disabled(h, "Refresh all"));
}

fn row_y(harness: &Harness<'_, App>, label: &str) -> f32 {
    harness
        .get_all_by_label(label)
        .next()
        .expect("a label")
        .rect()
        .center()
        .y
}

fn clear_notices(harness: &mut Harness<'_, App>) {
    harness.state_mut().shared_mut().notices.clear();
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

mod accounts;
mod fakes;
mod jeve;
mod login;
mod portraits;
mod refresh;
mod rows;
mod short_names;

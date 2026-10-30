#![cfg(test)]

//! Acceptance tests for the rail's pages: the panels each page shows, the saved character data and
//! jEve's profile the pages read, the place names, and the item picked in a table. The game data is
//! the build 3607705 fixture, imported in the test. The login is a fake, and jEve's profile is a
//! made-up file in a temp folder. The clock is fixed at 1,000,000.

mod assets;
mod build;
mod data;
mod rail;
mod today;
mod today_net_worth;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use egui::accesskit::{Role, Toggled};
use egui::{OutputCommand, Rect, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use eve_means_of_profit::build::prices::Prices;
use eve_means_of_profit::claims::ClaimsState;
use eve_means_of_profit::empty::{self, Fill};
use eve_means_of_profit::rail::Page;
use eve_means_of_profit::selection::Selection;
use eve_means_of_profit::session::{EveAccess, NoEve, PendingLogin, Session};
use eve_means_of_profit::theme::Style;
use eve_means_of_profit::what_to_build::{
    self, Blueprint, CharacterSlots, DeadEnd, RankRow, WhatToBuildPage,
};
use eve_means_of_profit::worker::Worker;
use eve_means_of_profit::{App, AppConfig, Shared};
use flate2::Compression;
use flate2::write::GzEncoder;
use mop_core::freight::Packing;
use mop_core::money::Money;
use mop_core::net_worth::{CharacterKey, Holdings, Record, Snapshot};
use mop_core::stock::{Asset, CharacterAssets};
use mop_esi::character_data::{
    CharacterData, Fetched, Job, JobStatus, Kind, Order, OwnedAssets, Refreshed,
};
use mop_esi::characters::{Character, Login};
use mop_esi::error::EsiError;
use mop_esi::industry::IndustryData;
use mop_esi::places::{NamingReport, StructureNames};
use mop_esi::saved;
use mop_market::book::PriceRule;
use mop_market::cache::{Http, HttpResponse};
use mop_market::error::MarketError;
use mop_market::fuzzwork::{BytesHttp, BytesResponse, FORGE, Side, SummaryRow};
use mop_sde::{Fetch, FetchError, import};
use mop_store::blueprints::BlueprintDefault;
use mop_store::price_history::{MarketKind, PointPrices};
use mop_store::{Density, PlanRecord, Store};
use rusqlite::Connection;
use tempfile::TempDir;

const NOW: i64 = 1_000_000;
const FIXTURE: &str = "../sde/tests/fixtures/sde-3607705";
const MAIN: u64 = 2_112_000_001;
const ALT: u64 = 2_112_000_002;
const HAULER: u64 = 2_112_000_003;
const JITA_4_4: i64 = 60_003_760;
const HOME_FORT: i64 = 1_035_466_617_946;
const REFUSED: i64 = 1_035_466_617_999;
/// The type the saved data and jEve's profile hold: R.A.M.- Robotics, as the fixture names it.
const RAM: u32 = 11_481;
const PAU: u32 = 11_688;
const MORPHITE: u32 = 11_399;
/// The Light Scout Drone, built from Morphite and Particle Accelerator Units.
const DRONE: u32 = 2_456;
/// The invention job's product in the Today fixture.
const HOBGOBLIN_II_BLUEPRINT: u32 = 2_457;
/// The ready reaction's product in the Today fixture.
const CRYSTALLINE_CARBONIDE: u32 = 16_670;
/// The blueprint the fixture's research, copying and reaction jobs use.
const RIFTER_BLUEPRINT: u32 = 691;
/// The Notice text when the saved data is a day old.
const OLD_NOTICE: &str = "Character data is 1d old. Refresh it on the Characters page.";

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

/// A market file with no rows, so the Buy list never reaches the network in these tests.
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

/// The Forge summary file the Today tests read: PAU sells cheapest at 61,200.00 and Morphite is
/// bought highest at 2,150.00.
struct Forge;

impl BytesHttp for Forge {
    fn get_bytes(
        &self,
        _url: &str,
        _if_none_match: Option<&str>,
    ) -> Result<BytesResponse, MarketError> {
        let text = format!(
            "what,weightedaverage,maxval,minval,stddev,median,volume,numorders,fivepercent,orderSet\n\
             10000002|{PAU}|false,61500.00,65000.00,61200.00,0.0,62000.00,500,3,61300.00,175414\n\
             10000002|{MORPHITE}|true,2100.00,2150.00,2000.00,0.0,2050.00,40,2,2100.00,175414\n"
        );
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

/// A data folder with the game data imported and the user's store. The login data is saved by
/// each test.
struct Env {
    _dir: TempDir,
    data: PathBuf,
    cache: PathBuf,
}

fn env() -> Env {
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
    store
        .set_blueprint_default(BlueprintDefault::Own)
        .expect("the blueprint answer is saved");
    let places = format!(
        "{{\"version\":1,\"structures\":{{\"{HOME_FORT}\":{{\"name\":\"Home Fort\",\"system_id\":null,\"refused\":false,\"checked_at\":900000}}}}}}"
    );
    fs::write(data.join("esi").join("places.json"), places).expect("places written");
    Env {
        _dir: dir,
        data,
        cache,
    }
}

fn esi_dir(env: &Env) -> PathBuf {
    env.data.join("esi")
}

/// A login with the characters given, a jEve folder when there is one, and a count of refreshes.
#[derive(Clone)]
struct FakeEve {
    characters: Vec<Character>,
    jeve: Option<PathBuf>,
    refreshes: Arc<Mutex<usize>>,
}

impl FakeEve {
    fn new(characters: Vec<Character>, jeve: Option<PathBuf>) -> Self {
        Self {
            characters,
            jeve,
            refreshes: Arc::new(Mutex::new(0)),
        }
    }

    fn refreshes(&self) -> Arc<Mutex<usize>> {
        Arc::clone(&self.refreshes)
    }
}

impl EveAccess for FakeEve {
    fn characters(&self) -> Vec<Character> {
        self.characters.clone()
    }

    fn begin_login(&mut self) -> Result<PendingLogin, String> {
        Err("Logins are not started in these tests.".to_owned())
    }

    fn finish_login(&mut self, _code: &str) -> Result<Character, EsiError> {
        Err(EsiError::Cancelled)
    }

    fn refresh_all(
        &mut self,
        _data: &mut CharacterData,
        _now: i64,
    ) -> Vec<(Kind, Result<Refreshed, EsiError>)> {
        *self
            .refreshes
            .lock()
            .unwrap_or_else(PoisonError::into_inner) += 1;
        Vec::new()
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
        self.jeve.clone()
    }
}

fn character(id: u64, name: &str) -> Character {
    Character {
        id,
        name: name.to_owned(),
        scopes: BTreeSet::new(),
        added_at: NOW,
        login: Login::Ok,
    }
}

fn asset(item_id: i64, location_id: i64, quantity: u64) -> Asset {
    Asset {
        item_id,
        type_id: RAM,
        location_id,
        flag: Some("Hangar".to_owned()),
        quantity,
        singleton: false,
    }
}

/// One character's saved data with assets fetched at `fetched`.
fn assets_data(character_id: u64, fetched: i64, assets: Vec<Asset>) -> CharacterData {
    let stock = CharacterAssets {
        character_id: i64::try_from(character_id).expect("an id that fits"),
        updated: Some(fetched),
        assets,
    };
    CharacterData {
        character_id,
        assets: Some(Fetched {
            data: OwnedAssets {
                assets: stock,
                names: BTreeMap::new(),
            },
            fetched_at: fetched,
            last_modified: None,
            expires: None,
        }),
        orders: None,
        jobs: None,
        wallet: None,
    }
}

/// Main's saved data: 40 in Jita 4-4 and 120 in Home Fort, fetched at 990,000.
fn main_data() -> CharacterData {
    assets_data(
        MAIN,
        990_000,
        vec![asset(1, JITA_4_4, 40), asset(2, HOME_FORT, 120)],
    )
}

/// One industry job at Home Fort, as the jobs page's fixture makes them.
fn job(
    job_id: i64,
    activity_id: u32,
    product: Option<u32>,
    blueprint: u32,
    runs: u32,
    status: JobStatus,
    end: i64,
) -> Job {
    Job {
        job_id,
        activity_id,
        blueprint_type_id: blueprint,
        product_type_id: product,
        runs,
        status,
        start: end - 86_400,
        end,
        facility_id: HOME_FORT,
        station_id: HOME_FORT,
        output_location_id: HOME_FORT,
        installer_id: i64::try_from(MAIN).expect("an id that fits"),
        cost: None,
        licensed_runs: None,
    }
}

/// M1-26's jobs: an active PAU job, an invention of a Hobgoblin II Blueprint that has ended, a
/// ready reaction and a paused copy.
fn ready_jobs() -> Vec<Job> {
    vec![
        job(
            1,
            1,
            Some(PAU),
            RIFTER_BLUEPRINT,
            10,
            JobStatus::Active,
            NOW + 7_500,
        ),
        job(
            2,
            8,
            Some(HOBGOBLIN_II_BLUEPRINT),
            HOBGOBLIN_II_BLUEPRINT,
            1,
            JobStatus::Active,
            NOW - 10,
        ),
        job(
            3,
            11,
            Some(CRYSTALLINE_CARBONIDE),
            RIFTER_BLUEPRINT,
            5,
            JobStatus::Ready,
            NOW - 3_600,
        ),
        job(
            4,
            5,
            None,
            RIFTER_BLUEPRINT,
            1,
            JobStatus::Paused,
            NOW + 20_000,
        ),
    ]
}

/// One market order at Jita 4-4, as the orders page's fixture makes them.
fn order(
    order_id: i64,
    type_id: u32,
    is_buy: bool,
    price: Money,
    (remain, total): (u64, u64),
    expires: i64,
) -> Order {
    Order {
        order_id,
        type_id,
        location_id: JITA_4_4,
        region_id: 10_000_002,
        is_buy,
        price,
        volume_remain: remain,
        volume_total: total,
        issued: 900_000,
        expires,
        range: "station".to_owned(),
        escrow: is_buy.then(|| price.checked_mul_qty(total).expect("an escrow that fits")),
        min_volume: None,
        is_corporation: false,
    }
}

/// M1-26's orders, with the Morphite buy still open, plus a PAU sell at 61,000.00: two beaten
/// PAU sells, a beaten corp PAU sell and a beaten Morphite buy.
fn beaten_orders() -> Vec<Order> {
    let mut corp = order(
        3,
        PAU,
        false,
        Money::from_ten_thousandths(700_000_000),
        (50, 50),
        NOW + 260_000,
    );
    corp.is_corporation = true;
    vec![
        order(
            1,
            PAU,
            false,
            Money::from_ten_thousandths(615_000_000),
            (100, 200),
            NOW + 260_000,
        ),
        order(
            2,
            MORPHITE,
            true,
            Money::from_ten_thousandths(21_000_000),
            (10, 10),
            NOW + 100_000,
        ),
        corp,
        order(
            4,
            PAU,
            false,
            Money::from_ten_thousandths(610_000_000),
            (1, 1),
            NOW + 260_000,
        ),
    ]
}

/// Main's saved data with the jobs and orders given, fetched at the clock's time.
fn data_with(jobs: Option<Vec<Job>>, orders: Option<Vec<Order>>) -> CharacterData {
    CharacterData {
        character_id: MAIN,
        assets: None,
        orders: orders.map(|data| Fetched {
            data,
            fetched_at: NOW,
            last_modified: None,
            expires: None,
        }),
        jobs: jobs.map(|data| Fetched {
            data,
            fetched_at: NOW,
            last_modified: None,
            expires: None,
        }),
        wallet: None,
    }
}

/// The places file with one structure EVE refused, as the naming step writes it.
fn write_places(env: &Env) {
    let text = format!(
        "{{\"version\":1,\"structures\":{{\"{REFUSED}\":{{\"name\":null,\"system_id\":null,\"refused\":true,\"checked_at\":900000}}}}}}"
    );
    fs::write(esi_dir(env).join("places.json"), text).expect("places written");
}

/// A jEve profile folder with `#Default.db` built from `sql`.
fn jeve_folder(sql: &str) -> TempDir {
    let folder = TempDir::new().expect("a temp folder");
    let connection = Connection::open(folder.path().join("#Default.db")).expect("database");
    connection.execute_batch(sql).expect("fixture builds");
    drop(connection);
    folder
}

/// Alt (60 in Jita 4-4, updated 700,000) and Hauler (5 in Jita 4-4, never updated).
fn jeve_profile() -> TempDir {
    jeve_folder(
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
}

fn start(env: &Env, eve: FakeEve, clock: i64) -> Harness<'static, App> {
    let config = AppConfig {
        data_dir: env.data.clone(),
        cache_dir: env.cache.clone(),
        fetch: Box::new(NoNetwork),
        clock: Box::new(move || clock),
        eve: Box::new(eve),
        market: Box::new(crate::offline::Offline),
    };
    let mut harness = Harness::builder()
        .with_size(vec2(1280.0, 800.0))
        .with_max_steps(crate::SETTLE_STEPS)
        .build_eframe(move |cc| App::new(&cc.egui_ctx, config).expect("the app starts"));
    crate::ui::no_images(&mut harness);
    harness.state_mut().shared_mut().market = Arc::new(Prices::new(Box::new(EmptyForge)));
    harness
}

/// A harness whose orders read the Forge summary file.
fn start_with_forge(env: &Env, eve: FakeEve, clock: i64) -> Harness<'static, App> {
    let mut harness = start(env, eve, clock);
    harness.state_mut().shared_mut().market = Arc::new(Prices::new(Box::new(Forge)));
    harness
}

/// Asks the Forge for its file and runs frames until the file is held.
fn hold_forge(harness: &mut Harness<'_, App>) {
    let market = {
        let shared = harness.state_mut().shared_mut();
        let market = Arc::clone(&shared.market);
        market.refresh(&shared.worker, NOW, true);
        market
    };
    until(harness, "the Forge file", |_| {
        market.published_at().is_some()
    });
}

/// The text the last drawn frame put on the clipboard, if any.
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

/// The buttons labelled `Refresh all`, from the top of the window down.
fn refresh_buttons<'a>(harness: &'a Harness<'a, App>) -> Vec<egui_kittest::Node<'a>> {
    let mut buttons: Vec<egui_kittest::Node<'a>> = harness
        .get_all_by_label("Refresh all")
        .filter(|node| node.accesskit_node().role() == Role::Button)
        .collect();
    buttons.sort_by(|a, b| a.rect().center().y.total_cmp(&b.rect().center().y));
    buttons
}

/// The `explicit` marks of the price answers since the last call.
fn price_marks(harness: &mut Harness<'_, App>) -> Vec<bool> {
    harness
        .state_mut()
        .shared_mut()
        .market
        .take_outcomes()
        .into_iter()
        .map(|outcome| outcome.explicit)
        .collect()
}

/// Runs frames until the start-up character refresh and its price answer are over.
fn settle_start(harness: &mut Harness<'_, App>, refreshes: &Arc<Mutex<usize>>) {
    for _ in 0..500 {
        harness.step();
        let done = *refreshes.lock().unwrap_or_else(PoisonError::into_inner) >= 1
            && !harness.state_mut().shared_mut().market.running();
        if done {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("the start-up refresh did not finish");
}

/// The texts of the notices the app holds.
fn notices(harness: &mut Harness<'_, App>) -> Vec<String> {
    harness
        .state_mut()
        .shared_mut()
        .notices
        .iter()
        .map(|notice| notice.text.clone())
        .collect()
}

fn shown(harness: &Harness<'_, App>, text: &str) -> bool {
    harness.query_all_by_label(text).next().is_some()
}

/// Runs frames until `done` holds, letting the worker finish between them.
fn until(harness: &mut Harness<'_, App>, what: &str, done: impl Fn(&Harness<'_, App>) -> bool) {
    for _ in 0..500 {
        harness.run();
        if done(harness) {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("{what} did not show");
}

/// Waits until the start-up read has arrived: the Item panel stops saying it is loading.
fn loaded(harness: &mut Harness<'_, App>) {
    until(harness, "the saved data", |h| {
        !shown(h, "Loading character data…")
    });
}

/// Clicks the rail entry `label`. The rail is drawn first, so its entry is the first node with
/// that label.
fn rail(harness: &mut Harness<'_, App>, label: &str) {
    harness
        .get_all_by_label(label)
        .next()
        .expect("a rail entry")
        .click();
    harness.run();
}

/// Clicks the `pick`th node labelled `label`, so both panels' copies of a button can be tried.
fn click_nth(harness: &mut Harness<'_, App>, label: &str, pick: usize) {
    let rect = harness
        .query_all_by_label(label)
        .map(|node| node.rect())
        .nth(pick)
        .unwrap_or_else(|| panic!("no node {pick} labelled {label:?}"));
    {
        let node = harness
            .query_all_by_label(label)
            .find(|node| node.rect() == rect)
            .expect("the node");
        node.hover();
    }
    harness.run_steps(2);
    {
        let node = harness
            .query_all_by_label(label)
            .find(|node| node.rect() == rect)
            .expect("the node");
        node.click();
    }
    harness.run_steps(3);
}

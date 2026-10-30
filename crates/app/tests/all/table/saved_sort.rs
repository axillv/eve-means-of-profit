/// The saved sorts through a whole app: the Orders page's table, saved by its header and read
/// back by a new app on the same data folder.
use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};

use super::*;
use crate::ui::{click, shown, top, until};
use eve_means_of_profit::session::{EveAccess, PendingLogin};
use eve_means_of_profit::{App, AppConfig};
use mop_esi::character_data::{CharacterData, Fetched, Kind, Order, Refreshed};
use mop_esi::characters::{Character, Login};
use mop_esi::error::EsiError;
use mop_esi::places::{NamingReport, StructureNames};
use mop_esi::saved;
use mop_sde::{Fetch, FetchError, import};
use mop_store::Store;
use tempfile::TempDir;

const NOW: i64 = 1_000_000;
const MAIN: u64 = 2_112_000_001;
const JITA_4_4: i64 = 60_003_760;
const PAU: u32 = 11_688;
const MORPHITE: u32 = 11_399;
const FIXTURE: &str = "../sde/tests/fixtures/sde-3607705";

/// The setting key of the saved sort of the table named `table`.
fn key_of(table: &str) -> String {
    format!("table_sort.{:016x}", Id::new(table).value())
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

/// A login with the characters given; logins and refreshes are not started here.
struct FakeEve {
    characters: Vec<Character>,
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

    fn remove(&mut self, _character_id: u64) -> Result<(), EsiError> {
        Err(EsiError::UnknownCharacter(_character_id))
    }

    fn keychain_warning(&self) -> Option<String> {
        None
    }

    fn jeve_folder(&self) -> Option<PathBuf> {
        None
    }
}

/// A data folder with the game data imported and a user's store that checks no game data.
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
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE);
    import(&fixture, &data.join("sde.sqlite")).expect("the fixture imports");
    let mut store = Store::open(&data.join("user.sqlite"), NOW)
        .expect("the store opens")
        .store;
    let mut settings = store.settings().expect("settings");
    settings.last_sde_check = Some(NOW);
    store.save_settings(&settings).expect("settings saved");
    // The background price refresh stays off.
    let mut history = store.history_settings().expect("history settings");
    history.auto_refresh = false;
    store
        .save_history_settings(&history)
        .expect("history settings saved");
    Env {
        _dir: dir,
        data,
        cache,
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

/// Main's orders: 100 of 200 PAU for sale, 10 of 10 Morphite bought (expired), and 50 of
/// 50 PAU for sale by the corporation: PAU, Morphite, PAU in the given order.
fn main_data(fetched: i64) -> CharacterData {
    let mut corp = order(
        3,
        PAU,
        false,
        Money::from_ten_thousandths(700_000_000),
        (50, 50),
        NOW + 260_000,
    );
    corp.is_corporation = true;
    CharacterData {
        character_id: MAIN,
        assets: None,
        orders: Some(Fetched {
            data: vec![
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
                    NOW - 5,
                ),
                corp,
            ],
            fetched_at: fetched,
            last_modified: None,
            expires: None,
        }),
        jobs: None,
        wallet: None,
    }
}

fn start(env: &Env, characters: Vec<Character>) -> Harness<'static, App> {
    let config = AppConfig {
        data_dir: env.data.clone(),
        cache_dir: env.cache.clone(),
        fetch: Box::new(NoNetwork),
        clock: Box::new(|| NOW),
        eve: Box::new(FakeEve { characters }),
        market: Box::new(crate::offline::Offline),
    };
    Harness::builder()
        .with_size(vec2(1280.0, 800.0))
        .with_max_steps(crate::SETTLE_STEPS)
        .build_eframe(move |cc| App::new(&cc.egui_ctx, config).expect("the app starts"))
}

/// Runs frames until the start-up read has arrived: the page's loading line is gone.
fn loaded(harness: &mut Harness<'_, App>) {
    until(harness, "the saved data", |h| {
        !shown(h, "Loading character data…")
    });
}

fn setting(harness: &mut Harness<'_, App>, key: &str) -> Option<String> {
    harness
        .state_mut()
        .shared_mut()
        .store
        .setting(key)
        .expect("the setting reads")
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

#[test]
fn clicking_a_header_saves_the_sort_and_a_new_app_brings_it_back() {
    let env = env();
    saved::save_character(&env.data.join("esi"), &main_data(NOW - 120))
        .expect("Main's orders saved");
    let key = key_of("Orders");
    let mut harness = start(&env, vec![character(MAIN, "Main")]);
    loaded(&mut harness);
    click(&mut harness, "Orders");
    harness.run();
    // The given order is PAU, Morphite, PAU.
    assert!(top(&harness, "Particle Accelerator Unit") < top(&harness, "Morphite"));

    harness.get_by_label("Item").click();
    harness.run();
    assert_eq!(setting(&mut harness, &key).as_deref(), Some("item:asc"));
    assert!(top(&harness, "Morphite") < top(&harness, "Particle Accelerator Unit"));

    harness.get_by_label("Item").click();
    harness.run();
    assert_eq!(setting(&mut harness, &key).as_deref(), Some("item:desc"));
    assert!(top(&harness, "Particle Accelerator Unit") < top(&harness, "Morphite"));

    drop(harness);

    let mut restarted = start(&env, vec![character(MAIN, "Main")]);
    loaded(&mut restarted);
    click(&mut restarted, "Orders");
    restarted.run();
    assert!(
        top(&restarted, "Particle Accelerator Unit") < top(&restarted, "Morphite"),
        "the saved descending sort puts Morphite last with no click"
    );
}

#[test]
fn a_saved_sort_naming_a_missing_column_keeps_the_given_order_and_says_nothing() {
    let env = env();
    saved::save_character(&env.data.join("esi"), &main_data(NOW - 120))
        .expect("Main's orders saved");
    {
        let mut store = Store::open(&env.data.join("user.sqlite"), NOW)
            .expect("the store opens")
            .store;
        store
            .set_setting(&key_of("Orders"), "gone:asc")
            .expect("the sort saves");
    }
    let mut harness = start(&env, vec![character(MAIN, "Main")]);
    loaded(&mut harness);
    click(&mut harness, "Orders");
    harness.run();
    assert!(top(&harness, "Particle Accelerator Unit") < top(&harness, "Morphite"));
    let notices = notices(&mut harness);
    assert!(
        notices
            .iter()
            .all(|notice| !notice.contains("Could not read the settings")),
        "no settings notice: {notices:?}"
    );
}

//! Acceptance tests for the Characters page: the table of login characters, refresh, login,
//! remove, the keychain warning and the jEve panel. The login is a fake that records its calls;
//! jEve's profile is a made-up file in a temp folder. The clock is fixed at 1,000,000.
#[cfg(test)]
mod tests {

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
    use eve_means_of_profit::characters::{
        forget_state, set_client_id_present, set_opener, short_names,
    };
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

    const NOW: i64 = 1_000_000;
    const FIXTURE: &str = "../sde/tests/fixtures/sde-3607705";
    const MAIN: u64 = 2_112_000_001;
    const ALT: u64 = 2_112_000_002;
    const GAMMA: u64 = 2_112_000_004;
    const HOME_FORT: i64 = 1_035_466_617_946;
    const JITA_4_4: i64 = 60_003_760;
    /// The Particle Accelerator Unit: a type the game data names, holds in the fixture and builds.
    const PAU: u32 = 11_688;
    /// The blueprint the fixture's manufacturing job uses.
    const RIFTER_BLUEPRINT: u32 = 691;
    const RAM: u32 = 11_481;
    const KEYCHAIN: &str = "No system keychain found (no backend). Tokens are kept in a file.";
    /// The steps any wait in this file may run before it gives up. The app's jobs arrive in
    /// frames, and a machine that also runs other work draws those frames slowly; a generous
    /// budget keeps a slow machine from failing a test whose work still happens, while work
    /// that never happens still ends in the wait's own panic.
    const WAIT_STEPS: usize = 1_000;

    fn fixture() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE)
    }

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

    struct NoForge;

    impl BytesHttp for NoForge {
        fn get_bytes(
            &self,
            _url: &str,
            _if_none_match: Option<&str>,
        ) -> Result<BytesResponse, MarketError> {
            Err(MarketError::Network("not in these tests".to_owned()))
        }
    }

    /// An image server that never answers, so a test sees only the pictures it keeps itself.
    struct NoImages;

    impl images::Fetch for NoImages {
        fn get(&self, _url: &str) -> Result<Vec<u8>, String> {
            Err("offline".to_owned())
        }
    }

    struct Env {
        _dir: TempDir,
        data: PathBuf,
        cache: PathBuf,
    }

    fn env() -> Env {
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
        Env {
            _dir: dir,
            data,
            cache,
        }
    }

    fn esi_dir(env: &Env) -> PathBuf {
        env.data.join("esi")
    }

    type Results = Vec<(Kind, Result<Refreshed, EsiError>)>;
    /// What the fake's `refresh_all` does to the data and answers.
    type Script = fn(&mut CharacterData) -> Results;

    #[derive(Default)]
    struct Log {
        refreshed: Vec<u64>,
        named: Vec<(Vec<i64>, Vec<u64>)>,
        removed: Vec<u64>,
        opened: Vec<String>,
        begun: usize,
        keychain: usize,
    }

    #[derive(Clone)]
    struct FakeEve {
        characters: Vec<Character>,
        jeve: Option<PathBuf>,
        esi: PathBuf,
        script: Script,
        begin: Result<String, String>,
        /// Set to let the fake's login finish with Gamma.
        release: Arc<AtomicBool>,
        warning: Option<String>,
        failed_names: usize,
        /// While set, `refresh_all` waits.
        hold: Arc<AtomicBool>,
        /// The fake's login ends with `Cancelled` at once, though the player did not cancel.
        cancels_alone: bool,
        /// The fake's wait for the browser panics.
        panics: bool,
        log: Arc<Mutex<Log>>,
    }

    impl FakeEve {
        fn new(env: &Env, script: Script) -> Self {
            Self {
                characters: vec![character(MAIN, "Main", Login::Ok), alt()],
                jeve: None,
                esi: esi_dir(env),
                script,
                begin: Ok("https://login.example/start".to_owned()),
                release: Arc::new(AtomicBool::new(false)),
                warning: None,
                failed_names: 0,
                hold: Arc::new(AtomicBool::new(false)),
                cancels_alone: false,
                panics: false,
                log: Arc::new(Mutex::new(Log::default())),
            }
        }

        fn log<T>(&self, read: impl FnOnce(&Log) -> T) -> T {
            read(&self.log.lock().unwrap_or_else(PoisonError::into_inner))
        }

        fn write(&self, change: impl FnOnce(&mut Log)) {
            change(&mut self.log.lock().unwrap_or_else(PoisonError::into_inner));
        }
    }

    impl EveAccess for FakeEve {
        fn characters(&self) -> Vec<Character> {
            self.characters.clone()
        }

        fn begin_login(&mut self) -> Result<PendingLogin, String> {
            self.write(|log| log.begun += 1);
            let url = self.begin.clone()?;
            let release = Arc::clone(&self.release);
            let cancels_alone = self.cancels_alone;
            let panics = self.panics;
            Ok(PendingLogin::new(url, move |cancel| {
                if panics {
                    std::panic::resume_unwind(Box::new("the wait breaks"));
                }
                if cancels_alone {
                    return Err(EsiError::Cancelled);
                }
                loop {
                    if cancel.load(Ordering::Relaxed) {
                        return Err(EsiError::Cancelled);
                    }
                    if release.load(Ordering::Relaxed) {
                        return Ok("the code".to_owned());
                    }
                    thread::sleep(Duration::from_millis(5));
                }
            }))
        }

        fn finish_login(&mut self, code: &str) -> Result<Character, EsiError> {
            if code != "the code" {
                return Err(EsiError::StateMismatch);
            }
            Ok(character(GAMMA, "Gamma", Login::Ok))
        }

        fn refresh_all(&mut self, data: &mut CharacterData, _now: i64) -> Results {
            self.write(|log| log.refreshed.push(data.character_id));
            while self.hold.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_millis(5));
            }
            (self.script)(data)
        }

        fn name_structures(
            &mut self,
            names: &mut StructureNames,
            ids: &[i64],
            _now: i64,
        ) -> Result<NamingReport, EsiError> {
            let logins: Vec<u64> = self.characters.iter().map(|held| held.id).collect();
            self.write(|log| log.named.push((ids.to_vec(), logins)));
            for id in ids {
                names.insert(
                    *id,
                    StructureEntry {
                        name: Some("Home Fort".to_owned()),
                        system_id: None,
                        refused: false,
                        checked_at: NOW,
                    },
                );
            }
            Ok(NamingReport {
                asked: ids.len(),
                failed: (0..self.failed_names)
                    .map(|_| (HOME_FORT, EsiError::Network("down".to_owned())))
                    .collect(),
            })
        }

        fn save_character(&mut self, data: &CharacterData) -> Result<(), EsiError> {
            saved::save_character(&self.esi, data)
        }

        fn remove(&mut self, character_id: u64) -> Result<(), EsiError> {
            self.write(|log| log.removed.push(character_id));
            self.characters.retain(|held| held.id != character_id);
            Ok(())
        }

        fn keychain_warning(&self) -> Option<String> {
            self.write(|log| log.keychain += 1);
            self.warning.clone()
        }

        fn jeve_folder(&self) -> Option<PathBuf> {
            self.jeve.clone()
        }
    }

    fn character(id: u64, name: &str, login: Login) -> Character {
        Character {
            id,
            name: name.to_owned(),
            scopes: BTreeSet::new(),
            added_at: NOW,
            login,
        }
    }

    fn alt() -> Character {
        character(ALT, "Alt", Login::Expired)
    }

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

    fn wallet_fails(data: &mut CharacterData) -> Results {
        let mut results = partial(data);
        if let Some(last) = results.last_mut() {
            *last = (Kind::Wallet, Err(EsiError::Network("timed out".to_owned())));
        }
        results
    }

    /// EVE limits the assets only; the orders are fetched and the rest is not yet due.
    fn limited(data: &mut CharacterData) -> Results {
        data.orders = Some(held(Vec::new(), NOW, None));
        vec![
            (
                Kind::Assets,
                Err(EsiError::ErrorLimited { reset_seconds: 42 }),
            ),
            (Kind::Orders, Ok(Refreshed::Fetched)),
            (Kind::Jobs, Ok(Refreshed::NotYet { until: NOW + 90 })),
            (Kind::Wallet, Ok(Refreshed::NotYet { until: NOW + 90 })),
        ]
    }

    fn expired(_data: &mut CharacterData) -> Results {
        vec![(Kind::Assets, Err(EsiError::LoginExpired))]
    }

    /// Alt (updated 700,000) and Hauler (never updated), as the jEve profile of the pages tests.
    fn jeve_profile() -> TempDir {
        let folder = TempDir::new().expect("a temp folder");
        fill_jeve(folder.path());
        folder
    }

    /// A second profile, `Other.db`, holding only Zed (updated 900,000).
    fn fill_other(folder: &Path) {
        let connection = Connection::open(folder.join("Other.db")).expect("database");
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
                  ('j9', 2112000009, 'Zed', 1, 0, 900000000, 'esi-assets.read_assets.v1');",
            )
            .expect("fixture builds");
        drop(connection);
    }

    /// Saves the profile choice before the app starts.
    fn choose_profile(env: &Env, name: &str) {
        let mut store = Store::open(&env.data.join("user.sqlite"), NOW)
            .expect("the store opens")
            .store;
        store
            .set_setting("jeve.profile", name)
            .expect("the choice is saved");
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

    /// A jEve profile holding only Main, with one asset at Jita 4-4, updated 700,000 s ago (3d).
    fn fill_main_jeve(folder: &Path) {
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
                  ('j1', 2112000001, 'Main', 1, 0, 700000000, 'esi-assets.read_assets.v1');
                 INSERT INTO assets (accountid, itemid, typeid, locationid, flagid, flagstring, count, singleton, rawquantity) VALUES
                  ('j1', 7001, 11481, 60003760, 4, 'Hangar', 60, 0, NULL);",
            )
            .expect("fixture builds");
        drop(connection);
    }

    /// Main's sell order of the Particle Accelerator Unit at Jita 4-4, open for 3 more days.
    fn sell_order() -> Order {
        Order {
            order_id: 9_001,
            type_id: PAU,
            location_id: JITA_4_4,
            region_id: 10_000_002,
            is_buy: false,
            price: Money::from_ten_thousandths(615_000_000),
            volume_remain: 100,
            volume_total: 200,
            issued: 900_000,
            expires: NOW + 260_000,
            range: "station".to_owned(),
            escrow: None,
            min_volume: None,
            is_corporation: false,
        }
    }

    /// Main's active job, making one Particle Accelerator Unit at Home Fort.
    fn active_job() -> Job {
        Job {
            job_id: 5_001,
            activity_id: 1,
            blueprint_type_id: RIFTER_BLUEPRINT,
            product_type_id: Some(PAU),
            runs: 1,
            status: JobStatus::Active,
            start: NOW - 86_400,
            end: NOW + 7_500,
            facility_id: HOME_FORT,
            station_id: HOME_FORT,
            output_location_id: HOME_FORT,
            installer_id: i64::try_from(MAIN).expect("an id that fits"),
            cost: None,
            licensed_runs: None,
        }
    }

    /// Main's asset, order and job: a row for every table with a Character column. Nothing is
    /// fetched for the other characters, so their rows stay empty.
    fn tables(data: &mut CharacterData) -> Results {
        let found = data.character_id == MAIN;
        if found {
            let asset = Asset {
                item_id: 7,
                type_id: PAU,
                location_id: HOME_FORT,
                flag: Some("Hangar".to_owned()),
                quantity: 3,
                singleton: false,
            };
            data.assets = Some(held(owned(MAIN, vec![asset]), NOW, None));
            data.orders = Some(held(vec![sell_order()], NOW, None));
            data.jobs = Some(held(vec![active_job()], NOW, None));
        }
        Kind::ALL
            .into_iter()
            .map(|kind| {
                let answer = if found {
                    Ok(Refreshed::Fetched)
                } else {
                    Ok(Refreshed::NotYet { until: NOW + 90 })
                };
                (kind, answer)
            })
            .collect()
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

    /// Like [`open`], but saves no character file: the session holds only what the fake's login
    /// lists.
    fn open_without_saved(env: &Env, eve: &FakeEve) -> Harness<'static, App> {
        let mut harness = open_app(env, eve, true);
        drop(settle_start(&mut harness));
        clear_log(&mut harness, eve);
        harness
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

    /// Runs frames until `wanted` price answers have arrived, and returns their marks.
    fn settled_price_requests(harness: &mut Harness<'_, App>, wanted: usize) -> Vec<bool> {
        let mut all = Vec::new();
        for _ in 0..WAIT_STEPS {
            harness.step();
            all.extend(price_requests(harness));
            if all.len() >= wanted {
                return all;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("only {} price answers arrived, wanted {wanted}", all.len());
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

    /// Types into the text field that has this caption: clicks it, selects what it holds and types
    /// `text` over it.
    fn type_into(harness: &mut Harness<'_, App>, caption: &str, text: &str) {
        reveal_field(harness, caption);
        {
            let field = harness
                .query_all_by_label(caption)
                .find(|node| node.accesskit_node().role() == Role::TextInput)
                .unwrap_or_else(|| panic!("no text field captioned {caption}"));
            field.hover();
        }
        harness.run_steps(2);
        {
            let field = harness
                .query_all_by_label(caption)
                .find(|node| node.accesskit_node().role() == Role::TextInput)
                .unwrap_or_else(|| panic!("no text field captioned {caption}"));
            field.click();
        }
        harness.run_steps(2);
        harness.key_press_modifiers(Modifiers::COMMAND, Key::A);
        {
            let field = harness
                .query_all_by_label(caption)
                .find(|node| node.accesskit_node().role() == Role::TextInput)
                .unwrap_or_else(|| panic!("no text field captioned {caption}"));
            field.type_text(text);
        }
        harness.run_steps(3);
    }

    fn reveal_field(harness: &mut Harness<'_, App>, caption: &str) {
        {
            let field = harness
                .query_all_by_label(caption)
                .find(|node| node.accesskit_node().role() == Role::TextInput)
                .unwrap_or_else(|| panic!("no text field captioned {caption}"));
            field.scroll_to_me();
        }
        harness.run_steps(2);
    }

    /// The text the field with this caption holds.
    fn field_text(harness: &Harness<'_, App>, caption: &str) -> Option<String> {
        harness
            .query_all_by_label(caption)
            .find(|node| node.accesskit_node().role() == Role::TextInput)
            .and_then(|node| node.value())
    }

    /// Clicks the `index`th button labelled `label`, counting from the top of the page.
    fn click_nth(harness: &mut Harness<'_, App>, label: &str, index: usize) {
        {
            let target = harness
                .get_all_by_label(label)
                .filter(|node| node.accesskit_node().role() == Role::Button)
                .nth(index)
                .unwrap_or_else(|| panic!("no button labelled {label} number {index}"));
            target.click();
        }
        harness.run_steps(3);
    }

    /// Clicks the `index`th combo box that shows `value`, counting from the top of the page. A combo
    /// box has no label of its own, only the value it shows.
    fn click_combo(harness: &mut Harness<'_, App>, value: &str, index: usize) {
        let mut seen = 0;
        let found = reveal_combo_in(&harness.root(), value, index, &mut seen);
        assert!(found, "no combo box shows {value} number {index}");
        harness.run_steps(2);
        // A drop-down that was just closed by a pick is still open for a frame: move onto this one
        // and let that frame pass before the press, or the press only closes the old list.
        let mut seen = 0;
        hover_combo_in(&harness.root(), value, index, &mut seen);
        harness.run_steps(2);
        let mut seen = 0;
        click_combo_in(&harness.root(), value, index, &mut seen);
        harness.run_steps(3);
    }

    fn hover_combo_in(
        node: &egui_kittest::Node<'_>,
        value: &str,
        index: usize,
        seen: &mut usize,
    ) -> bool {
        let accesskit = node.accesskit_node();
        if accesskit.role() == Role::ComboBox && accesskit.value().as_deref() == Some(value) {
            if *seen == index {
                node.hover();
                return true;
            }
            *seen += 1;
        }
        node.children()
            .any(|child| hover_combo_in(&child, value, index, seen))
    }

    fn reveal_combo_in(
        node: &egui_kittest::Node<'_>,
        value: &str,
        index: usize,
        seen: &mut usize,
    ) -> bool {
        let accesskit = node.accesskit_node();
        if accesskit.role() == Role::ComboBox && accesskit.value().as_deref() == Some(value) {
            if *seen == index {
                node.scroll_to_me();
                return true;
            }
            *seen += 1;
        }
        node.children()
            .any(|child| reveal_combo_in(&child, value, index, seen))
    }

    fn click_combo_in(
        node: &egui_kittest::Node<'_>,
        value: &str,
        index: usize,
        seen: &mut usize,
    ) -> bool {
        let accesskit = node.accesskit_node();
        if accesskit.role() == Role::ComboBox && accesskit.value().as_deref() == Some(value) {
            if *seen == index {
                node.click();
                return true;
            }
            *seen += 1;
        }
        node.children()
            .any(|child| click_combo_in(&child, value, index, seen))
    }

    /// How many combo boxes show this value.
    fn combos_showing(harness: &Harness<'_, App>, value: &str) -> usize {
        let mut seen = 0;
        count_combos_in(&harness.root(), value, &mut seen);
        seen
    }

    fn count_combos_in(node: &egui_kittest::Node<'_>, value: &str, seen: &mut usize) {
        let accesskit = node.accesskit_node();
        if accesskit.role() == Role::ComboBox && accesskit.value().as_deref() == Some(value) {
            *seen += 1;
        }
        node.children()
            .for_each(|child| count_combos_in(&child, value, seen));
    }

    /// The accounts the app's own store holds.
    fn accounts_in(harness: &mut Harness<'_, App>) -> Vec<Account> {
        harness
            .state_mut()
            .shared_mut()
            .store
            .accounts()
            .expect("the accounts read")
    }

    /// Seeds an account through the store, with the characters given.
    fn seed_account(env: &Env, name: &str, characters: &[u64]) -> i64 {
        let mut store = Store::open(&env.data.join("user.sqlite"), NOW)
            .expect("the store opens")
            .store;
        let account = store.add_account(name).expect("the account");
        for character in characters {
            store
                .set_account(*character, Some(account))
                .expect("a character in it");
        }
        account
    }

    /// Seeds a PLEX count through the store.
    fn seed_plex(env: &Env, account: i64, count: u64, at: i64) {
        let mut store = Store::open(&env.data.join("user.sqlite"), NOW)
            .expect("the store opens")
            .store;
        store.set_plex(account, count, at).expect("the count");
    }

    /// The PLEX counts the app's own store holds for the account.
    fn plex_counts_in(harness: &mut Harness<'_, App>, account: i64) -> Vec<(i64, u64)> {
        harness
            .state_mut()
            .shared_mut()
            .store
            .plex_counts(account)
            .expect("the counts read")
    }

    fn clear_notices(harness: &mut Harness<'_, App>) {
        harness.state_mut().shared_mut().notices.clear();
    }

    /// Empties the text field that has this caption: selects all of it and deletes it.
    fn clear_field(harness: &mut Harness<'_, App>, caption: &str) {
        reveal_field(harness, caption);
        {
            let field = harness
                .query_all_by_label(caption)
                .find(|node| node.accesskit_node().role() == Role::TextInput)
                .unwrap_or_else(|| panic!("no text field captioned {caption}"));
            field.hover();
        }
        harness.run_steps(2);
        {
            let field = harness
                .query_all_by_label(caption)
                .find(|node| node.accesskit_node().role() == Role::TextInput)
                .unwrap_or_else(|| panic!("no text field captioned {caption}"));
            field.click();
        }
        harness.run_steps(2);
        harness.key_press_modifiers(Modifiers::COMMAND, Key::A);
        harness.key_press(Key::Backspace);
        harness.run_steps(3);
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
    fn rows_are_sorted_by_name_with_the_login_and_the_age_of_each_kind() {
        let env = env();
        // Nothing is due, so the start-up refresh leaves the saved ages as they are.
        let eve = FakeEve::new(&env, all_not_yet);
        let mut harness = open(&env, &eve);
        until(&mut harness, "the rows", |h| {
            shown(h, "login expired: log in again")
        });
        assert!(row_y(&harness, "Alt") < row_y(&harness, "Main"));
        assert!(shown(&harness, "ok"));
        assert!(shown(&harness, "Log in again"));
        for age in ["1h", "5m", "30s"] {
            assert_eq!(count(&harness, age), 1, "{age}");
        }
        // Alt has fetched nothing (four `never`); Main's jobs make the fifth.
        assert_eq!(count(&harness, "never"), 5);
    }

    #[test]
    fn each_age_and_the_login_carry_their_colour() {
        let style = Style::eve_dense();
        let assets = kind_view(Some((NOW - 4_000, Ok(NOW - 400))), NOW, &style);
        assert_eq!((assets.text.as_str(), assets.color), ("1h", style.stale));
        assert_eq!(assets.hover, "Can be refreshed now");
        let orders = kind_view(Some((NOW - 300, Ok(NOW + 900))), NOW, &style);
        assert_eq!((orders.text.as_str(), orders.color), ("5m", style.text));
        assert_eq!(orders.hover, "Next refresh allowed in 15m");
        let jobs = kind_view(None, NOW, &style);
        assert_eq!((jobs.text.as_str(), jobs.color), ("never", style.text));
        let wallet = kind_view(Some((NOW - 30, Ok(NOW + 90))), NOW, &style);
        assert_eq!((wallet.text.as_str(), wallet.color), ("30s", style.text));
        assert_eq!(login_view(false, &style), ("ok", style.text));
        assert_eq!(
            login_view(true, &style),
            ("login expired: log in again", style.neg)
        );
    }

    #[test]
    fn a_next_refresh_that_cannot_be_worked_out_shows_the_error() {
        let style = Style::eve_dense();
        let broken = kind_view(Some((NOW - 30, Err("bad time".to_owned()))), NOW, &style);
        assert_eq!(broken.text, "30s");
        assert_eq!(broken.color, style.neg);
        assert_eq!(
            broken.hover,
            "Could not work out the next refresh: bad time"
        );
    }

    #[test]
    fn hovering_an_age_tells_when_eve_allows_the_next_refresh() {
        let env = env();
        let eve = FakeEve::new(&env, partial);
        let mut harness = open(&env, &eve);
        until(&mut harness, "the rows", |h| shown(h, "5m"));
        harness
            .get_all_by_label("5m")
            .next()
            .expect("Main's orders age")
            .hover();
        frames(&mut harness);
        assert!(shown(&harness, "Next refresh allowed in 15m"));
    }

    #[test]
    fn the_session_load_refreshes_every_character_and_the_prices_once() {
        let env = env();
        let eve = FakeEve::new(&env, partial);
        let mut harness = open_raw(&env, &eve);
        assert_eq!(settle_start(&mut harness), vec![false]);
        assert_eq!(eve.log(|log| log.refreshed.clone()), vec![ALT, MAIN]);
        for _ in 0..20 {
            frames(&mut harness);
        }
        assert_eq!(eve.log(|log| log.refreshed.clone()), vec![ALT, MAIN]);
        assert!(price_requests(&mut harness).is_empty());
    }

    #[test]
    fn the_status_line_and_the_page_each_refresh_all() {
        let env = env();
        let eve = FakeEve::new(&env, partial);
        let mut harness = open(&env, &eve);
        assert_eq!(refresh_buttons(&harness).len(), 2);

        // The status line's button refreshes the characters and the prices, explicitly.
        press(&mut harness, "Refresh all", 1);
        assert_eq!(eve.log(|log| log.refreshed.clone()), vec![ALT, MAIN]);
        assert_eq!(settled_price_requests(&mut harness, 1), vec![true]);

        // The page's own button stays character-only.
        press(&mut harness, "Refresh all", 0);
        assert_eq!(
            eve.log(|log| log.refreshed.clone()),
            vec![ALT, MAIN, ALT, MAIN]
        );
        assert!(price_requests(&mut harness).is_empty());
    }

    #[test]
    fn refresh_all_without_characters_still_asks_for_prices() {
        let env = env();
        let mut eve = FakeEve::new(&env, partial);
        eve.characters.clear();
        // No character file is saved either, so nothing outside the fake's login can add one.
        let mut harness = open_without_saved(&env, &eve);
        // The login itself is empty, not just the fake's whitelist.
        assert!(
            harness
                .state_mut()
                .shared_mut()
                .session
                .characters()
                .is_empty()
        );
        assert!(eve.log(|log| log.refreshed.is_empty()));
        drop(price_requests(&mut harness));

        press(&mut harness, "Refresh all", 1);
        assert!(eve.log(|log| log.refreshed.is_empty()));
        assert_eq!(settled_price_requests(&mut harness, 1), vec![true]);
    }

    #[test]
    fn a_press_while_a_character_refresh_runs_starts_no_second_job() {
        let env = env();
        let eve = FakeEve::new(&env, partial);
        let mut harness = open(&env, &eve);
        eve.hold.store(true, Ordering::Relaxed);

        refresh_buttons(&harness)
            .into_iter()
            .nth(1)
            .expect("the status line's button")
            .click();
        until(&mut harness, "the first character", |_| {
            eve.log(|log| log.refreshed.len()) == 1
        });

        refresh_buttons(&harness)
            .into_iter()
            .nth(1)
            .expect("the status line's button")
            .click();
        for _ in 0..10 {
            harness.step();
        }
        assert_eq!(eve.log(|log| log.refreshed.len()), 1);

        eve.hold.store(false, Ordering::Relaxed);
        until(&mut harness, "the refresh", |h| !disabled(h, "Refresh all"));
        assert_eq!(eve.log(|log| log.refreshed.clone()), vec![ALT, MAIN]);
    }

    #[test]
    fn refresh_fetches_one_character_names_its_structures_and_saves() {
        let env = env();
        let eve = FakeEve::new(&env, partial);
        let mut harness = open(&env, &eve);
        let (main_before, alt_before) = {
            let session = &harness.state_mut().shared_mut().session;
            (session.revision(MAIN), session.revision(ALT))
        };
        press(&mut harness, "Refresh", 1);
        assert_eq!(eve.log(|log| log.refreshed.clone()), vec![MAIN]);
        assert_eq!(
            eve.log(|log| log.named.clone()),
            vec![(vec![HOME_FORT], vec![MAIN, ALT])]
        );
        let saved_main = saved::load_character(&esi_dir(&env), MAIN)
            .expect("the file reads")
            .expect("Main's file");
        assert!(saved_main.jobs.is_some());
        let session = &harness.state_mut().shared_mut().session;
        assert_eq!(session.revision(MAIN), main_before + 1);
        assert_eq!(session.revision(ALT), alt_before);
        let named = session
            .structures()
            .get(&HOME_FORT)
            .and_then(|entry| entry.name.clone());
        assert_eq!(named.as_deref(), Some("Home Fort"));
        assert!(notices(&mut harness).is_empty());
    }

    #[test]
    fn refresh_all_with_nothing_due_replaces_nothing_and_says_when() {
        let env = env();
        let eve = FakeEve::new(&env, all_not_yet);
        let mut harness = open(&env, &eve);
        clear_notices(&mut harness);
        // The page's own button is the first one: the status line draws one below the page.
        press(&mut harness, "Refresh all", 0);
        assert_eq!(eve.log(|log| log.refreshed.clone()), vec![ALT, MAIN]);
        assert!(price_requests(&mut harness).is_empty());
        let session = &harness.state_mut().shared_mut().session;
        assert_eq!(session.revision(MAIN), 0);
        assert_eq!(session.revision(ALT), 0);
        assert_eq!(
            notices(&mut harness),
            vec!["Nothing to refresh yet: EVE allows it again in 1m.".to_owned()]
        );
    }

    #[test]
    fn a_failed_kind_is_named_and_the_others_are_kept() {
        let env = env();
        let eve = FakeEve::new(&env, wallet_fails);
        let mut harness = open(&env, &eve);
        clear_notices(&mut harness);
        let before = harness.state_mut().shared_mut().session.revision(MAIN);
        press(&mut harness, "Refresh", 1);
        assert_eq!(
            notices(&mut harness),
            vec![format!(
                "Could not refresh Main wallet: {}",
                EsiError::Network("timed out".to_owned())
            )]
        );
        let session = &harness.state_mut().shared_mut().session;
        assert_eq!(session.revision(MAIN), before + 1);
        assert!(
            session
                .character_data(MAIN)
                .is_some_and(|data| data.jobs.is_some())
        );
    }

    #[test]
    fn an_error_limit_gives_one_notice_for_the_character() {
        let env = env();
        let eve = FakeEve::new(&env, limited);
        let mut harness = open(&env, &eve);
        clear_notices(&mut harness);
        press(&mut harness, "Refresh", 1);
        assert_eq!(
            notices(&mut harness),
            vec!["Could not refresh Main: EVE asks to wait 42 s.".to_owned()]
        );
    }

    #[test]
    fn an_expired_login_found_on_refresh_is_marked() {
        let env = env();
        let eve = FakeEve::new(&env, expired);
        let mut harness = open(&env, &eve);
        clear_notices(&mut harness);
        press(&mut harness, "Refresh", 1);
        assert_eq!(
            notices(&mut harness),
            vec!["Could not refresh Main: login expired, log in again.".to_owned()]
        );
        until(&mut harness, "Main's login", |h| {
            count(h, "Log in again") == 2
        });
    }

    #[test]
    fn failing_to_name_structures_keeps_the_refresh() {
        let env = env();
        let mut eve = FakeEve::new(&env, partial);
        eve.failed_names = 1;
        let mut harness = open(&env, &eve);
        clear_notices(&mut harness);
        let before = harness.state_mut().shared_mut().session.revision(MAIN);
        press(&mut harness, "Refresh", 1);
        assert_eq!(
            notices(&mut harness),
            vec!["Could not get 1 structure names from EVE. The last names are kept.".to_owned()]
        );
        let session = &harness.state_mut().shared_mut().session;
        assert_eq!(session.revision(MAIN), before + 1);
    }

    #[test]
    fn add_character_opens_the_browser_and_adds_the_character_that_logs_in() {
        let env = env();
        let eve = FakeEve::new(&env, partial);
        let mut harness = open(&env, &eve);
        harness.get_by_label("Add character").click();
        frames(&mut harness);
        until(&mut harness, "the waiting text", |h| {
            shown(h, "Waiting for the EVE login in your browser…")
        });
        assert_eq!(
            eve.log(|log| log.opened.clone()),
            vec!["https://login.example/start".to_owned()]
        );
        assert!(disabled(&harness, "Add character"));
        harness.get_by_label("Add character").hover();
        frames(&mut harness);
        assert!(shown(&harness, "Busy: waiting for the EVE login"));
        eve.release.store(true, Ordering::Relaxed);
        until(&mut harness, "Gamma's row", |h| shown(h, "Gamma"));
        until(&mut harness, "Gamma's refresh", |h| {
            !disabled(h, "Refresh all")
        });
        assert_eq!(eve.log(|log| log.refreshed.clone()), vec![GAMMA]);
        assert!(!shown(
            &harness,
            "Waiting for the EVE login in your browser…"
        ));
    }

    #[test]
    fn cancel_while_waiting_is_a_notice_and_not_an_error() {
        let env = env();
        let eve = FakeEve::new(&env, partial);
        let mut harness = open(&env, &eve);
        harness.get_by_label("Add character").click();
        frames(&mut harness);
        until(&mut harness, "the waiting text", |h| shown(h, "Cancel"));
        harness.get_by_label("Cancel").click();
        until_notice(&mut harness);
        let held = &harness.state_mut().shared_mut().notices;
        assert_eq!(held.len(), 1);
        assert_eq!(
            held.first().map(|n| n.text.as_str()),
            Some("Login cancelled.")
        );
        assert_eq!(held.first().map(|n| n.level), Some(Level::Notice));
        assert!(eve.log(|log| log.refreshed.is_empty()));
    }

    #[test]
    fn a_login_that_cannot_start_is_an_error() {
        let env = env();
        let mut eve = FakeEve::new(&env, partial);
        eve.begin = Err("This build has no EVE login yet.".to_owned());
        let mut harness = open(&env, &eve);
        harness.get_by_label("Add character").click();
        until_notice(&mut harness);
        let held = &harness.state_mut().shared_mut().notices;
        assert_eq!(
            held.first().map(|n| n.text.as_str()),
            Some("Could not log in: This build has no EVE login yet.")
        );
        assert_eq!(held.first().map(|n| n.level), Some(Level::Error));
    }

    #[test]
    fn without_a_client_id_both_login_buttons_are_disabled() {
        let env = env();
        let eve = FakeEve::new(&env, partial);
        let mut harness = open(&env, &eve);
        set_client_id_present(&harness.ctx, false);
        frames(&mut harness);
        frames(&mut harness);
        assert!(disabled(&harness, "Add character"));
        assert!(disabled(&harness, "Log in again"));
        assert!(!disabled(&harness, "Refresh all"));
        harness.get_by_label("Add character").hover();
        frames(&mut harness);
        assert!(shown(&harness, "This build has no EVE login yet."));
    }

    /// Presses the first `Remove…` button through its accesskit action: the row's action buttons
    /// sit past the right edge of the window once the Short name column is drawn, and the action
    /// clicks a button that is scrolled out of view.
    fn press_remove(harness: &mut Harness<'_, App>) {
        harness
            .get_all_by_label("Remove…")
            .next()
            .expect("a Remove… button")
            .click_accesskit();
    }

    #[test]
    fn remove_asks_first_and_forgets_only_that_character() {
        let env = env();
        let eve = FakeEve::new(&env, partial);
        let mut harness = open(&env, &eve);
        press_remove(&mut harness);
        frames(&mut harness);
        let ask = "Remove Alt? Its saved data is deleted. Net worth history is kept.";
        assert!(shown(&harness, ask));
        harness.get_by_label("Keep").click();
        frames(&mut harness);
        assert!(!shown(&harness, ask));
        assert!(eve.log(|log| log.removed.is_empty()));
        assert!(shown(&harness, "Alt"));

        press_remove(&mut harness);
        frames(&mut harness);
        harness.get_by_label("Remove").click();
        until(&mut harness, "Alt's row to go", |h| !shown(h, "Alt"));
        assert_eq!(eve.log(|log| log.removed.clone()), vec![ALT]);
        let session = &harness.state_mut().shared_mut().session;
        assert!(session.character_data(MAIN).is_some());
        assert_eq!(session.characters().len(), 1);
    }

    #[test]
    fn the_keychain_warning_is_a_notice_once_and_stays_above_the_table() {
        let env = env();
        let mut eve = FakeEve::new(&env, partial);
        eve.warning = Some(KEYCHAIN.to_owned());
        let mut harness = open(&env, &eve);
        until_notice(&mut harness);
        assert_eq!(notices(&mut harness), vec![KEYCHAIN.to_owned()]);
        // The notice can arrive one frame before the table draws its warning line.
        until(&mut harness, "the warning above the table", |h| {
            let Some(main) = h.get_all_by_label("Main").next() else {
                return false;
            };
            let main_y = main.rect().center().y;
            h.query_all_by_label(KEYCHAIN)
                .any(|node| node.rect().center().y < main_y)
        });
        let main_y = row_y(&harness, "Main");
        assert!(
            harness
                .query_all_by_label(KEYCHAIN)
                .any(|node| node.rect().center().y < main_y)
        );
        harness.state_mut().shared_mut().notices.clear();
        rail(&mut harness, "Build");
        rail(&mut harness, "Characters");
        for _ in 0..5 {
            frames(&mut harness);
        }
        assert!(notices(&mut harness).is_empty());
        assert!(shown(&harness, KEYCHAIN));
    }

    #[test]
    fn no_characters_says_how_to_add_one() {
        let env = env();
        let mut eve = FakeEve::new(&env, partial);
        eve.characters.clear();
        let harness = open(&env, &eve);
        assert!(shown(
            &harness,
            "No characters yet. Add character opens the EVE login in your browser."
        ));
    }

    #[test]
    fn the_jeve_panel_lists_its_characters_and_whether_the_app_uses_them() {
        let env = env();
        let jeve = jeve_profile();
        // Nothing is due, so the start-up refresh leaves Alt's jEve data in use until the test
        // replaces it below.
        let mut eve = FakeEve::new(&env, all_not_yet);
        eve.jeve = Some(jeve.path().to_path_buf());
        let mut harness = open(&env, &eve);
        until(&mut harness, "the jEve rows", |h| shown(h, "Hauler"));
        let on_row = |h: &Harness<'_, App>, anchor: &str, text: &str| {
            let y = h
                .get_all_by_label(anchor)
                .last()
                .expect("an anchor")
                .rect()
                .center()
                .y;
            h.query_all_by_label(text)
                .any(|node| (node.rect().center().y - y).abs() < 2.0)
        };
        assert!(on_row(&harness, "Alt", "3d"));
        assert!(on_row(&harness, "Alt", "yes"));
        assert!(on_row(&harness, "Hauler", "never"));
        assert!(on_row(&harness, "Hauler", "yes"));

        let alt_assets = CharacterData {
            character_id: ALT,
            assets: Some(held(owned(ALT, Vec::new()), NOW, None)),
            orders: None,
            jobs: None,
            wallet: None,
        };
        harness.state_mut().shared_mut().session.replace(alt_assets);
        frames(&mut harness);
        assert!(on_row(&harness, "Alt", "3d"));
        assert!(on_row(&harness, "Alt", "no: logged in to the app"));

        fs::remove_file(jeve.path().join("#Default.db")).expect("the file goes");
        harness.get_by_label("Read again").click();
        let message = format!("No jEve data file found at {}.", jeve.path().display());
        until(&mut harness, "the message", |h| shown(h, &message));
        assert!(!shown(&harness, "Hauler"));
        // Only Alt's login row is left; the jEve row and its age are gone.
        assert_eq!(count(&harness, "Alt"), 1);
        assert!(!shown(&harness, "3d"));
        assert!(harness.state_mut().shared_mut().session.jeve().is_none());
    }

    #[test]
    fn a_jeve_read_during_a_login_wait_does_not_wait_for_the_login() {
        let env = env();
        let jeve = jeve_profile();
        let mut eve = FakeEve::new(&env, partial);
        eve.jeve = Some(jeve.path().to_path_buf());
        let mut harness = open(&env, &eve);
        until(&mut harness, "the jEve rows", |h| shown(h, "Hauler"));
        harness.get_by_label("Add character").click();
        until(&mut harness, "the waiting text", |h| {
            shown(h, "Waiting for the EVE login in your browser…")
        });
        let slot = harness.state_mut().shared_mut().session.eve();
        fs::remove_file(jeve.path().join("#Default.db")).expect("the file goes");
        harness.get_by_label("Read again").click();
        let message = format!("No jEve data file found at {}.", jeve.path().display());
        until(&mut harness, "the message", |h| shown(h, &message));
        assert!(shown(
            &harness,
            "Waiting for the EVE login in your browser…"
        ));
        // The wait holds no lock on the login.
        drop(slot.try_lock().expect("the login is free"));
        eve.release.store(true, Ordering::Relaxed);
        until(&mut harness, "Gamma's row", |h| shown(h, "Gamma"));
        // The real login is back in its slot afterwards.
        assert_eq!(lock_eve(&slot).characters().len(), 2);
    }

    #[test]
    fn the_saved_profile_is_read_and_a_missing_one_falls_back_to_the_default() {
        let env = env();
        let jeve = jeve_profile();
        fill_other(jeve.path());
        choose_profile(&env, "Other");
        let mut eve = FakeEve::new(&env, partial);
        eve.jeve = Some(jeve.path().to_path_buf());
        let mut harness = open(&env, &eve);
        until(&mut harness, "Zed", |h| shown(h, "Zed"));
        assert!(!shown(&harness, "Hauler"));

        let env = self::env();
        choose_profile(&env, "Gone");
        let mut eve = FakeEve::new(&env, partial);
        eve.jeve = Some(jeve.path().to_path_buf());
        let mut harness = open(&env, &eve);
        until(&mut harness, "Hauler", |h| shown(h, "Hauler"));
        frames(&mut harness);
        assert!(!shown(&harness, "Zed"));
    }

    #[test]
    fn picking_a_profile_reads_it_and_keeps_the_choice() {
        let env = env();
        let jeve = jeve_profile();
        fill_other(jeve.path());
        let mut eve = FakeEve::new(&env, partial);
        eve.jeve = Some(jeve.path().to_path_buf());
        let mut harness = open(&env, &eve);
        until(&mut harness, "Hauler", |h| shown(h, "Hauler"));
        // The Characters panel's account dropdowns come first in the tree; the jEve panel's
        // profile dropdown is the last one.
        harness
            .query_all(by().role(Role::ComboBox))
            .last()
            .expect("the profile dropdown")
            .click();
        frames(&mut harness);
        harness.get_by_label("Other").click();
        until(&mut harness, "Zed", |h| shown(h, "Zed"));
        let kept = harness
            .state_mut()
            .shared_mut()
            .store
            .setting("jeve.profile")
            .expect("the setting reads");
        assert_eq!(kept.as_deref(), Some("Other"));
    }

    #[test]
    fn a_cancel_the_player_did_not_cause_is_an_error() {
        let env = env();
        let mut eve = FakeEve::new(&env, partial);
        eve.cancels_alone = true;
        let mut harness = open(&env, &eve);
        harness.get_by_label("Add character").click();
        until_notice(&mut harness);
        let held = &harness.state_mut().shared_mut().notices;
        assert_eq!(
            held.first().map(|n| n.text.clone()),
            Some(format!("Could not log in: {}", EsiError::Cancelled))
        );
        assert_eq!(held.first().map(|n| n.level), Some(Level::Error));
    }

    #[test]
    fn a_browser_that_will_not_open_is_an_error_and_the_login_goes_on() {
        let env = env();
        let eve = FakeEve::new(&env, partial);
        let mut harness = open(&env, &eve);
        set_opener(&harness.ctx, Arc::new(|_| Err("no browser".to_owned())));
        harness.get_by_label("Add character").click();
        until_notice(&mut harness);
        let held = &harness.state_mut().shared_mut().notices;
        assert_eq!(
            held.first().map(|n| n.text.as_str()),
            Some(
                "Could not open the browser (no browser). Open this address yourself: https://login.example/start"
            )
        );
        assert_eq!(held.first().map(|n| n.level), Some(Level::Error));
        frames(&mut harness);
        assert!(shown(
            &harness,
            "Waiting for the EVE login in your browser…"
        ));
        harness.get_by_label("Cancel").click();
        until(&mut harness, "the end of the wait", |h| {
            !shown(h, "Waiting for the EVE login in your browser…")
        });
    }

    #[test]
    fn the_buttons_say_what_is_running_while_it_runs() {
        let env = env();
        let eve = FakeEve::new(&env, partial);
        let mut harness = open(&env, &eve);
        eve.hold.store(true, Ordering::Relaxed);
        harness
            .get_all_by_label("Refresh")
            .nth(1)
            .expect("Main's button")
            .click();
        frames(&mut harness);
        assert!(disabled(&harness, "Refresh all"));
        refresh_buttons(&harness)
            .into_iter()
            .next()
            .expect("the page's button")
            .hover();
        frames(&mut harness);
        assert!(shown(&harness, "Busy: refreshing Main"));
        eve.hold.store(false, Ordering::Relaxed);
        until(&mut harness, "the refresh", |h| !disabled(h, "Refresh all"));
    }

    #[test]
    fn this_build_has_an_eve_login_and_the_page_knows_it_without_a_hook() {
        let env = env();
        let eve = FakeEve::new(&env, partial);
        let mut harness = open_with(&env, &eve, false);
        frames(&mut harness);
        assert!(!disabled(&harness, "Add character"));
        assert!(!disabled(&harness, "Log in again"));
        harness.get_by_label("Add character").hover();
        frames(&mut harness);
        assert!(!shown(&harness, "This build has no EVE login yet."));
        assert_eq!(eve.log(|log| log.begun), 0);
    }

    /// Alt, Hauler and `corporations` corporations (owners whose scopes hold the corporation
    /// roles).
    fn jeve_with_corporations(corporations: usize) -> TempDir {
        let folder = jeve_profile();
        let connection = Connection::open(folder.path().join("#Default.db")).expect("database");
        for number in 0..corporations {
            connection
                .execute(
                    "INSERT INTO owners (accountid, ownerid, name, show, invalid, scopes) \
                     VALUES (?1, ?2, 'Corp', 1, 0, 'esi-characters.read_corporation_roles.v1')",
                    (
                        format!("c{number}"),
                        98_000_000 + i64::try_from(number).expect("small"),
                    ),
                )
                .expect("a corporation");
        }
        drop(connection);
        folder
    }

    #[test]
    fn corporations_left_out_are_counted_in_the_singular_and_the_plural() {
        for (corporations, line) in [
            (1, "1 corporation left out"),
            (2, "2 corporations left out"),
        ] {
            let env = env();
            let jeve = jeve_with_corporations(corporations);
            let mut eve = FakeEve::new(&env, partial);
            eve.jeve = Some(jeve.path().to_path_buf());
            let mut harness = open(&env, &eve);
            until(&mut harness, "the line", |h| shown(h, line));
        }
        let env = env();
        let jeve = jeve_profile();
        let mut eve = FakeEve::new(&env, partial);
        eve.jeve = Some(jeve.path().to_path_buf());
        let mut harness = open(&env, &eve);
        until(&mut harness, "Hauler", |h| shown(h, "Hauler"));
        assert!(
            harness
                .query_all_by_label_contains("left out")
                .next()
                .is_none()
        );
    }

    #[test]
    fn without_a_jeve_folder_the_panel_says_there_is_no_file() {
        let env = env();
        let eve = FakeEve::new(&env, partial);
        let mut harness = open(&env, &eve);
        until(&mut harness, "the message", |h| {
            shown(h, "No jEve data file found.")
        });
        assert!(shown(&harness, "Read again"));
    }

    /// Starts a login that waits, then makes the page forget its state, so that its first
    /// listing of jEve's folder and its first ask for the keychain warning happen during the wait.
    fn first_show_during_a_login(eve: &FakeEve, env: &Env) -> Harness<'static, App> {
        let mut harness = open(env, eve);
        if eve.warning.is_some() {
            // The start-up ask may still be on its way; let it land before the state is forgotten.
            until(&mut harness, "the keychain notice", |h| shown(h, KEYCHAIN));
        }
        harness.get_by_label("Add character").click();
        until(&mut harness, "the waiting text", |h| {
            shown(h, "Waiting for the EVE login in your browser…")
        });
        forget_state(&harness.ctx);
        harness
    }

    #[test]
    fn the_jeve_panel_listed_during_a_login_wait_finds_the_real_folder() {
        let env = env();
        let jeve = jeve_profile();
        let mut eve = FakeEve::new(&env, partial);
        eve.jeve = Some(jeve.path().to_path_buf());
        let mut harness = first_show_during_a_login(&eve, &env);
        let file = jeve.path().join("#Default.db");
        let line = format!("File: {}", file.display());
        until(&mut harness, "the file line", |h| shown(h, &line));
        assert!(!shown(&harness, "No jEve data file found."));
        eve.release.store(true, Ordering::Relaxed);
    }

    #[test]
    fn the_keychain_warning_asked_during_a_login_wait_is_shown() {
        let env = env();
        let mut eve = FakeEve::new(&env, partial);
        eve.warning = Some(KEYCHAIN.to_owned());
        let mut harness = first_show_during_a_login(&eve, &env);
        // The first show gave one notice; the second, during the wait, gives another.
        for _ in 0..WAIT_STEPS {
            frames(&mut harness);
            if notices(&mut harness).len() >= 2 {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            notices(&mut harness),
            vec![KEYCHAIN.to_owned(), KEYCHAIN.to_owned()],
            "keychain asks: {}",
            eve.log(|log| log.keychain)
        );
        frames(&mut harness);
        assert!(shown(&harness, KEYCHAIN));
        eve.release.store(true, Ordering::Relaxed);
    }

    #[test]
    fn a_login_that_panics_ends_the_wait_with_an_error() {
        let env = env();
        let mut eve = FakeEve::new(&env, partial);
        eve.panics = true;
        let mut harness = open(&env, &eve);
        harness.get_by_label("Add character").click();
        until_notice(&mut harness);
        frames(&mut harness);
        let held = &harness.state_mut().shared_mut().notices;
        assert_eq!(
            held.first().map(|n| n.text.as_str()),
            Some("Could not log in: the login stopped unexpectedly.")
        );
        assert_eq!(held.first().map(|n| n.level), Some(Level::Error));
        assert!(!shown(
            &harness,
            "Waiting for the EVE login in your browser…"
        ));
        assert!(!shown(&harness, "Cancel"));
        assert!(!disabled(&harness, "Add character"));
        assert!(!disabled(&harness, "Refresh all"));
    }

    #[test]
    fn a_fresh_store_shows_no_account_and_a_new_account_empties_the_field() {
        let env = env();
        let eve = FakeEve::new(&env, partial);
        let mut harness = open(&env, &eve);
        until(&mut harness, "the rows", |h| shown(h, "5m"));
        // Both rows say (none) in the Account column.
        assert_eq!(combos_showing(&harness, "(none)"), 2);
        type_into(&mut harness, "New account", " Main acct ");
        press(&mut harness, "Add account", 0);
        let accounts = accounts_in(&mut harness);
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].name, "Main acct");
        assert!(accounts[0].characters.is_empty());
        assert_eq!(field_text(&harness, "New account").as_deref(), Some(""));
        // Both dropdowns still say (none), and both list the new account.
        assert_eq!(combos_showing(&harness, "(none)"), 2);
        click_combo(&mut harness, "(none)", 0);
        assert!(shown(&harness, "(none)"));
        assert!(shown(&harness, "Main acct"));
    }

    #[test]
    fn picking_an_account_for_a_character_stores_it_and_a_new_app_shows_the_same() {
        let env = env();
        let eve = FakeEve::new(&env, partial);
        let mut harness = open(&env, &eve);
        type_into(&mut harness, "New account", "Main acct");
        press(&mut harness, "Add account", 0);
        // Rows sort by name, so Alt's dropdown is the first one saying (none).
        click_combo(&mut harness, "(none)", 0);
        click_nth(&mut harness, "Main acct", 0);
        until(&mut harness, "Alt's account", |h| {
            combos_showing(h, "Main acct") == 1
        });
        assert_eq!(combos_showing(&harness, "(none)"), 1);
        let accounts = accounts_in(&mut harness);
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].name, "Main acct");
        assert_eq!(accounts[0].characters, vec![ALT]);

        // A new app on the same data folder shows the same.
        let mut again = open(&env, &eve);
        until(&mut again, "the accounts", |h| {
            combos_showing(h, "Main acct") == 1
        });
        assert_eq!(combos_showing(&again, "(none)"), 1);

        // Picking (none) for Alt empties the account.
        click_combo(&mut harness, "Main acct", 0);
        click_nth(&mut harness, "(none)", 0);
        until(&mut harness, "the account to go", |h| {
            combos_showing(h, "(none)") == 2
        });
        assert!(accounts_in(&mut harness)[0].characters.is_empty());
    }

    #[test]
    fn a_taken_or_blank_account_name_is_an_error_and_stores_nothing() {
        let env = env();
        let eve = FakeEve::new(&env, partial);
        let mut harness = open(&env, &eve);
        type_into(&mut harness, "New account", " Main acct ");
        press(&mut harness, "Add account", 0);
        type_into(&mut harness, "New account", "Main acct");
        press(&mut harness, "Add account", 0);
        type_into(&mut harness, "New account", "  ");
        press(&mut harness, "Add account", 0);
        assert_eq!(
            notices(&mut harness),
            vec![
                "An account called \"Main acct\" already exists.".to_owned(),
                "Type a name for the account.".to_owned(),
            ]
        );
        let accounts = accounts_in(&mut harness);
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].name, "Main acct");
        assert!(accounts[0].characters.is_empty());
    }

    #[test]
    fn an_account_with_three_other_characters_is_listed_but_disabled() {
        let env = env();
        let eve = FakeEve::new(&env, partial);
        let mut harness = open(&env, &eve);
        let full = {
            let store = &mut harness.state_mut().shared_mut().store;
            let full = store.add_account("Full").expect("the account");
            for character in [901, 902, 903] {
                store
                    .set_account(character, Some(full))
                    .expect("a character in it");
            }
            full
        };
        frames(&mut harness);
        // Rows sort by name, so Main's dropdown is the second one saying (none).
        click_combo(&mut harness, "(none)", 1);
        assert!(disabled(&harness, "Full"));
        harness
            .get_all_by_label("Full")
            .find(|node| node.accesskit_node().role() == Role::Button)
            .expect("the entry")
            .hover();
        frames(&mut harness);
        assert!(shown(&harness, "This account already has 3 characters."));

        // A character already in the account is allowed to stay in it: one of the three moves out
        // first, or a fourth would be refused.
        {
            let store = &mut harness.state_mut().shared_mut().store;
            store.set_account(903, None).expect("903 out");
            store.set_account(ALT, Some(full)).expect("Alt in Full");
        }
        frames(&mut harness);
        // Alt's dropdown now shows Full, and its Full entry is enabled.
        click_combo(&mut harness, "Full", 0);
        assert!(!disabled(&harness, "Full"));
    }

    #[test]
    fn plex_counts_are_grouped_and_dates_read_both_ways() {
        assert_eq!(plex_text(0), "0");
        assert_eq!(plex_text(999), "999");
        assert_eq!(plex_text(1_250), "1,250");
        assert_eq!(plex_text(1_234_567), "1,234,567");
        assert_eq!(parse_date("1970-01-10"), Some(9));
        assert_eq!(parse_date("1970-02-30"), None);
        assert_eq!(parse_date("10 Jan"), None);
        assert_eq!(date_text(11), "1970-01-12");
    }

    #[test]
    fn an_account_block_lists_its_characters_and_the_date_starts_today() {
        let env = env();
        seed_account(&env, "Main acct", &[MAIN, ALT]);
        seed_account(&env, "Spare", &[]);
        let eve = FakeEve::new(&env, partial);
        let mut harness = open(&env, &eve);
        until(&mut harness, "the account block", |h| {
            shown(h, "no PLEX count yet")
        });
        assert!(shown(&harness, "Main acct"));
        assert!(shown(&harness, "Alt, Main"));
        assert!(shown(&harness, "Rename Main acct"));
        assert!(shown(&harness, "PLEX in Main acct"));
        assert!(shown(&harness, "no characters"));
        assert_eq!(
            field_text(&harness, "Date (UTC)").as_deref(),
            Some("1970-01-12")
        );
    }

    #[test]
    fn setting_plex_stores_the_count_and_clears_the_field() {
        let env = env();
        let main = seed_account(&env, "Main acct", &[MAIN, ALT]);
        let eve = FakeEve::new(&env, partial);
        let mut harness = open(&env, &eve);
        until(&mut harness, "the account block", |h| {
            shown(h, "no PLEX count yet")
        });
        type_into(&mut harness, "PLEX in Main acct", "1,250");
        press(&mut harness, "Set PLEX", 0);
        until(&mut harness, "the count line", |h| {
            shown(h, "1,250 PLEX, set 0s ago")
        });
        assert_eq!(plex_counts_in(&mut harness, main), vec![(NOW, 1_250)]);
        assert_eq!(
            field_text(&harness, "PLEX in Main acct").as_deref(),
            Some("")
        );
        assert_eq!(
            field_text(&harness, "Date (UTC)").as_deref(),
            Some("1970-01-12")
        );
    }

    #[test]
    fn a_seeded_count_shows_its_age_and_a_new_count_is_added() {
        let env = env();
        let main = seed_account(&env, "Main acct", &[MAIN, ALT]);
        seed_plex(&env, main, 650, 740_800);
        let eve = FakeEve::new(&env, partial);
        let mut harness = open(&env, &eve);
        until(&mut harness, "the seeded count", |h| {
            shown(h, "650 PLEX, set 3d ago")
        });
        type_into(&mut harness, "PLEX in Main acct", "1,250");
        press(&mut harness, "Set PLEX", 0);
        until(&mut harness, "the new count", |h| {
            shown(h, "1,250 PLEX, set 0s ago")
        });
        assert_eq!(
            plex_counts_in(&mut harness, main),
            vec![(740_800, 650), (NOW, 1_250)]
        );
    }

    #[test]
    fn bad_plex_counts_and_dates_are_errors_and_a_past_day_is_kept() {
        let env = env();
        let main = seed_account(&env, "Main acct", &[MAIN, ALT]);
        let eve = FakeEve::new(&env, partial);
        let mut harness = open(&env, &eve);
        until(&mut harness, "the account block", |h| {
            shown(h, "no PLEX count yet")
        });

        type_into(&mut harness, "PLEX in Main acct", "0");
        clear_field(&mut harness, "PLEX in Main acct");
        press(&mut harness, "Set PLEX", 0);
        assert_eq!(
            notices(&mut harness),
            vec!["Type the number of PLEX.".to_owned()]
        );
        clear_notices(&mut harness);

        type_into(&mut harness, "PLEX in Main acct", "12x");
        press(&mut harness, "Set PLEX", 0);
        assert_eq!(
            notices(&mut harness),
            vec!["Type a whole number of PLEX.".to_owned()]
        );
        clear_notices(&mut harness);

        type_into(&mut harness, "PLEX in Main acct", "900");
        type_into(&mut harness, "Date (UTC)", "1970-01-13");
        press(&mut harness, "Set PLEX", 0);
        assert_eq!(
            notices(&mut harness),
            vec!["The date cannot be in the future.".to_owned()]
        );
        clear_notices(&mut harness);

        type_into(&mut harness, "Date (UTC)", "1970-02-30");
        press(&mut harness, "Set PLEX", 0);
        assert_eq!(
            notices(&mut harness),
            vec!["Type the date as year-month-day, like 2026-10-18.".to_owned()]
        );
        clear_notices(&mut harness);

        type_into(&mut harness, "Date (UTC)", "10 Jan");
        press(&mut harness, "Set PLEX", 0);
        assert_eq!(
            notices(&mut harness),
            vec!["Type the date as year-month-day, like 2026-10-18.".to_owned()]
        );
        clear_notices(&mut harness);

        // A good count on a past day is kept at that day's 00:00 UTC.
        type_into(&mut harness, "PLEX in Main acct", "900");
        type_into(&mut harness, "Date (UTC)", "1970-01-10");
        press(&mut harness, "Set PLEX", 0);
        until(&mut harness, "the past count", |h| {
            shown(h, "900 PLEX, set 2d ago")
        });
        assert_eq!(plex_counts_in(&mut harness, main), vec![(777_600, 900)]);
        clear_notices(&mut harness);

        // The same date replaces its count instead of adding another.
        type_into(&mut harness, "PLEX in Main acct", "950");
        type_into(&mut harness, "Date (UTC)", "1970-01-10");
        press(&mut harness, "Set PLEX", 0);
        until(&mut harness, "the replaced count", |h| {
            shown(h, "950 PLEX, set 2d ago")
        });
        assert_eq!(plex_counts_in(&mut harness, main), vec![(777_600, 950)]);
        clear_notices(&mut harness);

        // 0 is a valid count, and an empty date means today.
        type_into(&mut harness, "PLEX in Main acct", "0");
        clear_field(&mut harness, "Date (UTC)");
        press(&mut harness, "Set PLEX", 0);
        until(&mut harness, "the zero count", |h| {
            shown(h, "0 PLEX, set 0s ago")
        });
        assert_eq!(
            plex_counts_in(&mut harness, main),
            vec![(777_600, 950), (NOW, 0)]
        );
    }

    #[test]
    fn an_account_is_renamed_and_a_taken_or_blank_name_is_an_error() {
        let env = env();
        seed_account(&env, "Main acct", &[MAIN, ALT]);
        seed_account(&env, "Spare", &[]);
        let eve = FakeEve::new(&env, partial);
        let mut harness = open(&env, &eve);
        until(&mut harness, "the account block", |h| {
            shown(h, "no PLEX count yet")
        });

        type_into(&mut harness, "Rename Main acct", " Pilots ");
        press(&mut harness, "Rename", 0);
        until(&mut harness, "the new name", |h| shown(h, "Rename Pilots"));
        assert!(accounts_in(&mut harness).iter().any(|a| a.name == "Pilots"));
        assert_eq!(
            field_text(&harness, "Rename Pilots").as_deref(),
            Some("Pilots")
        );

        type_into(&mut harness, "Rename Pilots", "Spare");
        press(&mut harness, "Rename", 0);
        assert_eq!(
            notices(&mut harness),
            vec!["An account called \"Spare\" already exists.".to_owned()]
        );
        clear_notices(&mut harness);

        type_into(&mut harness, "Rename Pilots", "   ");
        press(&mut harness, "Rename", 0);
        assert_eq!(
            notices(&mut harness),
            vec!["Type a name for the account.".to_owned()]
        );
        assert!(accounts_in(&mut harness).iter().any(|a| a.name == "Pilots"));
    }

    /// A 16 by 16 picture that is red everywhere.
    const RED_PNG: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x10, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f,
        0xf3, 0xff, 0x61, 0x00, 0x00, 0x00, 0x19, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63, 0xf8,
        0xcf, 0xc0, 0xf0, 0x9f, 0x12, 0xcc, 0x30, 0x6a, 0xc0, 0xa8, 0x01, 0xa3, 0x06, 0x0c, 0x17,
        0x03, 0x00, 0x30, 0xc4, 0xfe, 0x10, 0x8a, 0xc6, 0x4c, 0xe5, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];

    #[test]
    fn a_portrait_fills_the_row_less_its_gap_at_every_density() {
        assert_eq!(portrait_size(Density::Dense), 16.0);
        assert_eq!(portrait_size(Density::Normal), 20.0);
        assert_eq!(portrait_size(Density::Sparse), 24.0);
    }

    #[test]
    fn a_kept_portrait_draws_with_the_name_and_a_missing_one_stays_blank() {
        let env = env();
        let dir = env.data.join("images");
        fs::create_dir_all(&dir).expect("the image folder");
        fs::write(images::cache_path(&dir, Picture::Character(MAIN)), RED_PNG)
            .expect("the kept portrait");
        let eve = FakeEve::new(&env, partial);
        let mut harness = open(&env, &eve);
        // The app's own fetcher would reach the image server; keep the missing picture offline.
        // `init` replaces the state, so this also forgets what the app loaded before.
        images::init(&harness.ctx, &dir, Arc::new(NoImages));
        assert!(!images::is_loaded(&harness.ctx, Picture::Character(MAIN)));
        until(&mut harness, "Main's portrait", |h| {
            images::is_loaded(&h.ctx, Picture::Character(MAIN))
        });
        assert!(images::is_loaded(&harness.ctx, Picture::Character(MAIN)));
        assert!(!images::is_loaded(&harness.ctx, Picture::Character(ALT)));
        assert!(shown(&harness, "Main"));
        assert!(shown(&harness, "Alt"));
    }

    #[test]
    fn the_characters_page_matches_its_snapshot() {
        let env = env();
        // A fixed folder, so the path the panel shows is the same on every run.
        let jeve = std::env::temp_dir().join("mop-characters-snapshot");
        drop(fs::remove_dir_all(&jeve));
        fs::create_dir_all(&jeve).expect("the folder");
        fill_jeve(&jeve);
        let mut eve = FakeEve::new(&env, partial);
        eve.jeve = Some(jeve.clone());
        eve.warning = Some(KEYCHAIN.to_owned());
        let mut harness = open(&env, &eve);
        until(&mut harness, "the jEve rows", |h| shown(h, "Hauler"));
        frames(&mut harness);
        crate::ui::no_images(&mut harness);
        harness.snapshot("characters");
        fs::remove_dir_all(&jeve).expect("the folder goes");
    }

    /// The short name the app's own store holds for Main.
    fn main_short_name(harness: &mut Harness<'_, App>) -> Option<String> {
        harness
            .state_mut()
            .shared_mut()
            .store
            .setting("character.short.2112000001")
            .expect("the setting reads")
    }

    /// The Short name field of the row whose character cell shows `anchor`: the text input drawn
    /// in the same table row.
    fn short_field<'a>(harness: &'a Harness<'a, App>, anchor: &str) -> egui_kittest::Node<'a> {
        let y = row_y(harness, anchor);
        harness
            .query_all(by().role(Role::TextInput))
            .find(|node| (node.rect().center().y - y).abs() < 12.0)
            .unwrap_or_else(|| panic!("no Short name field on the {anchor} row"))
    }

    /// Types into the Short name field of the row whose character cell shows `anchor`, selecting
    /// what the field holds first.
    fn type_short(harness: &mut Harness<'_, App>, anchor: &str, text: &str) {
        short_field(harness, anchor).hover();
        harness.run_steps(2);
        short_field(harness, anchor).click();
        harness.run_steps(2);
        harness.key_press_modifiers(Modifiers::COMMAND, Key::A);
        short_field(harness, anchor).type_text(text);
        harness.run_steps(3);
    }

    /// Empties the Short name field of the row whose character cell shows `anchor`.
    fn clear_short(harness: &mut Harness<'_, App>, anchor: &str) {
        short_field(harness, anchor).hover();
        harness.run_steps(2);
        short_field(harness, anchor).click();
        harness.run_steps(2);
        harness.key_press_modifiers(Modifiers::COMMAND, Key::A);
        harness.key_press(Key::Backspace);
        harness.run_steps(3);
    }

    #[test]
    fn a_short_name_is_saved_trimmed_shown_on_hover_and_cleared() {
        let env = env();
        let eve = FakeEve::new(&env, all_not_yet);
        let mut harness = open(&env, &eve);
        clear_notices(&mut harness);
        let ctx = harness.ctx.clone();

        // A short name is trimmed, saved under the character's key and drawn in its place.
        {
            let cx = harness.state_mut().shared_mut();
            assert!(short_names::set(&ctx, cx, MAIN, "  Indy 2  "));
            assert_eq!(
                cx.store
                    .setting("character.short.2112000001")
                    .expect("the setting reads")
                    .as_deref(),
                Some("Indy 2")
            );
            assert_eq!(short_names::current(&ctx, cx, MAIN, "Main"), "Indy 2");
        }
        until(&mut harness, "the short name on Main's row", |h| {
            shown(h, "Indy 2")
        });
        assert_eq!(hover(&mut harness, "Indy 2"), vec!["Main".to_owned()]);

        // One over the long limit is refused: the saved name stands and the reason is said.
        let long = "1234567890123456789012345";
        assert_eq!(long.chars().count(), 25);
        {
            let cx = harness.state_mut().shared_mut();
            assert!(!short_names::set(&ctx, cx, MAIN, long));
            assert_eq!(
                cx.store
                    .setting("character.short.2112000001")
                    .expect("the setting reads")
                    .as_deref(),
                Some("Indy 2")
            );
            assert_eq!(short_names::current(&ctx, cx, MAIN, "Main"), "Indy 2");
        }
        assert_eq!(
            notices(&mut harness),
            vec!["A short name can have at most 24 characters.".to_owned()]
        );
        clear_notices(&mut harness);

        // An empty one clears the key: the row draws the full name again, with nothing on hover.
        harness.remove_cursor();
        {
            let cx = harness.state_mut().shared_mut();
            assert!(short_names::set(&ctx, cx, MAIN, ""));
            assert_eq!(
                cx.store
                    .setting("character.short.2112000001")
                    .expect("the setting reads"),
                None
            );
            assert_eq!(short_names::current(&ctx, cx, MAIN, "Main"), "Main");
        }
        until(&mut harness, "the full name on Main's row", |h| {
            shown(h, "Main")
        });
        assert!(hover(&mut harness, "Main").is_empty());
    }

    #[test]
    fn typing_a_short_name_saves_it_and_clearing_the_field_removes_it() {
        let env = env();
        let eve = FakeEve::new(&env, all_not_yet);
        // The player types the name into Main's Short name field and presses Enter.
        {
            let mut harness = open(&env, &eve);
            type_short(&mut harness, "Main", "Indy 2");
            harness.key_press(Key::Enter);
            until(&mut harness, "the row redrawn", |h| shown(h, "Indy 2"));
            assert_eq!(main_short_name(&mut harness).as_deref(), Some("Indy 2"));
            assert_eq!(hover(&mut harness, "Indy 2"), vec!["Main".to_owned()]);
        }

        // A new app reads the saved name back.
        let mut harness = open(&env, &eve);
        until(&mut harness, "the short name after a restart", |h| {
            shown(h, "Indy 2")
        });

        // Clearing the field and pressing Enter clears the key: the full name is drawn again.
        clear_short(&mut harness, "Indy 2");
        harness.key_press(Key::Enter);
        until(&mut harness, "the full name on the row", |h| {
            shown(h, "Main")
        });
        assert_eq!(main_short_name(&mut harness), None);
        harness.remove_cursor();
        assert!(hover(&mut harness, "Main").is_empty());
    }

    #[test]
    fn every_table_draws_the_short_name_with_the_full_name_on_hover() {
        let env = env();
        let jeve = TempDir::new().expect("a temp folder");
        fill_main_jeve(jeve.path());
        let mut eve = FakeEve::new(&env, tables);
        eve.jeve = Some(jeve.path().to_path_buf());
        let mut harness = open(&env, &eve);
        let ctx = harness.ctx.clone();
        clear_notices(&mut harness);
        {
            let cx = harness.state_mut().shared_mut();
            assert!(short_names::set(&ctx, cx, MAIN, "Indy 2"));
        }

        // The Characters page: the login table's row and the jEve table's row.
        until(&mut harness, "the short name in both tables", |h| {
            count(h, "Indy 2") >= 2
        });
        assert_eq!(hover(&mut harness, "Indy 2"), vec!["Main".to_owned()]);

        rail(&mut harness, "Orders");
        until(&mut harness, "Main's order row", |h| shown(h, "Indy 2"));
        assert_eq!(hover(&mut harness, "Indy 2"), vec!["Main".to_owned()]);

        rail(&mut harness, "Industry jobs");
        until(&mut harness, "Main's job row", |h| shown(h, "Indy 2"));
        assert_eq!(hover(&mut harness, "Indy 2"), vec!["Main".to_owned()]);

        rail(&mut harness, "Assets");
        until(&mut harness, "Main's asset row", |h| shown(h, "Indy 2"));
        assert_eq!(hover(&mut harness, "Indy 2"), vec!["Main".to_owned()]);

        // The Item panel's two tables: who holds the item, and the job making it.
        rail(&mut harness, "Build");
        harness.state_mut().shared_mut().selection.item = Some(PAU);
        frames(&mut harness);
        until(&mut harness, "the Owned and Running jobs rows", |h| {
            count(h, "Indy 2") >= 2
        });
        assert_eq!(hover(&mut harness, "Indy 2"), vec!["Main".to_owned()]);

        // Without a saved key every table draws the full name again, with nothing on hover.
        harness.remove_cursor();
        {
            let cx = harness.state_mut().shared_mut();
            assert!(short_names::set(&ctx, cx, MAIN, ""));
        }
        frames(&mut harness);
        rail(&mut harness, "Orders");
        until(&mut harness, "the full name on the order row", |h| {
            shown(h, "Main")
        });
        assert!(hover(&mut harness, "Main").is_empty());
    }
}

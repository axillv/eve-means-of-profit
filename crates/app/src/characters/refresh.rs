//! Refresh on the worker: one character after another, in name order.

use std::path::Path;
use std::sync::Mutex;

use mop_esi::character_data::{CharacterData, JobStatus, Kind, Refreshed};
use mop_esi::error::EsiError;
use mop_esi::places::{FIRST_STRUCTURE_ID, StructureNames};
use mop_store::changes::{AssetRow, JobRow, OrderRow};
use mop_store::{Store, StoreError};

use super::Msg;
use crate::build::prices::age_text;
use crate::session::{EveAccess, lock_eve};
use crate::status::Notice;

/// A character to refresh, with a copy of the data it holds.
pub(super) struct Target {
    pub(super) id: u64,
    pub(super) name: String,
    pub(super) data: CharacterData,
}

/// What one character's refresh gives the page to apply.
pub(super) struct Outcome {
    pub(super) id: u64,
    /// The new data, when at least one kind was fetched.
    pub(super) data: Option<CharacterData>,
    /// The structure names after naming, when naming ran and EVE answered.
    pub(super) structures: Option<StructureNames>,
    pub(super) notices: Vec<Notice>,
    /// The login turned out to be expired.
    pub(super) expired: bool,
}

/// How many kinds answered and how many of those were not yet due.
#[derive(Debug, Default, Clone, Copy)]
struct Tally {
    asked: usize,
    not_yet: usize,
    soonest: Option<i64>,
}

/// Refreshes `targets` in the order given. Sends one [`Msg::Refreshed`] per character, then
/// [`Msg::RefreshDone`].
pub(super) fn run(
    eve: &Mutex<Box<dyn EveAccess>>,
    targets: Vec<Target>,
    mut names: StructureNames,
    data_dir: &Path,
    now: i64,
    send: &dyn Fn(Msg),
) {
    let mut store = match Store::open(&data_dir.join(crate::USER_DB), now) {
        Ok(opened) => Some(opened.store),
        Err(error) => {
            send(Msg::Notice(Notice::error(format!(
                "Could not open the user database: {error}"
            ))));
            None
        }
    };
    let mut all = Tally::default();
    for target in targets {
        let (outcome, tally) = {
            let mut login = lock_eve(eve);
            refresh_one(&mut **login, target, &names, now, &mut store)
        };
        // Cannot saturate: both counts are at most targets times four kinds, far below usize::MAX.
        all.asked = all.asked.saturating_add(tally.asked);
        // Cannot saturate: not_yet is never more than asked, which is bounded as above.
        all.not_yet = all.not_yet.saturating_add(tally.not_yet);
        all.soonest = match (all.soonest, tally.soonest) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        if let Some(named) = &outcome.structures {
            names = named.clone();
        }
        send(Msg::Refreshed(Box::new(outcome)));
    }
    let nothing = match all.soonest {
        Some(until) if all.asked > 0 && all.not_yet == all.asked => {
            // A floor at 0 is meant here: EVE may already allow the refresh when `until` is not after `now`, and the notice then says zero time left.
            let wait = until.saturating_sub(now);
            Some(Notice::info(format!(
                "Nothing to refresh yet: EVE allows it again in {}.",
                age_text(wait)
            )))
        }
        Some(_) | None => None,
    };
    send(Msg::RefreshDone(nothing));
}

fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Assets => "assets",
        Kind::Orders => "orders",
        Kind::Jobs => "jobs",
        Kind::Wallet => "wallet",
    }
}

/// Records the kind's fetched rows in the change list at the time they were fetched. A kind that
/// was not fetched, and the wallet, record nothing.
fn record_fetched(
    store: &mut Store,
    data: &CharacterData,
    kind: Kind,
) -> Result<usize, StoreError> {
    match kind {
        Kind::Assets => {
            let Some(held) = &data.assets else {
                return Ok(0);
            };
            let rows: Vec<AssetRow> = held
                .data
                .assets
                .assets
                .iter()
                .map(|asset| AssetRow {
                    item_id: asset.item_id,
                    type_id: asset.type_id,
                    quantity: asset.quantity,
                    location_id: asset.location_id,
                    flag: asset.flag.clone(),
                    singleton: asset.singleton,
                })
                .collect();
            store.record_assets(data.character_id, held.fetched_at, &rows)
        }
        Kind::Orders => {
            let Some(held) = &data.orders else {
                return Ok(0);
            };
            let rows: Vec<OrderRow> = held
                .data
                .iter()
                .map(|order| OrderRow {
                    order_id: order.order_id,
                    type_id: order.type_id,
                    location_id: order.location_id,
                    is_buy: order.is_buy,
                    price: order.price,
                    volume_remain: order.volume_remain,
                })
                .collect();
            store.record_orders(data.character_id, held.fetched_at, &rows)
        }
        Kind::Jobs => {
            let Some(held) = &data.jobs else {
                return Ok(0);
            };
            let rows: Vec<JobRow> = held
                .data
                .iter()
                .map(|job| JobRow {
                    job_id: job.job_id,
                    type_id: job.product_type_id.unwrap_or(job.blueprint_type_id),
                    runs: job.runs,
                    facility_id: job.facility_id,
                    state: job_status_name(job.status).to_owned(),
                    cost: job.cost,
                })
                .collect();
            store.record_jobs(data.character_id, held.fetched_at, &rows)
        }
        Kind::Wallet => Ok(0),
    }
}

/// A job status as EVE writes it, lower case.
fn job_status_name(status: JobStatus) -> &'static str {
    match status {
        JobStatus::Active => "active",
        JobStatus::Cancelled => "cancelled",
        JobStatus::Delivered => "delivered",
        JobStatus::Paused => "paused",
        JobStatus::Ready => "ready",
        JobStatus::Reverted => "reverted",
    }
}

fn refresh_one(
    eve: &mut dyn EveAccess,
    target: Target,
    structures: &StructureNames,
    now: i64,
    store: &mut Option<Store>,
) -> (Outcome, Tally) {
    let Target { id, name, mut data } = target;
    let results = eve.refresh_all(&mut data, now);
    let mut notices = Vec::new();
    let mut tally = Tally::default();
    let mut fetched = false;
    let mut expired = false;
    let mut limited: Option<u64> = None;
    for (kind, result) in results {
        // Cannot saturate: this adds one per kind, and there are only four kinds per target.
        tally.asked = tally.asked.saturating_add(1);
        match result {
            Ok(Refreshed::Fetched) => {
                fetched = true;
                if let Some(store) = store.as_mut()
                    && let Err(error) = record_fetched(store, &data, kind)
                {
                    notices.push(Notice::error(format!(
                        "Could not record the {} of {name}: {error}",
                        kind_name(kind)
                    )));
                }
            }
            Ok(Refreshed::NotYet { until }) => {
                // Cannot saturate: not_yet counts at most the same kinds that asked counts.
                tally.not_yet = tally.not_yet.saturating_add(1);
                tally.soonest = Some(tally.soonest.map_or(until, |soonest| soonest.min(until)));
            }
            Err(EsiError::LoginExpired) => expired = true,
            Err(EsiError::ErrorLimited { reset_seconds }) => {
                limited = limited.or(Some(reset_seconds));
            }
            Err(error) => notices.push(Notice::error(format!(
                "Could not refresh {name} {}: {error}",
                kind_name(kind)
            ))),
        }
    }
    if let Some(seconds) = limited {
        notices.push(Notice::error(format!(
            "Could not refresh {name}: EVE asks to wait {seconds} s."
        )));
    }
    if expired {
        notices.push(Notice::error(format!(
            "Could not refresh {name}: login expired, log in again."
        )));
    }
    if fetched && let Err(error) = eve.save_character(&data) {
        notices.push(Notice::error(format!(
            "Could not save the data of {name}: {error}"
        )));
    }
    let named = name_structures(eve, &data, structures, now, &mut notices);
    let outcome = Outcome {
        id,
        data: fetched.then_some(data),
        structures: named,
        notices,
        expired,
    };
    (outcome, tally)
}

/// Names the structures the data mentions. Failures become one notice; the names that did arrive
/// are kept, and a call that failed whole keeps the last names.
fn name_structures(
    eve: &mut dyn EveAccess,
    data: &CharacterData,
    structures: &StructureNames,
    now: i64,
    notices: &mut Vec<Notice>,
) -> Option<StructureNames> {
    let ids = structure_ids(data);
    if ids.is_empty() {
        return None;
    }
    let mut names = structures.clone();
    let (failed, kept) = match eve.name_structures(&mut names, &ids, now) {
        Ok(report) => (report.failed.len(), Some(names)),
        Err(_) => (ids.len(), None),
    };
    if failed > 0 {
        notices.push(Notice::info(format!(
            "Could not get {failed} structure names from EVE. The last names are kept."
        )));
    }
    kept
}

/// The structure ids in a character's assets, orders and jobs, sorted and without repeats.
fn structure_ids(data: &CharacterData) -> Vec<i64> {
    let mut ids: Vec<i64> = Vec::new();
    if let Some(held) = &data.assets {
        ids.extend(
            held.data
                .assets
                .assets
                .iter()
                .map(|asset| asset.location_id),
        );
    }
    if let Some(held) = &data.orders {
        ids.extend(held.data.iter().map(|order| order.location_id));
    }
    if let Some(held) = &data.jobs {
        ids.extend(held.data.iter().map(|job| job.facility_id));
    }
    ids.retain(|id| *id >= FIRST_STRUCTURE_ID);
    ids.sort_unstable();
    ids.dedup();
    ids
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    use mop_core::money::Money;
    use mop_core::stock::{Asset, CharacterAssets};
    use mop_esi::character_data::{Fetched, Job, JobStatus, Order, OwnedAssets};
    use mop_esi::characters::Character;
    use mop_esi::places::NamingReport;
    use mop_store::changes::{ChangeRow, Event, Kind as ChangeKind};
    use tempfile::TempDir;

    use super::*;
    use crate::session::PendingLogin;

    const CHARACTER: u64 = 90_000_001;
    const STATION: i64 = 60_003_760;

    fn held<T>(data: T, fetched_at: i64) -> Fetched<T> {
        Fetched {
            data,
            fetched_at,
            last_modified: None,
            expires: None,
        }
    }

    fn open_temp() -> (TempDir, Store) {
        let dir = TempDir::new().expect("a temp folder");
        let store = Store::open(&dir.path().join("eve.db"), 1_000)
            .expect("the store opens")
            .store;
        (dir, store)
    }

    fn asset(
        item_id: i64,
        type_id: u32,
        quantity: u64,
        location_id: i64,
        flag: Option<&str>,
        singleton: bool,
    ) -> Asset {
        Asset {
            item_id,
            type_id,
            location_id,
            flag: flag.map(str::to_owned),
            quantity,
            singleton,
        }
    }

    fn assets_data() -> CharacterData {
        let owned = OwnedAssets {
            assets: CharacterAssets {
                character_id: i64::try_from(CHARACTER).expect("an id that fits"),
                updated: Some(1_000),
                assets: vec![asset(500, 34, 10, STATION, Some("Hangar"), false)],
            },
            names: BTreeMap::new(),
        };
        CharacterData {
            character_id: CHARACTER,
            assets: Some(held(owned, 1_000)),
            orders: None,
            jobs: None,
            wallet: None,
        }
    }

    fn order(order_id: i64, is_buy: bool, price: i64, volume_remain: u64) -> Order {
        Order {
            order_id,
            type_id: 34,
            location_id: STATION,
            region_id: 10_000_002,
            is_buy,
            price: Money::from_ten_thousandths(price),
            volume_remain,
            volume_total: volume_remain,
            issued: 0,
            expires: 0,
            range: "station".to_owned(),
            escrow: None,
            min_volume: None,
            is_corporation: false,
        }
    }

    fn job(
        job_id: i64,
        product: Option<u32>,
        runs: u32,
        status: JobStatus,
        cost: Option<i64>,
    ) -> Job {
        Job {
            job_id,
            activity_id: 1,
            blueprint_type_id: 35,
            product_type_id: product,
            runs,
            status,
            start: 0,
            end: 0,
            facility_id: STATION,
            station_id: 0,
            output_location_id: 0,
            installer_id: 0,
            cost: cost.map(Money::from_ten_thousandths),
            licensed_runs: None,
        }
    }

    #[test]
    fn a_fetched_asset_and_order_are_recorded_at_their_fetch_time() {
        let (_dir, mut store) = open_temp();
        let mut data = assets_data();
        assert_eq!(
            record_fetched(&mut store, &data, Kind::Assets).expect("assets"),
            1
        );
        assert_eq!(
            store.changes(CHARACTER, ChangeKind::Asset).expect("assets"),
            vec![ChangeRow {
                kind: ChangeKind::Asset,
                entity_id: 500,
                at: 1_000,
                event: Event::Appeared,
                type_id: 34,
                quantity: 10,
                place: STATION,
                price: None,
                state: Some("Hangar".to_owned()),
            }]
        );

        data.orders = Some(held(vec![order(7001, true, 57_500, 10)], 1_100));
        assert_eq!(
            record_fetched(&mut store, &data, Kind::Orders).expect("orders"),
            1
        );
        assert_eq!(
            store.changes(CHARACTER, ChangeKind::Order).expect("orders"),
            vec![ChangeRow {
                kind: ChangeKind::Order,
                entity_id: 7001,
                at: 1_100,
                event: Event::Appeared,
                type_id: 34,
                quantity: 10,
                place: STATION,
                price: Some(Money::from_ten_thousandths(57_500)),
                state: Some("buy".to_owned()),
            }]
        );
        assert!(
            store
                .changes(CHARACTER, ChangeKind::Job)
                .expect("jobs")
                .is_empty()
        );
    }

    #[test]
    fn a_job_records_its_product_or_blueprint_lower_case_status_and_cost() {
        let (_dir, mut store) = open_temp();
        let mut data = assets_data();
        data.jobs = Some(held(
            vec![
                job(8001, Some(36), 10, JobStatus::Ready, Some(153_770_000)),
                job(8002, None, 5, JobStatus::Active, None),
            ],
            900,
        ));
        assert_eq!(
            record_fetched(&mut store, &data, Kind::Jobs).expect("jobs"),
            2
        );
        let changes = store.changes(CHARACTER, ChangeKind::Job).expect("jobs");
        assert_eq!(
            changes.first(),
            Some(&ChangeRow {
                kind: ChangeKind::Job,
                entity_id: 8001,
                at: 900,
                event: Event::Appeared,
                type_id: 36,
                quantity: 10,
                place: STATION,
                price: Some(Money::from_ten_thousandths(153_770_000)),
                state: Some("ready".to_owned()),
            })
        );
        assert_eq!(
            changes.last(),
            Some(&ChangeRow {
                kind: ChangeKind::Job,
                entity_id: 8002,
                at: 900,
                event: Event::Appeared,
                type_id: 35,
                quantity: 5,
                place: STATION,
                price: None,
                state: Some("active".to_owned()),
            })
        );
    }

    #[test]
    fn a_missing_kind_and_the_wallet_record_nothing() {
        let (_dir, mut store) = open_temp();
        let mut data = CharacterData::new(CHARACTER);
        for kind in [Kind::Assets, Kind::Orders, Kind::Jobs, Kind::Wallet] {
            assert_eq!(record_fetched(&mut store, &data, kind).expect("nothing"), 0);
        }
        data.wallet = Some(held(Money::from_ten_thousandths(5), 1_000));
        assert_eq!(
            record_fetched(&mut store, &data, Kind::Wallet).expect("wallet"),
            0
        );
        for kind in [ChangeKind::Asset, ChangeKind::Order, ChangeKind::Job] {
            assert!(store.changes(CHARACTER, kind).expect("changes").is_empty());
        }
    }

    type Results = Vec<(Kind, Result<Refreshed, EsiError>)>;

    struct FakeEve {
        script: fn(&mut CharacterData) -> Results,
    }

    impl EveAccess for FakeEve {
        fn characters(&self) -> Vec<Character> {
            Vec::new()
        }

        fn begin_login(&mut self) -> Result<PendingLogin, String> {
            unreachable!("a refresh does not log in")
        }

        fn finish_login(&mut self, _code: &str) -> Result<Character, EsiError> {
            unreachable!("a refresh does not log in")
        }

        fn refresh_all(&mut self, data: &mut CharacterData, _now: i64) -> Results {
            (self.script)(data)
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
            Ok(())
        }

        fn keychain_warning(&self) -> Option<String> {
            None
        }

        fn jeve_folder(&self) -> Option<std::path::PathBuf> {
            None
        }
    }

    fn assets_fetched(data: &mut CharacterData) -> Results {
        data.assets = assets_data().assets;
        vec![(Kind::Assets, Ok(Refreshed::Fetched))]
    }

    fn target() -> Target {
        Target {
            id: CHARACTER,
            name: "Main".to_owned(),
            data: CharacterData::new(CHARACTER),
        }
    }

    #[test]
    fn run_opens_the_user_database_once_and_records_each_refresh() {
        let dir = TempDir::new().expect("a temp folder");
        let eve: Mutex<Box<dyn EveAccess>> = Mutex::new(Box::new(FakeEve {
            script: assets_fetched,
        }));
        let seen = RefCell::new(Vec::new());
        run(
            &eve,
            vec![target()],
            StructureNames::new(),
            dir.path(),
            1_000,
            &|msg| seen.borrow_mut().push(msg),
        );
        let mut store = Store::open(&dir.path().join(crate::USER_DB), 1_000)
            .expect("the store opens")
            .store;
        let changes = store
            .changes(CHARACTER, ChangeKind::Asset)
            .expect("changes");
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].event, Event::Appeared);
        assert_eq!(changes[0].at, 1_000);
        assert!(
            seen.borrow()
                .iter()
                .any(|msg| matches!(msg, Msg::Refreshed(_)))
        );
    }

    #[test]
    fn a_database_that_does_not_open_sends_one_notice() {
        let dir = TempDir::new().expect("a temp folder");
        std::fs::create_dir(dir.path().join(crate::USER_DB))
            .expect("a folder in place of the database");
        let eve: Mutex<Box<dyn EveAccess>> = Mutex::new(Box::new(FakeEve {
            script: assets_fetched,
        }));
        let seen = RefCell::new(Vec::new());
        run(
            &eve,
            vec![target()],
            StructureNames::new(),
            dir.path(),
            1_000,
            &|msg| seen.borrow_mut().push(msg),
        );
        let notices = seen
            .borrow()
            .iter()
            .filter(|msg| matches!(msg, Msg::Notice(_)))
            .count();
        assert_eq!(notices, 1);
    }
}

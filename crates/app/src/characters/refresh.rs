//! Refresh on the worker: one character after another, in name order.

use std::sync::Mutex;

use mop_esi::character_data::{CharacterData, Kind, Refreshed};
use mop_esi::error::EsiError;
use mop_esi::places::{FIRST_STRUCTURE_ID, StructureNames};

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
    now: i64,
    send: &dyn Fn(Msg),
) {
    let mut all = Tally::default();
    for target in targets {
        let (outcome, tally) = {
            let mut login = lock_eve(eve);
            refresh_one(&mut **login, target, &names, now)
        };
        all.asked = all.asked.saturating_add(tally.asked);
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

fn refresh_one(
    eve: &mut dyn EveAccess,
    target: Target,
    structures: &StructureNames,
    now: i64,
) -> (Outcome, Tally) {
    let Target { id, name, mut data } = target;
    let results = eve.refresh_all(&mut data, now);
    let mut notices = Vec::new();
    let mut tally = Tally::default();
    let mut fetched = false;
    let mut expired = false;
    let mut limited: Option<u64> = None;
    for (kind, result) in results {
        tally.asked = tally.asked.saturating_add(1);
        match result {
            Ok(Refreshed::Fetched) => fetched = true,
            Ok(Refreshed::NotYet { until }) => {
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

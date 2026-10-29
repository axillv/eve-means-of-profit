//! Records what each character holds when its data has changed, and the Forge prices of those
//! items at that moment, so the net worth chart can value the past.

use std::collections::{BTreeMap, BTreeSet};

use mop_core::industry::{Activity, RecipeBook, TypeId};
use mop_core::money::Money;
use mop_core::net_worth::{CharacterKey, Holdings, Record, Snapshot, points};
use mop_core::stock::Asset;
use mop_esi::character_data::{CharacterData, Job, JobStatus, Order};
use mop_sde::query::Sde;
use mop_store::Store;
use mop_store::price_history::PointPrices;

use crate::Shared;
use crate::status::Notice;

/// The category of blueprints: their stacks are not valued.
const CATEGORY_BLUEPRINT: u32 = 9;
const TOO_LARGE: &str = "the sum is too large";

/// Records the holdings of each character whose data changed since the last call, then copies the
/// prices of the items of the latest point. The first call only fills `seen`, the revisions of the
/// characters seen so far. `started` is kept apart from `seen`: a first call that sees no
/// characters still counts as the first call, so the characters that appear later are recorded.
pub fn tick(shared: &mut Shared, seen: &mut BTreeMap<u64, u64>, started: &mut bool) {
    let first = !*started;
    *started = true;
    let ids: Vec<u64> = shared.session.characters().iter().map(|c| c.id).collect();
    let mut changed = Vec::new();
    let mut revisions = Vec::new();
    for id in ids {
        let revision = shared.session.revision(id);
        if seen.get(&id) != Some(&revision) {
            changed.push(id);
        }
        revisions.push((id, revision));
    }
    if first || changed.is_empty() {
        seen.extend(revisions);
        return;
    }
    let now = (shared.clock)();
    // Without game data nothing can be valued: no record is written, and blueprints are never
    // counted as items. `seen` is left as it was, so the change is recorded once the data loads.
    let Some(sde) = shared.sde.as_ref() else {
        shared.notices.push(Notice::error(
            "Could not record net worth: no game data is loaded",
        ));
        return;
    };
    // A failed SDE read is an error, not an empty recipe book: nothing is recorded from it, and
    // `seen` is left as it was.
    let recipes = match sde.recipe_book() {
        Ok(recipes) => recipes,
        Err(error) => {
            shared.notices.push(Notice::error(format!(
                "Could not record net worth: {error}"
            )));
            return;
        }
    };
    // The refresh goes through from here on, so the revisions are taken as seen.
    seen.extend(revisions);
    let mut recorded = Vec::new();
    for id in changed {
        let snapshot = match holdings(shared.session.character_data(id), sde, &recipes) {
            Ok((holdings, missing_recipe)) => {
                if missing_recipe {
                    shared.notices.push(Notice::error(concat!(
                        "Could not record net worth: a job's product has no recipe ",
                        "for its activity in the game data, so the job adds nothing",
                    )));
                }
                Snapshot::Seen(holdings)
            }
            Err(message) => {
                shared.notices.push(Notice::error(format!(
                    "Could not record net worth: {message}"
                )));
                continue;
            }
        };
        let record = Record {
            character: CharacterKey::Id(id),
            at: now,
            snapshot,
        };
        match shared.store.add_record(&record) {
            Ok(_) => recorded.push(id),
            Err(error) => shared.notices.push(Notice::error(format!(
                "Could not record net worth: {error}"
            ))),
        }
    }
    if !recorded.is_empty() {
        copy_prices(shared, now, &recorded);
    }
}

/// The holdings of a character, and whether a job had no recipe for its activity.
fn holdings(
    data: Option<&CharacterData>,
    sde: &Sde,
    recipes: &RecipeBook,
) -> Result<(Holdings, bool), String> {
    let Some(data) = data else {
        let holdings = Holdings {
            wallet: None,
            assets: BTreeMap::new(),
            sell_orders: None,
            buy_escrow: None,
            jobs_output: BTreeMap::new(),
        };
        return Ok((holdings, false));
    };
    let orders = data.orders.as_ref().map(|fetched| fetched.data.as_slice());
    let assets = match data.assets.as_ref() {
        Some(fetched) => asset_units(&fetched.data.assets.assets, sde)?,
        None => BTreeMap::new(),
    };
    let (jobs_output, missing_recipe) = match data.jobs.as_ref() {
        Some(fetched) => jobs_output(&fetched.data, recipes)?,
        None => (BTreeMap::new(), false),
    };
    let holdings = Holdings {
        wallet: data.wallet.as_ref().map(|fetched| fetched.data),
        assets,
        sell_orders: orders.map(sell_total).transpose()?,
        buy_escrow: orders.map(escrow_total).transpose()?,
        jobs_output,
    };
    Ok((holdings, missing_recipe))
}

/// The units held by type, blueprints left out. A failed SDE lookup is an error, since the
/// blueprint check would otherwise be wrong.
fn asset_units(assets: &[Asset], sde: &Sde) -> Result<BTreeMap<TypeId, u64>, String> {
    let mut units: BTreeMap<TypeId, u64> = BTreeMap::new();
    for asset in assets {
        if is_blueprint(asset.type_id, sde)? {
            continue;
        }
        let total = units.entry(asset.type_id).or_insert(0);
        *total = total
            .checked_add(asset.quantity)
            .ok_or_else(|| TOO_LARGE.to_owned())?;
    }
    Ok(units)
}

fn is_blueprint(type_id: TypeId, sde: &Sde) -> Result<bool, String> {
    let item = sde.item(type_id).map_err(|error| error.to_string())?;
    Ok(item.is_some_and(|item| item.category_id == CATEGORY_BLUEPRINT))
}

/// The price times the units left, of the personal sell orders.
fn sell_total(orders: &[Order]) -> Result<Money, String> {
    let mut total: i128 = 0;
    for order in orders.iter().filter(|o| !o.is_buy && !o.is_corporation) {
        let line = i128::from(order.price.ten_thousandths())
            .checked_mul(i128::from(order.volume_remain))
            .ok_or_else(|| TOO_LARGE.to_owned())?;
        total = total
            .checked_add(line)
            .ok_or_else(|| TOO_LARGE.to_owned())?;
    }
    i64::try_from(total)
        .ok()
        .map(Money::from_ten_thousandths)
        .ok_or_else(|| TOO_LARGE.to_owned())
}

/// The escrow held by the personal buy orders.
fn escrow_total(orders: &[Order]) -> Result<Money, String> {
    let mut total = Money::ZERO;
    for order in orders.iter().filter(|o| o.is_buy && !o.is_corporation) {
        let escrow = order.escrow.unwrap_or(Money::ZERO);
        total = total
            .checked_add(escrow)
            .ok_or_else(|| TOO_LARGE.to_owned())?;
    }
    Ok(total)
}

/// The recipe activity a job's activity id runs: manufacturing, or the reactions (9 and 11).
fn activity_of(activity_id: u32) -> Option<Activity> {
    match activity_id {
        1 => Some(Activity::Manufacturing),
        9 | 11 => Some(Activity::Reaction),
        _ => None,
    }
}

/// The runs times the output per run, by product, of the jobs still making something. A job whose
/// product has no recipe for its activity adds nothing, and the flag is set.
fn jobs_output(
    jobs: &[Job],
    recipes: &RecipeBook,
) -> Result<(BTreeMap<TypeId, u64>, bool), String> {
    let mut output: BTreeMap<TypeId, u64> = BTreeMap::new();
    let mut missing_recipe = false;
    for job in jobs.iter().filter(|job| is_running(job)) {
        let Some(product) = job.product_type_id else {
            continue;
        };
        let Some(activity) = activity_of(job.activity_id) else {
            continue;
        };
        let Some(recipe) = recipes
            .get(product)
            .filter(|recipe| recipe.activity == activity)
        else {
            missing_recipe = true;
            continue;
        };
        let units = u64::from(job.runs)
            .checked_mul(recipe.output_per_run)
            .ok_or_else(|| TOO_LARGE.to_owned())?;
        let total = output.entry(product).or_insert(0);
        *total = total
            .checked_add(units)
            .ok_or_else(|| TOO_LARGE.to_owned())?;
    }
    Ok((output, missing_recipe))
}

fn is_running(job: &Job) -> bool {
    matches!(
        job.status,
        JobStatus::Active | JobStatus::Paused | JobStatus::Ready
    ) && activity_of(job.activity_id).is_some()
}

/// The types of the held assets and of the job outputs of every member of the latest point.
fn latest_types(store: &mut Store, now: i64) -> Result<BTreeSet<TypeId>, String> {
    let records = store.records(0, now).map_err(|error| error.to_string())?;
    let mut types = BTreeSet::new();
    let Some(point) = points(&records).points.last().cloned() else {
        return Ok(types);
    };
    for member in point.members.values() {
        if let Snapshot::Seen(holdings) = &member.snapshot {
            types.extend(holdings.assets.keys().copied());
            types.extend(holdings.jobs_output.keys().copied());
        }
    }
    Ok(types)
}

/// Saves the held prices of the assets, the job outputs and the orders' items at `now`.
fn copy_prices(shared: &mut Shared, now: i64, recorded: &[u64]) {
    let held = shared.market.held();
    let Some(published_at) = held.published_at else {
        return;
    };
    let mut types = match latest_types(&mut shared.store, now) {
        Ok(types) => types,
        Err(error) => {
            shared.notices.push(Notice::error(format!(
                "Could not save the prices of this point: {error}"
            )));
            return;
        }
    };
    for id in recorded {
        let orders = shared
            .session
            .character_data(*id)
            .and_then(|data| data.orders.as_ref());
        if let Some(orders) = orders {
            types.extend(orders.data.iter().map(|order| order.type_id));
        }
    }
    let rows = held
        .rows
        .iter()
        .filter(|((type_id, _), _)| types.contains(type_id))
        .map(|(key, row)| (*key, row.clone()))
        .collect();
    let prices = PointPrices {
        at: now,
        published_at: Some(published_at),
        rows,
    };
    if let Err(error) = shared.store.save_point_prices(&prices) {
        shared.notices.push(Notice::error(format!(
            "Could not save the prices of this point: {error}"
        )));
    }
}

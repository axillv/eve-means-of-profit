//! The job cost of each built step and the build-or-buy decision of each part, for the Steps table.
//! Each activity's set-up comes from the facility the plan picks; a set-up that cannot be made
//! gives the message the table shows instead of a figure.

use std::collections::{BTreeMap, BTreeSet};

use egui::{Context, Id, Ui};
use mop_core::industry::{Activity, Modifier, RecipeBook, TypeId};
use mop_core::job_cost::{CostSetup, SCC_SURCHARGE, StepCost, step_cost};
use mop_core::make_or_buy::{Decision, MakeOrBuyInput, make_or_buy};
use mop_core::money::Money;
use mop_core::plan::Plan;
use mop_core::volume::Volume;
use mop_sde::{Facility, Sde, Security};
use mop_store::{FacilityCosts, FacilityRow, PlanRecord};

use super::buy_list;
use super::prices;
use super::state::{BuildPage, NO_REACTIONS, me_of};
use crate::Shared;

/// Shown while the industry data is still loading.
pub const PENDING: &str = "…";

/// What the cost code worked out for the Steps table this frame.
#[derive(Debug, Default)]
pub struct Costs {
    /// The job cost of each built step, or the message that says why it has none.
    pub jobs: BTreeMap<TypeId, Result<StepCost, String>>,
    /// The decision of each part that has a recipe.
    pub decisions: BTreeMap<TypeId, Decision>,
    /// The reason line of each decision, with item names.
    pub reasons: BTreeMap<TypeId, String>,
    /// The set-up message of each part whose activity cannot be costed.
    pub messages: BTreeMap<TypeId, String>,
    /// Seconds since the cost indices were fetched, `None` when not fetched.
    pub systems_age: Option<i64>,
    /// Seconds since the adjusted prices were fetched, `None` when not fetched.
    pub prices_age: Option<i64>,
    /// Industry data is still loading: the figures show as pending and the total stays plain.
    pub pending: bool,
    /// A cost or comparison that could not be worked out; the figures are then empty.
    pub error: Option<String>,
}

impl Costs {
    /// The sum of the built steps' totals, and whether any of them is incomplete.
    pub fn total(&self) -> (Option<Money>, bool) {
        let mut sum = Some(Money::from_ten_thousandths(0));
        let mut incomplete = !self.messages.is_empty();
        for outcome in self.jobs.values() {
            match outcome {
                Ok(step) => {
                    incomplete |= !step.missing_prices.is_empty();
                    sum = sum.and_then(|sum| sum.checked_add(step.cost.total));
                }
                Err(_) => incomplete = true,
            }
        }
        (sum, incomplete)
    }
}

/// The facility an activity is costed at, from the plan: the saved one, or the NPC station for
/// manufacturing when none is picked. `None` when a reaction has no reactions facility.
fn facility_of(
    record: &PlanRecord,
    facilities: &[FacilityRow],
    activity: Activity,
) -> Option<Facility> {
    match activity {
        Activity::Manufacturing => match record.manufacturing {
            None => Some(NPC_STATION),
            Some(id) => facilities
                .iter()
                .find(|row| row.id == id)
                .map(|row| row.facility.clone()),
        },
        Activity::Reaction => {
            let id = record.reactions?;
            facilities
                .iter()
                .find(|row| row.id == id)
                .map(|row| row.facility.clone())
        }
    }
}

/// The NPC station in high sec, used when no Manufacturing facility is picked.
const NPC_STATION: Facility = Facility {
    structure: None,
    security: Security::High,
    rigs: Vec::new(),
};

/// How long the saved facilities and their costs are kept before the store is read again. The
/// frames in between reuse the last read, like the Buy list's freight services.
const FACILITIES_KEPT_SECS: f64 = 1.0;

/// The saved facilities and each one's system and tax (or why they cannot be read), and when
/// they were read.
#[derive(Clone)]
struct SavedFacilities {
    read_at: f64,
    rows: Vec<FacilityRow>,
    costs: BTreeMap<i64, Result<FacilityCosts, String>>,
}

fn facilities_key() -> Id {
    Id::new("build costs saved facilities")
}

/// The saved facilities and their costs, read from the store at most once a second.
fn saved_facilities(ctx: &Context, cx: &mut Shared) -> Result<SavedFacilities, String> {
    let now = ctx.input(|input| input.time);
    let kept = ctx.data(|data| data.get_temp::<SavedFacilities>(facilities_key()));
    if let Some(kept) = kept
        && now - kept.read_at < FACILITIES_KEPT_SECS
    {
        return Ok(kept);
    }
    let rows = cx.store.facilities().map_err(|error| error.to_string())?;
    let costs = rows
        .iter()
        .map(|row| {
            let costs = cx
                .store
                .facility_costs(row.id)
                .map_err(|error| error.to_string());
            (row.id, costs)
        })
        .collect();
    let read = SavedFacilities {
        read_at: now,
        rows,
        costs,
    };
    ctx.data_mut(|data| data.insert_temp(facilities_key(), read.clone()));
    Ok(read)
}

/// The job cost set-up of one activity, or the message that says why it cannot be made.
fn setup(
    activity: Activity,
    record: &PlanRecord,
    saved_facilities: &SavedFacilities,
    cx: &Shared,
) -> Result<CostSetup, String> {
    let saved = match activity {
        Activity::Manufacturing => record.manufacturing,
        Activity::Reaction => record.reactions,
    };
    let Some(id) = saved else {
        return Err(match activity {
            Activity::Manufacturing => "pick a saved facility for job costs".to_owned(),
            Activity::Reaction => NO_REACTIONS.to_owned(),
        });
    };
    let gone = || "the facility is no longer saved".to_owned();
    let row = saved_facilities
        .rows
        .iter()
        .find(|row| row.id == id)
        .ok_or_else(gone)?;
    let facility = row.facility.clone();
    let costs = saved_facilities.costs.get(&id).ok_or_else(gone)?.clone()?;
    let system = costs
        .system
        .ok_or_else(|| format!("no system on {}", row.name))?;
    let tax = costs.tax.ok_or_else(|| format!("no tax on {}", row.name))?;
    if cx.industry.systems.is_none() {
        return Err(PENDING.to_owned());
    }
    let sde = cx
        .sde
        .as_ref()
        .ok_or_else(|| "the game data is not loaded yet".to_owned())?;
    let index = cx.industry.cost_index(system, activity).ok_or_else(|| {
        let name = sde
            .system(system)
            .ok()
            .flatten()
            .map_or_else(|| system.to_string(), |found| found.name);
        format!("no cost index for {name}")
    })?;
    let structure_multiplier = match facility.structure {
        Some(structure) => sde
            .structure_job_cost(structure)
            .map_err(|error| error.to_string())?,
        None => None,
    };
    Ok(CostSetup {
        cost_index: index,
        structure_multiplier,
        facility_tax: tax,
        scc: SCC_SURCHARGE,
    })
}

/// Every type reachable from the product through the recipes, the product included.
fn reachable(product: TypeId, recipes: &RecipeBook) -> BTreeSet<TypeId> {
    let mut seen = BTreeSet::new();
    let mut stack = vec![product];
    while let Some(type_id) = stack.pop() {
        if !seen.insert(type_id) {
            continue;
        }
        if let Some(recipe) = recipes.get(type_id) {
            stack.extend(recipe.materials.iter().map(|(material, _)| *material));
        }
    }
    seen
}

/// The modifier of every type with a recipe below the product. A type whose modifier cannot be
/// read gets none; the decision then leaves it out.
fn modifiers(
    record: &PlanRecord,
    types: &BTreeSet<TypeId>,
    recipes: &RecipeBook,
    sde: &Sde,
    facilities: &[FacilityRow],
) -> BTreeMap<TypeId, Modifier> {
    let mut modifiers = BTreeMap::new();
    for &type_id in types {
        let Some(recipe) = recipes.get(type_id) else {
            continue;
        };
        let (facility, me) = match recipe.activity {
            Activity::Manufacturing => (
                facility_of(record, facilities, Activity::Manufacturing),
                me_of(record, type_id, recipe.made_by_invention),
            ),
            Activity::Reaction => (facility_of(record, facilities, Activity::Reaction), 0),
        };
        let Some(facility) = facility else {
            continue;
        };
        if let Ok(modifier) = sde.modifier(&facility, recipe.activity, type_id, me) {
            modifiers.insert(type_id, modifier);
        }
    }
    modifiers
}

/// Works out the costs and decisions of the plan shown. Nothing is requested here: the industry
/// data and the prices are read from what is held.
pub fn compute(
    ui: &Ui,
    page: &BuildPage,
    record: &PlanRecord,
    plan: &Plan,
    cx: &mut Shared,
    now: i64,
) -> Costs {
    let mut costs = Costs {
        systems_age: cx
            .industry
            .systems
            .as_ref()
            .map(|fetched| now - fetched.fetched_at),
        prices_age: cx
            .industry
            .prices
            .as_ref()
            .map(|fetched| now - fetched.fetched_at),
        pending: cx.industry.systems.is_none() || cx.industry.prices.is_none(),
        ..Costs::default()
    };
    match compute_inner(ui, page, record, plan, cx, now, &mut costs) {
        Ok(()) => costs,
        Err(error) => Costs {
            error: Some(error),
            systems_age: costs.systems_age,
            prices_age: costs.prices_age,
            pending: costs.pending,
            ..Costs::default()
        },
    }
}

fn compute_inner(
    ui: &Ui,
    page: &BuildPage,
    record: &PlanRecord,
    plan: &Plan,
    cx: &mut Shared,
    now: i64,
    costs: &mut Costs,
) -> Result<(), String> {
    let saved_facilities = saved_facilities(ui.ctx(), cx)?;
    let facilities = &saved_facilities.rows;
    let manufacturing = setup(Activity::Manufacturing, record, &saved_facilities, cx);
    let reaction = setup(Activity::Reaction, record, &saved_facilities, cx);
    if costs.pending {
        for step in &plan.steps {
            costs.jobs.insert(step.type_id, Err(PENDING.to_owned()));
        }
        return Ok(());
    }
    let recipes = &page.recipes;
    let adjusted = cx.industry.adjusted_prices();
    let setup_of = |activity: Activity| match activity {
        Activity::Manufacturing => &manufacturing,
        Activity::Reaction => &reaction,
    };
    for step in &plan.steps {
        let Some(recipe) = recipes.get(step.type_id) else {
            continue;
        };
        let outcome = match setup_of(step.activity) {
            Ok(setup) => Ok(step_cost(&recipe.materials, &step.jobs, &adjusted, setup)
                .map_err(|error| error.to_string())?),
            Err(message) => Err(message.clone()),
        };
        if let Err(message) = &outcome {
            costs.messages.insert(step.type_id, message.clone());
        }
        costs.jobs.insert(step.type_id, outcome);
    }
    let held = cx.market.held();
    // A buy list that cannot be worked out has no chosen freight: the decision would then leave
    // freight out silently, so it is the costs' error line instead.
    let card = buy_list::chosen_card(ui, record, plan, cx, &held, now)?;
    let types = reachable(record.product, recipes);
    let modifiers = modifiers(
        record,
        &types,
        recipes,
        cx.sde.as_ref().ok_or("the game data is not loaded yet")?,
        facilities,
    );
    let sde = cx.sde.as_ref().ok_or("the game data is not loaded yet")?;
    let volumes: BTreeMap<TypeId, Volume> = types
        .iter()
        .filter_map(|&type_id| {
            let volume = sde.item(type_id).ok().flatten()?.haul_volume()?;
            Some((type_id, volume))
        })
        .collect();
    let unit_price = prices::decision_prices(
        &held,
        record.price_rule,
        types.iter().copied(),
        |type_id| planned_quantity(plan, type_id),
        |type_id| cx.market.manual(record.id, type_id),
    );
    let input = MakeOrBuyInput {
        plan,
        recipes,
        modifiers: &modifiers,
        unit_price: &unit_price,
        unit_volume: &volumes,
        freight: card.as_ref(),
        adjusted: &adjusted,
        manufacturing: manufacturing.clone().ok(),
        reaction: reaction.clone().ok(),
    };
    let decisions = make_or_buy(&input).map_err(|error| error.to_string())?;
    for (type_id, decision) in &decisions {
        if let Some(recipe) = recipes.get(*type_id)
            && let Err(message) = setup_of(recipe.activity)
        {
            costs.messages.insert(*type_id, message.clone());
        }
        costs
            .reasons
            .insert(*type_id, reason_with_names(decision, cx.sde.as_ref()));
        costs.decisions.insert(*type_id, decision.clone());
    }
    Ok(())
}

/// A decision's reason line, with item names where the game data knows them.
fn reason_with_names(decision: &Decision, sde: Option<&Sde>) -> String {
    decision.reason_text_named(|id| {
        sde.and_then(|sde| sde.item(id).ok().flatten())
            .map_or_else(|| format!("type {id}"), |item| item.name)
    })
}

/// How many units of a type the plan needs, or 1 for a type only below a bought part.
fn planned_quantity(plan: &Plan, type_id: TypeId) -> u64 {
    plan.steps
        .iter()
        .find(|step| step.type_id == type_id)
        .map(|step| step.needed)
        .or_else(|| {
            plan.buy
                .iter()
                .find(|line| line.type_id == type_id)
                .map(|line| line.needed)
        })
        .unwrap_or(1)
}

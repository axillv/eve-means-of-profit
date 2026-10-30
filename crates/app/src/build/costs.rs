//! The job cost of each built step and the build-or-buy decision of each part, for the Steps table.
//! Each activity's set-up comes from the facility the plan picks; a set-up that cannot be made
//! gives the message the table shows instead of a figure.

use std::collections::{BTreeMap, BTreeSet};

use egui::{Context, Id, Ui};
use mop_core::industry::{Activity, Modifier, Recipe, RecipeBook, TypeId};
use mop_core::invention::{AttemptRule, Decryptor, Skills};
use mop_core::job_cost::{CostSetup, SCC_SURCHARGE, StepCost, job_cost, step_cost};
use mop_core::make_or_buy::{Decision, MakeOrBuyInput, make_or_buy};
use mop_core::money::Money;
use mop_core::plan::{Plan, PlanProduct};
use mop_core::volume::Volume;
use mop_sde::invention::{InventionActivity, InventionProduct, split_skills};
use mop_sde::{Facility, Sde, Security};
use mop_store::{FacilityCosts, FacilityRow, InventionChoice, PlanRecord};

use super::buy_list;
use super::invention::{
    InventionInput, InventionLine, Numbers, fee_per_attempt, invention_numbers, skills_for,
};
use super::prices::{self, Held};
use super::state::{BuildPage, NO_REACTIONS, me_with, type_name};
use crate::Shared;

/// Shown while the industry data is still loading.
pub const PENDING: &str = "…";

/// The text for a part whose invention skills cannot be split into two sciences and an
/// encryption skill.
const SKILLS_UNUSABLE: &str = "Invention: this item needs skills the app cannot use yet.";

/// What the Steps table shows of one T2 part's invention: its numbers, or why it has none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InventionShown {
    /// The plan does not invent the part.
    Not,
    /// The numbers cannot be worked out; the text says why.
    Unavailable(String),
    /// The numbers the part is invented at.
    Counted {
        line: InventionLine,
        numbers: Numbers,
        fee: Money,
        /// Why the fee is not in the numbers, when it could not be worked out.
        fee_note: Option<String>,
    },
}

/// The invention of every T2 part the plan builds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inventions {
    /// The shown numbers of each part, or why it has none.
    pub parts: BTreeMap<TypeId, InventionShown>,
    /// A shown part has a skill with no saved level. A level of 0 that was typed is saved.
    pub skills_unset: bool,
    /// How many attempts the plan buys.
    pub rule: AttemptRule,
}

impl Default for Inventions {
    fn default() -> Self {
        Self {
            parts: BTreeMap::new(),
            skills_unset: false,
            rule: AttemptRule::Expected,
        }
    }
}

/// What one part's invention numbers are worked out from.
pub struct CountInput<'a> {
    /// The T2 part the plan builds. The line names it; the product names the blueprint invented.
    pub type_id: TypeId,
    pub activity: &'a InventionActivity,
    pub product: &'a InventionProduct,
    pub t2_runs: u64,
    pub choice: InventionChoice,
    pub skills: Option<Skills>,
    pub rule: AttemptRule,
    /// The datacores an attempt uses: quantity per attempt and unit price, in file order.
    pub datacores: &'a [(u64, Money)],
    pub decryptor: Option<&'a Decryptor>,
    pub decryptor_price: Option<Money>,
    /// The fee of one attempt, or why it could not be worked out.
    pub fee: Result<Money, String>,
}

/// The invention numbers of one part, or why it has none.
pub fn count_part(input: &CountInput<'_>) -> InventionShown {
    if !input.choice.invent {
        return InventionShown::Not;
    }
    let Some(skills) = input.skills else {
        return InventionShown::Unavailable(SKILLS_UNUSABLE.to_owned());
    };
    if let Some(id) = input.choice.decryptor
        && input.decryptor.is_none()
    {
        return InventionShown::Unavailable(format!("Invention: type {id} is not a decryptor."));
    }
    // A fee that cannot be worked out leaves the materials' cost alone and says so on the line.
    let (fee, fee_note) = match &input.fee {
        Ok(fee) => (*fee, None),
        Err(text) => (Money::ZERO, Some(format!("fee not counted: {text}"))),
    };
    match invention_numbers(&InventionInput {
        product: input.product,
        t2_runs: input.t2_runs,
        skills,
        decryptor: input.decryptor,
        rule: input.rule,
        datacores: input.datacores,
        decryptor_price: input.decryptor_price,
        fee,
    }) {
        Ok(numbers) => InventionShown::Counted {
            line: InventionLine {
                type_id: input.type_id,
                need: numbers.need,
                attempt_cost: numbers.attempt_cost,
                materials: input.activity.materials.clone(),
                decryptor: input.decryptor.map(|decryptor| decryptor.type_id),
            },
            numbers,
            fee,
            fee_note,
        },
        Err(error) => InventionShown::Unavailable(format!("Invention: {error}")),
    }
}

/// What every part of a plan is worked out from.
struct PartSetup<'a> {
    record: &'a PlanRecord,
    saved_facilities: &'a SavedFacilities,
    manufacturing: &'a Result<CostSetup, String>,
    adjusted: &'a BTreeMap<TypeId, Money>,
    prices: &'a BTreeMap<TypeId, Money>,
    saved_skills: &'a BTreeMap<TypeId, u8>,
    rule: AttemptRule,
    recipes: &'a RecipeBook,
}

/// One T2 part the plan builds, with what its invention is worked out from.
struct Part {
    type_id: TypeId,
    runs: u64,
    activity: InventionActivity,
    product: InventionProduct,
    choice: InventionChoice,
    decryptor: Option<Decryptor>,
}

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
    /// The invention numbers of each built T2 part.
    pub inventions: Inventions,
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
pub(super) struct SavedFacilities {
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
pub(super) fn setup(
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

/// Every type reachable from any of the products through the recipes, the products included.
fn reachable(products: &[PlanProduct], recipes: &RecipeBook) -> BTreeSet<TypeId> {
    let mut seen = BTreeSet::new();
    let mut stack: Vec<TypeId> = products.iter().map(|product| product.type_id).collect();
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

/// The modifier of every type with a recipe that the plan can build: the types reachable from the
/// products, the record's Build ticks and every product. A type whose modifier cannot be read gets
/// none; the decision then leaves it out.
fn modifiers(
    record: &PlanRecord,
    types: &BTreeSet<TypeId>,
    products: &[PlanProduct],
    recipes: &RecipeBook,
    sde: &Sde,
    facilities: &[FacilityRow],
    copy_mes: &BTreeMap<TypeId, u8>,
) -> BTreeMap<TypeId, Modifier> {
    let covered: BTreeSet<TypeId> = types
        .iter()
        .copied()
        .chain(record.build.iter().copied())
        .chain(products.iter().map(|product| product.type_id))
        .collect();
    let mut modifiers = BTreeMap::new();
    for type_id in covered {
        let Some(recipe) = recipes.get(type_id) else {
            continue;
        };
        let (facility, me) = match recipe.activity {
            Activity::Manufacturing => (
                facility_of(record, facilities, Activity::Manufacturing),
                me_with(record, type_id, recipe.made_by_invention, copy_mes),
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

/// The invention numbers of every T2 part the plan builds, worked out from the saved facilities,
/// the manufacturing set-up and the prices held. Empty while the industry data loads.
///
/// The buy list calls this too; it is not part of [`compute`], so that the two can share it
/// without either working everything out.
pub fn invention_lines(
    ui: &Ui,
    page: &BuildPage,
    record: &PlanRecord,
    plan: &Plan,
    cx: &mut Shared,
    held: &Held,
) -> Result<Inventions, String> {
    let rule = cx.store.attempt_rule().map_err(|error| error.to_string())?;
    let mut inventions = Inventions {
        rule,
        ..Inventions::default()
    };
    if cx.industry.systems.is_none() || cx.industry.prices.is_none() || cx.sde.is_none() {
        return Ok(inventions);
    }
    let saved_facilities = saved_facilities(ui.ctx(), cx)?;
    let manufacturing = setup(Activity::Manufacturing, record, &saved_facilities, cx);
    let adjusted = cx.industry.adjusted_prices();
    let saved_skills = cx
        .store
        .invention_skills()
        .map_err(|error| error.to_string())?;
    let sde = cx.sde.as_ref().ok_or("the game data is not loaded yet")?;
    let parts = part_sources(plan, page, sde);
    inventions.skills_unset = parts
        .iter()
        .any(|part| skills_missing(&part.activity, &saved_skills));
    let mut types = BTreeSet::new();
    for part in &parts {
        types.extend(part.activity.materials.iter().map(|(type_id, _)| *type_id));
        if let Some(decryptor) = &part.decryptor {
            types.insert(decryptor.type_id);
        }
    }
    let prices = prices::decision_prices(
        held,
        record.price_rule,
        types,
        |_| 1,
        |type_id| cx.market.manual(record.id, type_id),
    );
    let setup = PartSetup {
        record,
        saved_facilities: &saved_facilities,
        manufacturing: &manufacturing,
        adjusted: &adjusted,
        prices: &prices,
        saved_skills: &saved_skills,
        rule,
        recipes: &page.recipes,
    };
    for part in &parts {
        let shown = shown_part(part, &setup, cx);
        inventions.parts.insert(part.type_id, shown);
    }
    Ok(inventions)
}

/// The lines the Buy list shows: the counted invention of each part, with the parts the plan
/// does not invent or cannot count left out.
fn counted_lines(inventions: &Inventions) -> Vec<InventionLine> {
    inventions
        .parts
        .values()
        .filter_map(|shown| match shown {
            InventionShown::Counted { line, .. } => Some(line.clone()),
            InventionShown::Not | InventionShown::Unavailable(_) => None,
        })
        .collect()
}

/// The plan's T2 steps: made by invention and with at least one run, one entry per type. A type
/// the game data has no invention for is left out here.
fn t2_steps(plan: &Plan, recipes: &RecipeBook) -> Vec<(TypeId, u64)> {
    plan.steps
        .iter()
        .filter(|step| step.runs >= 1)
        .filter(|step| {
            recipes
                .get(step.type_id)
                .is_some_and(|recipe| recipe.made_by_invention)
        })
        .map(|step| (step.type_id, step.runs))
        .collect::<BTreeMap<_, _>>()
        .into_iter()
        .collect()
}

/// The plan's T2 parts, with their invention activities, products, choices and decryptors.
fn part_sources(plan: &Plan, page: &BuildPage, sde: &Sde) -> Vec<Part> {
    let mut parts = BTreeMap::new();
    for (type_id, runs) in t2_steps(plan, &page.recipes) {
        let Ok(Some((activity, product))) = sde.invention_for(type_id) else {
            continue;
        };
        let choice = page.invention.get(&type_id).copied().unwrap_or_default();
        let decryptor = match choice.decryptor {
            Some(id) if choice.invent => sde.decryptor(id).unwrap_or_default(),
            _ => None,
        };
        parts.insert(
            type_id,
            Part {
                type_id,
                runs,
                activity,
                product,
                choice,
                decryptor,
            },
        );
    }
    parts.into_values().collect()
}

/// Whether any of an activity's three skills has no saved level. A typed 0 is a saved level.
fn skills_missing(activity: &InventionActivity, saved: &BTreeMap<TypeId, u8>) -> bool {
    split_skills(activity).is_some_and(|(one, two, encryption)| {
        [one, two, encryption]
            .iter()
            .any(|type_id| !saved.contains_key(type_id))
    })
}

/// One part's shown invention, or why it has none.
fn shown_part(part: &Part, setup: &PartSetup<'_>, cx: &mut Shared) -> InventionShown {
    if !part.choice.invent {
        return InventionShown::Not;
    }
    let skills = skills_for(&part.activity, setup.saved_skills);
    if skills.is_none() {
        return InventionShown::Unavailable(SKILLS_UNUSABLE.to_owned());
    }
    if let Some(id) = part.choice.decryptor
        && part.decryptor.is_none()
    {
        return InventionShown::Unavailable(format!("Invention: type {id} is not a decryptor."));
    }
    let datacores = match priced_datacores(&part.activity.materials, setup.prices) {
        Ok(datacores) => datacores,
        Err(type_id) => {
            let name = type_name(type_id, cx.sde.as_ref(), &mut cx.notices);
            return InventionShown::Unavailable(no_price(&name));
        }
    };
    let decryptor_price = match &part.decryptor {
        Some(decryptor) => match setup.prices.get(&decryptor.type_id) {
            Some(price) => Some(*price),
            None => {
                let name = type_name(decryptor.type_id, cx.sde.as_ref(), &mut cx.notices);
                return InventionShown::Unavailable(no_price(&name));
            }
        },
        None => None,
    };
    let fee = match setup.recipes.get(part.type_id) {
        Some(recipe) => attempt_fee(setup, recipe, cx),
        None => Err("the part has no recipe".to_owned()),
    };
    count_part(&CountInput {
        type_id: part.type_id,
        activity: &part.activity,
        product: &part.product,
        t2_runs: part.runs,
        choice: part.choice,
        skills,
        rule: setup.rule,
        datacores: &datacores,
        decryptor: part.decryptor.as_ref(),
        decryptor_price,
        fee,
    })
}

/// The unit prices of an activity's datacores, in file order, with the first type without one.
fn priced_datacores(
    materials: &[(TypeId, u64)],
    prices: &BTreeMap<TypeId, Money>,
) -> Result<Vec<(u64, Money)>, TypeId> {
    let mut datacores = Vec::new();
    for &(type_id, quantity) in materials {
        let Some(price) = prices.get(&type_id) else {
            return Err(type_id);
        };
        datacores.push((quantity, *price));
    }
    Ok(datacores)
}

/// The text for a price the plan has not got.
fn no_price(name: &str) -> String {
    format!("Invention: no price for {name}.")
}

/// The fee of one invention attempt of a part: a one-run T2 job's item value at the manufacturing
/// set-up, charged at the invention cost index of the facility's system.
fn attempt_fee(setup: &PartSetup<'_>, recipe: &Recipe, cx: &Shared) -> Result<Money, String> {
    let manufacturing = setup.manufacturing.as_ref().map_err(|text| text.clone())?;
    let item_value = job_cost(&recipe.materials, 1, setup.adjusted, manufacturing)
        .map_err(|error| error.to_string())?
        .cost
        .item_value;
    let system = manufacturing_system(setup.record, setup.saved_facilities)?;
    let index = cx.industry.invention_index(system).ok_or_else(|| {
        format!(
            "no invention cost index for {}",
            system_name(cx.sde.as_ref(), system)
        )
    })?;
    fee_per_attempt(item_value, manufacturing, index)
}

/// The system of the plan's saved manufacturing facility, or why it has none.
fn manufacturing_system(record: &PlanRecord, saved: &SavedFacilities) -> Result<u32, String> {
    let Some(id) = record.manufacturing else {
        return Err("pick a saved facility for job costs".to_owned());
    };
    let gone = || "the facility is no longer saved".to_owned();
    let row = saved
        .rows
        .iter()
        .find(|row| row.id == id)
        .ok_or_else(gone)?;
    let costs = saved.costs.get(&id).ok_or_else(gone)?.clone()?;
    costs
        .system
        .ok_or_else(|| format!("no system on {}", row.name))
}

/// The name of a system in the game data, or its id when the data has no such system.
fn system_name(sde: Option<&Sde>, system: u32) -> String {
    sde.and_then(|sde| sde.system(system).ok().flatten())
        .map_or_else(|| system.to_string(), |found| found.name)
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
    costs.inventions = invention_lines(ui, page, record, plan, cx, &held)?;
    // A buy list that cannot be worked out has no chosen freight: the decision would then leave
    // freight out silently, so it is the costs' error line instead.
    let invention = counted_lines(&costs.inventions);
    let card = buy_list::chosen_card(ui, record, plan, cx, &held, now, &invention)?;
    let types = reachable(&page.products, recipes);
    let modifiers = modifiers(
        record,
        &types,
        &page.products,
        recipes,
        cx.sde.as_ref().ok_or("the game data is not loaded yet")?,
        facilities,
        &page.copy_mes,
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

#[cfg(test)]
mod tests {
    use mop_core::industry::{JobSplit, Recipe};
    use mop_core::invention::InventedCopy;
    use mop_core::money::Rate;
    use mop_core::plan::Step;
    use mop_sde::invention::SkillRequirement;
    use mop_sde::{Sde, import};
    use std::path::Path;
    use tempfile::TempDir;

    use super::*;

    const HOBGOBLIN_II: TypeId = 2456;
    const HOBGOBLIN_I: TypeId = 2454;
    /// The blueprint the invention makes for Hobgoblin II, as the game data's product line gives it.
    const HOBGOBLIN_II_BLUEPRINT: TypeId = 2457;

    const ATTAINMENT: Decryptor = Decryptor {
        type_id: 34202,
        chance_hundredths: 180,
        me: -1,
        te: 4,
        runs: 4,
    };

    const SKILLS_444: Skills = Skills {
        science1: 4,
        science2: 4,
        encryption: 4,
    };

    fn isk(text: &str) -> Money {
        Money::parse_isk(text).expect("valid ISK amount")
    }

    /// The invention product line of Hobgoblin II: the blueprint it makes, not the part.
    fn product() -> InventionProduct {
        InventionProduct {
            type_id: HOBGOBLIN_II_BLUEPRINT,
            probability: Rate::from_ppm(340_000),
            quantity: 10,
        }
    }

    /// Hobgoblin II's invention activity, with the two sciences before the encryption skill.
    fn activity() -> InventionActivity {
        InventionActivity {
            blueprint: 0,
            products: vec![product()],
            materials: vec![(20418, 1), (20419, 1)],
            skills: vec![
                SkillRequirement {
                    type_id: 11453,
                    name: "Gallente Starship Engineering".to_owned(),
                    level: 1,
                },
                SkillRequirement {
                    type_id: 11446,
                    name: "Mechanical Engineering".to_owned(),
                    level: 1,
                },
                SkillRequirement {
                    type_id: 23121,
                    name: "Gallente Encryption Methods".to_owned(),
                    level: 1,
                },
            ],
        }
    }

    fn datacores() -> Vec<(u64, Money)> {
        vec![(1, isk("50,000.00")), (1, isk("43,750.00"))]
    }

    /// The acceptance's inputs with the default choice and a fee of 89.00; a test overrides what
    /// it changes.
    fn input<'a>(
        activity: &'a InventionActivity,
        product: &'a InventionProduct,
        datacores: &'a [(u64, Money)],
    ) -> CountInput<'a> {
        CountInput {
            type_id: HOBGOBLIN_II,
            activity,
            product,
            t2_runs: 8,
            choice: InventionChoice::default(),
            skills: Some(SKILLS_444),
            rule: AttemptRule::Expected,
            datacores,
            decryptor: None,
            decryptor_price: None,
            fee: Ok(isk("89.00")),
        }
    }

    /// The parts of a counted line, or a panic naming what was shown instead.
    fn counted(shown: &InventionShown) -> (&InventionLine, &Numbers, Money, Option<&str>) {
        match shown {
            InventionShown::Counted {
                line,
                numbers,
                fee,
                fee_note,
            } => (line, numbers, *fee, fee_note.as_deref()),
            other @ InventionShown::Not | other @ InventionShown::Unavailable(_) => {
                panic!("expected counted numbers, got {other:?}")
            }
        }
    }

    #[test]
    fn counted_lines_keeps_only_the_counted_parts() {
        let activity = activity();
        let product = product();
        let datacores = datacores();
        let inventions = Inventions {
            parts: BTreeMap::from([
                (1, InventionShown::Not),
                (2, InventionShown::Unavailable("why".to_owned())),
                (
                    HOBGOBLIN_II,
                    count_part(&input(&activity, &product, &datacores)),
                ),
            ]),
            ..Inventions::default()
        };
        let lines = counted_lines(&inventions);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].type_id, HOBGOBLIN_II);
        assert_eq!(lines[0].materials, vec![(20418, 1), (20419, 1)]);
    }

    fn recipe(made_by_invention: bool) -> Recipe {
        Recipe {
            activity: Activity::Manufacturing,
            output_per_run: 1,
            max_runs_per_job: 10,
            materials: Vec::new(),
            made_by_invention,
        }
    }

    fn step(type_id: TypeId, runs: u64) -> Step {
        Step {
            type_id,
            activity: Activity::Manufacturing,
            depth: 0,
            needed: runs,
            from_stock: 0,
            to_build: runs,
            runs,
            jobs: JobSplit {
                full_jobs: 0,
                runs_per_full_job: 1,
                last_job_runs: runs,
            },
            spare: 0,
        }
    }

    #[test]
    fn the_default_choice_gives_the_expected_numbers() {
        let activity = activity();
        let product = product();
        let datacores = datacores();
        let shown = count_part(&input(&activity, &product, &datacores));
        let (line, numbers, fee, fee_note) = counted(&shown);
        assert_eq!(numbers.chance.percent_string(), "46.47");
        assert_eq!(numbers.chance.ppm(), 464_667);
        assert_eq!(
            numbers.copy,
            InventedCopy {
                runs: 10,
                me: 2,
                te: 4,
            }
        );
        assert_eq!(numbers.need.copies, 1);
        assert_eq!(numbers.need.attempts, 3);
        assert_eq!(numbers.attempt_cost.format_isk(), "93,839.00");
        assert_eq!(numbers.need.cost_per_copy.format_isk(), "201,949.07");
        assert_eq!(numbers.need.carried.format_isk(), "161,559.25");
        assert_eq!(numbers.need.spare_runs, 2);
        assert_eq!(line.type_id, HOBGOBLIN_II);
        assert_eq!(line.materials, vec![(20418, 1), (20419, 1)]);
        assert_eq!(line.decryptor, None);
        assert_eq!(line.need, numbers.need);
        assert_eq!(line.attempt_cost, numbers.attempt_cost);
        assert_eq!(fee, isk("89.00"));
        assert_eq!(fee_note, None);
    }

    #[test]
    fn the_attainment_decryptor_changes_the_numbers() {
        let activity = activity();
        let product = product();
        let datacores = datacores();
        let shown = count_part(&CountInput {
            choice: InventionChoice {
                invent: true,
                decryptor: Some(34202),
            },
            decryptor: Some(&ATTAINMENT),
            decryptor_price: Some(isk("1,200,000.00")),
            ..input(&activity, &product, &datacores)
        });
        let (line, numbers, _, fee_note) = counted(&shown);
        assert_eq!(numbers.chance.percent_string(), "83.64");
        assert_eq!(
            numbers.copy,
            InventedCopy {
                runs: 14,
                me: 1,
                te: 8,
            }
        );
        assert_eq!(numbers.need.copies, 1);
        assert_eq!(numbers.need.attempts, 2);
        assert_eq!(numbers.attempt_cost.format_isk(), "1,293,839.00");
        assert_eq!(numbers.need.cost_per_copy.format_isk(), "1,546,914.16");
        assert_eq!(numbers.need.carried.format_isk(), "883,950.95");
        assert_eq!(numbers.need.spare_runs, 6);
        assert_eq!(line.decryptor, Some(34202));
        assert_eq!(fee_note, None);
    }

    #[test]
    fn an_unticked_part_is_not_invented() {
        let activity = activity();
        let product = product();
        let datacores = datacores();
        let shown = count_part(&CountInput {
            choice: InventionChoice {
                invent: false,
                decryptor: None,
            },
            ..input(&activity, &product, &datacores)
        });
        assert_eq!(shown, InventionShown::Not);
    }

    #[test]
    fn skills_that_do_not_split_say_so() {
        let activity = activity();
        let product = product();
        let datacores = datacores();
        let shown = count_part(&CountInput {
            skills: None,
            ..input(&activity, &product, &datacores)
        });
        assert_eq!(
            shown,
            InventionShown::Unavailable(
                "Invention: this item needs skills the app cannot use yet.".to_owned()
            )
        );
    }

    #[test]
    fn a_decryptor_that_is_not_one_says_so() {
        let activity = activity();
        let product = product();
        let datacores = datacores();
        let shown = count_part(&CountInput {
            choice: InventionChoice {
                invent: true,
                decryptor: Some(999_999),
            },
            ..input(&activity, &product, &datacores)
        });
        assert_eq!(
            shown,
            InventionShown::Unavailable("Invention: type 999999 is not a decryptor.".to_owned())
        );
    }

    #[test]
    fn a_fee_that_cannot_be_worked_out_is_noted() {
        let activity = activity();
        let product = product();
        let datacores = datacores();
        let shown = count_part(&CountInput {
            fee: Err("no system on NPC Jita".to_owned()),
            ..input(&activity, &product, &datacores)
        });
        let (_, numbers, fee, fee_note) = counted(&shown);
        assert_eq!(fee, Money::ZERO);
        assert_eq!(fee_note, Some("fee not counted: no system on NPC Jita"));
        assert_eq!(numbers.attempt_cost.format_isk(), "93,750.00");
        assert_eq!(numbers.need.cost_per_copy.format_isk(), "201,757.53");
    }

    #[test]
    fn an_error_from_the_numbers_is_unavailable() {
        let activity = activity();
        let impossible = InventionProduct {
            probability: Rate::from_ppm(0),
            ..product()
        };
        let datacores = datacores();
        let shown = count_part(&input(&activity, &impossible, &datacores));
        assert_eq!(
            shown,
            InventionShown::Unavailable("Invention: the invention chance is zero".to_owned())
        );
    }

    #[test]
    fn the_first_datacore_without_a_price_is_the_missing_one() {
        let materials = [(20418, 1), (20419, 1)];
        let one = BTreeMap::from([(20418, isk("50,000.00"))]);
        assert_eq!(priced_datacores(&materials, &one), Err(20419));
        let both = BTreeMap::from([(20418, isk("50,000.00")), (20419, isk("43,750.00"))]);
        assert_eq!(
            priced_datacores(&materials, &both),
            Ok(vec![(1, isk("50,000.00")), (1, isk("43,750.00"))])
        );
        assert_eq!(
            no_price("Datacore - Graviton Physics"),
            "Invention: no price for Datacore - Graviton Physics."
        );
    }

    #[test]
    fn missing_skill_levels_are_told_apart_from_typed_zeros() {
        let activity = activity();
        let partial = BTreeMap::from([(11453, 4), (11446, 4)]);
        assert!(skills_missing(&activity, &partial));
        let typed_zero = BTreeMap::from([(11453, 4), (11446, 4), (23121, 0)]);
        assert!(!skills_missing(&activity, &typed_zero));
        let mut unsplittable = activity;
        unsplittable.skills.pop();
        assert!(!skills_missing(&unsplittable, &BTreeMap::new()));
    }

    #[test]
    fn only_t2_steps_with_runs_are_parts() {
        let mut recipes = RecipeBook::new();
        recipes.insert(HOBGOBLIN_II, recipe(true));
        recipes.insert(HOBGOBLIN_I, recipe(false));
        let plan = Plan {
            steps: vec![
                step(HOBGOBLIN_II, 8),
                step(HOBGOBLIN_I, 5),
                step(HOBGOBLIN_II, 0),
            ],
            buy: Vec::new(),
            unused_stock: Vec::new(),
        };
        assert_eq!(t2_steps(&plan, &recipes), vec![(HOBGOBLIN_II, 8)]);
    }

    #[test]
    fn a_plan_with_no_invented_step_has_no_parts() {
        let mut recipes = RecipeBook::new();
        recipes.insert(HOBGOBLIN_I, recipe(false));
        let plan = Plan {
            steps: vec![step(HOBGOBLIN_I, 5)],
            buy: Vec::new(),
            unused_stock: Vec::new(),
        };
        assert_eq!(t2_steps(&plan, &recipes), Vec::new());
    }

    #[test]
    fn inventions_default_to_no_parts_and_the_expected_rule() {
        let inventions = Inventions::default();
        assert!(inventions.parts.is_empty());
        assert!(!inventions.skills_unset);
        assert_eq!(inventions.rule, AttemptRule::Expected);
    }

    const RIFTER: TypeId = 587;
    const ROBOTICS: TypeId = 11_481;

    fn plan_product(type_id: TypeId, quantity: u64) -> PlanProduct {
        PlanProduct { type_id, quantity }
    }

    fn chain_recipe(
        activity: Activity,
        output: u64,
        max: u64,
        materials: &[(TypeId, u64)],
    ) -> Recipe {
        Recipe {
            activity,
            output_per_run: output,
            max_runs_per_job: max,
            materials: materials.to_vec(),
            made_by_invention: false,
        }
    }

    /// The recipes of the acceptance's products and their materials, from SDE build 3607705.
    fn game_book() -> RecipeBook {
        let mut book = RecipeBook::new();
        book.insert(
            ROBOTICS,
            chain_recipe(
                Activity::Manufacturing,
                100,
                200,
                &[(34, 556), (35, 444), (36, 222), (37, 82), (38, 36)],
            ),
        );
        book.insert(
            RIFTER,
            chain_recipe(
                Activity::Manufacturing,
                1,
                30,
                &[(34, 32_000), (35, 6_000), (36, 2_500), (37, 500)],
            ),
        );
        book
    }

    /// The build 3607705 fixture imported into `dir`.
    fn fixture_sde(dir: &TempDir) -> Sde {
        let source =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../sde/tests/fixtures/sde-3607705");
        let db = dir.path().join("sde.sqlite");
        import(&source, &db).expect("the fixture imports");
        Sde::open(&db).expect("the fixture opens")
    }

    fn record(build: BTreeSet<TypeId>) -> PlanRecord {
        PlanRecord {
            id: 1,
            name: "Test plan".to_owned(),
            product: ROBOTICS,
            quantity: 1,
            build,
            me: BTreeMap::new(),
            manufacturing: None,
            reactions: None,
            price_rule: Default::default(),
            route: None,
            service: None,
            packing: Default::default(),
            pasted_stock: String::new(),
            created_at: 0,
            updated_at: 0,
        }
    }

    #[test]
    fn reachable_covers_every_product_and_its_materials() {
        let products = [plan_product(ROBOTICS, 150), plan_product(RIFTER, 2)];
        assert_eq!(
            reachable(&products, &game_book()),
            BTreeSet::from([34, 35, 36, 37, 38, RIFTER, ROBOTICS])
        );
    }

    #[test]
    fn modifiers_cover_every_product_and_the_record_build_ticks() {
        let dir = TempDir::new().expect("a temp folder");
        let sde = fixture_sde(&dir);
        let recipes = sde.recipe_book().expect("the fixture recipe book");
        let products = [plan_product(ROBOTICS, 150), plan_product(RIFTER, 2)];
        let types = reachable(&products, &recipes);
        let found = modifiers(
            &record(BTreeSet::new()),
            &types,
            &products,
            &recipes,
            &sde,
            &[],
            &BTreeMap::new(),
        );
        assert!(found.contains_key(&ROBOTICS));
        assert!(found.contains_key(&RIFTER));

        // A type in the record's Build ticks gets a modifier even when no product reaches it.
        let types = reachable(&[plan_product(ROBOTICS, 150)], &recipes);
        let found = modifiers(
            &record(BTreeSet::from([HOBGOBLIN_II])),
            &types,
            &[plan_product(ROBOTICS, 150)],
            &recipes,
            &sde,
            &[],
            &BTreeMap::new(),
        );
        assert!(found.contains_key(&HOBGOBLIN_II));
    }
}

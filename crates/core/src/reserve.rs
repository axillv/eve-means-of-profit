//! Reservations: what each plan takes from the hangars and from its running jobs, served in the
//! order the plans are listed. Claims are live: they are recomputed from the plans and the current
//! stock on every call, nothing is stored per claim.
//!
//! Integers only, checked arithmetic everywhere. Pure: no I/O, no globals.

#![deny(
    clippy::arithmetic_side_effects,
    clippy::float_arithmetic,
    clippy::as_conversions,
    clippy::integer_division
)]

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::industry::{RecipeBook, TypeId};
use crate::plan::{Plan, PlanError, PlanInput};
use crate::stock::{CharacterId, ItemId, LocationId, Places, ResolvedAsset, StockScope};
use crate::stockpile::{
    Held, Line, Source, Status, Stock, Stockpile, StockpileError, have, matches_any,
};

/// How a stockpile's Short counts stock: after the plans' claims, or with everything in stock.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ShortMode {
    /// Short counts the stock left after the plans' claims.
    #[default]
    AfterPlans,
    /// Short counts all stock; the claims are only shown.
    RightNow,
}

/// A plan that reserves stock. `assigned` is the units of running jobs assigned to it, per product.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReservingPlan {
    pub plan_id: i64,
    pub input: PlanInput,
    pub scope: StockScope,
    pub assigned: BTreeMap<TypeId, u64>,
}

/// Every plan's claim on the hangar entries, in serving order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Claims {
    /// The plan ids in the order they were served.
    pub order: Vec<i64>,
    /// Per hangar entry, the (plan id, units) taken from it, in serving order.
    pub by_asset: BTreeMap<ItemId, Vec<(i64, u64)>>,
    pub by_plan: BTreeMap<i64, PlanClaim>,
}

/// What one plan takes: from its assigned jobs and from hangar entries, and the plan it computed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanClaim {
    pub from_assets: BTreeMap<TypeId, u64>,
    pub from_jobs: BTreeMap<TypeId, u64>,
    pub plan: Plan,
}

/// A running manufacturing or reaction job, with the units it yields and the place it delivers to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunningJob {
    pub job_id: i64,
    pub character_id: CharacterId,
    pub product: TypeId,
    pub units: u64,
    pub place: LocationId,
}

/// What a plan builds, for suggesting jobs to it. `characters` is `None` when any character matches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanForJobs {
    pub plan_id: i64,
    pub product: TypeId,
    pub build: BTreeSet<TypeId>,
    pub characters: Option<BTreeSet<CharacterId>>,
}

/// One stockpile line with what the reserving plans claim from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimedLine {
    pub line: Line,
    pub claimed: u64,
    /// Per plan, in serving order; plans with nothing claimed are left out.
    pub by: Vec<(i64, u64)>,
    pub free: u64,
    pub short: u64,
    pub status: Status,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReserveError {
    OutOfRange,
    Plan { plan_id: i64, error: PlanError },
}

impl fmt::Display for ReserveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OutOfRange => write!(f, "number out of range"),
            Self::Plan { plan_id, error } => write!(f, "plan {plan_id}: {error}"),
        }
    }
}

impl std::error::Error for ReserveError {}

/// Serves the plans in the given order. Each plan's stock is the hangar units no earlier plan
/// claimed, plus its assigned jobs; the jobs are used first, then the hangar entries in order.
pub fn claims(
    plans: &[ReservingPlan],
    assets: &[ResolvedAsset],
    recipes: &RecipeBook,
) -> Result<Claims, ReserveError> {
    let mut claims = Claims::default();
    let mut claimed_on: BTreeMap<ItemId, u64> = BTreeMap::new();
    for reserving in plans {
        let entries = entries_of(&reserving.scope, assets);
        let mut stock: BTreeMap<TypeId, u64> = BTreeMap::new();
        for &row in &entries {
            let left = unclaimed(row, &claimed_on)?;
            if left > 0 {
                add(&mut stock, row.asset.type_id, left)?;
            }
        }
        for (&type_id, &units) in &reserving.assigned {
            add(&mut stock, type_id, units)?;
        }

        let input = PlanInput {
            stock,
            ..reserving.input.clone()
        };
        let computed = crate::plan::plan(&input, recipes).map_err(|error| ReserveError::Plan {
            plan_id: reserving.plan_id,
            error,
        })?;

        let mut used: BTreeMap<TypeId, u64> = BTreeMap::new();
        for step in &computed.steps {
            add(&mut used, step.type_id, step.from_stock)?;
        }
        for line in &computed.buy {
            add(&mut used, line.type_id, line.from_stock)?;
        }
        let mut from_jobs = BTreeMap::new();
        let mut from_assets = BTreeMap::new();
        for (&type_id, &units) in &used {
            let assigned = reserving.assigned.get(&type_id).copied().unwrap_or(0);
            let jobs = units.min(assigned);
            let hangar = units.checked_sub(jobs).ok_or(ReserveError::OutOfRange)?;
            if jobs > 0 {
                from_jobs.insert(type_id, jobs);
            }
            if hangar > 0 {
                from_assets.insert(type_id, hangar);
            }
        }

        claims.order.push(reserving.plan_id);
        for (&type_id, &wanted) in &from_assets {
            let mut remaining = wanted;
            for &row in entries.iter().filter(|row| row.asset.type_id == type_id) {
                if remaining == 0 {
                    break;
                }
                let take = unclaimed(row, &claimed_on)?.min(remaining);
                if take == 0 {
                    continue;
                }
                remaining = remaining
                    .checked_sub(take)
                    .ok_or(ReserveError::OutOfRange)?;
                add(&mut claimed_on, row.asset.item_id, take)?;
                claims
                    .by_asset
                    .entry(row.asset.item_id)
                    .or_default()
                    .push((reserving.plan_id, take));
            }
            // The free units cover every hangar unit the plan uses, so nothing is left over.
            if remaining > 0 {
                return Err(ReserveError::OutOfRange);
            }
        }
        claims.by_plan.insert(
            reserving.plan_id,
            PlanClaim {
                from_assets,
                from_jobs,
                plan: computed,
            },
        );
    }
    Ok(claims)
}

/// The stock a plan's Build page uses before its assigned jobs: the in-scope hangar units, less
/// the claims of the plans served before `plan_id`. Without `plan_id` in the order, less every claim.
pub fn available(
    scope: &StockScope,
    assets: &[ResolvedAsset],
    claims: &Claims,
    plan_id: Option<i64>,
) -> Result<BTreeMap<TypeId, u64>, ReserveError> {
    let cut = plan_id
        .and_then(|id| claims.order.iter().position(|&served| served == id))
        .unwrap_or(claims.order.len());
    let before: BTreeSet<i64> = claims.order.iter().take(cut).copied().collect();

    let rows = entries_of(scope, assets);
    let mut left: BTreeMap<TypeId, u64> = BTreeMap::new();
    for row in &rows {
        add(&mut left, row.asset.type_id, row.asset.quantity)?;
    }
    let by_item: BTreeMap<ItemId, &ResolvedAsset> =
        rows.iter().map(|row| (row.asset.item_id, *row)).collect();
    for (item_id, served) in &claims.by_asset {
        let Some(row) = by_item.get(item_id) else {
            continue;
        };
        for (plan, units) in served {
            if before.contains(plan) {
                take(&mut left, row.asset.type_id, *units)?;
            }
        }
    }
    left.retain(|_, units| *units > 0);
    Ok(left)
}

/// Each stockpile line with the claims on the hangar entries that the stockpile's own filters
/// count. `AfterPlans` shows what is short after the plans; `RightNow` keeps the plain line.
pub fn claimed_lines(
    id: i64,
    all: &BTreeMap<i64, Stockpile>,
    stock: &Stock<'_>,
    locate: &dyn Fn(LocationId) -> Option<(u32, u32)>,
    claims: &Claims,
    mode: ShortMode,
) -> Result<Vec<ClaimedLine>, StockpileError> {
    let lines = have(id, all, stock, locate)?;
    let filters = &all.get(&id).ok_or(StockpileError::Missing(id))?.filters;

    let mut claimed: BTreeMap<TypeId, u64> = BTreeMap::new();
    let mut by_type: BTreeMap<TypeId, BTreeMap<i64, u64>> = BTreeMap::new();
    for row in stock.assets.iter().filter(|row| {
        row.counts_as_stock
            && matches_any(filters, row.character_id, row.place, locate, Source::Assets)
    }) {
        let Some(served) = claims.by_asset.get(&row.asset.item_id) else {
            continue;
        };
        for (plan_id, units) in served {
            let total = claimed.entry(row.asset.type_id).or_insert(0);
            *total = total
                .checked_add(*units)
                .ok_or(StockpileError::OutOfRange)?;
            let per_plan = by_type
                .entry(row.asset.type_id)
                .or_default()
                .entry(*plan_id)
                .or_insert(0);
            *per_plan = per_plan
                .checked_add(*units)
                .ok_or(StockpileError::OutOfRange)?;
        }
    }

    lines
        .into_iter()
        .map(|line| {
            let claimed_units = claimed.get(&line.type_id).copied().unwrap_or(0);
            let per_plan = by_type.get(&line.type_id).cloned().unwrap_or_default();
            let by = in_serving_order(&claims.order, per_plan);
            let free = line
                .total
                .checked_sub(claimed_units)
                .ok_or(StockpileError::OutOfRange)?;
            let (short, status) = match mode {
                // A floor at 0 is meant here: free stock above the target is short 0, never a negative count.
                ShortMode::AfterPlans => (
                    line.target.saturating_sub(free),
                    status_of(free, line.target),
                ),
                ShortMode::RightNow => (line.short, line.status),
            };
            Ok(ClaimedLine {
                line,
                claimed: claimed_units,
                by,
                free,
                short,
                status,
            })
        })
        .collect()
}

/// The running jobs that no plan has been assigned to yet, as stock held at their place.
pub fn unassigned(jobs: &[RunningJob], assignments: &BTreeMap<i64, i64>) -> Vec<Held> {
    jobs.iter()
        .filter(|job| !assignments.contains_key(&job.job_id))
        .map(|job| Held {
            character_id: job.character_id,
            type_id: job.product,
            place: job.place,
            units: job.units,
        })
        .collect()
}

/// Units per product of the jobs assigned to `plan_id`.
pub fn assigned_to(
    plan_id: i64,
    jobs: &[RunningJob],
    assignments: &BTreeMap<i64, i64>,
) -> Result<BTreeMap<TypeId, u64>, ReserveError> {
    let mut units_by_type: BTreeMap<TypeId, u64> = BTreeMap::new();
    for job in jobs
        .iter()
        .filter(|job| assignments.get(&job.job_id) == Some(&plan_id))
    {
        add(&mut units_by_type, job.product, job.units)?;
    }
    Ok(units_by_type)
}

/// The plans a job can be assigned to: those that build its product, for a character they accept.
/// Nothing is suggested for a job that is already assigned.
pub fn suggest(
    job: &RunningJob,
    plans: &[PlanForJobs],
    assignments: &BTreeMap<i64, i64>,
) -> Vec<i64> {
    if assignments.contains_key(&job.job_id) {
        return Vec::new();
    }
    plans
        .iter()
        .filter(|plan| {
            (plan.product == job.product || plan.build.contains(&job.product))
                && plan
                    .characters
                    .as_ref()
                    .is_none_or(|set| set.contains(&job.character_id))
        })
        .map(|plan| plan.plan_id)
        .collect()
}

/// The hangar entries a scope counts, in the order claims are taken from them.
fn entries_of<'a>(scope: &StockScope, assets: &'a [ResolvedAsset]) -> Vec<&'a ResolvedAsset> {
    let mut rows: Vec<&ResolvedAsset> = assets
        .iter()
        .filter(|row| row.counts_as_stock && row.asset.quantity > 0 && in_scope(scope, row))
        .collect();
    rows.sort_by_key(|row| (row.place, row.character_id, row.asset.item_id));
    rows
}

fn in_scope(scope: &StockScope, row: &ResolvedAsset) -> bool {
    let place_ok = match &scope.places {
        Places::All => true,
        Places::Only(places) => places.contains(&row.place),
    };
    scope.characters.contains(&row.character_id) && place_ok
}

/// Units of an entry that no earlier claim has taken.
fn unclaimed(row: &ResolvedAsset, claimed_on: &BTreeMap<ItemId, u64>) -> Result<u64, ReserveError> {
    let taken = claimed_on.get(&row.asset.item_id).copied().unwrap_or(0);
    row.asset
        .quantity
        .checked_sub(taken)
        .ok_or(ReserveError::OutOfRange)
}

/// The (plan id, units) pairs in serving order, each plan once.
fn in_serving_order(order: &[i64], mut per_plan: BTreeMap<i64, u64>) -> Vec<(i64, u64)> {
    order
        .iter()
        .filter_map(|&id| per_plan.remove(&id).map(|units| (id, units)))
        .collect()
}

fn status_of(free: u64, target: u64) -> Status {
    if free >= target {
        Status::Ok
    } else if free == 0 {
        Status::Short
    } else {
        Status::Part
    }
}

fn add<K: Ord>(map: &mut BTreeMap<K, u64>, key: K, units: u64) -> Result<(), ReserveError> {
    let total = map.entry(key).or_insert(0);
    *total = total.checked_add(units).ok_or(ReserveError::OutOfRange)?;
    Ok(())
}

fn take(map: &mut BTreeMap<TypeId, u64>, type_id: TypeId, units: u64) -> Result<(), ReserveError> {
    let left = map.get_mut(&type_id).ok_or(ReserveError::OutOfRange)?;
    *left = left.checked_sub(units).ok_or(ReserveError::OutOfRange)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::industry::{Activity, Modifier, Recipe};
    use crate::stock::Asset;
    use crate::stockpile::{Characters, Filter, Place, Target};

    const MAIN: CharacterId = 2_112_000_001;
    const ALT: CharacterId = 2_112_000_002;
    const P1: LocationId = 60_003_760;
    const P2: LocationId = 1_035_466_617_946;
    const SYSTEM: u32 = 30_000_142;
    const REGION: u32 = 10_000_002;

    fn locate(place: LocationId) -> Option<(u32, u32)> {
        (place == P1 || place == P2).then_some((SYSTEM, REGION))
    }

    fn recipe(output: u64, materials: &[(TypeId, u64)]) -> Recipe {
        Recipe {
            activity: Activity::Manufacturing,
            output_per_run: output,
            max_runs_per_job: 1000,
            materials: materials.to_vec(),
            made_by_invention: false,
        }
    }

    /// Widget 100 = 2 × Part 200 per run; Part 200 = 10 × Mineral 300 per run.
    fn book() -> RecipeBook {
        let mut book = RecipeBook::new();
        book.insert(100, recipe(1, &[(200, 2)]));
        book.insert(200, recipe(1, &[(300, 10)]));
        book
    }

    fn asset(
        item_id: ItemId,
        type_id: TypeId,
        character_id: CharacterId,
        place: LocationId,
        quantity: u64,
        counts_as_stock: bool,
    ) -> ResolvedAsset {
        ResolvedAsset {
            character_id,
            asset: Asset {
                item_id,
                type_id,
                location_id: place,
                flag: Some("Hangar".to_owned()),
                quantity,
                singleton: false,
            },
            place,
            counts_as_stock,
        }
    }

    fn assets() -> Vec<ResolvedAsset> {
        vec![
            asset(1, 200, MAIN, P1, 4, true),
            asset(2, 200, ALT, P1, 3, true),
            asset(3, 300, MAIN, P1, 25, true),
            asset(4, 300, MAIN, P2, 30, true),
            asset(5, 200, MAIN, P1, 9, false),
            asset(6, 300, MAIN, P2, 10, true),
        ]
    }

    fn scope(characters: &[CharacterId], places: Places) -> StockScope {
        StockScope {
            characters: characters.iter().copied().collect(),
            places,
        }
    }

    fn only(places: &[LocationId]) -> Places {
        Places::Only(places.iter().copied().collect())
    }

    fn widget_input(quantity: u64) -> PlanInput {
        PlanInput {
            product: 100,
            quantity,
            build: BTreeSet::from([200]),
            modifiers: BTreeMap::from([(100, Modifier::one()), (200, Modifier::one())]),
            stock: BTreeMap::new(),
        }
    }

    fn plan_a() -> ReservingPlan {
        ReservingPlan {
            plan_id: 11,
            input: widget_input(5),
            scope: scope(&[MAIN, ALT], only(&[P1])),
            assigned: BTreeMap::new(),
        }
    }

    fn plan_b() -> ReservingPlan {
        ReservingPlan {
            plan_id: 12,
            input: widget_input(2),
            scope: scope(&[MAIN], Places::All),
            assigned: BTreeMap::from([(200, 1)]),
        }
    }

    fn kit(filters: Vec<Filter>) -> BTreeMap<i64, Stockpile> {
        BTreeMap::from([(
            1,
            Stockpile {
                id: 1,
                name: "Kit".to_owned(),
                group: None,
                multiplier: 1,
                targets: vec![
                    Target {
                        type_id: 200,
                        units: 10,
                        ignore_multiplier: false,
                    },
                    Target {
                        type_id: 300,
                        units: 60,
                        ignore_multiplier: false,
                    },
                ],
                includes: Vec::new(),
                filters,
            },
        )])
    }

    fn location_filter(characters: Characters, place: Place, source: Source) -> Filter {
        Filter {
            characters,
            place,
            sources: BTreeSet::from([source]),
        }
    }

    fn line_for(lines: &[ClaimedLine], type_id: TypeId) -> &ClaimedLine {
        lines
            .iter()
            .find(|line| line.line.type_id == type_id)
            .expect("line is listed")
    }

    fn stock_of(assets: &[ResolvedAsset]) -> Stock<'_> {
        Stock {
            assets,
            sell_orders: &[],
            buy_orders: &[],
            jobs: &[],
        }
    }

    #[test]
    fn plans_claim_in_list_order() {
        let got = claims(&[plan_a(), plan_b()], &assets(), &book()).expect("claims");
        assert_eq!(got.order, vec![11, 12]);

        let a = &got.by_plan[&11];
        assert_eq!(a.from_assets, BTreeMap::from([(200, 7), (300, 25)]));
        assert!(a.from_jobs.is_empty());
        let part = a
            .plan
            .steps
            .iter()
            .find(|s| s.type_id == 200)
            .expect("part");
        assert_eq!((part.needed, part.from_stock, part.to_build), (10, 7, 3));
        let mineral = a
            .plan
            .buy
            .iter()
            .find(|b| b.type_id == 300)
            .expect("mineral");
        assert_eq!(
            (mineral.needed, mineral.from_stock, mineral.to_buy),
            (30, 25, 5)
        );

        let b = &got.by_plan[&12];
        assert_eq!(b.from_assets, BTreeMap::from([(300, 30)]));
        assert_eq!(b.from_jobs, BTreeMap::from([(200, 1)]));
        let part = b
            .plan
            .steps
            .iter()
            .find(|s| s.type_id == 200)
            .expect("part");
        assert_eq!((part.needed, part.from_stock, part.to_build), (4, 1, 3));
        let mineral = b
            .plan
            .buy
            .iter()
            .find(|b| b.type_id == 300)
            .expect("mineral");
        assert_eq!((mineral.needed, mineral.from_stock), (30, 30));

        assert_eq!(
            got.by_asset,
            BTreeMap::from([
                (1, vec![(11, 4)]),
                (2, vec![(11, 3)]),
                (3, vec![(11, 25)]),
                (4, vec![(12, 30)]),
            ])
        );
    }

    #[test]
    fn reversed_order_changes_who_gets_the_shared_units() {
        let got = claims(&[plan_b(), plan_a()], &assets(), &book()).expect("claims");
        assert_eq!(got.order, vec![12, 11]);

        let b = &got.by_plan[&12];
        assert_eq!(b.from_assets, BTreeMap::from([(200, 3)]));
        assert_eq!(b.from_jobs, BTreeMap::from([(200, 1)]));
        let part = b
            .plan
            .steps
            .iter()
            .find(|s| s.type_id == 200)
            .expect("part");
        assert_eq!((part.from_stock, part.to_build), (4, 0));

        let a = &got.by_plan[&11];
        assert_eq!(a.from_assets, BTreeMap::from([(200, 4), (300, 25)]));
        let part = a
            .plan
            .steps
            .iter()
            .find(|s| s.type_id == 200)
            .expect("part");
        assert_eq!((part.from_stock, part.to_build), (4, 6));
        let mineral = a
            .plan
            .buy
            .iter()
            .find(|b| b.type_id == 300)
            .expect("mineral");
        assert_eq!(
            (mineral.needed, mineral.from_stock, mineral.to_buy),
            (60, 25, 35)
        );

        assert_eq!(
            got.by_asset,
            BTreeMap::from([
                (1, vec![(12, 3), (11, 1)]),
                (2, vec![(11, 3)]),
                (3, vec![(11, 25)]),
            ])
        );
    }

    #[test]
    fn available_leaves_out_the_claims_of_later_plans() {
        let got = claims(&[plan_a(), plan_b()], &assets(), &book()).expect("claims");
        let a_scope = scope(&[MAIN, ALT], only(&[P1]));
        assert_eq!(
            available(&a_scope, &assets(), &got, Some(11)),
            Ok(BTreeMap::from([(200, 7), (300, 25)]))
        );
        assert_eq!(
            available(&scope(&[MAIN], Places::All), &assets(), &got, Some(12)),
            Ok(BTreeMap::from([(300, 40)]))
        );
        let everyone = scope(&[MAIN, ALT], Places::All);
        assert_eq!(
            available(&everyone, &assets(), &got, None),
            Ok(BTreeMap::from([(300, 10)]))
        );
        assert_eq!(
            available(&everyone, &assets(), &got, Some(99)),
            Ok(BTreeMap::from([(300, 10)]))
        );
    }

    #[test]
    fn overflowing_assigned_units_are_out_of_range() {
        let mut huge = plan_b();
        huge.assigned = BTreeMap::from([(200, u64::MAX)]);
        assert_eq!(
            claims(&[huge], &assets(), &book()),
            Err(ReserveError::OutOfRange)
        );
    }

    #[test]
    fn plan_without_recipe_reports_its_error() {
        let no_recipe = ReservingPlan {
            plan_id: 14,
            input: PlanInput {
                product: 34,
                quantity: 1,
                build: BTreeSet::new(),
                modifiers: BTreeMap::from([(34, Modifier::one())]),
                stock: BTreeMap::new(),
            },
            scope: scope(&[MAIN], Places::All),
            assigned: BTreeMap::new(),
        };
        assert_eq!(
            claims(&[no_recipe], &assets(), &book()),
            Err(ReserveError::Plan {
                plan_id: 14,
                error: PlanError::NoRecipe(34),
            })
        );
    }

    #[test]
    fn a_plan_served_twice_claims_nothing_the_second_time() {
        let got = claims(&[plan_a(), plan_a()], &assets(), &book()).expect("claims");
        let single = claims(&[plan_a()], &assets(), &book()).expect("claims");
        assert_eq!(got.by_asset, single.by_asset);

        let second = &got.by_plan[&11];
        assert!(second.from_assets.is_empty());
        assert!(second.from_jobs.is_empty());
        let part = second
            .plan
            .steps
            .iter()
            .find(|s| s.type_id == 200)
            .expect("part");
        assert_eq!((part.from_stock, part.to_build), (0, 10));
        let mineral = second
            .plan
            .buy
            .iter()
            .find(|b| b.type_id == 300)
            .expect("mineral");
        assert_eq!(
            (mineral.needed, mineral.from_stock, mineral.to_buy),
            (100, 0, 100)
        );
    }

    #[test]
    fn claimed_lines_after_plans_and_right_now() {
        let got = claims(&[plan_a(), plan_b()], &assets(), &book()).expect("claims");
        let all = kit(vec![location_filter(
            Characters::All,
            Place::Location(P1),
            Source::Assets,
        )]);
        let rows = assets();
        let stock = stock_of(&rows);

        let after =
            claimed_lines(1, &all, &stock, &locate, &got, ShortMode::AfterPlans).expect("lines");
        let part = line_for(&after, 200);
        assert_eq!(part.line.total, 7);
        assert_eq!(part.claimed, 7);
        assert_eq!(part.by, vec![(11, 7)]);
        assert_eq!((part.free, part.short, part.status), (0, 10, Status::Short));
        let mineral = line_for(&after, 300);
        assert_eq!(mineral.line.total, 25);
        assert_eq!(mineral.claimed, 25);
        assert_eq!(mineral.by, vec![(11, 25)]);
        assert_eq!(
            (mineral.free, mineral.short, mineral.status),
            (0, 60, Status::Short)
        );

        let now =
            claimed_lines(1, &all, &stock, &locate, &got, ShortMode::RightNow).expect("lines");
        let part = line_for(&now, 200);
        assert_eq!((part.short, part.status), (3, Status::Part));
        assert_eq!((part.claimed, part.by.clone()), (7, vec![(11, 7)]));
        let mineral = line_for(&now, 300);
        assert_eq!((mineral.short, mineral.status), (35, Status::Part));
        assert_eq!((mineral.claimed, mineral.by.clone()), (25, vec![(11, 25)]));
    }

    #[test]
    fn claimed_lines_count_only_the_stockpile_filters() {
        let got = claims(&[plan_a(), plan_b()], &assets(), &book()).expect("claims");
        let rows = assets();
        let stock = stock_of(&rows);

        let main_only = kit(vec![location_filter(
            Characters::Only(BTreeSet::from([MAIN])),
            Place::Everywhere,
            Source::Assets,
        )]);
        let lines = claimed_lines(1, &main_only, &stock, &locate, &got, ShortMode::AfterPlans)
            .expect("lines");
        let mineral = line_for(&lines, 300);
        assert_eq!(mineral.line.total, 65);
        assert_eq!(mineral.claimed, 55);
        assert_eq!(mineral.by, vec![(11, 25), (12, 30)]);
        assert_eq!(
            (mineral.free, mineral.short, mineral.status),
            (10, 50, Status::Part)
        );
        let lines = claimed_lines(1, &main_only, &stock, &locate, &got, ShortMode::RightNow)
            .expect("lines");
        let mineral = line_for(&lines, 300);
        assert_eq!((mineral.short, mineral.status), (0, Status::Ok));

        let two_filters = kit(vec![
            location_filter(Characters::All, Place::Location(P1), Source::Assets),
            location_filter(
                Characters::Only(BTreeSet::from([MAIN])),
                Place::System(SYSTEM),
                Source::Assets,
            ),
        ]);
        let lines = claimed_lines(
            1,
            &two_filters,
            &stock,
            &locate,
            &got,
            ShortMode::AfterPlans,
        )
        .expect("lines");
        assert_eq!(line_for(&lines, 200).claimed, 7);

        let sell_only = kit(vec![location_filter(
            Characters::All,
            Place::Everywhere,
            Source::SellOrders,
        )]);
        let lines = claimed_lines(1, &sell_only, &stock, &locate, &got, ShortMode::AfterPlans)
            .expect("lines");
        assert!(
            lines
                .iter()
                .all(|line| line.claimed == 0 && line.by.is_empty())
        );
    }

    #[test]
    fn jobs_split_into_unassigned_and_assigned() {
        let jobs = [
            RunningJob {
                job_id: 501,
                character_id: MAIN,
                product: 200,
                units: 1,
                place: P2,
            },
            RunningJob {
                job_id: 502,
                character_id: ALT,
                product: 100,
                units: 2,
                place: P1,
            },
            RunningJob {
                job_id: 503,
                character_id: MAIN,
                product: 200,
                units: 5,
                place: P1,
            },
        ];
        let assignments = BTreeMap::from([(501, 12), (999, 11)]);
        assert_eq!(
            unassigned(&jobs, &assignments),
            vec![
                Held {
                    character_id: ALT,
                    type_id: 100,
                    place: P1,
                    units: 2,
                },
                Held {
                    character_id: MAIN,
                    type_id: 200,
                    place: P1,
                    units: 5,
                },
            ]
        );
        assert_eq!(
            assigned_to(12, &jobs, &assignments),
            Ok(BTreeMap::from([(200, 1)]))
        );
        assert_eq!(assigned_to(11, &jobs, &assignments), Ok(BTreeMap::new()));
    }

    #[test]
    fn suggestions_follow_product_build_and_character() {
        let plans = [
            PlanForJobs {
                plan_id: 11,
                product: 100,
                build: BTreeSet::from([200]),
                characters: Some(BTreeSet::from([MAIN, ALT])),
            },
            PlanForJobs {
                plan_id: 12,
                product: 100,
                build: BTreeSet::from([200]),
                characters: Some(BTreeSet::from([MAIN])),
            },
            PlanForJobs {
                plan_id: 13,
                product: 300,
                build: BTreeSet::new(),
                characters: None,
            },
        ];
        let assignments = BTreeMap::from([(501, 12)]);
        let job = |job_id, character_id, product| RunningJob {
            job_id,
            character_id,
            product,
            units: 1,
            place: P1,
        };
        assert_eq!(
            suggest(&job(503, MAIN, 200), &plans, &assignments),
            vec![11, 12]
        );
        assert_eq!(suggest(&job(502, ALT, 100), &plans, &assignments), vec![11]);
        assert!(suggest(&job(501, MAIN, 200), &plans, &assignments).is_empty());
        assert_eq!(suggest(&job(504, ALT, 300), &plans, &assignments), vec![13]);
    }

    #[test]
    fn errors_read_as_sentences() {
        assert_eq!(ReserveError::OutOfRange.to_string(), "number out of range");
        assert_eq!(
            ReserveError::Plan {
                plan_id: 14,
                error: PlanError::NoRecipe(34),
            }
            .to_string(),
            "plan 14: type 34 has no recipe"
        );
    }
}

//! Build or buy, per part of a plan: which way is cheaper, and why.
//!
//! A part is priced two ways: delivered from the market, or built in a facility from its own
//! materials, each of which is priced the same way. Money is exact: every product is checked and
//! rounded once to 0.0001 ISK, and nothing here uses floats. The result only suggests; the plan
//! itself is not changed.

#![deny(
    clippy::arithmetic_side_effects,
    clippy::float_arithmetic,
    clippy::as_conversions,
    clippy::integer_division
)]

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::freight::RateCard;
use crate::industry::{
    Activity, IndustryError, Modifier, Recipe, RecipeBook, TypeId, split_jobs, split_quantity,
};
use crate::job_cost::{CostSetup, JobCostError, step_cost};
use crate::money::Money;
use crate::plan::Plan;
use crate::volume::Volume;

/// What the comparison reads. Unit prices are at the plan's price rule.
#[derive(Debug, Clone)]
pub struct MakeOrBuyInput<'a> {
    pub plan: &'a Plan,
    pub recipes: &'a RecipeBook,
    /// The material modifier of every type the facility could build.
    pub modifiers: &'a BTreeMap<TypeId, Modifier>,
    /// Buy price per unit.
    pub unit_price: &'a BTreeMap<TypeId, Money>,
    pub unit_volume: &'a BTreeMap<TypeId, Volume>,
    /// The route's picked hauling service; `None` charges no freight.
    pub freight: Option<&'a RateCard>,
    pub adjusted: &'a BTreeMap<TypeId, Money>,
    pub manufacturing: Option<CostSetup>,
    pub reaction: Option<CostSetup>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Build,
    Buy,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reason {
    Cheaper,
    Tie,
    NoMarketPrice,
    NoFacility(Activity),
    /// Materials, ascending, with neither a price nor a way to build.
    MaterialsUnpriced(Vec<TypeId>),
    /// The part needs itself below a bought part, so it cannot be built.
    Loop,
}

/// The verdict for one part, at the quantity the plan needs (or the first quantity met below a bought part).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub choice: Choice,
    pub quantity: u64,
    pub buy_total: Option<Money>,
    pub build_total: Option<Money>,
    pub reason: Reason,
    /// Materials whose adjusted price is missing from this part's jobs; their share is left out of the build total.
    pub job_cost_incomplete: Vec<TypeId>,
}

impl Decision {
    /// The line the panel shows: the chosen side first, money with two decimals.
    pub fn reason_text(&self) -> String {
        let buy = self.buy_total.map_or_else(String::new, Money::format_isk);
        let build = self.build_total.map_or_else(String::new, Money::format_isk);
        match (self.choice, &self.reason) {
            (Choice::Unknown, _) => "no price either way".to_string(),
            (Choice::Build, Reason::Cheaper) => format!("build: {build} vs buy {buy}"),
            (Choice::Buy, Reason::Cheaper) => format!("buy: {buy} vs build {build}"),
            (_, Reason::Tie) => format!("buy: {buy} (same as build)"),
            (_, Reason::NoMarketPrice) => format!("build: {build} (no market price)"),
            (_, Reason::NoFacility(activity)) => {
                format!("buy: {buy} (no facility for {})", activity_name(*activity))
            }
            (_, Reason::MaterialsUnpriced(ids)) => {
                format!("buy: {buy} (no price for materials: {})", join_ids(ids))
            }
            (_, Reason::Loop) => format!("buy: {buy} (needs itself)"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MakeOrBuyError {
    Overflow,
    Industry(IndustryError),
    JobCost(JobCostError),
}

impl fmt::Display for MakeOrBuyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Overflow => write!(
                f,
                "the build or buy comparison does not fit the supported range"
            ),
            Self::Industry(e) => write!(f, "{e}"),
            Self::JobCost(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for MakeOrBuyError {}

/// The decision for every part of the plan that has a recipe, and for every such part below a bought part.
pub fn make_or_buy(input: &MakeOrBuyInput) -> Result<BTreeMap<TypeId, Decision>, MakeOrBuyError> {
    let mut plan_qty = BTreeMap::new();
    let mut candidates = Vec::new();
    let lines = input
        .plan
        .steps
        .iter()
        .filter(|step| step.depth > 0)
        .map(|step| (step.type_id, step.to_build))
        .chain(
            input
                .plan
                .buy
                .iter()
                .map(|line| (line.type_id, line.to_buy)),
        );
    for (type_id, quantity) in lines {
        if quantity == 0 {
            continue;
        }
        plan_qty.insert(type_id, quantity);
        if let Some(recipe) = recipe_of(input.recipes, type_id) {
            candidates.push((type_id, quantity, recipe));
        }
    }
    let mut shown: BTreeSet<TypeId> = candidates.iter().map(|(type_id, _, _)| *type_id).collect();

    let mut solver = Solver {
        input,
        plan_qty,
        decided: BTreeMap::new(),
        edges: BTreeSet::new(),
    };
    for (type_id, quantity, recipe) in candidates {
        solver.choice_of(type_id, quantity, recipe, &mut Vec::new())?;
    }

    // Every part below a bought part is shown too, at the amount that bought part needs, even when
    // the plan itself needs none of it from the market.
    let mut below = BTreeSet::new();
    let mut frontier: Vec<TypeId> = input
        .plan
        .buy
        .iter()
        .filter(|line| line.to_buy > 0)
        .map(|line| line.type_id)
        .collect();
    while let Some(parent) = frontier.pop() {
        for &(_, child) in solver.edges.iter().filter(|(from, _)| *from == parent) {
            if below.insert(child) {
                frontier.push(child);
            }
        }
    }
    shown.extend(below);

    Ok(solver
        .decided
        .into_iter()
        .filter(|(type_id, _)| shown.contains(type_id))
        .map(|(type_id, eval)| (type_id, eval.decision))
        .collect())
}

/// A decision, with what a parent needs to know when it is short of this part.
#[derive(Debug, Clone)]
struct Eval {
    decision: Decision,
    /// The types blocking this part when it has no price either way.
    unpriced: Vec<TypeId>,
}

/// What one material costs at the amount its parent needs; `None` names the types that block it.
struct Side {
    total: Option<Money>,
    unpriced: Vec<TypeId>,
}

impl Side {
    fn buy(type_id: TypeId, total: Option<Money>) -> Side {
        let unpriced = if total.is_none() {
            vec![type_id]
        } else {
            Vec::new()
        };
        Side { total, unpriced }
    }
}

/// The build side of one part: its total, or why it cannot be built, and its jobs' missing prices.
struct Build {
    total: Result<Money, Reason>,
    incomplete: Vec<TypeId>,
}

struct Solver<'i, 'a> {
    input: &'i MakeOrBuyInput<'a>,
    /// The quantity each plan line needs. A part's decision is taken at it.
    plan_qty: BTreeMap<TypeId, u64>,
    decided: BTreeMap<TypeId, Eval>,
    /// Which part each decided part's build needs: (parent, material).
    edges: BTreeSet<(TypeId, TypeId)>,
}

impl Solver<'_, '_> {
    /// The decision for a part, taken once: at its plan quantity, else at the first quantity met.
    fn choice_of(
        &mut self,
        type_id: TypeId,
        quantity: u64,
        recipe: &Recipe,
        path: &mut Vec<TypeId>,
    ) -> Result<Eval, MakeOrBuyError> {
        if let Some(eval) = self.decided.get(&type_id) {
            return Ok(eval.clone());
        }
        let decided_at = self.plan_qty.get(&type_id).copied().unwrap_or(quantity);
        let eval = self.evaluate(type_id, decided_at, recipe, path)?;
        self.decided.insert(type_id, eval.clone());
        Ok(eval)
    }

    fn evaluate(
        &mut self,
        type_id: TypeId,
        quantity: u64,
        recipe: &Recipe,
        path: &mut Vec<TypeId>,
    ) -> Result<Eval, MakeOrBuyError> {
        path.push(type_id);
        let buy_total = self.buy_at(type_id, quantity);
        let build = self.build_at(type_id, quantity, recipe, path);
        path.pop();
        let buy_total = buy_total?;
        let build = build?;
        let (choice, reason) = decide(buy_total, &build.total);
        let unpriced = match &reason {
            Reason::MaterialsUnpriced(ids) => ids.clone(),
            Reason::Cheaper
            | Reason::Tie
            | Reason::NoMarketPrice
            | Reason::NoFacility(_)
            | Reason::Loop => {
                vec![type_id]
            }
        };
        Ok(Eval {
            decision: Decision {
                choice,
                quantity,
                buy_total,
                build_total: build.total.as_ref().ok().copied(),
                reason,
                job_cost_incomplete: build.incomplete,
            },
            unpriced,
        })
    }

    fn build_at(
        &mut self,
        type_id: TypeId,
        quantity: u64,
        recipe: &Recipe,
        path: &mut Vec<TypeId>,
    ) -> Result<Build, MakeOrBuyError> {
        let input = self.input;
        let Some(&modifier) = input.modifiers.get(&type_id) else {
            return Ok(Build {
                total: Err(Reason::NoFacility(recipe.activity)),
                incomplete: Vec::new(),
            });
        };
        let runs = quantity.div_ceil(recipe.output_per_run);
        let jobs = split_jobs(runs, recipe.max_runs_per_job).map_err(MakeOrBuyError::Industry)?;
        let mut looped = false;
        let mut known = Some(Money::ZERO);
        let mut unpriced = BTreeSet::new();
        for &(material, base) in &recipe.materials {
            let units = split_quantity(base, jobs, modifier).map_err(MakeOrBuyError::Industry)?;
            looped |= path.contains(&material);
            self.edges.insert((type_id, material));
            let side = self.side_at(material, units, path)?;
            known = match (known, side.total) {
                (Some(sum), Some(cost)) => {
                    Some(sum.checked_add(cost).ok_or(MakeOrBuyError::Overflow)?)
                }
                _ => None,
            };
            unpriced.extend(side.unpriced);
        }

        let setup = match recipe.activity {
            Activity::Manufacturing => input.manufacturing.as_ref(),
            Activity::Reaction => input.reaction.as_ref(),
        };
        let Some(setup) = setup else {
            return Ok(Build {
                total: Err(Reason::NoFacility(recipe.activity)),
                incomplete: Vec::new(),
            });
        };
        let step = step_cost(&recipe.materials, &jobs, input.adjusted, setup)
            .map_err(MakeOrBuyError::JobCost)?;
        let total = if looped {
            Err(Reason::Loop)
        } else if let Some(materials) = known {
            Ok(materials
                .checked_add(step.cost.total)
                .ok_or(MakeOrBuyError::Overflow)?)
        } else {
            Err(Reason::MaterialsUnpriced(unpriced.into_iter().collect()))
        };
        Ok(Build {
            total,
            incomplete: step.missing_prices,
        })
    }

    /// The cost of a material at the amount its parent needs, on the side its own decision chose.
    fn side_at(
        &mut self,
        material: TypeId,
        quantity: u64,
        path: &mut Vec<TypeId>,
    ) -> Result<Side, MakeOrBuyError> {
        let input = self.input;
        let recipe = match recipe_of(input.recipes, material) {
            // A part that needs itself can only be bought.
            Some(recipe) if !path.contains(&material) => recipe,
            _ => {
                let total = self.buy_at(material, quantity)?;
                return Ok(Side::buy(material, total));
            }
        };
        let eval = self.choice_of(material, quantity, recipe, path)?;
        let other = if eval.decision.quantity == quantity {
            None
        } else {
            Some(self.evaluate(material, quantity, recipe, path)?)
        };
        let here = other.as_ref().unwrap_or(&eval);
        Ok(match eval.decision.choice {
            Choice::Build => match here.decision.build_total {
                Some(total) => Side {
                    total: Some(total),
                    unpriced: Vec::new(),
                },
                None => Side {
                    total: None,
                    unpriced: here.unpriced.clone(),
                },
            },
            Choice::Buy => Side::buy(material, here.decision.buy_total),
            Choice::Unknown => Side {
                total: None,
                unpriced: here.unpriced.clone(),
            },
        })
    }

    /// The buy price of `quantity` units; `None` without a unit price.
    fn buy_at(&self, type_id: TypeId, quantity: u64) -> Result<Option<Money>, MakeOrBuyError> {
        let Some(unit) = self.delivered(type_id)? else {
            return Ok(None);
        };
        unit.checked_mul_qty(quantity)
            .map(Some)
            .ok_or(MakeOrBuyError::Overflow)
    }

    /// The unit price, plus the hauling per m³ and the collateral charge when a freight service is picked.
    fn delivered(&self, type_id: TypeId) -> Result<Option<Money>, MakeOrBuyError> {
        let input = self.input;
        let Some(&unit) = input.unit_price.get(&type_id) else {
            return Ok(None);
        };
        let Some(card) = input.freight else {
            return Ok(Some(unit));
        };
        let hauling = match (card.per_m3, input.unit_volume.get(&type_id)) {
            (Some(per_m3), Some(volume)) => per_m3
                .checked_mul_volume(*volume)
                .ok_or(MakeOrBuyError::Overflow)?,
            _ => Money::ZERO,
        };
        let collateral = match card.collateral {
            Some(rate) => unit
                .checked_mul_rate(rate)
                .ok_or(MakeOrBuyError::Overflow)?,
            None => Money::ZERO,
        };
        let total = unit
            .checked_add(hauling)
            .and_then(|sum| sum.checked_add(collateral))
            .ok_or(MakeOrBuyError::Overflow)?;
        Ok(Some(total))
    }
}

/// Build against buy. A tie buys: no job slot is spent.
fn decide(buy: Option<Money>, build: &Result<Money, Reason>) -> (Choice, Reason) {
    match (buy, build) {
        (Some(buy), Ok(build)) => {
            if *build < buy {
                (Choice::Build, Reason::Cheaper)
            } else if buy < *build {
                (Choice::Buy, Reason::Cheaper)
            } else {
                (Choice::Buy, Reason::Tie)
            }
        }
        (None, Ok(_)) => (Choice::Build, Reason::NoMarketPrice),
        (Some(_), Err(reason)) => (Choice::Buy, reason.clone()),
        (None, Err(reason)) => (Choice::Unknown, reason.clone()),
    }
}

/// The recipe of a type that can be built. A recipe that yields nothing is not one.
fn recipe_of(recipes: &RecipeBook, type_id: TypeId) -> Option<&Recipe> {
    recipes
        .get(type_id)
        .filter(|recipe| recipe.output_per_run > 0)
}

fn activity_name(activity: Activity) -> &'static str {
    match activity {
        Activity::Manufacturing => "manufacturing",
        Activity::Reaction => "reactions",
    }
}

fn join_ids(ids: &[TypeId]) -> String {
    ids.iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use super::{Choice, MakeOrBuyError, MakeOrBuyInput, Reason, make_or_buy};
    use crate::freight::RateCard;
    use crate::industry::{Activity, IndustryError, Modifier, Recipe, RecipeBook, TypeId};
    use crate::job_cost::{CostSetup, JobCostError, NPC_STATION_TAX, SCC_SURCHARGE};
    use crate::money::{Money, Rate};
    use crate::plan::{Plan, PlanInput, plan};
    use crate::volume::Volume;

    const PRODUCT: TypeId = 100;
    const A: TypeId = 200;
    const B: TypeId = 300;
    const M: TypeId = 400;
    const N: TypeId = 500;

    fn isk(text: &str) -> Money {
        Money::parse_isk(text).expect("a valid amount")
    }

    fn m3(text: &str) -> Volume {
        Volume::parse_decimal(text).expect("a valid volume")
    }

    fn recipe(output: u64, max_runs: u64, materials: &[(TypeId, u64)]) -> Recipe {
        Recipe {
            activity: Activity::Manufacturing,
            output_per_run: output,
            max_runs_per_job: max_runs,
            materials: materials.to_vec(),
            made_by_invention: false,
        }
    }

    /// The made-up chain: P = 10 × A + 5 × B per run; A = 100 × M per run, 10 units per run.
    /// The plan asks for 2 units of P.
    #[derive(Clone)]
    struct World {
        build: BTreeSet<TypeId>,
        stock: BTreeMap<TypeId, u64>,
        recipes: RecipeBook,
        modifiers: BTreeMap<TypeId, Modifier>,
        unit_price: BTreeMap<TypeId, Money>,
        unit_volume: BTreeMap<TypeId, Volume>,
        freight: Option<RateCard>,
        adjusted: BTreeMap<TypeId, Money>,
        manufacturing: Option<CostSetup>,
    }

    impl World {
        fn standard() -> World {
            let mut recipes = RecipeBook::new();
            recipes.insert(PRODUCT, recipe(1, 100, &[(A, 10), (B, 5)]));
            recipes.insert(A, recipe(10, 100, &[(M, 100)]));
            World {
                build: BTreeSet::new(),
                stock: BTreeMap::new(),
                recipes,
                modifiers: BTreeMap::from([
                    (PRODUCT, Modifier::one()),
                    (A, Modifier::one()),
                    (M, Modifier::one()),
                ]),
                unit_price: BTreeMap::from([(A, isk("1000")), (B, isk("50")), (M, isk("9"))]),
                unit_volume: BTreeMap::from([(A, m3("1")), (B, m3("0.1")), (M, m3("0.01"))]),
                freight: Some(RateCard {
                    per_m3: Some(isk("100")),
                    per_jump: Some(isk("1000")),
                    collateral: Some(Rate::from_ppm(10_000)),
                    minimum: Some(isk("5000000")),
                    max_m3: None,
                    max_collateral: None,
                }),
                adjusted: BTreeMap::from([(M, isk("8"))]),
                manufacturing: Some(CostSetup {
                    cost_index: Rate::from_ppm(50_000),
                    structure_multiplier: None,
                    facility_tax: NPC_STATION_TAX,
                    scc: SCC_SURCHARGE,
                }),
            }
        }

        fn chain(&self) -> Plan {
            plan(
                &PlanInput {
                    product: PRODUCT,
                    quantity: 2,
                    build: self.build.clone(),
                    modifiers: self.modifiers.clone(),
                    stock: self.stock.clone(),
                },
                &self.recipes,
            )
            .unwrap_or_else(|error| panic!("the plan is valid: {error}"))
        }

        fn decide(&self) -> Result<BTreeMap<TypeId, super::Decision>, MakeOrBuyError> {
            let chain = self.chain();
            make_or_buy(&MakeOrBuyInput {
                plan: &chain,
                recipes: &self.recipes,
                modifiers: &self.modifiers,
                unit_price: &self.unit_price,
                unit_volume: &self.unit_volume,
                freight: self.freight.as_ref(),
                adjusted: &self.adjusted,
                manufacturing: self.manufacturing.clone(),
                reaction: None,
            })
        }

        fn decided(&self) -> BTreeMap<TypeId, super::Decision> {
            self.decide().expect("the comparison fits")
        }
    }

    #[test]
    fn a_bought_part_is_built_when_building_is_cheaper() {
        let result = World::standard().decided();
        assert_eq!(result.keys().copied().collect::<Vec<_>>(), vec![A]);
        assert_eq!(result[&A].choice, Choice::Build);
        assert_eq!(result[&A].quantity, 20);
        assert_eq!(result[&A].buy_total, Some(isk("22200")));
        assert_eq!(result[&A].build_total, Some(isk("2166")));
        assert_eq!(result[&A].reason, Reason::Cheaper);
        assert!(result[&A].job_cost_incomplete.is_empty());
        assert_eq!(result[&A].reason_text(), "build: 2,166.00 vs buy 22,200.00");
    }

    #[test]
    fn a_tie_buys_and_a_hundredth_of_a_cent_decides() {
        let mut world = World::standard();
        world.freight = None;
        world.unit_price.insert(A, isk("97.40"));
        let tie = world.decided();
        assert_eq!(tie[&A].choice, Choice::Buy);
        assert_eq!(tie[&A].reason, Reason::Tie);
        assert_eq!(tie[&A].buy_total, Some(isk("1948")));
        assert_eq!(tie[&A].build_total, Some(isk("1948")));
        assert_eq!(tie[&A].reason_text(), "buy: 1,948.00 (same as build)");

        world.unit_price.insert(A, isk("97.4001"));
        let dearer = world.decided();
        assert_eq!(dearer[&A].buy_total, Some(isk("1948.002")));
        assert_eq!(dearer[&A].choice, Choice::Build);
        assert_eq!(dearer[&A].reason, Reason::Cheaper);

        world.unit_price.insert(A, isk("97.3999"));
        let cheaper = world.decided();
        assert_eq!(cheaper[&A].buy_total, Some(isk("1947.998")));
        assert_eq!(cheaper[&A].choice, Choice::Buy);
        assert_eq!(cheaper[&A].reason, Reason::Cheaper);
    }

    #[test]
    fn no_price_for_the_part_means_build() {
        let mut world = World::standard();
        world.unit_price.remove(&A);
        let result = world.decided();
        assert_eq!(result[&A].choice, Choice::Build);
        assert_eq!(result[&A].reason, Reason::NoMarketPrice);
        assert_eq!(result[&A].buy_total, None);
        assert_eq!(
            result[&A].reason_text(),
            "build: 2,166.00 (no market price)"
        );
    }

    #[test]
    fn no_price_for_a_material_means_buy() {
        let mut world = World::standard();
        world.unit_price.remove(&M);
        let result = world.decided();
        assert_eq!(result[&A].choice, Choice::Buy);
        assert_eq!(result[&A].reason, Reason::MaterialsUnpriced(vec![M]));
        assert_eq!(result[&A].build_total, None);
        assert_eq!(
            result[&A].reason_text(),
            "buy: 22,200.00 (no price for materials: 400)"
        );
    }

    #[test]
    fn no_price_either_way_is_unknown() {
        let mut world = World::standard();
        world.unit_price.remove(&A);
        world.unit_price.remove(&M);
        let result = world.decided();
        assert_eq!(result[&A].choice, Choice::Unknown);
        assert_eq!(result[&A].reason_text(), "no price either way");
    }

    #[test]
    fn no_facility_means_buy() {
        let mut world = World::standard();
        world.manufacturing = None;
        let result = world.decided();
        assert_eq!(result[&A].choice, Choice::Buy);
        assert_eq!(
            result[&A].reason,
            Reason::NoFacility(Activity::Manufacturing)
        );
        assert_eq!(result[&A].build_total, None);
        assert_eq!(
            result[&A].reason_text(),
            "buy: 22,200.00 (no facility for manufacturing)"
        );
    }

    #[test]
    fn no_modifier_means_no_facility() {
        let mut world = World::standard();
        world.modifiers.remove(&A);
        let result = world.decided();
        assert_eq!(result[&A].choice, Choice::Buy);
        assert_eq!(
            result[&A].reason,
            Reason::NoFacility(Activity::Manufacturing)
        );
    }

    #[test]
    fn a_built_part_is_decided_at_its_plan_quantity() {
        let mut world = World::standard();
        world.build.insert(A);
        let result = world.decided();
        assert_eq!(result[&A].quantity, 20);
        assert_eq!(result[&A], World::standard().decided()[&A]);
    }

    #[test]
    fn stock_reduces_the_bought_quantity() {
        let mut world = World::standard();
        world.stock.insert(A, 4);
        let result = world.decided();
        assert_eq!(result[&A].quantity, 16);
        assert_eq!(result[&A].buy_total, Some(isk("17760")));
        assert_eq!(result[&A].build_total, Some(isk("2166")));
        assert_eq!(result[&A].choice, Choice::Build);
    }

    #[test]
    fn a_part_below_a_bought_part_is_decided_at_the_amount_that_part_needs() {
        let mut world = World::standard();
        world.recipes.insert(M, recipe(1, 1000, &[(N, 2)]));
        world.modifiers.insert(N, Modifier::one());
        world.unit_price.insert(N, isk("1"));
        world.unit_volume.insert(N, m3("0.01"));
        world.adjusted.insert(N, isk("1"));
        let result = world.decided();
        assert_eq!(result[&M].choice, Choice::Build);
        assert_eq!(result[&M].quantity, 200);
        assert_eq!(result[&M].buy_total, Some(isk("2018")));
        assert_eq!(result[&M].build_total, Some(isk("841")));
        assert_eq!(result[&M].reason, Reason::Cheaper);
        assert_eq!(result[&A].build_total, Some(isk("989")));
    }

    #[test]
    fn a_missing_adjusted_price_leaves_the_job_share_out() {
        let mut world = World::standard();
        world.adjusted.remove(&M);
        let result = world.decided();
        assert_eq!(result[&A].choice, Choice::Build);
        assert_eq!(result[&A].build_total, Some(isk("2018")));
        assert_eq!(result[&A].job_cost_incomplete, vec![M]);
    }

    #[test]
    fn a_material_without_volume_has_no_hauling_charge() {
        let mut world = World::standard();
        world.unit_volume.remove(&M);
        let result = world.decided();
        assert_eq!(result[&A].build_total, Some(isk("1966")));
    }

    #[test]
    fn a_part_that_needs_itself_below_a_bought_part_cannot_be_built() {
        let mut world = World::standard();
        world.recipes.insert(M, recipe(1, 1000, &[(A, 1)]));
        let result = world.decided();
        assert_eq!(result[&M].choice, Choice::Buy);
        assert_eq!(result[&M].reason, Reason::Loop);
        assert_eq!(result[&M].buy_total, Some(isk("2018")));
        assert_eq!(result[&M].build_total, None);
        assert_eq!(result[&M].reason_text(), "buy: 2,018.00 (needs itself)");
    }

    #[test]
    fn an_overflowing_price_is_an_error() {
        let mut world = World::standard();
        world
            .unit_price
            .insert(A, Money::from_ten_thousandths(i64::MAX));
        assert_eq!(world.decide(), Err(MakeOrBuyError::Overflow));
    }

    #[test]
    fn a_shared_part_is_priced_at_each_parents_amount_but_decided_at_its_plan_amount() {
        // M is bought on its own line (2 units for P), and A's build needs 200 of it.
        let mut world = World::standard();
        world
            .recipes
            .insert(PRODUCT, recipe(1, 100, &[(A, 10), (B, 5), (M, 1)]));
        world.recipes.insert(M, recipe(1, 1000, &[(N, 2)]));
        world.modifiers.insert(N, Modifier::one());
        world.unit_price.insert(N, isk("1"));
        world.unit_volume.insert(N, m3("0.01"));
        world.adjusted.insert(N, isk("1"));
        let result = world.decided();
        assert_eq!(result[&M].quantity, 2);
        assert_eq!(result[&M].choice, Choice::Build);
        assert_eq!(result[&M].buy_total, Some(isk("20.18")));
        // The job's fees round to whole ISK, so this job costs nothing beyond its materials.
        assert_eq!(result[&M].build_total, Some(isk("8.04")));
        assert_eq!(result[&A].build_total, Some(isk("989")));
    }

    #[test]
    fn a_recipe_that_yields_nothing_is_not_decided() {
        let mut world = World::standard();
        world.recipes.insert(A, recipe(0, 100, &[(M, 100)]));
        assert!(world.decided().is_empty());
    }

    #[test]
    fn errors_read_as_sentences() {
        assert_eq!(
            MakeOrBuyError::Overflow.to_string(),
            "the build or buy comparison does not fit the supported range"
        );
        assert_eq!(
            MakeOrBuyError::Industry(IndustryError::MeOutOfRange(11)).to_string(),
            "material efficiency 11 is outside 0 to 10"
        );
        assert_eq!(
            MakeOrBuyError::JobCost(JobCostError::BadMultiplier(101)).to_string(),
            "structure cost multiplier 101 hundredths is above 100"
        );
    }

    #[test]
    fn a_bought_parts_hauling_and_collateral_are_charged_per_unit() {
        let world = World::standard();
        let chain = world.chain();
        let input = MakeOrBuyInput {
            plan: &chain,
            recipes: &world.recipes,
            modifiers: &world.modifiers,
            unit_price: &world.unit_price,
            unit_volume: &world.unit_volume,
            freight: world.freight.as_ref(),
            adjusted: &world.adjusted,
            manufacturing: world.manufacturing.clone(),
            reaction: None,
        };
        let solver = super::Solver {
            input: &input,
            plan_qty: BTreeMap::new(),
            decided: BTreeMap::new(),
            edges: BTreeSet::new(),
        };
        // B: 50 + 100 × 0.1 m³ + 1% of 50, per unit, for 10 units.
        assert_eq!(solver.buy_at(B, 10), Ok(Some(isk("605"))));
    }

    #[test]
    fn a_part_the_plan_has_in_stock_is_still_shown_below_a_bought_part() {
        // P needs 1 M itself, and 500 M are in stock, so the plan buys none of M. A's build needs 200.
        let mut world = World::standard();
        world
            .recipes
            .insert(PRODUCT, recipe(1, 100, &[(A, 10), (B, 5), (M, 1)]));
        world.recipes.insert(M, recipe(1, 1000, &[(N, 2)]));
        world.modifiers.insert(N, Modifier::one());
        world.unit_price.insert(N, isk("1"));
        world.unit_volume.insert(N, m3("0.01"));
        world.adjusted.insert(N, isk("1"));
        world.stock.insert(M, 500);
        let result = world.decided();
        assert_eq!(result[&M].quantity, 200);
        assert_eq!(result[&M].build_total, Some(isk("841")));
        assert_eq!(result[&A].build_total, Some(isk("989")));
    }

    #[test]
    fn a_part_in_stock_under_a_built_part_is_not_shown() {
        // A is built and needs M; the stock covers M, so M and what M needs stay out of the result.
        let mut world = World::standard();
        world.recipes.insert(M, recipe(1, 1000, &[(N, 2)]));
        world.recipes.insert(N, recipe(1, 1000, &[(B, 1)]));
        world.modifiers.insert(N, Modifier::one());
        world.unit_price.insert(N, isk("1"));
        world.build.insert(A);
        world.stock.insert(M, 500);
        let result = world.decided();
        assert_eq!(result.keys().copied().collect::<Vec<_>>(), vec![A]);
    }

    #[test]
    fn an_unpriced_material_two_levels_down_is_named() {
        // M needs N; neither M nor N has a price, so A's build is blocked by N.
        let mut world = World::standard();
        world.recipes.insert(M, recipe(1, 1000, &[(N, 2)]));
        world.modifiers.insert(N, Modifier::one());
        world.unit_price.remove(&M);
        let result = world.decided();
        assert_eq!(result[&A].choice, Choice::Buy);
        assert_eq!(result[&A].reason, Reason::MaterialsUnpriced(vec![N]));
        assert_eq!(
            result[&A].reason_text(),
            "buy: 22,200.00 (no price for materials: 500)"
        );
    }
}

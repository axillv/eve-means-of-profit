//! Plans for building units of a product: every step of the chain, and the exact list of what is left to buy.
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

use crate::industry::{
    Activity, IndustryError, JobSplit, Modifier, Recipe, RecipeBook, TypeId, split_jobs,
    split_quantity,
};

/// What the player asks for. `quantity` counts units of `product`, which is always built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanInput {
    pub product: TypeId,
    pub quantity: u64,
    pub build: BTreeSet<TypeId>,
    pub modifiers: BTreeMap<TypeId, Modifier>,
    pub stock: BTreeMap<TypeId, u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub steps: Vec<Step>,
    pub buy: Vec<BuyLine>,
    pub unused_stock: Vec<(TypeId, u64)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub type_id: TypeId,
    pub activity: Activity,
    pub depth: u32,
    pub needed: u64,
    pub from_stock: u64,
    pub to_build: u64,
    pub runs: u64,
    pub jobs: JobSplit,
    pub spare: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuyLine {
    pub type_id: TypeId,
    pub depth: u32,
    pub needed: u64,
    pub from_stock: u64,
    pub to_buy: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanError {
    QuantityZero,
    NoRecipe(TypeId),
    NoModifier(TypeId),
    Loop(TypeId),
    ZeroOutput(TypeId),
    Industry(IndustryError),
    Overflow,
}

impl fmt::Display for PlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::QuantityZero => write!(f, "the quantity to build must be at least one"),
            Self::NoRecipe(t) => write!(f, "type {t} has no recipe"),
            Self::NoModifier(t) => write!(f, "type {t} is built but has no material modifier"),
            Self::Loop(t) => write!(f, "type {t} needs itself through the built types"),
            Self::ZeroOutput(t) => write!(f, "type {t} has a recipe that yields no units per run"),
            Self::Industry(e) => write!(f, "{e}"),
            Self::Overflow => write!(f, "the plan does not fit the supported range"),
        }
    }
}

impl std::error::Error for PlanError {}

/// Works out the chain for `input.quantity` units of `input.product`.
pub fn plan(input: &PlanInput, recipes: &RecipeBook) -> Result<Plan, PlanError> {
    if input.quantity == 0 {
        return Err(PlanError::QuantityZero);
    }
    let mut built = BTreeSet::new();
    visit(input.product, input, recipes, &mut Vec::new(), &mut built)?;
    let depth = depths(input.product, &built, recipes)?;
    let mut order: Vec<(u32, TypeId)> = depth.iter().map(|(t, d)| (*d, *t)).collect();
    order.sort_unstable();

    let mut demand = BTreeMap::from([(input.product, input.quantity)]);
    let mut used_from_stock = BTreeMap::new();
    let mut steps = Vec::new();
    let mut buy = Vec::new();
    for (d, t) in order {
        let needed = demand.get(&t).copied().unwrap_or(0);
        let owned = if t == input.product {
            0
        } else {
            input.stock.get(&t).copied().unwrap_or(0)
        };
        let from_stock = owned.min(needed);
        used_from_stock.insert(t, from_stock);
        if built.contains(&t) {
            steps.push(build_step(
                t,
                d,
                needed,
                from_stock,
                input,
                recipes,
                &mut demand,
            )?);
        } else {
            buy.push(BuyLine {
                type_id: t,
                depth: d,
                needed,
                from_stock,
                // Cannot saturate: from_stock is min(owned, needed), so it is never above needed.
                to_buy: needed.saturating_sub(from_stock),
            });
        }
    }
    let unused_stock = input
        .stock
        .iter()
        .filter_map(|(t, owned)| {
            let used = used_from_stock.get(t).copied().unwrap_or(0);
            // Cannot saturate: used is this type's from_stock, min(owned, needed), so used <= owned.
            let left = owned.saturating_sub(used);
            (left > 0).then_some((*t, left))
        })
        .collect();
    Ok(Plan {
        steps,
        buy,
        unused_stock,
    })
}

fn recipe_of(t: TypeId, recipes: &RecipeBook) -> Result<&Recipe, PlanError> {
    recipes.get(t).ok_or(PlanError::NoRecipe(t))
}

/// Depth-first walk from the product through built types, in recipe order. It checks every built
/// type and finds loops; the first problem met wins. `done` collects the built types.
fn visit(
    t: TypeId,
    input: &PlanInput,
    recipes: &RecipeBook,
    path: &mut Vec<TypeId>,
    done: &mut BTreeSet<TypeId>,
) -> Result<(), PlanError> {
    let recipe = recipe_of(t, recipes)?;
    if recipe.output_per_run == 0 {
        return Err(PlanError::ZeroOutput(t));
    }
    if !input.modifiers.contains_key(&t) {
        return Err(PlanError::NoModifier(t));
    }
    path.push(t);
    for (material, _) in &recipe.materials {
        if path.contains(material) {
            return Err(PlanError::Loop(*material));
        }
        let built = *material == input.product || input.build.contains(material);
        if built && !done.contains(material) {
            visit(*material, input, recipes, path, done)?;
        }
    }
    path.pop();
    done.insert(t);
    Ok(())
}

/// Longest path from the product to each reached type, over built edges. Built types are
/// expanded only once all their built consumers are, so each depth is final when used.
/// Bought types only receive a depth.
fn depths(
    product: TypeId,
    built: &BTreeSet<TypeId>,
    recipes: &RecipeBook,
) -> Result<BTreeMap<TypeId, u32>, PlanError> {
    let mut pending: BTreeMap<TypeId, u32> = BTreeMap::new();
    for t in built {
        for (material, _) in &recipe_of(*t, recipes)?.materials {
            if built.contains(material) {
                let count = pending.entry(*material).or_insert(0);
                *count = count.checked_add(1).ok_or(PlanError::Overflow)?;
            }
        }
    }
    let mut depth = BTreeMap::from([(product, 0u32)]);
    let mut ready = vec![product];
    while let Some(t) = ready.pop() {
        let next = depth
            .get(&t)
            .copied()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or(PlanError::Overflow)?;
        for (material, _) in &recipe_of(t, recipes)?.materials {
            let reached = depth.entry(*material).or_insert(0);
            *reached = (*reached).max(next);
            if let Some(left) = pending.get_mut(material) {
                // Cannot saturate: each built edge into this type is counted once in pending and
                // decremented once, when its consumer is popped, so it never goes below 0.
                *left = left.saturating_sub(1);
                if *left == 0 {
                    ready.push(*material);
                }
            }
        }
    }
    Ok(depth)
}

/// The step for a built type, and the demand it puts on each material.
fn build_step(
    t: TypeId,
    depth: u32,
    needed: u64,
    from_stock: u64,
    input: &PlanInput,
    recipes: &RecipeBook,
    demand: &mut BTreeMap<TypeId, u64>,
) -> Result<Step, PlanError> {
    let recipe = recipe_of(t, recipes)?;
    let modifier = input
        .modifiers
        .get(&t)
        .copied()
        .ok_or(PlanError::NoModifier(t))?;
    // Cannot saturate: from_stock is min(owned, needed) from the caller, so it is never above needed.
    let to_build = needed.saturating_sub(from_stock);
    let runs = to_build.div_ceil(recipe.output_per_run);
    let produced = runs
        .checked_mul(recipe.output_per_run)
        .ok_or(PlanError::Overflow)?;
    let jobs = split_jobs(runs, recipe.max_runs_per_job).map_err(industry)?;
    for (material, base) in &recipe.materials {
        let units = split_quantity(*base, jobs, modifier).map_err(industry)?;
        let total = demand.entry(*material).or_insert(0);
        *total = total.checked_add(units).ok_or(PlanError::Overflow)?;
    }
    Ok(Step {
        type_id: t,
        activity: recipe.activity,
        depth,
        needed,
        from_stock,
        to_build,
        runs,
        jobs,
        // Cannot saturate: runs is to_build rounded up to whole runs, so produced >= to_build.
        spare: produced.saturating_sub(to_build),
    })
}

/// Overflow keeps its own variant; every other industry error is passed through.
fn industry(e: IndustryError) -> PlanError {
    match e {
        IndustryError::Overflow => PlanError::Overflow,
        IndustryError::MeOutOfRange(_)
        | IndustryError::BadStructureBonus(_)
        | IndustryError::BadRigFactor { .. }
        | IndustryError::ZeroMaxRuns => PlanError::Industry(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recipe(activity: Activity, output: u64, max: u64, materials: &[(TypeId, u64)]) -> Recipe {
        Recipe {
            activity,
            output_per_run: output,
            max_runs_per_job: max,
            materials: materials.to_vec(),
            made_by_invention: false,
        }
    }

    /// The Hobgoblin chain and the reaction chain, from SDE build 3607705.
    fn game_book() -> RecipeBook {
        let mut book = RecipeBook::new();
        insert_hobgoblin_and_reactions(&mut book);
        insert_ram_and_rifter(&mut book);
        book
    }

    fn insert_hobgoblin_and_reactions(book: &mut RecipeBook) {
        book.insert(
            2456,
            Recipe {
                made_by_invention: true,
                ..recipe(
                    Activity::Manufacturing,
                    1,
                    10,
                    &[
                        (11399, 1),
                        (11688, 1),
                        (9834, 1),
                        (2454, 1),
                        (9848, 1),
                        (11481, 1),
                    ],
                )
            },
        );
        book.insert(
            2454,
            recipe(
                Activity::Manufacturing,
                1,
                600,
                &[(34, 509), (35, 7), (37, 4), (38, 2)],
            ),
        );
        book.insert(
            11688,
            recipe(
                Activity::Manufacturing,
                1,
                4000,
                &[(16682, 1), (16670, 31), (16679, 11)],
            ),
        );
        book.insert(
            16670,
            recipe(
                Activity::Reaction,
                10000,
                1000,
                &[(4247, 5), (16655, 100), (16659, 100)],
            ),
        );
        book.insert(
            16655,
            recipe(
                Activity::Reaction,
                200,
                1000,
                &[(4247, 5), (16640, 100), (16643, 100)],
            ),
        );
        book.insert(
            16659,
            recipe(
                Activity::Reaction,
                200,
                1000,
                &[(4247, 5), (16633, 100), (16636, 100)],
            ),
        );
    }

    fn insert_ram_and_rifter(book: &mut RecipeBook) {
        book.insert(
            11481,
            recipe(
                Activity::Manufacturing,
                100,
                200,
                &[(34, 556), (35, 444), (36, 222), (37, 82), (38, 36)],
            ),
        );
        book.insert(
            587,
            recipe(
                Activity::Manufacturing,
                1,
                30,
                &[(34, 32000), (35, 6000), (36, 2500), (37, 500)],
            ),
        );
    }

    /// Small books for the error cases.
    fn error_book() -> RecipeBook {
        let mut book = RecipeBook::new();
        book.insert(
            900001,
            recipe(Activity::Manufacturing, 1, 10, &[(900002, 1)]),
        );
        book.insert(
            900002,
            recipe(Activity::Manufacturing, 1, 10, &[(900001, 1)]),
        );
        book.insert(
            900003,
            recipe(Activity::Manufacturing, 1, 10, &[(900003, 1)]),
        );
        book.insert(900010, recipe(Activity::Manufacturing, 0, 10, &[(34, 1)]));
        book.insert(900020, recipe(Activity::Manufacturing, 1, 0, &[(34, 1)]));
        book
    }

    fn me2() -> Modifier {
        Modifier::one().with_me(2).expect("ME 2 is valid")
    }

    fn me10() -> Modifier {
        Modifier::one().with_me(10).expect("ME 10 is valid")
    }

    fn drone2() -> Modifier {
        Modifier::one()
            .with_me(2)
            .and_then(|m| m.with_structure(99))
            .and_then(|m| m.with_rig(-24, 21))
            .expect("drone set is valid")
    }

    fn drone10() -> Modifier {
        Modifier::one()
            .with_me(10)
            .and_then(|m| m.with_structure(99))
            .and_then(|m| m.with_rig(-24, 21))
            .expect("drone set is valid")
    }

    fn rait10() -> Modifier {
        Modifier::one()
            .with_me(10)
            .and_then(|m| m.with_structure(99))
            .expect("rait set is valid")
    }

    fn comp2() -> Modifier {
        Modifier::one()
            .with_rig(-24, 11)
            .expect("composite rig is valid")
    }

    fn rifter() -> Modifier {
        Modifier::one()
            .with_me(10)
            .and_then(|m| m.with_structure(99))
            .and_then(|m| m.with_rig(-20, 21))
            .expect("rifter set is valid")
    }

    fn input(
        product: TypeId,
        quantity: u64,
        build: &[TypeId],
        modifiers: &[(TypeId, Modifier)],
        stock: &[(TypeId, u64)],
    ) -> PlanInput {
        PlanInput {
            product,
            quantity,
            build: build.iter().copied().collect(),
            modifiers: modifiers.iter().copied().collect(),
            stock: stock.iter().copied().collect(),
        }
    }

    /// `amounts` is (needed, from_stock, to_build); `jobs` is (full_jobs, runs_per_full_job, last_job_runs).
    fn step(
        t: TypeId,
        activity: Activity,
        depth: u32,
        amounts: (u64, u64, u64),
        runs: u64,
        jobs: (u64, u64, u64),
        spare: u64,
    ) -> Step {
        let (needed, from_stock, to_build) = amounts;
        let (full_jobs, runs_per_full_job, last_job_runs) = jobs;
        Step {
            type_id: t,
            activity,
            depth,
            needed,
            from_stock,
            to_build,
            runs,
            jobs: JobSplit {
                full_jobs,
                runs_per_full_job,
                last_job_runs,
            },
            spare,
        }
    }

    fn buy(t: TypeId, depth: u32, needed: u64, from_stock: u64, to_buy: u64) -> BuyLine {
        BuyLine {
            type_id: t,
            depth,
            needed,
            from_stock,
            to_buy,
        }
    }

    fn p1(build: &[TypeId]) -> PlanInput {
        input(2456, 10, build, &[(2456, me2())], &[])
    }

    fn p3_input() -> PlanInput {
        input(
            2456,
            10,
            &[2454, 11688],
            &[(2456, drone2()), (2454, drone10()), (11688, rait10())],
            &[(11688, 4), (34, 1000)],
        )
    }

    #[test]
    fn p1_buys_the_whole_first_level() {
        let got = plan(&p1(&[]), &game_book()).expect("P1 plans");
        assert_eq!(
            got,
            Plan {
                steps: vec![step(
                    2456,
                    Activity::Manufacturing,
                    0,
                    (10, 0, 10),
                    10,
                    (1, 10, 0),
                    0
                )],
                buy: vec![
                    buy(2454, 1, 10, 0, 10),
                    buy(9834, 1, 10, 0, 10),
                    buy(9848, 1, 10, 0, 10),
                    buy(11399, 1, 10, 0, 10),
                    buy(11481, 1, 10, 0, 10),
                    buy(11688, 1, 10, 0, 10),
                ],
                unused_stock: vec![],
            }
        );
    }

    #[test]
    fn p2_uses_stock_and_keeps_the_product_units() {
        let stock = [(2454, 25), (11399, 5), (34, 3000), (2456, 37)];
        let got = plan(
            &input(2456, 10, &[], &[(2456, me2())], &stock),
            &game_book(),
        )
        .expect("P2 plans");
        assert_eq!(
            got.steps,
            plan(&p1(&[]), &game_book()).expect("P1 plans").steps
        );
        assert_eq!(
            got.buy,
            vec![
                buy(2454, 1, 10, 10, 0),
                buy(9834, 1, 10, 0, 10),
                buy(9848, 1, 10, 0, 10),
                buy(11399, 1, 10, 5, 5),
                buy(11481, 1, 10, 0, 10),
                buy(11688, 1, 10, 0, 10),
            ]
        );
        assert_eq!(got.unused_stock, vec![(34, 3000), (2454, 15), (2456, 37)]);
    }

    #[test]
    fn p3_builds_two_levels() {
        let got = plan(&p3_input(), &game_book()).expect("P3 plans");
        assert_eq!(
            got.steps,
            vec![
                step(
                    2456,
                    Activity::Manufacturing,
                    0,
                    (10, 0, 10),
                    10,
                    (1, 10, 0),
                    0
                ),
                step(
                    2454,
                    Activity::Manufacturing,
                    1,
                    (10, 0, 10),
                    10,
                    (0, 600, 10),
                    0
                ),
                step(
                    11688,
                    Activity::Manufacturing,
                    1,
                    (10, 4, 6),
                    6,
                    (0, 4000, 6),
                    0
                ),
            ]
        );
        assert_eq!(
            got.buy,
            vec![
                buy(9834, 1, 10, 0, 10),
                buy(9848, 1, 10, 0, 10),
                buy(11399, 1, 10, 0, 10),
                buy(11481, 1, 10, 0, 10),
                buy(34, 2, 4307, 1000, 3307),
                buy(35, 2, 60, 0, 60),
                buy(37, 2, 34, 0, 34),
                buy(38, 2, 17, 0, 17),
                buy(16670, 2, 166, 0, 166),
                buy(16679, 2, 59, 0, 59),
                buy(16682, 2, 6, 0, 6),
            ]
        );
        assert_eq!(got.unused_stock, vec![]);
    }

    #[test]
    fn p3b_builds_a_reaction_under_a_build_step() {
        let mut with_16670 = p3_input();
        with_16670.build.insert(16670);
        with_16670.modifiers.insert(16670, comp2());
        let got = plan(&with_16670, &game_book()).expect("P3b plans");
        assert_eq!(
            got.steps[3],
            step(
                16670,
                Activity::Reaction,
                2,
                (166, 0, 166),
                1,
                (0, 1000, 1),
                9834
            )
        );
        assert_eq!(
            got.buy,
            vec![
                buy(9834, 1, 10, 0, 10),
                buy(9848, 1, 10, 0, 10),
                buy(11399, 1, 10, 0, 10),
                buy(11481, 1, 10, 0, 10),
                buy(34, 2, 4307, 1000, 3307),
                buy(35, 2, 60, 0, 60),
                buy(37, 2, 34, 0, 34),
                buy(38, 2, 17, 0, 17),
                buy(16679, 2, 59, 0, 59),
                buy(16682, 2, 6, 0, 6),
                buy(4247, 3, 5, 0, 5),
                buy(16655, 3, 98, 0, 98),
                buy(16659, 3, 98, 0, 98),
            ]
        );
    }

    #[test]
    fn p4_reactions_use_stock_and_skip_a_zero_run_step() {
        let got = plan(
            &input(
                16670,
                10000,
                &[16655, 16659],
                &[
                    (16670, Modifier::one()),
                    (16655, Modifier::one()),
                    (16659, Modifier::one()),
                ],
                &[(16659, 34000), (4247, 4412)],
            ),
            &game_book(),
        )
        .expect("P4 plans");
        assert_eq!(
            got.steps,
            vec![
                step(
                    16670,
                    Activity::Reaction,
                    0,
                    (10000, 0, 10000),
                    1,
                    (0, 1000, 1),
                    0
                ),
                step(
                    16655,
                    Activity::Reaction,
                    1,
                    (100, 0, 100),
                    1,
                    (0, 1000, 1),
                    100
                ),
                step(
                    16659,
                    Activity::Reaction,
                    1,
                    (100, 100, 0),
                    0,
                    (0, 1000, 0),
                    0
                ),
            ]
        );
        assert_eq!(
            got.buy,
            vec![
                buy(4247, 2, 10, 10, 0),
                buy(16633, 2, 0, 0, 0),
                buy(16636, 2, 0, 0, 0),
                buy(16640, 2, 100, 0, 100),
                buy(16643, 2, 100, 0, 100),
            ]
        );
        assert_eq!(got.unused_stock, vec![(4247, 4402), (16659, 33900)]);
    }

    #[test]
    fn p5_output_above_one_per_run() {
        let got = plan(
            &input(11481, 150, &[], &[(11481, me10())], &[]),
            &game_book(),
        )
        .expect("P5 plans");
        assert_eq!(
            got.steps,
            vec![step(
                11481,
                Activity::Manufacturing,
                0,
                (150, 0, 150),
                2,
                (0, 200, 2),
                50
            )]
        );
        assert_eq!(
            got.buy,
            vec![
                buy(34, 1, 1001, 0, 1001),
                buy(35, 1, 800, 0, 800),
                buy(36, 1, 400, 0, 400),
                buy(37, 1, 148, 0, 148),
                buy(38, 1, 65, 0, 65),
            ]
        );
    }

    #[test]
    fn p6_job_run_limit() {
        let got =
            plan(&input(587, 31, &[], &[(587, rifter())], &[]), &game_book()).expect("P6 plans");
        assert_eq!(
            got.steps,
            vec![step(
                587,
                Activity::Manufacturing,
                0,
                (31, 0, 31),
                31,
                (1, 30, 1),
                0
            )]
        );
        assert_eq!(
            got.buy,
            vec![
                buy(34, 1, 846750, 0, 846750),
                buy(35, 1, 158767, 0, 158767),
                buy(36, 1, 66153, 0, 66153),
                buy(37, 1, 13231, 0, 13231),
            ]
        );
    }

    #[test]
    fn quantity_zero_is_rejected() {
        assert_eq!(
            plan(&input(2456, 0, &[], &[], &[]), &game_book()),
            Err(PlanError::QuantityZero)
        );
    }

    #[test]
    fn product_without_recipe_is_rejected() {
        assert_eq!(
            plan(
                &input(34, 1, &[], &[(34, Modifier::one())], &[]),
                &game_book()
            ),
            Err(PlanError::NoRecipe(34))
        );
    }

    #[test]
    fn bought_type_without_recipe_is_not_an_error() {
        assert_eq!(plan(&p1(&[34]), &game_book()), plan(&p1(&[]), &game_book()));
    }

    #[test]
    fn built_type_without_recipe_is_rejected() {
        assert_eq!(
            plan(&p3_input_with_34(), &game_book()),
            Err(PlanError::NoRecipe(34))
        );
    }

    fn p3_input_with_34() -> PlanInput {
        let mut p = p3_input();
        p.build.insert(34);
        p
    }

    #[test]
    fn built_type_without_modifier_is_rejected() {
        assert_eq!(
            plan(&input(2456, 10, &[], &[], &[]), &game_book()),
            Err(PlanError::NoModifier(2456))
        );
    }

    #[test]
    fn loop_through_built_types_is_rejected() {
        assert_eq!(
            plan(
                &input(
                    900001,
                    1,
                    &[900002],
                    &[(900001, Modifier::one()), (900002, Modifier::one())],
                    &[]
                ),
                &error_book()
            ),
            Err(PlanError::Loop(900001))
        );
    }

    #[test]
    fn self_loop_is_rejected() {
        assert_eq!(
            plan(
                &input(900003, 1, &[], &[(900003, Modifier::one())], &[]),
                &error_book()
            ),
            Err(PlanError::Loop(900003))
        );
    }

    #[test]
    fn loop_is_found_before_overflow() {
        assert_eq!(
            plan(
                &input(
                    900001,
                    u64::MAX,
                    &[900002],
                    &[(900001, Modifier::one()), (900002, Modifier::one())],
                    &[]
                ),
                &error_book()
            ),
            Err(PlanError::Loop(900001))
        );
    }

    #[test]
    fn zero_output_on_a_built_type_is_rejected() {
        assert_eq!(
            plan(
                &input(900010, 1, &[], &[(900010, Modifier::one())], &[]),
                &error_book()
            ),
            Err(PlanError::ZeroOutput(900010))
        );
    }

    #[test]
    fn zero_max_runs_is_an_industry_error() {
        assert_eq!(
            plan(
                &input(900020, 1, &[], &[(900020, Modifier::one())], &[]),
                &error_book()
            ),
            Err(PlanError::Industry(IndustryError::ZeroMaxRuns))
        );
    }

    #[test]
    fn errors_read_as_sentences() {
        assert_eq!(
            PlanError::QuantityZero.to_string(),
            "the quantity to build must be at least one"
        );
        assert_eq!(PlanError::NoRecipe(34).to_string(), "type 34 has no recipe");
        assert_eq!(
            PlanError::NoModifier(2456).to_string(),
            "type 2456 is built but has no material modifier"
        );
        assert_eq!(
            PlanError::Loop(900001).to_string(),
            "type 900001 needs itself through the built types"
        );
        assert_eq!(
            PlanError::ZeroOutput(900010).to_string(),
            "type 900010 has a recipe that yields no units per run"
        );
        assert_eq!(
            PlanError::Industry(IndustryError::ZeroMaxRuns).to_string(),
            "a job must allow at least one run"
        );
        assert_eq!(
            PlanError::Overflow.to_string(),
            "the plan does not fit the supported range"
        );
    }

    #[test]
    fn huge_quantity_overflows_without_panic() {
        assert_eq!(
            plan(
                &input(2454, u64::MAX, &[], &[(2454, Modifier::one())], &[]),
                &game_book()
            ),
            Err(PlanError::Overflow)
        );
    }
}

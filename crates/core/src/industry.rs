//! Quantities of materials that an industry job consumes, computed exactly.
//!
//! Integers only: no floats, no `as` casts, checked arithmetic everywhere.
//! A [`Modifier`] is an exact product of decimal factors, so every result is
//! reproducible to the unit.

#![deny(
    clippy::arithmetic_side_effects,
    clippy::float_arithmetic,
    clippy::as_conversions,
    clippy::integer_division
)]

use std::collections::BTreeMap;
use std::fmt;

pub type TypeId = u32;

/// Most decimal places a modifier keeps. 10^38 is the largest power of ten in `u128`.
const MAX_EXP: u8 = 38;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activity {
    Manufacturing,
    Reaction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recipe {
    pub activity: Activity,
    pub output_per_run: u64,
    pub max_runs_per_job: u64,
    /// Materials in the order the game data lists them.
    pub materials: Vec<(TypeId, u64)>,
    pub made_by_invention: bool,
}

/// Recipes keyed by the product's type id.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecipeBook {
    recipes: BTreeMap<TypeId, Recipe>,
}

impl RecipeBook {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, product: TypeId, recipe: Recipe) {
        self.recipes.insert(product, recipe);
    }

    pub fn get(&self, product: TypeId) -> Option<&Recipe> {
        self.recipes.get(&product)
    }

    pub fn len(&self) -> usize {
        self.recipes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.recipes.is_empty()
    }
}

/// An exact product of decimal factors, stored as `num / 10^exp`.
///
/// Trailing zeros are removed after every step, so equal values compare equal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Modifier {
    num: u128,
    exp: u8,
}

impl Modifier {
    pub fn one() -> Self {
        Self { num: 1, exp: 0 }
    }

    /// Multiplies by `(100 - me) / 100`. Material efficiency runs from 0 to 10.
    pub fn with_me(self, me: u8) -> Result<Self, IndustryError> {
        if me > 10 {
            return Err(IndustryError::MeOutOfRange(me));
        }
        self.times(u128::from(100u8.saturating_sub(me)), 2)
    }

    /// Multiplies by `hundredths / 100`, so a Raitaru's 99 is 0.99. Valid range is 1 to 100.
    pub fn with_structure(self, hundredths: u32) -> Result<Self, IndustryError> {
        if !(1..=100).contains(&hundredths) {
            return Err(IndustryError::BadStructureBonus(hundredths));
        }
        self.times(u128::from(hundredths), 2)
    }

    /// Multiplies by `1 + bonus × security / 10000`, with the rig bonus in tenths of a percent
    /// (negative) and the system security in tenths. A T2 rig in null sec is `with_rig(-24, 21)`,
    /// which is 0.9496.
    pub fn with_rig(
        self,
        bonus_tenths_percent: i32,
        security_tenths: u32,
    ) -> Result<Self, IndustryError> {
        let err = || IndustryError::BadRigFactor {
            bonus_tenths_percent,
            security_tenths,
        };
        if bonus_tenths_percent >= 0 {
            return Err(err());
        }
        let scaled = i64::from(bonus_tenths_percent)
            .checked_mul(i64::from(security_tenths))
            .and_then(|v| v.checked_add(10_000))
            .ok_or(IndustryError::Overflow)?;
        let factor = u128::try_from(scaled)
            .ok()
            .filter(|f| *f > 0)
            .ok_or_else(err)?;
        self.times(factor, 4)
    }

    /// Exact decimal form with trailing zeros removed, such as `"0.8460936"`, or `"1"`.
    pub fn to_decimal_string(self) -> String {
        if self.exp == 0 {
            return self.num.to_string();
        }
        let width = usize::from(self.exp).saturating_add(1);
        let digits = format!("{:0>width$}", self.num);
        let point = digits.len().saturating_sub(usize::from(self.exp));
        let (whole, fraction) = digits.split_at(point);
        format!("{whole}.{fraction}")
    }

    fn times(self, factor: u128, factor_exp: u8) -> Result<Self, IndustryError> {
        let num = self
            .num
            .checked_mul(factor)
            .ok_or(IndustryError::Overflow)?;
        let exp = self
            .exp
            .checked_add(factor_exp)
            .ok_or(IndustryError::Overflow)?;
        let product = normalized(num, exp);
        if product.exp > MAX_EXP {
            return Err(IndustryError::Overflow);
        }
        Ok(product)
    }
}

fn normalized(mut num: u128, mut exp: u8) -> Modifier {
    while exp > 0 && num.checked_rem(10) == Some(0) {
        let Some(shorter) = num.checked_div(10) else {
            break;
        };
        num = shorter;
        exp = exp.saturating_sub(1);
    }
    Modifier { num, exp }
}

/// Units of one material for one job of `runs` runs from base quantity `base`:
/// `max(runs, ceil(round2(base × runs × modifier)))`, where `round2` rounds to two decimals, half up.
/// Zero runs give zero units.
pub fn job_quantity(base: u64, runs: u64, modifier: Modifier) -> Result<u64, IndustryError> {
    if runs == 0 {
        return Ok(0);
    }
    let product = u128::from(base)
        .checked_mul(u128::from(runs))
        .and_then(|v| v.checked_mul(modifier.num))
        .ok_or(IndustryError::Overflow)?;
    let scale = 10u128
        .checked_pow(u32::from(modifier.exp))
        .ok_or(IndustryError::Overflow)?;
    // hundredths = round(100 × product / scale), half up, as floor((200 × product + scale) / (2 × scale))
    let doubled = product
        .checked_mul(200)
        .and_then(|v| v.checked_add(scale))
        .ok_or(IndustryError::Overflow)?;
    let hundredths = doubled
        .checked_div(scale.checked_mul(2).ok_or(IndustryError::Overflow)?)
        .ok_or(IndustryError::Overflow)?;
    // ceil(hundredths / 100)
    let units = hundredths
        .checked_add(99)
        .and_then(|v| v.checked_div(100))
        .ok_or(IndustryError::Overflow)?;
    let units = u64::try_from(units).ok().ok_or(IndustryError::Overflow)?;
    Ok(units.max(runs))
}

/// How a run count is cut into jobs: `full_jobs` jobs of `runs_per_full_job` runs, then one job of `last_job_runs`
/// runs when that is not zero. Compact on purpose, so a huge run count never allocates one entry per job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobSplit {
    pub full_jobs: u64,
    pub runs_per_full_job: u64,
    pub last_job_runs: u64,
}

impl JobSplit {
    pub fn job_count(&self) -> u64 {
        self.full_jobs
            .saturating_add(u64::from(self.last_job_runs > 0))
    }
}

/// Splits `runs` into as few jobs as possible, none longer than `max_runs_per_job`.
pub fn split_jobs(runs: u64, max_runs_per_job: u64) -> Result<JobSplit, IndustryError> {
    let (Some(full_jobs), Some(last_job_runs)) = (
        runs.checked_div(max_runs_per_job),
        runs.checked_rem(max_runs_per_job),
    ) else {
        return Err(IndustryError::ZeroMaxRuns);
    };
    Ok(JobSplit {
        full_jobs,
        runs_per_full_job: max_runs_per_job,
        last_job_runs,
    })
}

/// Units of one material across all jobs of `split`, each job rounded on its own.
pub fn split_quantity(
    base: u64,
    split: JobSplit,
    modifier: Modifier,
) -> Result<u64, IndustryError> {
    let full_total = if split.full_jobs == 0 {
        0
    } else {
        job_quantity(base, split.runs_per_full_job, modifier)?
            .checked_mul(split.full_jobs)
            .ok_or(IndustryError::Overflow)?
    };
    let last = job_quantity(base, split.last_job_runs, modifier)?;
    full_total.checked_add(last).ok_or(IndustryError::Overflow)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndustryError {
    MeOutOfRange(u8),
    BadStructureBonus(u32),
    BadRigFactor {
        bonus_tenths_percent: i32,
        security_tenths: u32,
    },
    ZeroMaxRuns,
    Overflow,
}

impl fmt::Display for IndustryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MeOutOfRange(me) => write!(f, "material efficiency {me} is outside 0 to 10"),
            Self::BadStructureBonus(hundredths) => {
                write!(
                    f,
                    "structure bonus {hundredths} hundredths is outside 1 to 100"
                )
            }
            Self::BadRigFactor {
                bonus_tenths_percent,
                security_tenths,
            } => write!(
                f,
                "rig bonus {bonus_tenths_percent} tenths of a percent at security {security_tenths} tenths is not a penalty that leaves a positive factor"
            ),
            Self::ZeroMaxRuns => write!(f, "a job must allow at least one run"),
            Self::Overflow => write!(f, "the quantity does not fit the supported range"),
        }
    }
}

impl std::error::Error for IndustryError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn drone_set() -> Modifier {
        Modifier::one()
            .with_me(10)
            .and_then(|m| m.with_structure(99))
            .and_then(|m| m.with_rig(-24, 21))
            .expect("drone set is valid")
    }

    fn rifter_set() -> Modifier {
        Modifier::one()
            .with_me(10)
            .and_then(|m| m.with_structure(99))
            .and_then(|m| m.with_rig(-20, 21))
            .expect("rifter set is valid")
    }

    fn quantity(base: u64, runs: u64, modifier: Modifier) -> u64 {
        job_quantity(base, runs, modifier).expect("quantity fits")
    }

    #[test]
    fn drone_set_matches_the_game() {
        let m = drone_set();
        assert_eq!(m.to_decimal_string(), "0.8460936");
        assert_eq!(quantity(509, 10, m), 4307);
        assert_eq!(quantity(7, 10, m), 60);
        assert_eq!(quantity(4, 10, m), 34);
        assert_eq!(quantity(2, 10, m), 17);
    }

    #[test]
    fn low_sec_rig_one_matches_the_game() {
        let m = Modifier::one()
            .with_me(10)
            .and_then(|m| m.with_structure(99))
            .and_then(|m| m.with_rig(-20, 19))
            .expect("low sec set is valid");
        assert_eq!(quantity(509, 10, m), 4363);
        assert_eq!(quantity(7, 10, m), 60);
        assert_eq!(quantity(4, 10, m), 35);
        assert_eq!(quantity(2, 10, m), 18);
    }

    #[test]
    fn rounds_to_two_decimals_before_rounding_up() {
        let m = drone_set();
        // Exact 1061.0013744: plain rounding up would give 1062.
        assert_eq!(quantity(627, 2, m), 1061);
        // Exact 1050.0021576.
        assert_eq!(quantity(73, 17, m), 1050);
    }

    #[test]
    fn never_below_one_per_run() {
        assert_eq!(quantity(1, 10, drone_set()), 10);
        let me2 = Modifier::one().with_me(2).expect("ME 2 is valid");
        assert_eq!(quantity(1, 10, me2), 10);
    }

    #[test]
    fn structure_without_rig() {
        let m = Modifier::one()
            .with_me(10)
            .and_then(|m| m.with_structure(99))
            .expect("structure set is valid");
        assert_eq!(m.to_decimal_string(), "0.891");
        assert_eq!(quantity(31, 10, m), 277);
        assert_eq!(quantity(11, 10, m), 99);
        assert_eq!(quantity(1, 10, m), 10);
        assert_eq!(quantity(31, 1, m), 28);
        assert_eq!(quantity(11, 1, m), 10);
        assert_eq!(quantity(1, 1, m), 1);
    }

    #[test]
    fn reaction_with_composite_rig() {
        let m = Modifier::one()
            .with_rig(-24, 11)
            .expect("composite rig is valid");
        assert_eq!(m.to_decimal_string(), "0.9736");
        assert_eq!(quantity(100, 1, m), 98);
        assert_eq!(quantity(5, 1, m), 5);
        assert_eq!(quantity(100, 10, m), 974);
        assert_eq!(quantity(5, 10, m), 49);
    }

    #[test]
    fn rifter_set_matches_the_game() {
        let m = rifter_set();
        assert_eq!(m.to_decimal_string(), "0.853578");
        assert_eq!(quantity(32000, 30, m), 819435);
        assert_eq!(quantity(6000, 30, m), 153645);
        assert_eq!(quantity(2500, 30, m), 64019);
        assert_eq!(quantity(500, 30, m), 12804);
        assert_eq!(quantity(32000, 1, m), 27315);
        assert_eq!(quantity(6000, 1, m), 5122);
        assert_eq!(quantity(2500, 1, m), 2134);
        assert_eq!(quantity(500, 1, m), 427);
    }

    #[test]
    fn no_bonus_is_exact() {
        assert_eq!(quantity(509, 10, Modifier::one()), 5090);
        assert_eq!(quantity(509, 0, drone_set()), 0);
        assert_eq!(quantity(0, 0, Modifier::one()), 0);
        assert_eq!(Modifier::one().to_decimal_string(), "1");
        let me0 = Modifier::one().with_me(0).expect("ME 0 is valid");
        assert_eq!(me0, Modifier::one());
    }

    #[test]
    fn splits_runs_into_jobs() {
        let cases = [
            ((31, 30), (1, 30, 1), 2),
            ((60, 30), (2, 30, 0), 2),
            ((5, 10), (0, 10, 5), 1),
            ((1000, 600), (1, 600, 400), 2),
            ((0, 30), (0, 30, 0), 0),
        ];
        for ((runs, max), (full, per_full, last), jobs) in cases {
            let split = split_jobs(runs, max).expect("max is positive");
            assert_eq!(
                split,
                JobSplit {
                    full_jobs: full,
                    runs_per_full_job: per_full,
                    last_job_runs: last,
                }
            );
            assert_eq!(split.job_count(), jobs);
        }
        assert_eq!(split_jobs(5, 0), Err(IndustryError::ZeroMaxRuns));
    }

    #[test]
    fn split_quantity_sums_the_jobs() {
        let m = rifter_set();
        let split = split_jobs(31, 30).expect("max is positive");
        assert_eq!(split_quantity(6000, split, m), Ok(158767));
        assert_eq!(split_quantity(32000, split, m), Ok(846750));
        let huge = split_jobs(u64::MAX, 600).expect("max is positive");
        assert_eq!(
            split_quantity(509, huge, Modifier::one()),
            Err(IndustryError::Overflow)
        );
    }

    #[test]
    fn rejects_out_of_range_inputs() {
        assert_eq!(
            Modifier::one().with_me(11),
            Err(IndustryError::MeOutOfRange(11))
        );
        assert_eq!(
            Modifier::one().with_structure(0),
            Err(IndustryError::BadStructureBonus(0))
        );
        assert_eq!(
            Modifier::one().with_structure(101),
            Err(IndustryError::BadStructureBonus(101))
        );
        assert_eq!(
            Modifier::one().with_rig(0, 21),
            Err(IndustryError::BadRigFactor {
                bonus_tenths_percent: 0,
                security_tenths: 21,
            })
        );
        assert_eq!(
            Modifier::one().with_rig(5, 21),
            Err(IndustryError::BadRigFactor {
                bonus_tenths_percent: 5,
                security_tenths: 21,
            })
        );
        assert_eq!(
            Modifier::one().with_rig(-1000, 10),
            Err(IndustryError::BadRigFactor {
                bonus_tenths_percent: -1000,
                security_tenths: 10,
            })
        );
        assert_eq!(
            job_quantity(u64::MAX, u64::MAX, Modifier::one()),
            Err(IndustryError::Overflow)
        );
    }

    #[test]
    fn recipe_book_keeps_recipes_by_product() {
        let recipe = |output| Recipe {
            activity: Activity::Manufacturing,
            output_per_run: output,
            max_runs_per_job: 10,
            materials: vec![(34, 5)],
            made_by_invention: false,
        };
        let mut book = RecipeBook::new();
        assert!(book.is_empty());
        book.insert(1, recipe(1));
        book.insert(2, recipe(2));
        assert!(!book.is_empty());
        assert_eq!(book.get(1), Some(&recipe(1)));
        assert_eq!(book.get(2), Some(&recipe(2)));
        assert_eq!(book.get(3), None);
        assert_eq!(book.len(), 2);
    }

    #[test]
    fn normalization_only_removes_trailing_zeros() {
        assert_eq!(normalized(10, 0), Modifier { num: 10, exp: 0 });
        assert_eq!(normalized(150, 2), Modifier { num: 15, exp: 1 });
    }

    #[test]
    fn decimal_places_stop_at_thirty_eight() {
        let mut m = Modifier::one();
        for _ in 0..19 {
            m = m.with_structure(1).expect("38 places are allowed");
        }
        assert_eq!(m.to_decimal_string(), format!("0.{}1", "0".repeat(37)));
        assert_eq!(m.with_structure(1), Err(IndustryError::Overflow));
        assert_eq!(m.with_structure(10), Err(IndustryError::Overflow));
    }

    #[test]
    fn errors_read_as_sentences() {
        assert_eq!(
            IndustryError::MeOutOfRange(11).to_string(),
            "material efficiency 11 is outside 0 to 10"
        );
        assert_eq!(
            IndustryError::BadStructureBonus(0).to_string(),
            "structure bonus 0 hundredths is outside 1 to 100"
        );
        assert_eq!(
            IndustryError::BadRigFactor {
                bonus_tenths_percent: 5,
                security_tenths: 21,
            }
            .to_string(),
            "rig bonus 5 tenths of a percent at security 21 tenths is not a penalty that leaves a positive factor"
        );
        assert_eq!(
            IndustryError::ZeroMaxRuns.to_string(),
            "a job must allow at least one run"
        );
        assert_eq!(
            IndustryError::Overflow.to_string(),
            "the quantity does not fit the supported range"
        );
    }
}

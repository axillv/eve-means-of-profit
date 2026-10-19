//! What the game charges to install an industry job: the estimated item value and its fees.
//!
//! Each part is rounded to whole ISK, half away from zero, one job at a time. Integers only.

#![deny(
    clippy::arithmetic_side_effects,
    clippy::float_arithmetic,
    clippy::as_conversions,
    clippy::integer_division
)]

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::decimal::{UNIT, div_round};
use crate::industry::{JobSplit, TypeId};
use crate::money::{Money, Rate};

/// The surcharge the game adds on the estimated item value of every job: 4%.
pub const SCC_SURCHARGE: Rate = Rate::from_ppm(40_000);
/// The facility tax of an NPC station: 0.25%.
pub const NPC_STATION_TAX: Rate = Rate::from_ppm(2_500);

/// A rate of 100% in parts per million.
const FULL_RATE: i128 = 1_000_000;
/// A structure multiplier of 100 hundredths means no bonus.
const FULL_MULTIPLIER: u32 = 100;

/// The facility a job is installed in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CostSetup {
    /// The system's cost index for the activity: manufacturing or reaction.
    pub cost_index: Rate,
    /// The structure's job cost multiplier in hundredths (97 for a Raitaru); `None` for an NPC station.
    pub structure_multiplier: Option<u32>,
    pub facility_tax: Rate,
    pub scc: Rate,
}

/// The cost of one job. Every field is a whole number of ISK.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobCost {
    pub item_value: Money,
    pub system_cost: Money,
    /// The amount the structure takes off the system cost; never negative.
    pub structure_bonus: Money,
    pub facility_tax: Money,
    pub scc: Money,
    pub total: Money,
}

impl JobCost {
    /// The total per unit made, rounded to 0.0001 ISK; `None` for zero units.
    pub fn per_unit(&self, units: u64) -> Option<Money> {
        self.total.checked_div_qty(units)
    }

    fn zero() -> JobCost {
        JobCost {
            item_value: Money::ZERO,
            system_cost: Money::ZERO,
            structure_bonus: Money::ZERO,
            facility_tax: Money::ZERO,
            scc: Money::ZERO,
            total: Money::ZERO,
        }
    }

    fn plus(&self, other: &JobCost) -> Result<JobCost, JobCostError> {
        let add = |a: Money, b: Money| a.checked_add(b).ok_or(JobCostError::Overflow);
        Ok(JobCost {
            item_value: add(self.item_value, other.item_value)?,
            system_cost: add(self.system_cost, other.system_cost)?,
            structure_bonus: add(self.structure_bonus, other.structure_bonus)?,
            facility_tax: add(self.facility_tax, other.facility_tax)?,
            scc: add(self.scc, other.scc)?,
            total: add(self.total, other.total)?,
        })
    }

    /// This job repeated `jobs` times. Every job is identical, so each part is multiplied.
    fn times(&self, jobs: u64) -> Result<JobCost, JobCostError> {
        let scale = |amount: Money| amount.checked_mul_qty(jobs).ok_or(JobCostError::Overflow);
        Ok(JobCost {
            item_value: scale(self.item_value)?,
            system_cost: scale(self.system_cost)?,
            structure_bonus: scale(self.structure_bonus)?,
            facility_tax: scale(self.facility_tax)?,
            scc: scale(self.scc)?,
            total: scale(self.total)?,
        })
    }
}

/// The cost of all jobs of one step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepCost {
    pub cost: JobCost,
    /// Materials with no adjusted price, ascending by type id. Their cost is not in `cost`.
    pub missing_prices: Vec<TypeId>,
}

impl StepCost {
    fn none() -> StepCost {
        StepCost {
            cost: JobCost::zero(),
            missing_prices: Vec::new(),
        }
    }

    fn plus(self, other: StepCost) -> Result<StepCost, JobCostError> {
        let cost = self.cost.plus(&other.cost)?;
        let missing: BTreeSet<TypeId> = self
            .missing_prices
            .into_iter()
            .chain(other.missing_prices)
            .collect();
        Ok(StepCost {
            cost,
            missing_prices: missing.into_iter().collect(),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobCostError {
    /// A sum or product does not fit the supported range.
    Overflow,
    /// The structure multiplier is above 100 hundredths.
    BadMultiplier(u32),
}

impl fmt::Display for JobCostError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Overflow => write!(f, "the job cost does not fit the supported range"),
            Self::BadMultiplier(hundredths) => {
                write!(
                    f,
                    "structure cost multiplier {hundredths} hundredths is above 100"
                )
            }
        }
    }
}

impl std::error::Error for JobCostError {}

/// The cost of one job of `runs` runs. `materials` are base quantities per run.
///
/// The estimated item value sums the adjusted prices times the base quantities, and ignores
/// material efficiency and rigs. A material without an adjusted price adds nothing and is listed
/// in `missing_prices`. Zero runs is no job and costs nothing.
pub fn job_cost(
    materials: &[(TypeId, u64)],
    runs: u64,
    adjusted: &BTreeMap<TypeId, Money>,
    setup: &CostSetup,
) -> Result<StepCost, JobCostError> {
    if runs == 0 {
        return Ok(StepCost::none());
    }
    let mut exact: i128 = 0;
    let mut missing = BTreeSet::new();
    for &(type_id, base) in materials {
        let Some(price) = adjusted.get(&type_id) else {
            missing.insert(type_id);
            continue;
        };
        let units = i128::from(price.ten_thousandths())
            .checked_mul(i128::from(base))
            .and_then(|units| units.checked_mul(i128::from(runs)))
            .ok_or(JobCostError::Overflow)?;
        exact = exact.checked_add(units).ok_or(JobCostError::Overflow)?;
    }
    let item_value = round_isk(exact, 1)?;
    Ok(StepCost {
        cost: fees(item_value, setup)?,
        missing_prices: missing.into_iter().collect(),
    })
}

/// The cost of a step: `split.full_jobs` jobs of `runs_per_full_job` runs, then one job of
/// `last_job_runs` runs when that is not zero. Each job is rounded on its own, then they are summed.
pub fn step_cost(
    materials: &[(TypeId, u64)],
    split: &JobSplit,
    adjusted: &BTreeMap<TypeId, Money>,
    setup: &CostSetup,
) -> Result<StepCost, JobCostError> {
    let full = if split.full_jobs == 0 {
        StepCost::none()
    } else {
        let one = job_cost(materials, split.runs_per_full_job, adjusted, setup)?;
        StepCost {
            cost: one.cost.times(split.full_jobs)?,
            missing_prices: one.missing_prices,
        }
    };
    let last = job_cost(materials, split.last_job_runs, adjusted, setup)?;
    full.plus(last)
}

/// The system, bonus, tax and surcharge parts of a job whose estimated item value is `item_value`.
fn fees(item_value: Money, setup: &CostSetup) -> Result<JobCost, JobCostError> {
    let system_cost = share(item_value, i128::from(setup.cost_index.ppm()), FULL_RATE)?;
    let structure_bonus = match setup.structure_multiplier {
        None => Money::ZERO,
        Some(multiplier) => {
            let discount = FULL_MULTIPLIER
                .checked_sub(multiplier)
                .ok_or(JobCostError::BadMultiplier(multiplier))?;
            share(
                system_cost,
                i128::from(discount),
                i128::from(FULL_MULTIPLIER),
            )?
        }
    };
    let facility_tax = share(item_value, i128::from(setup.facility_tax.ppm()), FULL_RATE)?;
    let scc = share(item_value, i128::from(setup.scc.ppm()), FULL_RATE)?;
    let total = system_cost
        .checked_sub(structure_bonus)
        .and_then(|sum| sum.checked_add(facility_tax))
        .and_then(|sum| sum.checked_add(scc))
        .ok_or(JobCostError::Overflow)?;
    Ok(JobCost {
        item_value,
        system_cost,
        structure_bonus,
        facility_tax,
        scc,
        total,
    })
}

/// `isk` times `numerator / denominator`, rounded to whole ISK.
fn share(isk: Money, numerator: i128, denominator: i128) -> Result<Money, JobCostError> {
    let scaled = i128::from(isk.ten_thousandths())
        .checked_mul(numerator)
        .ok_or(JobCostError::Overflow)?;
    round_isk(scaled, denominator)
}

/// `numerator / denominator` in 0.0001 ISK, rounded to whole ISK, half away from zero.
fn round_isk(numerator: i128, denominator: i128) -> Result<Money, JobCostError> {
    let scale = denominator
        .checked_mul(i128::from(UNIT))
        .ok_or(JobCostError::Overflow)?;
    let whole = div_round(numerator, scale).ok_or(JobCostError::Overflow)?;
    let units = whole
        .checked_mul(i128::from(UNIT))
        .ok_or(JobCostError::Overflow)?;
    let units = i64::try_from(units).ok().ok_or(JobCostError::Overflow)?;
    Ok(Money::from_ten_thousandths(units))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{CostSetup, JobCost, JobCostError, SCC_SURCHARGE, job_cost, step_cost};
    use crate::industry::{JobSplit, TypeId};
    use crate::money::{Money, Rate};

    /// Hammerhead II per run: type and base quantity, from the game data fixture.
    const HAMMERHEAD: [(TypeId, u64); 6] = [
        (2183, 1),
        (11688, 1),
        (9834, 3),
        (9848, 3),
        (11481, 1),
        (11399, 3),
    ];

    fn isk(text: &str) -> Money {
        Money::parse_isk(text).expect("a valid amount")
    }

    fn prices(list: &[(TypeId, &str)]) -> BTreeMap<TypeId, Money> {
        list.iter().map(|&(id, text)| (id, isk(text))).collect()
    }

    fn hammerhead_prices() -> BTreeMap<TypeId, Money> {
        prices(&[
            (2183, "20964.1068"),
            (11688, "19672.0649"),
            (9834, "16388.6509"),
            (9848, "41616.5351"),
            (11481, "573.8664"),
            (11399, "17166.0204"),
        ])
    }

    fn setup(index_ppm: u32, multiplier: Option<u32>, tax_ppm: u32) -> CostSetup {
        CostSetup {
            cost_index: Rate::from_ppm(index_ppm),
            structure_multiplier: multiplier,
            facility_tax: Rate::from_ppm(tax_ppm),
            scc: SCC_SURCHARGE,
        }
    }

    fn one_job(
        materials: &[(TypeId, u64)],
        runs: u64,
        adjusted: &BTreeMap<TypeId, Money>,
        setup: &CostSetup,
    ) -> JobCost {
        job_cost(materials, runs, adjusted, setup)
            .expect("the job fits")
            .cost
    }

    #[test]
    fn jita_npc_station_job_matches_the_cost_api() {
        let cost = one_job(
            &HAMMERHEAD,
            10,
            &hammerhead_prices(),
            &setup(167_200, None, 0),
        );
        assert_eq!(
            cost,
            JobCost {
                item_value: isk("2667237"),
                system_cost: isk("445962"),
                structure_bonus: Money::ZERO,
                facility_tax: Money::ZERO,
                scc: isk("106689"),
                total: isk("552651"),
            }
        );
        assert_eq!(cost.per_unit(10), Some(isk("55265.1")));
        assert_eq!(cost.per_unit(0), None);
    }

    #[test]
    fn jita_raitaru_job_takes_the_structure_bonus_off_the_system_part() {
        let cost = one_job(
            &HAMMERHEAD,
            10,
            &hammerhead_prices(),
            &setup(167_200, Some(97), 10_000),
        );
        assert_eq!(
            cost,
            JobCost {
                item_value: isk("2667237"),
                system_cost: isk("445962"),
                structure_bonus: isk("13379"),
                facility_tax: isk("26672"),
                scc: isk("106689"),
                total: isk("565944"),
            }
        );
    }

    #[test]
    fn eve_guru_parameters_match_its_breakdown() {
        let cost = one_job(
            &HAMMERHEAD,
            10,
            &hammerhead_prices(),
            &setup(67_900, None, 2_500),
        );
        assert_eq!(cost.system_cost, isk("181105"));
        assert_eq!(cost.facility_tax, isk("6668"));
        assert_eq!(cost.scc, isk("106689"));
        assert_eq!(cost.total, isk("294462"));
    }

    #[test]
    fn hobgoblin_case_matches_both_tools() {
        let prices = prices(&[(1, "99709.2")]);
        let guru = one_job(&[(1, 1)], 10, &prices, &setup(67_900, None, 2_500));
        assert_eq!(
            guru,
            JobCost {
                item_value: isk("997092"),
                system_cost: isk("67703"),
                structure_bonus: Money::ZERO,
                facility_tax: isk("2493"),
                scc: isk("39884"),
                total: isk("110080"),
            }
        );
        let jita = one_job(&[(1, 1)], 10, &prices, &setup(167_200, None, 2_500));
        assert_eq!(jita.system_cost, isk("166714"));
        assert_eq!(jita.facility_tax, isk("2493"));
        assert_eq!(jita.scc, isk("39884"));
        assert_eq!(jita.total, isk("209091"));
    }

    #[test]
    fn parts_round_half_away_from_zero() {
        let half = prices(&[(1, "0.5")]);
        assert_eq!(
            one_job(&[(1, 1)], 1, &half, &setup(0, None, 0)).item_value,
            isk("1")
        );
        let just_under = prices(&[(1, "0.4999")]);
        assert_eq!(
            one_job(&[(1, 1)], 1, &just_under, &setup(0, None, 0)).item_value,
            Money::ZERO
        );
        let one = prices(&[(1, "1")]);
        assert_eq!(
            one_job(&[(1, 1)], 1, &one, &setup(500_000, None, 0)).system_cost,
            isk("1")
        );
    }

    #[test]
    fn split_jobs_are_rounded_one_by_one() {
        let prices = prices(&[(1, "1.5")]);
        let split = JobSplit {
            full_jobs: 1,
            runs_per_full_job: 30,
            last_job_runs: 1,
        };
        let step = step_cost(&[(1, 1)], &split, &prices, &setup(100_000, None, 0)).expect("fits");
        assert_eq!(step.cost.item_value, isk("47"));
        assert_eq!(step.cost.system_cost, isk("5"));
        assert_eq!(step.cost.scc, isk("2"));
        assert_eq!(step.cost.total, isk("7"));
        assert!(step.missing_prices.is_empty());

        let full = one_job(&[(1, 1)], 30, &prices, &setup(100_000, None, 0));
        assert_eq!(full.item_value, isk("45"));
        assert_eq!(full.total, isk("7"));
        let last = one_job(&[(1, 1)], 1, &prices, &setup(100_000, None, 0));
        assert_eq!(last.item_value, isk("2"));
        assert_eq!(last.system_cost, Money::ZERO);
        assert_eq!(last.scc, Money::ZERO);
        assert_eq!(last.total, Money::ZERO);

        let three = JobSplit {
            full_jobs: 3,
            runs_per_full_job: 30,
            last_job_runs: 0,
        };
        let step = step_cost(&[(1, 1)], &three, &prices, &setup(100_000, None, 0)).expect("fits");
        assert_eq!(step.cost.item_value, isk("135"));
        assert_eq!(step.cost.system_cost, isk("15"));
        assert_eq!(step.cost.scc, isk("6"));
        assert_eq!(step.cost.total, isk("21"));
    }

    #[test]
    fn zero_jobs_cost_nothing() {
        let split = JobSplit {
            full_jobs: 0,
            runs_per_full_job: 30,
            last_job_runs: 0,
        };
        let step = step_cost(
            &HAMMERHEAD,
            &split,
            &hammerhead_prices(),
            &setup(167_200, None, 0),
        )
        .expect("fits");
        assert_eq!(step.cost, JobCost::zero());
        assert!(step.missing_prices.is_empty());
        assert_eq!(
            one_job(
                &HAMMERHEAD,
                0,
                &hammerhead_prices(),
                &setup(167_200, None, 0)
            ),
            JobCost::zero()
        );
    }

    #[test]
    fn missing_prices_add_nothing_and_are_listed_in_order() {
        let mut partial = hammerhead_prices();
        partial.remove(&11399);
        let step = job_cost(&HAMMERHEAD, 10, &partial, &setup(167_200, None, 0)).expect("fits");
        assert_eq!(step.cost.item_value, isk("2152256"));
        assert_eq!(step.missing_prices, vec![11399]);

        partial.remove(&9834);
        let step = job_cost(&HAMMERHEAD, 10, &partial, &setup(167_200, None, 0)).expect("fits");
        assert_eq!(step.missing_prices, vec![9834, 11399]);
    }

    #[test]
    fn structure_multiplier_limits() {
        let full = one_job(
            &[(1, 1)],
            10,
            &prices(&[(1, "99709.2")]),
            &setup(167_200, Some(100), 0),
        );
        assert_eq!(full.structure_bonus, Money::ZERO);
        assert_eq!(
            job_cost(
                &[(1, 1)],
                10,
                &prices(&[(1, "1")]),
                &setup(167_200, Some(101), 0)
            ),
            Err(JobCostError::BadMultiplier(101))
        );
    }

    #[test]
    fn overflow_is_an_error() {
        let huge = BTreeMap::from([(1, Money::from_ten_thousandths(i64::MAX))]);
        assert_eq!(
            job_cost(&[(1, 2)], 1, &huge, &setup(0, None, 0)),
            Err(JobCostError::Overflow)
        );
        let split = JobSplit {
            full_jobs: u64::MAX,
            runs_per_full_job: 1,
            last_job_runs: 0,
        };
        assert_eq!(
            step_cost(
                &HAMMERHEAD,
                &split,
                &hammerhead_prices(),
                &setup(167_200, None, 0)
            ),
            Err(JobCostError::Overflow)
        );
    }

    #[test]
    fn errors_read_as_sentences() {
        assert_eq!(
            JobCostError::Overflow.to_string(),
            "the job cost does not fit the supported range"
        );
        assert_eq!(
            JobCostError::BadMultiplier(101).to_string(),
            "structure cost multiplier 101 hundredths is above 100"
        );
    }

    #[test]
    fn negative_price_is_used_as_given() {
        let negative = prices(&[(1, "-10")]);
        assert_eq!(
            one_job(&[(1, 1)], 1, &negative, &setup(0, None, 0)).item_value,
            isk("-10")
        );
    }
}

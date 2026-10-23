//! What invention costs a plan: the price of one attempt, the attempts to buy, and what the copy runs
//! the plan uses carry.
//!
//! Integers only. Every money amount is a `Money` (0.0001 ISK); products are computed in `i128`.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::float_arithmetic,
    clippy::as_conversions,
    clippy::integer_division
)]

use std::fmt;

use crate::decimal::div_round;
use crate::invention::{
    AttemptRule, Chance, InventedCopy, InventionError, attempts, copies_needed,
    expected_cost_per_copy,
};
use crate::money::Money;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InventionCostError {
    /// A material, decryptor or fee price is below zero.
    NegativeCost,
    /// The cost does not fit the supported range.
    Overflow,
    /// The invention numbers themselves are out of range.
    Invention(InventionError),
}

impl fmt::Display for InventionCostError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NegativeCost => write!(f, "a price or fee is negative"),
            Self::Overflow => write!(f, "the invention cost is too large"),
            Self::Invention(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for InventionCostError {}

/// What the plan needs for one invented copy and what it carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InventionNeed {
    /// Copies the plan uses: its T2 runs divided by the runs per copy, rounded up.
    pub copies: u64,
    /// Attempts to buy under the rule.
    pub attempts: u64,
    /// The expected cost of one copy: one attempt's cost divided by the chance.
    pub cost_per_copy: Money,
    /// The part of the copies' cost the plan uses: `cost_per_copy` times the runs used, over the runs per copy.
    pub carried: Money,
    /// Runs left on the last copy that the plan does not use.
    pub spare_runs: u64,
}

/// The cost of one attempt: the materials, the decryptor (if any) and the fee of a one-run invention job.
///
/// `materials` are `(quantity, unit price)` pairs.
pub fn attempt_cost(
    materials: &[(u64, Money)],
    decryptor: Option<Money>,
    fee: Money,
) -> Result<Money, InventionCostError> {
    let mut total: i128 = 0;
    for &(quantity, price) in materials {
        total = plus(total, priced(quantity, price)?)?;
    }
    for price in decryptor.into_iter().chain(std::iter::once(fee)) {
        total = plus(total, priced(1, price)?)?;
    }
    let units = i64::try_from(total)
        .ok()
        .ok_or(InventionCostError::Overflow)?;
    Ok(Money::from_ten_thousandths(units))
}

/// The cost of `quantity` units at `price`, in 0.0001 ISK.
fn priced(quantity: u64, price: Money) -> Result<i128, InventionCostError> {
    if price.ten_thousandths() < 0 {
        return Err(InventionCostError::NegativeCost);
    }
    i128::from(quantity)
        .checked_mul(i128::from(price.ten_thousandths()))
        .ok_or(InventionCostError::Overflow)
}

fn plus(total: i128, part: i128) -> Result<i128, InventionCostError> {
    total.checked_add(part).ok_or(InventionCostError::Overflow)
}

/// What a plan needs to make `t2_runs` runs of a T2 product from copies that give `copy`.
///
/// The cost per copy is the expected cost, and the carried cost is the part of it the plan's runs
/// use. Zero T2 runs needs nothing.
pub fn invention_need(
    t2_runs: u64,
    copy: InventedCopy,
    chance: Chance,
    rule: AttemptRule,
    attempt_cost: Money,
) -> Result<InventionNeed, InventionCostError> {
    let copies = copies_needed(t2_runs, copy.runs).map_err(InventionCostError::Invention)?;
    let needed_attempts = attempts(copies, chance, rule).map_err(InventionCostError::Invention)?;
    if t2_runs == 0 {
        return Ok(InventionNeed {
            copies: 0,
            attempts: 0,
            cost_per_copy: Money::ZERO,
            carried: Money::ZERO,
            spare_runs: 0,
        });
    }
    let cost_per_copy =
        expected_cost_per_copy(attempt_cost, 1, chance).map_err(InventionCostError::Invention)?;
    let carried = carried_cost(cost_per_copy, t2_runs, copy.runs)?;
    let made = copies
        .checked_mul(u64::from(copy.runs))
        .ok_or(InventionCostError::Overflow)?;
    let spare_runs = made
        .checked_sub(t2_runs)
        .ok_or(InventionCostError::Overflow)?;
    Ok(InventionNeed {
        copies,
        attempts: needed_attempts,
        cost_per_copy,
        carried,
        spare_runs,
    })
}

/// `cost_per_copy` times `t2_runs` over `runs_per_copy`, rounded to 0.0001 ISK.
fn carried_cost(
    cost_per_copy: Money,
    t2_runs: u64,
    runs_per_copy: u32,
) -> Result<Money, InventionCostError> {
    let numerator = i128::from(cost_per_copy.ten_thousandths())
        .checked_mul(i128::from(t2_runs))
        .ok_or(InventionCostError::Overflow)?;
    // copies_needed has already rejected zero runs per copy.
    let whole =
        div_round(numerator, i128::from(runs_per_copy)).ok_or(InventionCostError::Overflow)?;
    let units = i64::try_from(whole)
        .ok()
        .ok_or(InventionCostError::Overflow)?;
    Ok(Money::from_ten_thousandths(units))
}

#[cfg(test)]
mod tests {
    use super::{InventionCostError, InventionNeed, attempt_cost, invention_need};
    use crate::invention::{
        AttemptRule, Chance, Decryptor, InventedCopy, InventionError, Skills, chance,
    };
    use crate::money::{Money, Rate};

    const ATTAINMENT: Decryptor = Decryptor {
        type_id: 34202,
        chance_hundredths: 180,
        me: -1,
        te: 4,
        runs: 4,
    };

    fn isk(text: &str) -> Money {
        Money::parse_isk(text).expect("valid ISK amount")
    }

    fn skills(science1: u8, science2: u8, encryption: u8) -> Skills {
        Skills {
            science1,
            science2,
            encryption,
        }
    }

    /// Chance a: base 340,000 ppm, skills 4/4/4, no decryptor (46.47%).
    fn a() -> Chance {
        chance(Rate::from_ppm(340_000), skills(4, 4, 4), None).expect("valid chance")
    }

    /// Chance z: base 340,000 ppm, skills 0/0/0 (34%).
    fn z() -> Chance {
        chance(Rate::from_ppm(340_000), skills(0, 0, 0), None).expect("valid chance")
    }

    /// Chance t: base 340,000 ppm, skills 4/4/4, Attainment (83.64%).
    fn t() -> Chance {
        chance(Rate::from_ppm(340_000), skills(4, 4, 4), Some(&ATTAINMENT)).expect("valid chance")
    }

    /// A copy of 10 runs, ME 2, TE 4: a base copy with no decryptor.
    const COPY_10: InventedCopy = InventedCopy {
        runs: 10,
        me: 2,
        te: 4,
    };

    /// A copy of 14 runs, ME 1, TE 8: the Attainment copy.
    const COPY_14: InventedCopy = InventedCopy {
        runs: 14,
        me: 1,
        te: 8,
    };

    #[test]
    fn attempt_cost_sums_materials_decryptor_and_fee() {
        let materials = [(1, isk("50,000.00")), (1, isk("43,750.00"))];
        assert_eq!(
            attempt_cost(&materials, None, isk("89.00")),
            Ok(isk("93,839.00"))
        );
        assert_eq!(
            attempt_cost(&materials, Some(isk("1,200,000.00")), isk("89.00")),
            Ok(isk("1,293,839.00"))
        );
        assert_eq!(
            attempt_cost(&[(3, isk("0.0001"))], None, Money::ZERO),
            Ok(isk("0.0003"))
        );
    }

    #[test]
    fn attempt_cost_errors() {
        assert_eq!(
            attempt_cost(&[(1, isk("-0.0001"))], None, Money::ZERO),
            Err(InventionCostError::NegativeCost)
        );
        assert_eq!(
            attempt_cost(
                &[(u64::MAX, Money::from_ten_thousandths(i64::MAX))],
                None,
                Money::ZERO
            ),
            Err(InventionCostError::Overflow)
        );
        assert_eq!(
            InventionCostError::NegativeCost.to_string(),
            "a price or fee is negative"
        );
    }

    #[test]
    fn expected_plan_for_a_part_copy_carries_the_runs_used() {
        let need = invention_need(8, COPY_10, a(), AttemptRule::Expected, isk("93,839.00"))
            .expect("valid need");
        assert_eq!(need.copies, 1);
        assert_eq!(need.attempts, 3);
        assert_eq!(need.cost_per_copy, isk("201,949.0674"));
        assert_eq!(need.carried, isk("161,559.2539"));
        assert_eq!(need.spare_runs, 2);
    }

    #[test]
    fn expected_plan_for_a_whole_copy() {
        let need = invention_need(10, COPY_10, a(), AttemptRule::Expected, isk("93,839.00"))
            .expect("valid need");
        assert_eq!(need.copies, 1);
        assert_eq!(need.attempts, 3);
        assert_eq!(need.cost_per_copy, isk("201,949.0674"));
        assert_eq!(need.carried, isk("201,949.0674"));
        assert_eq!(need.spare_runs, 0);
    }

    #[test]
    fn confidence_sets_the_attempts_and_leaves_the_cost_alone() {
        let at = |ppm| AttemptRule::Confidence(Rate::from_ppm(ppm));
        let cost = isk("93,839.00");
        let expected =
            invention_need(100, COPY_10, a(), AttemptRule::Expected, cost).expect("need");
        assert_eq!(expected.copies, 10);
        assert_eq!(expected.attempts, 22);
        assert_eq!(expected.cost_per_copy, isk("201,949.0674"));
        assert_eq!(expected.carried, isk("2,019,490.6740"));
        assert_eq!(expected.spare_runs, 0);
        for (level, attempts) in [(900_000, 28), (950_000, 31), (990_000, 35)] {
            let need = invention_need(100, COPY_10, a(), at(level), cost).expect("need");
            assert_eq!(need.attempts, attempts);
            assert_eq!(need.cost_per_copy, expected.cost_per_copy);
            assert_eq!(need.carried, expected.carried);
        }
    }

    #[test]
    fn low_chance_plan() {
        let need = invention_need(100, COPY_10, z(), AttemptRule::Expected, isk("93,839.00"))
            .expect("valid need");
        assert_eq!(need.copies, 10);
        assert_eq!(need.attempts, 30);
        assert_eq!(need.cost_per_copy, isk("275,997.0588"));
        assert_eq!(need.carried, isk("2,759,970.5880"));
    }

    #[test]
    fn zero_runs_need_nothing() {
        let need = invention_need(0, COPY_10, a(), AttemptRule::Expected, isk("93,839.00"))
            .expect("valid need");
        assert_eq!(
            need,
            InventionNeed {
                copies: 0,
                attempts: 0,
                cost_per_copy: Money::ZERO,
                carried: Money::ZERO,
                spare_runs: 0,
            }
        );
    }

    #[test]
    fn attainment_copy_plan() {
        let need = invention_need(20, COPY_14, t(), AttemptRule::Expected, isk("1,293,839.00"))
            .expect("valid need");
        assert_eq!(need.copies, 2);
        assert_eq!(need.attempts, 3);
        assert_eq!(need.cost_per_copy, isk("1,546,914.1559"));
        assert_eq!(need.carried, isk("2,209,877.3656"));
        assert_eq!(need.spare_runs, 8);
    }

    #[test]
    fn invention_errors_come_through() {
        let cost = isk("93,839.00");
        let copy_of_zero = InventedCopy {
            runs: 0,
            me: 2,
            te: 4,
        };
        assert_eq!(
            invention_need(8, copy_of_zero, a(), AttemptRule::Expected, cost),
            Err(InventionCostError::Invention(InventionError::BadCopy))
        );
        assert_eq!(
            invention_need(
                10,
                COPY_10,
                a(),
                AttemptRule::Confidence(Rate::from_ppm(0)),
                cost
            ),
            Err(InventionCostError::Invention(InventionError::BadConfidence))
        );
        assert_eq!(
            invention_need(10, COPY_10, a(), AttemptRule::Expected, isk("-1.00")),
            Err(InventionCostError::Invention(InventionError::NegativeCost))
        );
        assert_eq!(
            invention_need(
                10,
                COPY_10,
                a(),
                AttemptRule::Expected,
                Money::from_ten_thousandths(i64::MAX)
            ),
            Err(InventionCostError::Invention(InventionError::Overflow))
        );
    }

    #[test]
    fn carried_cost_that_does_not_fit_is_overflow() {
        let certain =
            chance(Rate::from_ppm(1_000_000), skills(0, 0, 0), None).expect("valid chance");
        let one_run = InventedCopy {
            runs: 1,
            me: 2,
            te: 4,
        };
        let huge = Money::from_ten_thousandths(4_000_000_000_000_000_000);
        assert_eq!(
            invention_need(3, one_run, certain, AttemptRule::Expected, huge),
            Err(InventionCostError::Overflow)
        );
    }
}

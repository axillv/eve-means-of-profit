//! The invention numbers of one T2 part: the chance of an attempt, the copy it makes, the attempts
//! the plan buys, the fee of an attempt, and what a copy costs and carries. Nothing here draws.

use std::collections::BTreeMap;

use mop_core::industry::TypeId;
use mop_core::invention::{
    AttemptRule, Chance, Decryptor, InventedCopy, Skills, chance, invented_copy,
};
use mop_core::invention_cost::{InventionNeed, attempt_cost, invention_need};
use mop_core::job_cost::{CostSetup, invention_job_cost};
use mop_core::money::{Money, Rate};
use mop_sde::invention::{InventionActivity, InventionProduct, split_skills};

/// What one attempt's invention numbers work out to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Numbers {
    pub chance: Chance,
    pub copy: InventedCopy,
    pub need: InventionNeed,
    pub attempt_cost: Money,
}

/// What the invention numbers of one part are worked out from.
///
/// `datacores` are `(quantity, unit price)` pairs per attempt; `decryptor_price` is the price of the
/// decryptor when one is used; `fee` is the invention job's fee for one attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InventionInput<'a> {
    pub product: &'a InventionProduct,
    pub t2_runs: u64,
    pub skills: Skills,
    pub decryptor: Option<&'a Decryptor>,
    pub rule: AttemptRule,
    pub datacores: &'a [(u64, Money)],
    pub decryptor_price: Option<Money>,
    pub fee: Money,
}

/// The invention of one T2 part, for the Steps table and the buy list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventionLine {
    pub type_id: TypeId,
    pub need: InventionNeed,
    pub attempt_cost: Money,
    /// The datacores an attempt uses: type id and quantity per attempt.
    pub materials: Vec<(TypeId, u64)>,
    /// The decryptor an attempt uses, if any.
    pub decryptor: Option<TypeId>,
}

/// The levels of an activity's two science skills and its encryption skill, from the saved levels.
///
/// A skill with no saved level counts as 0. `None` when the activity's skills cannot be split into
/// two sciences and an encryption skill.
pub fn skills_for(activity: &InventionActivity, saved: &BTreeMap<TypeId, u8>) -> Option<Skills> {
    let (science1, science2, encryption) = split_skills(activity)?;
    let level = |type_id| saved.get(&type_id).copied().unwrap_or(0);
    Some(Skills {
        science1: level(science1),
        science2: level(science2),
        encryption: level(encryption),
    })
}

/// The fee of one invention attempt: the fee of a one-run invention job at the invention index.
///
/// The job runs in the manufacturing facility, so only its cost index is replaced.
pub fn fee_per_attempt(
    t2_item_value: Money,
    manufacturing: &CostSetup,
    invention_index: Rate,
) -> Result<Money, String> {
    let setup = CostSetup {
        cost_index: invention_index,
        ..manufacturing.clone()
    };
    invention_job_cost(t2_item_value, 1, &setup)
        .map(|cost| cost.total)
        .map_err(|error| error.to_string())
}

/// The invention numbers of one attempt at the input's product, and what its T2 runs need.
pub fn invention_numbers(input: &InventionInput<'_>) -> Result<Numbers, String> {
    let chance = chance(input.product.probability, input.skills, input.decryptor)
        .map_err(|error| error.to_string())?;
    let copy = invented_copy(input.product.quantity, input.decryptor)
        .map_err(|error| error.to_string())?;
    let one_attempt = attempt_cost(input.datacores, input.decryptor_price, input.fee)
        .map_err(|error| error.to_string())?;
    let need = invention_need(input.t2_runs, copy, chance, input.rule, one_attempt)
        .map_err(|error| error.to_string())?;
    Ok(Numbers {
        chance,
        copy,
        need,
        attempt_cost: one_attempt,
    })
}

/// The line the Steps table and the buy list show for one part's invention numbers.
///
/// `fee` is the invention job's fee per attempt, shown on its own so the player can see what the
/// fee adds to the materials. The money amounts use the app's ISK format.
pub fn line_text(numbers: &Numbers, fee: Money) -> String {
    let copies = if numbers.need.copies == 1 {
        "copy"
    } else {
        "copies"
    };
    let mut text = format!(
        "{}% · {} {copies} of {} runs · {} attempts · fee {} per attempt · {} per copy · carried {}",
        numbers.chance.percent_string(),
        numbers.need.copies,
        numbers.copy.runs,
        numbers.need.attempts,
        fee.format_isk(),
        numbers.need.cost_per_copy.format_isk(),
        numbers.need.carried.format_isk(),
    );
    if numbers.need.spare_runs > 0 {
        let runs = if numbers.need.spare_runs == 1 {
            "run"
        } else {
            "runs"
        };
        text.push_str(&format!(
            " · {} {runs} left on the copy",
            numbers.need.spare_runs
        ));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use mop_core::job_cost::SCC_SURCHARGE;
    use mop_sde::invention::SkillRequirement;

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

    /// The invention product line of Hobgoblin II.
    fn hobgoblin() -> InventionProduct {
        InventionProduct {
            type_id: 2456,
            probability: Rate::from_ppm(340_000),
            quantity: 10,
        }
    }

    /// Hobgoblin II's invention activity, with the two sciences before the encryption skill.
    fn activity() -> InventionActivity {
        InventionActivity {
            blueprint: 0,
            products: vec![hobgoblin()],
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

    fn datacores() -> [(u64, Money); 2] {
        [(1, isk("50,000.00")), (1, isk("43,750.00"))]
    }

    fn manufacturing() -> CostSetup {
        CostSetup {
            cost_index: Rate::from_ppm(167_200),
            structure_multiplier: None,
            facility_tax: Rate::from_ppm(2_500),
            scc: SCC_SURCHARGE,
        }
    }

    /// The acceptance's inputs with the fee at 89.00; a test overrides what it changes.
    fn base<'a>(
        product: &'a InventionProduct,
        decryptor: Option<&'a Decryptor>,
        datacores: &'a [(u64, Money)],
    ) -> InventionInput<'a> {
        InventionInput {
            product,
            t2_runs: 8,
            skills: SKILLS_444,
            decryptor,
            rule: AttemptRule::Expected,
            datacores,
            decryptor_price: None,
            fee: isk("89.00"),
        }
    }

    #[test]
    fn the_skills_come_from_the_saved_levels() {
        let saved = BTreeMap::from([(11453, 4), (11446, 3), (23121, 2)]);
        assert_eq!(
            skills_for(&activity(), &saved),
            Some(Skills {
                science1: 4,
                science2: 3,
                encryption: 2,
            })
        );
        assert_eq!(
            skills_for(&activity(), &BTreeMap::new()),
            Some(Skills {
                science1: 0,
                science2: 0,
                encryption: 0,
            })
        );
    }

    #[test]
    fn unsplittable_skills_have_no_levels() {
        let mut unsplittable = activity();
        unsplittable.skills.pop();
        assert_eq!(skills_for(&unsplittable, &BTreeMap::new()), None);
    }

    #[test]
    fn the_fee_is_the_one_run_job_at_the_invention_index() {
        assert_eq!(
            fee_per_attempt(isk("101,786.00"), &manufacturing(), Rate::from_ppm(1_400)),
            Ok(isk("89.00"))
        );
        assert_eq!(
            fee_per_attempt(
                Money::from_ten_thousandths(-1),
                &manufacturing(),
                Rate::from_ppm(1_400)
            ),
            Err("the item value is negative".to_owned())
        );
    }

    #[test]
    fn a_part_copy_line_matches_the_expected_numbers() {
        let fee = isk("89.00");
        let product = hobgoblin();
        let cores = datacores();
        let numbers = invention_numbers(&base(&product, None, &cores)).expect("the numbers fit");
        assert_eq!(numbers.chance.ppm(), 464_667);
        assert_eq!(numbers.chance.percent_string(), "46.47");
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
        assert_eq!(
            line_text(&numbers, fee),
            "46.47% · 1 copy of 10 runs · 3 attempts · fee 89.00 per attempt · 201,949.07 per copy · carried 161,559.25 · 2 runs left on the copy"
        );
    }

    #[test]
    fn the_attainment_decryptor_line_matches_its_numbers() {
        let fee = isk("89.00");
        let product = hobgoblin();
        let cores = datacores();
        let numbers = invention_numbers(&InventionInput {
            decryptor_price: Some(isk("1,200,000.00")),
            ..base(&product, Some(&ATTAINMENT), &cores)
        })
        .expect("the numbers fit");
        assert_eq!(numbers.chance.percent_string(), "83.64");
        assert_eq!(
            numbers.copy,
            InventedCopy {
                runs: 14,
                me: 1,
                te: 8,
            }
        );
        assert_eq!(numbers.copy.me, 1);
        assert_eq!(numbers.need.copies, 1);
        assert_eq!(numbers.need.attempts, 2);
        assert_eq!(numbers.attempt_cost.format_isk(), "1,293,839.00");
        assert_eq!(numbers.need.cost_per_copy.format_isk(), "1,546,914.16");
        assert_eq!(numbers.need.carried.format_isk(), "883,950.95");
        assert_eq!(numbers.need.spare_runs, 6);
        assert_eq!(
            line_text(&numbers, fee),
            "83.64% · 1 copy of 14 runs · 2 attempts · fee 89.00 per attempt · 1,546,914.16 per copy · carried 883,950.95 · 6 runs left on the copy"
        );
    }

    #[test]
    fn no_saved_skills_count_as_zero() {
        let fee = isk("89.00");
        let product = hobgoblin();
        let cores = datacores();
        let skills = skills_for(&activity(), &BTreeMap::new()).expect("three skills");
        let numbers = invention_numbers(&InventionInput {
            skills,
            fee,
            ..base(&product, None, &cores)
        })
        .expect("the numbers fit");
        assert_eq!(numbers.chance.percent_string(), "34.00");
        assert_eq!(numbers.need.attempts, 3);
        assert_eq!(numbers.need.cost_per_copy.format_isk(), "275,997.06");
        assert_eq!(numbers.need.carried.format_isk(), "220,797.65");
    }

    #[test]
    fn confidence_counts_over_the_whole_batch() {
        let fee = isk("89.00");
        let product = hobgoblin();
        let cores = datacores();
        let numbers = invention_numbers(&InventionInput {
            t2_runs: 100,
            rule: AttemptRule::Confidence(Rate::from_ppm(900_000)),
            ..base(&product, None, &cores)
        })
        .expect("the numbers fit");
        assert_eq!(numbers.need.copies, 10);
        assert_eq!(numbers.need.attempts, 28);
        assert_eq!(numbers.need.carried.format_isk(), "2,019,490.67");
        assert_eq!(
            line_text(&numbers, fee),
            "46.47% · 10 copies of 10 runs · 28 attempts · fee 89.00 per attempt · 201,949.07 per copy · carried 2,019,490.67"
        );
    }

    #[test]
    fn a_free_fee_leaves_only_the_materials() {
        let product = hobgoblin();
        let cores = datacores();
        let numbers = invention_numbers(&InventionInput {
            fee: Money::ZERO,
            ..base(&product, None, &cores)
        })
        .expect("the numbers fit");
        assert_eq!(numbers.attempt_cost.format_isk(), "93,750.00");
        assert_eq!(numbers.need.cost_per_copy.format_isk(), "201,757.53");
    }

    #[test]
    fn one_spare_run_reads_as_a_single_run() {
        let fee = isk("89.00");
        let product = hobgoblin();
        let cores = datacores();
        let numbers = invention_numbers(&InventionInput {
            t2_runs: 9,
            ..base(&product, None, &cores)
        })
        .expect("the numbers fit");
        assert_eq!(numbers.need.spare_runs, 1);
        assert!(
            line_text(&numbers, fee).ends_with(" · 1 run left on the copy"),
            "one spare run is singular"
        );
    }

    #[test]
    fn errors_are_display_texts() {
        let zero_chance = InventionProduct {
            probability: Rate::from_ppm(0),
            ..hobgoblin()
        };
        let cores = datacores();
        assert_eq!(
            invention_numbers(&InventionInput {
                fee: Money::ZERO,
                ..base(&zero_chance, None, &cores)
            }),
            Err("the invention chance is zero".to_owned())
        );
        let negative = [(1, isk("-1.00"))];
        assert_eq!(
            invention_numbers(&InventionInput {
                fee: Money::ZERO,
                ..base(&hobgoblin(), None, &negative)
            }),
            Err("a price or fee is negative".to_owned())
        );
    }
}

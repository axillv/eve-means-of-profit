//! Invention: the chance of a T2 copy from skills and decryptor, what the copy gives, and how many attempts to buy.
//!
//! Chances are exact fractions, so the expected cost per copy is rounded once, at the end.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::float_arithmetic,
    clippy::as_conversions,
    clippy::integer_division
)]

use std::fmt;

use crate::decimal::div_round;
use crate::industry::TypeId;
use crate::money::{Money, Rate};

/// The highest level of an invention skill.
const MAX_SKILL: u8 = 5;

/// The most attempts the app will plan for one buy list.
const MAX_ATTEMPTS: u64 = 100_000;

/// Scale of the fixed-point probabilities in the binomial rule: 18 decimals.
const FP: u128 = 1_000_000_000_000_000_000;

/// 100% in parts per million.
const FULL_PPM: u128 = 1_000_000;

/// One part per million, in fixed-point units.
const FP_PER_PPM: u128 = 1_000_000_000_000;

/// Denominator of a chance: 1,000,000 ppm × 1200 (30 × 40, the skill divisors) × 100 (decryptor hundredths).
const CHANCE_DEN: u128 = 120_000_000_000;

/// Base ME and TE of a blueprint copy, before the decryptor.
const BASE_ME: i64 = 2;
const BASE_TE: i64 = 4;

/// The levels of the three invention skills, each 0..=5.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Skills {
    pub science1: u8,
    pub science2: u8,
    pub encryption: u8,
}

/// A decryptor and what it changes. The chance multiplier is in hundredths (1.8 is 180).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Decryptor {
    pub type_id: TypeId,
    pub chance_hundredths: u32,
    pub me: i8,
    pub te: i8,
    pub runs: i8,
}

/// The chance of one attempt, as an exact fraction, capped at 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chance {
    num: u128,
    den: u128,
}

impl Chance {
    /// The chance in parts per million, rounded half up.
    pub fn ppm(self) -> u32 {
        // num <= den (capped in `chance`), so the result is at most 1,000,000 and the fallback never applies.
        // Cannot saturate: num < den ≤ 1.2e11 (or 1 ÷ 1), so num × 1,000,000 stays below 1.2e17.
        u32::try_from(round_div(self.num.saturating_mul(FULL_PPM), self.den)).unwrap_or(1_000_000)
    }

    /// The chance as a percentage with two decimals, rounded half up: `"46.47"`.
    pub fn percent_string(self) -> String {
        // Hundredths of a percent: the value times 10,000.
        // Cannot saturate: num < den ≤ 1.2e11 (or 1 ÷ 1), so num × 10,000 stays below 1.2e15.
        let hundredths = round_div(self.num.saturating_mul(10_000), self.den);
        let whole = hundredths.div_euclid(100);
        let frac = hundredths.rem_euclid(100);
        format!("{whole}.{frac:02}")
    }

    pub fn is_certain(self) -> bool {
        self.num == self.den
    }
}

/// What an invented copy gives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InventedCopy {
    pub runs: u32,
    pub me: u8,
    pub te: u8,
}

/// How many attempts to buy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttemptRule {
    /// Enough attempts for the expected number of copies.
    Expected,
    /// Enough attempts to get all the copies with at least this chance (1 to 999,999 ppm).
    Confidence(Rate),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InventionError {
    SkillLevel(u8),
    ZeroChance,
    BadCopy,
    BadConfidence,
    TooManyAttempts,
    NoAttempts,
    NegativeCost,
    Overflow,
}

impl fmt::Display for InventionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InventionError::SkillLevel(level) => {
                write!(f, "skill level {level} is above {MAX_SKILL}")
            }
            InventionError::ZeroChance => write!(f, "the invention chance is zero"),
            InventionError::BadCopy => write!(f, "the invented copy is out of range"),
            InventionError::BadConfidence => {
                write!(f, "the confidence must be above 0% and below 100%")
            }
            InventionError::TooManyAttempts => {
                write!(f, "more than {MAX_ATTEMPTS} attempts are needed")
            }
            InventionError::NoAttempts => write!(f, "no attempts were bought"),
            InventionError::NegativeCost => write!(f, "the cost is negative"),
            InventionError::Overflow => write!(f, "the invention numbers are too large"),
        }
    }
}

impl std::error::Error for InventionError {}

/// The chance of one attempt: the SDE base, times skills, times the decryptor multiplier.
///
/// `base` is the SDE probability in ppm (0.34 is 340,000).
pub fn chance(
    base: Rate,
    skills: Skills,
    decryptor: Option<&Decryptor>,
) -> Result<Chance, InventionError> {
    for level in [skills.science1, skills.science2, skills.encryption] {
        if level > MAX_SKILL {
            return Err(InventionError::SkillLevel(level));
        }
    }
    if base.ppm() == 0 {
        return Err(InventionError::ZeroChance);
    }

    // 1 + (science1 + science2) / 30 + encryption / 40, written over 1200 (= 30 × 40).
    let science = ok(u128::from(skills.science1).checked_add(u128::from(skills.science2)))?;
    let factor = ok(1200u128
        .checked_add(ok(science.checked_mul(40))?)
        .and_then(|sum| sum.checked_add(u128::from(skills.encryption).checked_mul(30)?)))?;
    let multiplier = decryptor.map_or(100, |d| u128::from(d.chance_hundredths));

    let num = ok(u128::from(base.ppm())
        .checked_mul(factor)
        .and_then(|n| n.checked_mul(multiplier)))?;
    if num == 0 {
        // A decryptor multiplier of zero.
        return Err(InventionError::ZeroChance);
    }
    if num >= CHANCE_DEN {
        return Ok(Chance { num: 1, den: 1 });
    }
    Ok(Chance {
        num,
        den: CHANCE_DEN,
    })
}

/// The ME, TE and runs of an invented copy: the base copy plus the decryptor.
pub fn invented_copy(
    base_runs: u32,
    decryptor: Option<&Decryptor>,
) -> Result<InventedCopy, InventionError> {
    if base_runs == 0 {
        return Err(InventionError::BadCopy);
    }
    let (runs_delta, me_delta, te_delta) = decryptor.map_or((0, 0, 0), |d| (d.runs, d.me, d.te));

    let runs = ok(i64::from(base_runs).checked_add(i64::from(runs_delta)))?;
    let me = ok(BASE_ME.checked_add(i64::from(me_delta)))?;
    let te = ok(BASE_TE.checked_add(i64::from(te_delta)))?;

    Ok(InventedCopy {
        runs: u32::try_from(runs)
            .ok()
            .filter(|runs| *runs >= 1)
            .ok_or(InventionError::BadCopy)?,
        me: u8::try_from(me)
            .ok()
            .filter(|me| *me <= 10)
            .ok_or(InventionError::BadCopy)?,
        te: u8::try_from(te)
            .ok()
            .filter(|te| *te <= 20)
            .ok_or(InventionError::BadCopy)?,
    })
}

/// The copies a plan needs: `t2_runs` divided by the runs per copy, rounded up.
pub fn copies_needed(t2_runs: u64, runs_per_copy: u32) -> Result<u64, InventionError> {
    let per_copy = u64::from(runs_per_copy);
    if per_copy == 0 {
        return Err(InventionError::BadCopy);
    }
    let whole = t2_runs
        .checked_div(per_copy)
        .ok_or(InventionError::BadCopy)?;
    let partial = u64::from(
        t2_runs
            .checked_rem(per_copy)
            .ok_or(InventionError::BadCopy)?
            != 0,
    );
    ok(whole.checked_add(partial))
}

/// The attempts to buy for `copies` copies, under `rule`.
pub fn attempts(copies: u64, chance: Chance, rule: AttemptRule) -> Result<u64, InventionError> {
    let confidence = match rule {
        AttemptRule::Expected => None,
        AttemptRule::Confidence(level) => {
            if !(1..FULL_PPM).contains(&u128::from(level.ppm())) {
                return Err(InventionError::BadConfidence);
            }
            Some(level.ppm())
        }
    };
    if copies == 0 {
        return Ok(0);
    }
    if chance.is_certain() {
        return Ok(copies);
    }
    match confidence {
        None => expected_attempts(copies, chance),
        Some(ppm) => attempts_for_confidence(copies, chance, ppm),
    }
}

/// The cost per copy: the cost of the attempts, divided by the expected copies (attempts × chance).
pub fn expected_cost_per_copy(
    cost: Money,
    attempts: u64,
    chance: Chance,
) -> Result<Money, InventionError> {
    if attempts == 0 {
        return Err(InventionError::NoAttempts);
    }
    let cost = cost.ten_thousandths();
    if cost < 0 {
        return Err(InventionError::NegativeCost);
    }
    // cost ÷ (attempts × num / den), with cost in 0.0001 ISK.
    // den and num are at most 1.2e11 and attempts at most u64::MAX, so these products fit in i128.
    let den: i128 = narrow(chance.den)?;
    let num: i128 = narrow(chance.num)?;
    let numerator = ok(i128::from(cost).checked_mul(den))?;
    let denominator = ok(i128::from(attempts).checked_mul(num))?;
    let per_copy = ok(div_round(numerator, denominator))?;
    Ok(Money::from_ten_thousandths(narrow(per_copy)?))
}

/// ceil(copies ÷ chance), exact.
fn expected_attempts(copies: u64, chance: Chance) -> Result<u64, InventionError> {
    let needed = ok(u128::from(copies).checked_mul(chance.den))?;
    let whole = ok(needed.checked_div(chance.num))?;
    let partial = u128::from(ok(needed.checked_rem(chance.num))? != 0);
    let total = ok(whole.checked_add(partial))?;
    narrow(total)
}

/// The smallest n ≥ `copies` with P(at least `copies` successes in n attempts) ≥ the confidence.
///
/// Keeps the probability of exactly j successes for j < copies and of at least `copies`, one attempt at a time.
fn attempts_for_confidence(
    copies: u64,
    chance: Chance,
    confidence_ppm: u32,
) -> Result<u64, InventionError> {
    if copies > MAX_ATTEMPTS {
        return Err(InventionError::TooManyAttempts);
    }
    let successes: usize = usize::try_from(copies)
        .ok()
        .ok_or(InventionError::TooManyAttempts)?;
    let p = ok(ok(chance.num.checked_mul(FP))?.checked_div(chance.den))?;
    // p is at most FP because the chance is capped at 1.
    let q = ok(FP.checked_sub(p))?;
    let target = ok(u128::from(confidence_ppm).checked_mul(FP_PER_PPM))?;

    // Markov bound: P(at least k) ≤ n·p ÷ k, and n ≤ MAX_ATTEMPTS. If that bound is below the target, no n works.
    if ok(u128::from(MAX_ATTEMPTS).checked_mul(p))? < ok(u128::from(copies).checked_mul(target))? {
        return Err(InventionError::TooManyAttempts);
    }

    // below[j] = probability of exactly j successes so far, for j < copies.
    let mut below = vec![0u128; successes];
    if let Some(first) = below.first_mut() {
        *first = FP;
    }
    let mut at_least = 0u128;
    for n in 1..=MAX_ATTEMPTS {
        let top = below.last().copied().unwrap_or(0);
        at_least = ok(at_least.checked_add(fp_mul(top, p)?))?;
        // Walk up so each slot still holds its old value when the slot above it reads it.
        let mut prev_old = 0u128;
        for slot in below.iter_mut() {
            let current_old = *slot;
            *slot = ok(fp_mul(current_old, q)?.checked_add(fp_mul(prev_old, p)?))?;
            prev_old = current_old;
        }
        if at_least >= target {
            return Ok(n);
        }
    }
    Err(InventionError::TooManyAttempts)
}

/// `a × b` in fixed point, rounded down.
fn fp_mul(a: u128, b: u128) -> Result<u128, InventionError> {
    ok(a.checked_mul(b).and_then(|product| product.checked_div(FP)))
}

fn ok<T>(value: Option<T>) -> Result<T, InventionError> {
    value.ok_or(InventionError::Overflow)
}

/// Converts between integer types; a value that does not fit is `Overflow`.
fn narrow<T: TryFrom<U>, U>(value: U) -> Result<T, InventionError> {
    T::try_from(value).ok().ok_or(InventionError::Overflow)
}

/// num ÷ den rounded half up. `den` is never zero: a `Chance` always has den ≥ 1.
fn round_div(num: u128, den: u128) -> u128 {
    // Cannot saturate for the callers: num is at most 1.2e17 and den at most 1.2e11, far below u128::MAX.
    num.saturating_mul(2)
        .saturating_add(den)
        .div_euclid(den.saturating_mul(2))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ATTAINMENT: Decryptor = Decryptor {
        type_id: 34202,
        chance_hundredths: 180,
        me: -1,
        te: 4,
        runs: 4,
    };
    const AUGMENTATION: Decryptor = Decryptor {
        type_id: 34203,
        chance_hundredths: 60,
        me: -2,
        te: 2,
        runs: 9,
    };
    const PARITY: Decryptor = Decryptor {
        type_id: 34204,
        chance_hundredths: 150,
        me: 1,
        te: -2,
        runs: 3,
    };
    const PROCESS: Decryptor = Decryptor {
        type_id: 34205,
        chance_hundredths: 110,
        me: 3,
        te: 6,
        runs: 0,
    };
    const ACCELERANT: Decryptor = Decryptor {
        type_id: 34201,
        chance_hundredths: 120,
        me: 2,
        te: 10,
        runs: 1,
    };
    const OPTIMIZED_ATTAINMENT: Decryptor = Decryptor {
        type_id: 34207,
        chance_hundredths: 190,
        me: 1,
        te: -2,
        runs: 2,
    };

    fn skills(science1: u8, science2: u8, encryption: u8) -> Skills {
        Skills {
            science1,
            science2,
            encryption,
        }
    }

    fn chance_of(base_ppm: u32, skills: Skills, decryptor: Option<&Decryptor>) -> Chance {
        chance(Rate::from_ppm(base_ppm), skills, decryptor).expect("valid chance")
    }

    fn isk(text: &str) -> Money {
        Money::parse_isk(text).expect("valid ISK amount")
    }

    /// Chances a to g from the acceptance list.
    fn a() -> Chance {
        chance_of(340_000, skills(4, 4, 4), None)
    }
    fn b() -> Chance {
        chance_of(340_000, skills(0, 0, 0), None)
    }
    fn c() -> Chance {
        chance_of(340_000, skills(5, 5, 5), Some(&ATTAINMENT))
    }
    fn d() -> Chance {
        chance_of(400_000, skills(5, 5, 5), Some(&OPTIMIZED_ATTAINMENT))
    }
    fn e() -> Chance {
        chance_of(300_000, skills(3, 2, 1), Some(&PARITY))
    }
    fn f() -> Chance {
        chance_of(340_000, skills(5, 5, 5), Some(&OPTIMIZED_ATTAINMENT))
    }
    fn g() -> Chance {
        chance_of(340_000, skills(4, 4, 4), Some(&AUGMENTATION))
    }

    #[test]
    fn chances_match_iph_and_guru_numbers() {
        let cases = [
            (a(), 464_667, "46.47", false),
            (b(), 340_000, "34.00", false),
            (c(), 892_500, "89.25", false),
            (d(), 1_000_000, "100.00", true),
            (e(), 536_250, "53.63", false),
            (f(), 942_083, "94.21", false),
            (g(), 278_800, "27.88", false),
        ];
        for (chance, ppm, percent, certain) in cases {
            assert_eq!(chance.ppm(), ppm);
            assert_eq!(chance.percent_string(), percent);
            assert_eq!(chance.is_certain(), certain);
        }
    }

    #[test]
    fn chance_errors() {
        assert_eq!(
            chance(Rate::from_ppm(340_000), skills(6, 0, 0), None),
            Err(InventionError::SkillLevel(6))
        );
        assert_eq!(
            chance(Rate::from_ppm(340_000), skills(0, 0, 9), None),
            Err(InventionError::SkillLevel(9))
        );
        assert_eq!(
            chance(Rate::from_ppm(0), skills(4, 4, 4), None),
            Err(InventionError::ZeroChance)
        );
    }

    #[test]
    fn invented_copies() {
        let copy = |base, decryptor: Option<&Decryptor>| invented_copy(base, decryptor);
        assert_eq!(
            copy(10, None),
            Ok(InventedCopy {
                runs: 10,
                me: 2,
                te: 4
            })
        );
        assert_eq!(
            copy(10, Some(&ATTAINMENT)),
            Ok(InventedCopy {
                runs: 14,
                me: 1,
                te: 8
            })
        );
        assert_eq!(
            copy(10, Some(&AUGMENTATION)),
            Ok(InventedCopy {
                runs: 19,
                me: 0,
                te: 6
            })
        );
        assert_eq!(
            copy(10, Some(&PROCESS)),
            Ok(InventedCopy {
                runs: 10,
                me: 5,
                te: 10
            })
        );
        assert_eq!(
            copy(1, Some(&ACCELERANT)),
            Ok(InventedCopy {
                runs: 2,
                me: 4,
                te: 14
            })
        );
        assert_eq!(
            copy(1, Some(&PARITY)),
            Ok(InventedCopy {
                runs: 4,
                me: 3,
                te: 2
            })
        );
        assert_eq!(copy(0, None), Err(InventionError::BadCopy));
        let minus_three = Decryptor { me: -3, ..PARITY };
        assert_eq!(copy(10, Some(&minus_three)), Err(InventionError::BadCopy));
    }

    #[test]
    fn copies_needed_rounds_up() {
        assert_eq!(copies_needed(25, 10), Ok(3));
        assert_eq!(copies_needed(30, 10), Ok(3));
        assert_eq!(copies_needed(31, 10), Ok(4));
        assert_eq!(copies_needed(0, 10), Ok(0));
        assert_eq!(copies_needed(5, 0), Err(InventionError::BadCopy));
    }

    #[test]
    fn expected_attempts_are_exact() {
        assert_eq!(attempts(1, a(), AttemptRule::Expected), Ok(3));
        assert_eq!(attempts(1, b(), AttemptRule::Expected), Ok(3));
        assert_eq!(attempts(10, b(), AttemptRule::Expected), Ok(30));
        assert_eq!(attempts(17, c(), AttemptRule::Expected), Ok(20));
        assert_eq!(attempts(3, e(), AttemptRule::Expected), Ok(6));
        assert_eq!(attempts(2, g(), AttemptRule::Expected), Ok(8));
        assert_eq!(attempts(1, d(), AttemptRule::Expected), Ok(1));
        assert_eq!(attempts(0, a(), AttemptRule::Expected), Ok(0));
    }

    #[test]
    fn confidence_attempts() {
        let at = |ppm| AttemptRule::Confidence(Rate::from_ppm(ppm));
        let cases = [
            (1, a(), [2, 4, 8]),
            (1, b(), [2, 6, 12]),
            (10, b(), [29, 39, 50]),
            (17, c(), [19, 21, 23]),
            (3, e(), [5, 9, 13]),
            (2, g(), [6, 13, 21]),
            (1, d(), [1, 1, 1]),
        ];
        for (copies, chance, [fifty, ninety, ninety_nine]) in cases {
            assert_eq!(attempts(copies, chance, at(500_000)), Ok(fifty));
            assert_eq!(attempts(copies, chance, at(900_000)), Ok(ninety));
            assert_eq!(attempts(copies, chance, at(990_000)), Ok(ninety_nine));
        }
    }

    #[test]
    fn attempt_errors() {
        assert_eq!(
            attempts(1, a(), AttemptRule::Confidence(Rate::from_ppm(0))),
            Err(InventionError::BadConfidence)
        );
        assert_eq!(
            attempts(1, a(), AttemptRule::Confidence(Rate::from_ppm(1_000_000))),
            Err(InventionError::BadConfidence)
        );
        assert_eq!(
            attempts(
                100_000,
                b(),
                AttemptRule::Confidence(Rate::from_ppm(990_000))
            ),
            Err(InventionError::TooManyAttempts)
        );
    }

    #[test]
    fn expected_cost_per_copy_matches_iph_and_guru() {
        let per_copy =
            |cost: &str, attempts, chance| expected_cost_per_copy(isk(cost), attempts, chance);
        assert_eq!(
            per_copy("191,530.00", 1, a()).map(Money::ten_thousandths),
            Ok(4_121_879_484)
        );
        assert_eq!(
            per_copy("560,617.40", 3, b()).map(Money::ten_thousandths),
            Ok(5_496_249_020)
        );
        assert_eq!(
            per_copy("1,000,000.00", 1, c()).map(Money::ten_thousandths),
            Ok(11_204_481_793)
        );
        assert_eq!(
            per_copy("250,000.0001", 1, e()).map(Money::ten_thousandths),
            Ok(4_662_004_664)
        );
        assert_eq!(
            per_copy("500,000.00", 1, d()).map(Money::ten_thousandths),
            Ok(5_000_000_000)
        );
        assert_eq!(per_copy("0.00", 3, a()).map(Money::ten_thousandths), Ok(0));
    }

    #[test]
    fn expected_cost_errors() {
        assert_eq!(
            expected_cost_per_copy(isk("1.00"), 0, a()),
            Err(InventionError::NoAttempts)
        );
        assert_eq!(
            expected_cost_per_copy(isk("-1.00"), 1, a()),
            Err(InventionError::NegativeCost)
        );
        assert_eq!(
            expected_cost_per_copy(Money::from_ten_thousandths(i64::MAX), 1, b()),
            Err(InventionError::Overflow)
        );
    }

    #[test]
    fn errors_have_messages() {
        assert_eq!(
            InventionError::ZeroChance.to_string(),
            "the invention chance is zero"
        );
        assert_eq!(
            InventionError::SkillLevel(6).to_string(),
            "skill level 6 is above 5"
        );
    }

    #[test]
    fn more_copies_than_the_attempt_cap_are_too_many() {
        assert_eq!(
            attempts(
                MAX_ATTEMPTS + 1,
                f(),
                AttemptRule::Confidence(Rate::from_ppm(1))
            ),
            Err(InventionError::TooManyAttempts)
        );
    }

    #[test]
    fn equal_chances_compare_equal_whatever_their_inputs() {
        const HALF: Decryptor = Decryptor {
            type_id: 0,
            chance_hundredths: 50,
            me: 0,
            te: 0,
            runs: 0,
        };
        assert_eq!(
            chance_of(400_000, skills(0, 0, 0), Some(&HALF)),
            chance_of(200_000, skills(0, 0, 0), None)
        );
    }
}

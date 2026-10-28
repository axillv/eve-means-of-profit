//! How long a manufacturing run takes, computed exactly.
//!
//! The factor is a `u128` numerator over a power of ten: the blueprint's time efficiency,
//! the character's Industry and Advanced Industry levels, the structure's time bonus and one
//! factor per time rig. Only the final step rounds, up to the whole second.

#![deny(
    clippy::arithmetic_side_effects,
    clippy::float_arithmetic,
    clippy::as_conversions,
    clippy::integer_division
)]

use std::fmt;

/// Most decimal places the factor keeps. 10^38 is the largest power of ten in `u128`.
const MAX_EXP: u8 = 38;

/// The highest time efficiency a blueprint may have.
const MAX_TE: u8 = 20;
/// The highest level of an industry time skill.
const MAX_SKILL: u8 = 5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimeSkills {
    pub industry: u8,
    pub advanced_industry: u8,
}

/// A time rig: its bonus in tenths of a percent (negative) and the system security in tenths
/// (10 high, 19 low, 21 null). A T2 rig in null sec is `{ bonus_tenths_percent: -24,
/// security_tenths: 21 }`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimeRig {
    pub bonus_tenths_percent: i32,
    pub security_tenths: u32,
}

/// The facility a job runs in: its structure time multiplier in hundredths (100 for an NPC
/// station or no bonus; 85 / 80 / 70 for a Raitaru / Azbel / Sotiyo), and its fitted time rigs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimeFacility {
    pub structure_hundredths: u32,
    pub rigs: Vec<TimeRig>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobTimeInput {
    pub base_seconds: u64,
    pub te: u8,
    pub skills: TimeSkills,
    pub facility: TimeFacility,
}

/// The exact product of the time factors, as `num / 10^exp`.
///
/// Trailing zeros are removed after every step, so equal values compare equal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Factor {
    num: u128,
    exp: u8,
}

impl Factor {
    fn one() -> Self {
        Self { num: 1, exp: 0 }
    }

    /// Multiplies by `(100 - te) / 100`.
    fn with_te(self, te: u8) -> Result<Self, JobTimeError> {
        // Cannot saturate: te is checked to be at most 20 before this call, so 100 - te >= 80.
        self.times(u128::from(100u8.saturating_sub(te)), 2)
    }

    /// Multiplies by `(100 - 4 × level) / 100`.
    fn with_industry(self, level: u8) -> Result<Self, JobTimeError> {
        let points = u32::from(level).saturating_mul(4);
        // Cannot saturate: level is checked to be at most 5, so points <= 20.
        let left = 100u32.saturating_sub(points);
        self.times(u128::from(left), 2)
    }

    /// Multiplies by `(100 - 3 × level) / 100`.
    fn with_advanced_industry(self, level: u8) -> Result<Self, JobTimeError> {
        let points = u32::from(level).saturating_mul(3);
        // Cannot saturate: level is checked to be at most 5, so points <= 15.
        let left = 100u32.saturating_sub(points);
        self.times(u128::from(left), 2)
    }

    /// Multiplies by `hundredths / 100`, valid from 1 to 100.
    fn with_structure(self, hundredths: u32) -> Result<Self, JobTimeError> {
        self.times(u128::from(hundredths), 2)
    }

    /// Multiplies by `(10000 + bonus × security) / 10000`, the same rule as
    /// [`Modifier::with_rig`](crate::industry::Modifier::with_rig).
    fn with_rig(self, rig: &TimeRig) -> Result<Self, JobTimeError> {
        let err = || JobTimeError::BadRigFactor {
            bonus_tenths_percent: rig.bonus_tenths_percent,
            security_tenths: rig.security_tenths,
        };
        if rig.bonus_tenths_percent >= 0 {
            return Err(err());
        }
        let scaled = i64::from(rig.bonus_tenths_percent)
            .checked_mul(i64::from(rig.security_tenths))
            .and_then(|v| v.checked_add(10_000))
            .ok_or(JobTimeError::Overflow)?;
        let factor = u128::try_from(scaled)
            .ok()
            .filter(|f| *f > 0)
            .ok_or_else(err)?;
        self.times(factor, 4)
    }

    fn times(self, factor: u128, factor_exp: u8) -> Result<Self, JobTimeError> {
        let num = self.num.checked_mul(factor).ok_or(JobTimeError::Overflow)?;
        let exp = self
            .exp
            .checked_add(factor_exp)
            .ok_or(JobTimeError::Overflow)?;
        let product = normalized(num, exp);
        if product.exp > MAX_EXP {
            return Err(JobTimeError::Overflow);
        }
        Ok(product)
    }
}

fn normalized(mut num: u128, mut exp: u8) -> Factor {
    while exp > 0 && num.checked_rem(10) == Some(0) {
        let Some(shorter) = num.checked_div(10) else {
            break;
        };
        num = shorter;
        // Cannot saturate: the loop condition is exp > 0, so exp - 1 does not go below 0.
        exp = exp.saturating_sub(1);
    }
    Factor { num, exp }
}

/// Seconds one run takes: `base_seconds × factor`, rounded up to the whole second.
///
/// The factor is
/// `(100 − te)/100 × (100 − 4 × industry)/100 × (100 − 3 × advanced_industry)/100 ×
/// structure_hundredths/100 × each (10000 + bonus × security)/10000`.
pub fn run_seconds(input: &JobTimeInput) -> Result<u64, JobTimeError> {
    if input.te > MAX_TE {
        return Err(JobTimeError::TeOutOfRange(input.te));
    }
    if input.skills.industry > MAX_SKILL {
        return Err(JobTimeError::IndustryOutOfRange(input.skills.industry));
    }
    if input.skills.advanced_industry > MAX_SKILL {
        return Err(JobTimeError::AdvancedIndustryOutOfRange(
            input.skills.advanced_industry,
        ));
    }
    if !(1..=100).contains(&input.facility.structure_hundredths) {
        return Err(JobTimeError::BadStructureBonus(
            input.facility.structure_hundredths,
        ));
    }

    let mut factor = Factor::one()
        .with_te(input.te)?
        .with_industry(input.skills.industry)?
        .with_advanced_industry(input.skills.advanced_industry)?
        .with_structure(input.facility.structure_hundredths)?;
    for rig in &input.facility.rigs {
        factor = factor.with_rig(rig)?;
    }

    let product = u128::from(input.base_seconds)
        .checked_mul(factor.num)
        .ok_or(JobTimeError::Overflow)?;
    let scale = 10u128
        .checked_pow(u32::from(factor.exp))
        .ok_or(JobTimeError::Overflow)?;
    // ceil(product / scale), as floor((product + scale - 1) / scale).
    let ceiling = product
        .checked_add(scale.checked_sub(1).ok_or(JobTimeError::Overflow)?)
        .ok_or(JobTimeError::Overflow)?;
    let seconds = ceiling.checked_div(scale).ok_or(JobTimeError::Overflow)?;
    u64::try_from(seconds).ok().ok_or(JobTimeError::Overflow)
}

/// Seconds in one day: a job's day is a plain 24 hours.
pub const SECONDS_PER_DAY: u64 = 86_400;

/// A job's whole timing: one run, the whole run of `runs`, and how both fit a day.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobDuration {
    pub seconds_per_run: u64,
    pub total_seconds: u64,
    pub job_days: u64,
    pub runs_per_day: u64,
}

/// Seconds `runs` runs take: `seconds_per_run × runs`, checked and not rounded further.
pub fn job_seconds(seconds_per_run: u64, runs: u64) -> Result<u64, JobTimeError> {
    seconds_per_run
        .checked_mul(runs)
        .ok_or(JobTimeError::Overflow)
}

/// The whole days a job's `total_seconds` spans, rounded up; 0 for 0 seconds.
pub fn job_days(total_seconds: u64) -> u64 {
    total_seconds.div_ceil(SECONDS_PER_DAY)
}

/// How many runs of `seconds_per_run` a 24-hour day holds, floored.
///
/// A run of 0 seconds, or one longer than a day, holds none.
pub fn runs_per_day(seconds_per_run: u64) -> u64 {
    if seconds_per_run == 0 || seconds_per_run > SECONDS_PER_DAY {
        return 0;
    }
    SECONDS_PER_DAY
        .checked_div(seconds_per_run)
        .unwrap_or_default()
}

/// The whole timing of `runs` runs of `input`, computed from one [`run_seconds`].
pub fn duration(input: &JobTimeInput, runs: u64) -> Result<JobDuration, JobTimeError> {
    let seconds_per_run = run_seconds(input)?;
    let total_seconds = job_seconds(seconds_per_run, runs)?;
    Ok(JobDuration {
        seconds_per_run,
        total_seconds,
        job_days: job_days(total_seconds),
        runs_per_day: runs_per_day(seconds_per_run),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobTimeError {
    TeOutOfRange(u8),
    IndustryOutOfRange(u8),
    AdvancedIndustryOutOfRange(u8),
    BadStructureBonus(u32),
    BadRigFactor {
        bonus_tenths_percent: i32,
        security_tenths: u32,
    },
    Overflow,
}

impl fmt::Display for JobTimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TeOutOfRange(te) => write!(f, "time efficiency {te} is outside 0 to 20"),
            Self::IndustryOutOfRange(level) => {
                write!(f, "industry level {level} is outside 0 to 5")
            }
            Self::AdvancedIndustryOutOfRange(level) => {
                write!(f, "advanced industry level {level} is outside 0 to 5")
            }
            Self::BadStructureBonus(hundredths) => {
                write!(
                    f,
                    "structure time bonus {hundredths} hundredths is outside 1 to 100"
                )
            }
            Self::BadRigFactor {
                bonus_tenths_percent,
                security_tenths,
            } => write!(
                f,
                "rig bonus {bonus_tenths_percent} tenths of a percent at security {security_tenths} tenths is not a penalty that leaves a positive factor"
            ),
            Self::Overflow => write!(f, "the job time does not fit the supported range"),
        }
    }
}

impl std::error::Error for JobTimeError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(base_seconds: u64, te: u8, industry: u8, advanced_industry: u8) -> JobTimeInput {
        JobTimeInput {
            base_seconds,
            te,
            skills: TimeSkills {
                industry,
                advanced_industry,
            },
            facility: TimeFacility {
                structure_hundredths: 100,
                rigs: Vec::new(),
            },
        }
    }

    fn seconds(input: &JobTimeInput) -> u64 {
        run_seconds(input).expect("job time fits")
    }

    fn structure(base_seconds: u64, hundredths: u32, rigs: Vec<TimeRig>) -> JobTimeInput {
        JobTimeInput {
            base_seconds,
            te: 4,
            skills: TimeSkills {
                industry: 3,
                advanced_industry: 0,
            },
            facility: TimeFacility {
                structure_hundredths: hundredths,
                rigs,
            },
        }
    }

    #[test]
    fn matches_the_observed_copy() {
        // A Hobgoblin II copy at TE 4, built by Industry III: 15,000 × 0.96 × 0.88 = 12,672.
        assert_eq!(seconds(&input(15_000, 4, 3, 0)), 12_672);
    }

    #[test]
    fn structure_bonus_matches_the_game() {
        assert_eq!(seconds(&structure(15_000, 85, Vec::new())), 10_772);
        assert_eq!(seconds(&structure(15_000, 80, Vec::new())), 10_138);
        assert_eq!(seconds(&structure(15_000, 70, Vec::new())), 8_871);
    }

    #[test]
    fn rig_bonus_matches_the_game() {
        let rigs = vec![TimeRig {
            bonus_tenths_percent: -24,
            security_tenths: 21,
        }];
        assert_eq!(seconds(&structure(15_000, 85, rigs)), 10_229);
    }

    #[test]
    fn skills_reduce_the_time() {
        assert_eq!(seconds(&input(15_000, 20, 5, 5)), 8_160);
        assert_eq!(seconds(&input(15_000, 0, 0, 0)), 15_000);
        assert_eq!(seconds(&input(0, 20, 5, 5)), 0);
        assert_eq!(seconds(&input(0, 0, 0, 0)), 0);
    }

    #[test]
    fn rounds_up_to_the_whole_second() {
        // 1 × 0.96 rounds up to 1, not down to 0.
        assert_eq!(seconds(&input(1, 4, 0, 0)), 1);
        assert_eq!(seconds(&input(2, 4, 0, 0)), 2);
        // 10,771.2 and 10,137.6 both round up.
        assert_eq!(seconds(&structure(15_000, 85, Vec::new())), 10_772);
    }

    #[test]
    fn rejects_out_of_range_inputs() {
        assert_eq!(
            run_seconds(&input(15_000, 21, 0, 0)),
            Err(JobTimeError::TeOutOfRange(21))
        );
        assert_eq!(
            run_seconds(&input(15_000, 0, 6, 0)),
            Err(JobTimeError::IndustryOutOfRange(6))
        );
        assert_eq!(
            run_seconds(&input(15_000, 0, 0, 6)),
            Err(JobTimeError::AdvancedIndustryOutOfRange(6))
        );
        let mut bad = input(15_000, 0, 0, 0);
        bad.facility.structure_hundredths = 0;
        assert_eq!(run_seconds(&bad), Err(JobTimeError::BadStructureBonus(0)));
        bad.facility.structure_hundredths = 101;
        assert_eq!(run_seconds(&bad), Err(JobTimeError::BadStructureBonus(101)));

        let mut bad_rig = structure(15_000, 100, Vec::new());
        bad_rig.facility.rigs.push(TimeRig {
            bonus_tenths_percent: 0,
            security_tenths: 21,
        });
        assert_eq!(
            run_seconds(&bad_rig),
            Err(JobTimeError::BadRigFactor {
                bonus_tenths_percent: 0,
                security_tenths: 21,
            })
        );
    }

    #[test]
    fn a_rig_must_leave_a_positive_factor() {
        let mut bad = structure(15_000, 100, Vec::new());
        bad.facility.rigs.push(TimeRig {
            bonus_tenths_percent: -1000,
            security_tenths: 10,
        });
        assert_eq!(
            run_seconds(&bad),
            Err(JobTimeError::BadRigFactor {
                bonus_tenths_percent: -1000,
                security_tenths: 10,
            })
        );
    }

    #[test]
    fn overflow_is_reported() {
        // Five near-1 rigs leave a numerator of about 10^20; base times it no longer fits u128.
        let rigs = vec![
            TimeRig {
                bonus_tenths_percent: -1,
                security_tenths: 1,
            };
            5
        ];
        let huge = structure(u64::MAX, 100, rigs);
        assert_eq!(run_seconds(&huge), Err(JobTimeError::Overflow));
    }

    #[test]
    fn duration_reports_the_whole_job() {
        assert_eq!(
            duration(&input(15_000, 4, 3, 0), 10),
            Ok(JobDuration {
                seconds_per_run: 12_672,
                total_seconds: 126_720,
                job_days: 2,
                runs_per_day: 6,
            })
        );
    }

    #[test]
    fn zero_runs_give_no_time() {
        assert_eq!(
            duration(&input(15_000, 4, 3, 0), 0),
            Ok(JobDuration {
                seconds_per_run: 12_672,
                total_seconds: 0,
                job_days: 0,
                runs_per_day: 6,
            })
        );
    }

    #[test]
    fn runs_per_day_counts_whole_runs() {
        assert_eq!(runs_per_day(10_772), 8);
        assert_eq!(runs_per_day(8_160), 10);
        assert_eq!(runs_per_day(86_400), 1);
        assert_eq!(runs_per_day(86_401), 0);
        assert_eq!(runs_per_day(0), 0);
        assert_eq!(runs_per_day(1), 86_400);
    }

    #[test]
    fn job_days_rounds_up() {
        assert_eq!(job_days(0), 0);
        assert_eq!(job_days(1), 1);
        assert_eq!(job_days(86_399), 1);
        assert_eq!(job_days(86_400), 1);
        assert_eq!(job_days(86_401), 2);
    }

    #[test]
    fn job_seconds_multiplies_runs() {
        assert_eq!(job_seconds(0, 5), Ok(0));
        assert_eq!(job_seconds(7, 1), Ok(7));
        assert_eq!(job_seconds(u64::MAX, 1), Ok(u64::MAX));
    }

    #[test]
    fn job_seconds_reports_overflow() {
        assert_eq!(job_seconds(u64::MAX, 2), Err(JobTimeError::Overflow));
        assert_eq!(
            duration(&input(u64::MAX, 0, 0, 0), 2),
            Err(JobTimeError::Overflow)
        );
    }

    #[test]
    fn errors_read_as_sentences() {
        assert_eq!(
            JobTimeError::TeOutOfRange(21).to_string(),
            "time efficiency 21 is outside 0 to 20"
        );
        assert_eq!(
            JobTimeError::IndustryOutOfRange(6).to_string(),
            "industry level 6 is outside 0 to 5"
        );
        assert_eq!(
            JobTimeError::AdvancedIndustryOutOfRange(6).to_string(),
            "advanced industry level 6 is outside 0 to 5"
        );
        assert_eq!(
            JobTimeError::BadStructureBonus(0).to_string(),
            "structure time bonus 0 hundredths is outside 1 to 100"
        );
        assert_eq!(
            JobTimeError::BadRigFactor {
                bonus_tenths_percent: 0,
                security_tenths: 21,
            }
            .to_string(),
            "rig bonus 0 tenths of a percent at security 21 tenths is not a penalty that leaves a positive factor"
        );
        assert_eq!(
            JobTimeError::Overflow.to_string(),
            "the job time does not fit the supported range"
        );
    }
}

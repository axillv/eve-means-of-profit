//! The data of the Industry jobs table: every job of the login characters, with what it makes and
//! when it ends. Nothing here draws or asks the network.

use std::collections::{BTreeMap, BTreeSet};

use mop_core::industry::TypeId;
use mop_core::plan::PlanProduct;
use mop_core::reserve::{PlanForJobs, RunningJob, suggest};
use mop_esi::character_data::JobStatus;
use mop_store::reservations::PlanStock;

use crate::Shared;
use crate::build::prices::age_text;
use crate::build::state::{refresh_claims, report, type_name};
use crate::claims::refresh_plan_reads;
use crate::orders::rows::{grouped, utc_text};

/// What the Plan cell of a job holds: no plan can take it, the plan it is assigned to, or the plans
/// that could take it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PlanCell {
    /// The job is not running, so no plan applies to it.
    None,
    /// The job is assigned to a plan, shown by its id and name.
    Assigned { plan_id: i64, name: String },
    /// The plans that could take the unassigned job, in list order.
    Suggest(Vec<(i64, String)>),
}

/// One industry job, as the table shows it.
pub(crate) struct Entry {
    pub activity: String,
    pub product: String,
    pub runs: u32,
    pub character: String,
    pub location: String,
    pub status: JobStatus,
    pub end: i64,
    /// Whether the job has ended and is waiting for the player to take its output.
    pub ready: bool,
    /// Whether the job is over for good: delivered, cancelled or reverted.
    pub finished: bool,
    pub ends: String,
    pub type_id: TypeId,
    pub job_id: i64,
    pub plan: PlanCell,
}

/// Every job of every login character, the ones still going first, each group by its end time.
pub(crate) fn entries(cx: &mut Shared, now: i64) -> Vec<Entry> {
    refresh_claims(cx);
    let plans = plan_list(cx);
    let running = cx.claims.running.as_slice();
    let assignments = &cx.claims.assignments;
    let sde = cx.sde.as_ref();
    let mut entries = Vec::new();
    for character in cx.session.characters() {
        let Some(fetched) = cx
            .session
            .character_data(character.id)
            .and_then(|data| data.jobs.as_ref())
        else {
            continue;
        };
        for job in &fetched.data {
            let product_id = job.product_type_id.unwrap_or(job.blueprint_type_id);
            let ready = job.status == JobStatus::Ready
                || (job.status == JobStatus::Active && job.end <= now);
            let finished = matches!(
                job.status,
                JobStatus::Delivered | JobStatus::Cancelled | JobStatus::Reverted
            );
            let plan = match &plans {
                Some(plans) => plan_cell(job.job_id, running, plans, assignments),
                None => PlanCell::None,
            };
            entries.push(Entry {
                activity: activity_name(job.activity_id),
                product: type_name(product_id, sde, &mut cx.notices),
                runs: job.runs,
                character: character.name.clone(),
                location: cx.session.place_label(job.facility_id, sde),
                status: job.status,
                end: job.end,
                ready,
                finished,
                ends: ends_text(job.status, job.end, now),
                type_id: product_id,
                job_id: job.job_id,
                plan,
            });
        }
    }
    entries.sort_by_key(|entry| (entry.finished, entry.end));
    entries
}

/// The Plan cell of one job: `None` when the job is not running, the plan it is assigned to, or the
/// plans that could take it.
fn plan_cell(
    job_id: i64,
    running: &[RunningJob],
    plans: &[(PlanForJobs, String)],
    assignments: &BTreeMap<i64, i64>,
) -> PlanCell {
    let Some(job) = running.iter().find(|job| job.job_id == job_id) else {
        return PlanCell::None;
    };
    if let Some(&plan_id) = assignments.get(&job_id) {
        return PlanCell::Assigned {
            plan_id,
            name: plan_name(plan_id, plans),
        };
    }
    let for_jobs: Vec<PlanForJobs> = plans.iter().map(|(plan, _)| plan.clone()).collect();
    let suggested = suggest(job, &for_jobs, assignments)
        .into_iter()
        .map(|plan_id| (plan_id, plan_name(plan_id, plans)))
        .collect();
    PlanCell::Suggest(suggested)
}

/// The name of a plan, or `plan <id>` when no plan with that id is listed.
fn plan_name(plan_id: i64, plans: &[(PlanForJobs, String)]) -> String {
    plans
        .iter()
        .find(|(plan, _)| plan.plan_id == plan_id)
        .map(|(_, name)| name.clone())
        .unwrap_or_else(|| format!("plan {plan_id}"))
}

/// A plan as the job suggestions read it: its id, its first product and the rest of them built
/// alongside it, and the characters it accepts.
fn for_jobs(
    plan_id: i64,
    products: &[PlanProduct],
    build: &BTreeSet<TypeId>,
    stock: &PlanStock,
) -> PlanForJobs {
    let characters = match stock {
        PlanStock::Characters(scope) => Some(scope.characters.clone()),
        PlanStock::Pasted => None,
    };
    let product = products.first().map_or(0, |first| first.type_id);
    let mut build = build.clone();
    for later in products.iter().skip(1) {
        build.insert(later.type_id);
    }
    PlanForJobs {
        plan_id,
        product,
        build,
        characters,
    }
}

/// Every saved plan as the job suggestions read it, in list order, with its name. `None` when the
/// plans could not be read; the failure is reported then.
fn plan_list(cx: &mut Shared) -> Option<Vec<(PlanForJobs, String)>> {
    match read_plans(cx) {
        Ok(plans) => Some(plans),
        Err(error) => {
            report(cx, format!("Could not read the plans: {error}"));
            None
        }
    }
}

/// The saved plans in list order, read once per change as job suggestions, each with its name.
fn read_plans(cx: &mut Shared) -> mop_store::Result<Vec<(PlanForJobs, String)>> {
    let listed = cx.store.plans()?;
    refresh_plan_reads(cx, &listed)?;
    let reads = cx.claims.plan_reads();
    let mut plans = Vec::with_capacity(listed.len());
    for summary in listed {
        let Some(read) = reads.iter().find(|read| read.record.id == summary.id) else {
            continue;
        };
        let plan = for_jobs(summary.id, &read.products, &read.record.build, &read.stock);
        plans.push((plan, summary.name));
    }
    Ok(plans)
}

/// The name of an activity by its id, such as `Manufacturing` for 1.
pub(crate) fn activity_name(id: u32) -> String {
    match id {
        1 => "Manufacturing".to_owned(),
        3 => "TE research".to_owned(),
        4 => "ME research".to_owned(),
        5 => "Copying".to_owned(),
        7 => "Reverse engineering".to_owned(),
        8 => "Invention".to_owned(),
        9 | 11 => "Reactions".to_owned(),
        other => format!("Activity {other}"),
    }
}

/// The lowercase word for a job's status, such as `ready`.
pub(crate) fn status_text(status: JobStatus) -> &'static str {
    match status {
        JobStatus::Active => "active",
        JobStatus::Cancelled => "cancelled",
        JobStatus::Delivered => "delivered",
        JobStatus::Paused => "paused",
        JobStatus::Ready => "ready",
        JobStatus::Reverted => "reverted",
    }
}

/// When a job ends, as the table shows it: the time left, `ready to deliver`, `paused`, or the end
/// date for a job that is over.
fn ends_text(status: JobStatus, end: i64, now: i64) -> String {
    let left = end.checked_sub(now);
    match status {
        JobStatus::Active => match left {
            Some(seconds) if seconds > 0 => format!("in {}", age_text(seconds)),
            _ => "ready to deliver".to_owned(),
        },
        JobStatus::Ready => "ready to deliver".to_owned(),
        JobStatus::Paused => "paused".to_owned(),
        JobStatus::Cancelled | JobStatus::Delivered | JobStatus::Reverted => utc_text(end),
    }
}

/// The footer line for the jobs shown: their count and how many are ready to deliver.
pub(crate) fn footer(shown: &[&Entry]) -> String {
    let ready = shown.iter().filter(|entry| entry.ready).count();
    format!(
        "{} jobs · {} ready to deliver",
        grouped(u64::try_from(shown.len()).unwrap_or(u64::MAX)),
        grouped(u64::try_from(ready).unwrap_or(u64::MAX)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use mop_core::stock::{Places, StockScope};

    const MAIN: i64 = 2112000001;
    const ALT: i64 = 2112000002;
    const P1: i64 = 60003760;

    fn job(job_id: i64, character_id: i64, product: TypeId, units: u64) -> RunningJob {
        RunningJob {
            job_id,
            character_id,
            product,
            units,
            place: P1,
        }
    }

    fn running() -> Vec<RunningJob> {
        vec![
            job(501, MAIN, 2454, 1),
            job(503, MAIN, 2454, 5),
            job(601, ALT, 2454, 3),
            job(701, MAIN, 587, 1),
        ]
    }

    fn plan(plan_id: i64, name: &str, characters: Option<Vec<i64>>) -> (PlanForJobs, String) {
        (
            PlanForJobs {
                plan_id,
                product: 2456,
                build: BTreeSet::from([2454]),
                characters: characters.map(|ids| ids.into_iter().collect()),
            },
            name.to_owned(),
        )
    }

    fn plans() -> Vec<(PlanForJobs, String)> {
        vec![
            plan(11, "Hobgoblin A", Some(vec![MAIN, ALT])),
            plan(12, "Hobgoblin B", Some(vec![MAIN])),
            plan(13, "Hobgoblin C", None),
        ]
    }

    /// One product of a plan, as the store reads it.
    fn product(type_id: TypeId, quantity: u64) -> PlanProduct {
        PlanProduct { type_id, quantity }
    }

    #[test]
    fn an_unassigned_job_gets_the_plans_that_can_take_it() {
        let plans = plans();
        let assignments = BTreeMap::from([(501, 12)]);
        assert_eq!(
            plan_cell(503, &running(), &plans, &assignments),
            PlanCell::Suggest(vec![
                (11, "Hobgoblin A".to_owned()),
                (12, "Hobgoblin B".to_owned()),
                (13, "Hobgoblin C".to_owned()),
            ])
        );
        assert_eq!(
            plan_cell(601, &running(), &plans, &assignments),
            PlanCell::Suggest(vec![
                (11, "Hobgoblin A".to_owned()),
                (13, "Hobgoblin C".to_owned()),
            ])
        );
        assert_eq!(
            plan_cell(701, &running(), &plans, &assignments),
            PlanCell::Suggest(Vec::new())
        );
    }

    #[test]
    fn an_assigned_job_gets_its_plan_by_name() {
        let plans = plans();
        assert_eq!(
            plan_cell(501, &running(), &plans, &BTreeMap::from([(501, 12)])),
            PlanCell::Assigned {
                plan_id: 12,
                name: "Hobgoblin B".to_owned(),
            }
        );
        assert_eq!(
            plan_cell(501, &running(), &plans, &BTreeMap::from([(501, 999)])),
            PlanCell::Assigned {
                plan_id: 999,
                name: "plan 999".to_owned(),
            }
        );
    }

    #[test]
    fn a_job_that_is_not_running_gets_none() {
        // 502 is not in the running jobs, such as a research job. Even an assignment for it does
        // not make it a job a plan applies to.
        assert_eq!(
            plan_cell(502, &running(), &plans(), &BTreeMap::new()),
            PlanCell::None
        );
        assert_eq!(
            plan_cell(502, &running(), &plans(), &BTreeMap::from([(502, 12)])),
            PlanCell::None
        );
    }

    #[test]
    fn for_jobs_takes_the_characters_of_a_scope_and_none_for_pasted_stock() {
        let build = BTreeSet::from([2454]);
        let characters = StockScope {
            characters: BTreeSet::from([MAIN, ALT]),
            places: Places::All,
        };
        let scoped = for_jobs(
            11,
            &[product(2456, 10)],
            &build,
            &PlanStock::Characters(characters),
        );
        assert_eq!(scoped.characters, Some(BTreeSet::from([MAIN, ALT])));
        assert_eq!(scoped.product, 2456);
        assert_eq!(scoped.build, build);

        let pasted = for_jobs(11, &[product(2456, 10)], &build, &PlanStock::Pasted);
        assert_eq!(pasted.characters, None);
    }

    #[test]
    fn a_multi_product_plan_keeps_its_first_product_and_builds_the_later_ones() {
        let products = [product(2456, 10), product(587, 2)];
        let stock = PlanStock::Characters(StockScope {
            characters: BTreeSet::from([MAIN]),
            places: Places::All,
        });
        let plan = for_jobs(11, &products, &BTreeSet::new(), &stock);
        assert_eq!(plan.product, 2456);
        assert_eq!(plan.build, BTreeSet::from([587]));
        assert_eq!(plan.characters, Some(BTreeSet::from([MAIN])));

        // The running job for 587 is a later product, so the plan is suggested for it; the job
        // for 2454 is suggested only while 2454 sits in the build set.
        let plans = vec![(plan, "Hobgoblin A".to_owned())];
        assert_eq!(
            plan_cell(701, &running(), &plans, &BTreeMap::new()),
            PlanCell::Suggest(vec![(11, "Hobgoblin A".to_owned())])
        );
        assert_eq!(
            plan_cell(501, &running(), &plans, &BTreeMap::new()),
            PlanCell::Suggest(Vec::new())
        );

        let with_build = for_jobs(11, &products, &BTreeSet::from([2454]), &stock);
        let plans = vec![(with_build, "Hobgoblin A".to_owned())];
        assert_eq!(
            plan_cell(501, &running(), &plans, &BTreeMap::new()),
            PlanCell::Suggest(vec![(11, "Hobgoblin A".to_owned())])
        );
        assert_eq!(for_jobs(11, &[], &BTreeSet::new(), &stock).product, 0);
    }
}

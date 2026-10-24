//! The data of the Industry jobs table: every job of the login characters, with what it makes and
//! when it ends. Nothing here draws or asks the network.

use mop_core::industry::TypeId;
use mop_esi::character_data::JobStatus;

use crate::Shared;
use crate::build::prices::age_text;
use crate::build::state::type_name;
use crate::orders::rows::{grouped, utc_text};

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
}

/// Every job of every login character, the ones still going first, each group by its end time.
pub(crate) fn entries(cx: &mut Shared, now: i64) -> Vec<Entry> {
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
            });
        }
    }
    entries.sort_by_key(|entry| (entry.finished, entry.end));
    entries
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

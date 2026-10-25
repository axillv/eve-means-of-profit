//! The live reservations and the running manufacturing and reaction jobs, shared by every page.

use std::collections::BTreeMap;

use mop_core::industry::{Recipe, RecipeBook};
use mop_core::reserve::{Claims, ReserveError, RunningJob};
use mop_core::stock::{CharacterId, ResolvedAsset};
use mop_esi::character_data::{CharacterData, Job, JobStatus};

use crate::build::state::push_error;
use crate::status::Notice;

/// The activity ids of manufacturing (1) and the two reaction activities (9 and 11).
const PRODUCTION: [u32; 3] = [1, 9, 11];

/// The reservations and running jobs the pages read. Refreshed when its key changes or it is marked stale.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimsState {
    pub claims: Claims,
    pub running: Vec<RunningJob>,
    /// Job id to plan id.
    pub assignments: BTreeMap<i64, i64>,
    stale: bool,
    key: Vec<[u64; 4]>,
}

impl Default for ClaimsState {
    fn default() -> Self {
        Self {
            claims: Claims::default(),
            running: Vec::new(),
            assignments: BTreeMap::new(),
            stale: true,
            key: Vec::new(),
        }
    }
}

impl ClaimsState {
    /// Forces the next refresh, whatever the key.
    pub fn mark_stale(&mut self) {
        self.stale = true;
    }

    /// Whether the state must be recomputed for this key.
    pub fn needs_refresh(&self, key: &[[u64; 4]]) -> bool {
        self.stale || self.key != key
    }

    /// Stores a computed result. An error keeps the last good claims and reports the error once.
    pub fn set(
        &mut self,
        key: Vec<[u64; 4]>,
        running: Vec<RunningJob>,
        assignments: BTreeMap<i64, i64>,
        outcome: Result<Claims, ReserveError>,
        notices: &mut Vec<Notice>,
    ) {
        self.key = key;
        self.running = running;
        self.assignments = assignments;
        self.stale = false;
        match outcome {
            Ok(claims) => self.claims = claims,
            Err(error) => push_error(
                notices,
                format!("Could not work out plan reservations: {error}"),
            ),
        }
    }
}

/// The running manufacturing and reaction jobs of the given characters, in the order given.
/// Jobs that cannot be counted are left out; those that overflow get a message.
pub fn running_jobs(
    characters: &[&CharacterData],
    assets: &[ResolvedAsset],
    recipes: &RecipeBook,
) -> (Vec<RunningJob>, Vec<String>) {
    let mut running = Vec::new();
    let mut messages = Vec::new();
    for data in characters {
        let Some(jobs) = &data.jobs else { continue };
        for job in &jobs.data {
            if !PRODUCTION.contains(&job.activity_id) || !is_running(job.status) {
                continue;
            }
            let Some(product) = job.product_type_id else {
                continue;
            };
            let Some(recipe) = recipes.get(product) else {
                continue;
            };
            match counted_job(data.character_id, job, product, recipe, assets) {
                Some(counted) => running.push(counted),
                None => messages.push(format!("Job {} is too large to count.", job.job_id)),
            }
        }
    }
    (running, messages)
}

fn is_running(status: JobStatus) -> bool {
    matches!(status, JobStatus::Active | JobStatus::Ready)
}

/// One job as a running job, or `None` when its units or its character id do not fit.
fn counted_job(
    character: u64,
    job: &Job,
    product: u32,
    recipe: &Recipe,
    assets: &[ResolvedAsset],
) -> Option<RunningJob> {
    let units = u64::from(job.runs).checked_mul(recipe.output_per_run)?;
    let character_id = CharacterId::try_from(character).ok()?;
    let place = assets
        .iter()
        .find(|asset| asset.asset.item_id == job.output_location_id)
        .map_or(job.output_location_id, |asset| asset.place);
    Some(RunningJob {
        job_id: job.job_id,
        character_id,
        product,
        units,
        place,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mop_core::industry::Activity;
    use mop_core::stock::Asset;
    use mop_esi::character_data::Fetched;

    const MAIN: u64 = 2112000001;
    const ALT: u64 = 2112000002;
    const MAIN_ID: i64 = 2112000001;
    const ALT_ID: i64 = 2112000002;
    const P1: i64 = 60003760;
    const P2: i64 = 1035466617946;

    fn job(
        id: i64,
        activity: u32,
        product: Option<u32>,
        runs: u32,
        status: JobStatus,
        at: i64,
    ) -> Job {
        Job {
            job_id: id,
            activity_id: activity,
            blueprint_type_id: 1,
            product_type_id: product,
            runs,
            status,
            start: 0,
            end: 0,
            facility_id: 0,
            station_id: 0,
            output_location_id: at,
            installer_id: 0,
            cost: None,
            licensed_runs: None,
        }
    }

    fn character(id: u64, jobs: Vec<Job>) -> CharacterData {
        CharacterData {
            character_id: id,
            assets: None,
            orders: None,
            jobs: Some(Fetched {
                data: jobs,
                fetched_at: 0,
                last_modified: None,
                expires: None,
            }),
            wallet: None,
        }
    }

    fn recipe(activity: Activity, output_per_run: u64) -> Recipe {
        Recipe {
            activity,
            output_per_run,
            max_runs_per_job: 1,
            materials: Vec::new(),
            made_by_invention: false,
        }
    }

    fn recipes(hobgoblin_output: u64) -> RecipeBook {
        let mut book = RecipeBook::new();
        book.insert(2454, recipe(Activity::Manufacturing, hobgoblin_output));
        book.insert(16670, recipe(Activity::Reaction, 10_000));
        book
    }

    fn assets() -> Vec<ResolvedAsset> {
        vec![ResolvedAsset {
            character_id: MAIN_ID,
            asset: Asset {
                item_id: 3,
                type_id: 34,
                location_id: P1,
                flag: Some("Hangar".to_string()),
                quantity: 10,
                singleton: false,
            },
            place: P1,
            counts_as_stock: true,
        }]
    }

    fn main_jobs() -> Vec<Job> {
        vec![
            job(501, 1, Some(2454), 1, JobStatus::Active, P2),
            job(503, 1, Some(2454), 5, JobStatus::Ready, 3),
            job(502, 4, None, 1, JobStatus::Active, P1),
            job(505, 1, Some(2454), 1, JobStatus::Delivered, P1),
            job(506, 11, Some(16670), 2, JobStatus::Active, P1),
            job(507, 1, Some(2454), 1, JobStatus::Paused, P1),
            job(508, 1, Some(587), 1, JobStatus::Active, P1),
        ]
    }

    fn alt_jobs() -> Vec<Job> {
        vec![job(601, 1, Some(2454), 3, JobStatus::Active, P1)]
    }

    fn run(job_id: i64, character_id: i64, product: u32, units: u64, place: i64) -> RunningJob {
        RunningJob {
            job_id,
            character_id,
            product,
            units,
            place,
        }
    }

    #[test]
    fn counts_running_production_jobs_in_order() {
        let main = character(MAIN, main_jobs());
        let alt = character(ALT, alt_jobs());
        let (running, messages) = running_jobs(&[&main, &alt], &assets(), &recipes(1));
        assert_eq!(
            running,
            vec![
                run(501, MAIN_ID, 2454, 1, P2),
                run(503, MAIN_ID, 2454, 5, P1),
                run(506, MAIN_ID, 16670, 20_000, P1),
                run(601, ALT_ID, 2454, 3, P1),
            ]
        );
        assert!(messages.is_empty());
    }

    #[test]
    fn overflowing_jobs_are_left_out_with_a_message() {
        let mut jobs = main_jobs();
        jobs.push(job(509, 1, Some(2454), 2, JobStatus::Active, P1));
        let main = character(MAIN, jobs);
        let alt = character(ALT, alt_jobs());
        let (running, messages) = running_jobs(&[&main, &alt], &assets(), &recipes(u64::MAX));
        assert_eq!(
            running,
            vec![
                run(501, MAIN_ID, 2454, u64::MAX, P2),
                run(506, MAIN_ID, 16670, 20_000, P1),
            ]
        );
        assert_eq!(
            messages,
            vec![
                "Job 503 is too large to count.".to_string(),
                "Job 509 is too large to count.".to_string(),
                "Job 601 is too large to count.".to_string(),
            ]
        );
    }

    #[test]
    fn character_id_that_does_not_fit_is_left_out() {
        let main = character(
            u64::MAX,
            vec![job(701, 1, Some(2454), 1, JobStatus::Active, P1)],
        );
        let (running, messages) = running_jobs(&[&main], &assets(), &recipes(1));
        assert!(running.is_empty());
        assert_eq!(messages, vec!["Job 701 is too large to count.".to_string()]);
    }

    #[test]
    fn missing_jobs_are_skipped() {
        let mut main = character(MAIN, Vec::new());
        main.jobs = None;
        let (running, messages) = running_jobs(&[&main], &assets(), &recipes(1));
        assert!(running.is_empty());
        assert!(messages.is_empty());
    }

    #[test]
    fn new_state_needs_refresh() {
        let state = ClaimsState::default();
        assert!(state.needs_refresh(&[[1, 0, 2, 0]]));
    }

    #[test]
    fn set_stores_key_and_clears_stale() {
        let mut state = ClaimsState::default();
        let mut notices = Vec::new();
        state.set(
            vec![[1, 0, 2, 0]],
            Vec::new(),
            BTreeMap::new(),
            Ok(Claims::default()),
            &mut notices,
        );
        assert!(!state.needs_refresh(&[[1, 0, 2, 0]]));
        assert!(state.needs_refresh(&[[1, 1, 2, 0]]));
        state.mark_stale();
        assert!(state.needs_refresh(&[[1, 0, 2, 0]]));
    }

    #[test]
    fn error_keeps_claims_and_reports_once() {
        let mut state = ClaimsState::default();
        let mut notices = Vec::new();
        let good = Claims {
            order: vec![11, 12],
            ..Claims::default()
        };
        state.set(
            vec![[1, 0, 2, 0]],
            Vec::new(),
            BTreeMap::new(),
            Ok(good),
            &mut notices,
        );
        state.set(
            vec![[1, 0, 2, 0]],
            Vec::new(),
            BTreeMap::new(),
            Err(ReserveError::OutOfRange),
            &mut notices,
        );
        state.set(
            vec![[1, 0, 2, 0]],
            Vec::new(),
            BTreeMap::new(),
            Err(ReserveError::OutOfRange),
            &mut notices,
        );
        assert_eq!(state.claims.order, vec![11, 12]);
        assert_eq!(notices.len(), 1);
        assert_eq!(notices[0].level, crate::status::Level::Error);
        assert_eq!(
            notices[0].text,
            "Could not work out plan reservations: number out of range"
        );
    }
}

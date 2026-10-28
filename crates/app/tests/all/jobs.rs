//! Acceptance tests for the Industry jobs page: the rows of every login character's jobs, their
//! order, the footer and the Show finished chip. The clock is fixed at 1,000,000.
#[cfg(test)]
mod tests {

    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;
    use std::io::Write;
    use std::ops::ControlFlow;
    use std::path::{Path, PathBuf};

    use crate::ui::{click, on_row, shown, snapshot, top, until};
    use egui::vec2;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;
    use eve_means_of_profit::session::{EveAccess, PendingLogin};
    use eve_means_of_profit::{App, AppConfig};
    use mop_core::freight::Packing;
    use mop_core::stock::{Places, StockScope};
    use mop_esi::character_data::{CharacterData, Fetched, Job, JobStatus, Kind, Refreshed};
    use mop_esi::characters::{Character, Login};
    use mop_esi::error::EsiError;
    use mop_esi::places::{NamingReport, StructureNames};
    use mop_esi::saved;
    use mop_market::book::PriceRule;
    use mop_sde::{Fetch, FetchError, import};
    use mop_store::reservations::PlanStock;
    use mop_store::{PlanRecord, Store};
    use tempfile::TempDir;

    const NOW: i64 = 1_000_000;
    const FIXTURE: &str = "../sde/tests/fixtures/sde-3607705";
    const MAIN: u64 = 2_112_000_001;
    const MAIN_ID: i64 = 2_112_000_001;
    const HOME_FORT: i64 = 1_035_466_617_946;
    const PAU: u32 = 11_688;
    const HOBGOBLIN_I: u32 = 2_454;
    const HOBGOBLIN_I_BLUEPRINT: u32 = 2_455;
    const HOBGOBLIN_II: u32 = 2_456;
    const RIFTER: u32 = 587;
    const RIFTER_BLUEPRINT: u32 = 691;
    const HOBGOBLIN_II_BLUEPRINT: u32 = 2_457;
    const CRYSTALLINE_CARBONIDE: u32 = 16_670;
    const EMPTY: &str = "No industry jobs. Refresh on the Characters page to fetch them.";

    fn fixture() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE)
    }

    /// The game data is installed by the test, so the app never asks the network for anything.
    struct NoNetwork;

    impl Fetch for NoNetwork {
        fn get_text(&self, _url: &str) -> Result<String, FetchError> {
            Err(FetchError::Http { status: 503 })
        }

        fn download(
            &self,
            _url: &str,
            _dest: &mut dyn Write,
            _progress: &mut dyn FnMut(u64, Option<u64>) -> ControlFlow<()>,
        ) -> Result<u64, FetchError> {
            Err(FetchError::Http { status: 503 })
        }
    }

    /// A data folder with the game data imported, the user's store, and Home Fort named.
    struct Env {
        _dir: TempDir,
        data: PathBuf,
        cache: PathBuf,
    }

    fn env() -> Env {
        let dir = TempDir::new().expect("a temp folder");
        let data = dir.path().join("data");
        let cache = dir.path().join("cache");
        fs::create_dir_all(data.join("esi")).expect("the data folder");
        import(&fixture(), &data.join("sde.sqlite")).expect("the fixture imports");
        let mut store = Store::open(&data.join("user.sqlite"), NOW)
            .expect("the store opens")
            .store;
        let mut settings = store.settings().expect("settings");
        settings.last_sde_check = Some(NOW);
        store.save_settings(&settings).expect("settings saved");
        let places = format!(
            "{{\"version\":1,\"structures\":{{\"{HOME_FORT}\":{{\"name\":\"Home Fort\",\"system_id\":null,\"refused\":false,\"checked_at\":900000}}}}}}"
        );
        fs::write(data.join("esi").join("places.json"), places).expect("places written");
        Env {
            _dir: dir,
            data,
            cache,
        }
    }

    fn esi_dir(env: &Env) -> PathBuf {
        env.data.join("esi")
    }

    /// A login with the characters given. Nothing here refreshes or logs in.
    #[derive(Clone)]
    struct FakeEve {
        characters: Vec<Character>,
    }

    impl EveAccess for FakeEve {
        fn characters(&self) -> Vec<Character> {
            self.characters.clone()
        }

        fn begin_login(&mut self) -> Result<PendingLogin, String> {
            Err("Logins are not started in these tests.".to_owned())
        }

        fn finish_login(&mut self, _code: &str) -> Result<Character, EsiError> {
            Err(EsiError::Cancelled)
        }

        fn refresh_all(
            &mut self,
            _data: &mut CharacterData,
            _now: i64,
        ) -> Vec<(Kind, Result<Refreshed, EsiError>)> {
            Vec::new()
        }

        fn name_structures(
            &mut self,
            _names: &mut StructureNames,
            _ids: &[i64],
            _now: i64,
        ) -> Result<NamingReport, EsiError> {
            Ok(NamingReport {
                asked: 0,
                failed: Vec::new(),
            })
        }

        fn save_character(&mut self, _data: &CharacterData) -> Result<(), EsiError> {
            Ok(())
        }

        fn remove(&mut self, character_id: u64) -> Result<(), EsiError> {
            Err(EsiError::UnknownCharacter(character_id))
        }

        fn keychain_warning(&self) -> Option<String> {
            None
        }

        fn jeve_folder(&self) -> Option<PathBuf> {
            None
        }
    }

    fn character(id: u64, name: &str) -> Character {
        Character {
            id,
            name: name.to_owned(),
            scopes: BTreeSet::new(),
            added_at: NOW,
            login: Login::Ok,
        }
    }

    fn job(
        job_id: i64,
        activity_id: u32,
        product: Option<u32>,
        blueprint: u32,
        runs: u32,
        status: JobStatus,
        end: i64,
    ) -> Job {
        Job {
            job_id,
            activity_id,
            blueprint_type_id: blueprint,
            product_type_id: product,
            runs,
            status,
            start: end - 86_400,
            end,
            facility_id: HOME_FORT,
            station_id: HOME_FORT,
            output_location_id: HOME_FORT,
            installer_id: 2_112_000_001,
            cost: None,
            licensed_runs: None,
        }
    }

    /// Main's jobs, fetched at `NOW`: an active manufacturing, an active invention that has ended,
    /// a ready reaction, a paused copying, and a delivered manufacturing.
    fn main_jobs() -> Vec<Job> {
        vec![
            job(
                1,
                1,
                Some(PAU),
                RIFTER_BLUEPRINT,
                10,
                JobStatus::Active,
                NOW + 7_500,
            ),
            job(
                2,
                8,
                Some(HOBGOBLIN_II_BLUEPRINT),
                HOBGOBLIN_II_BLUEPRINT,
                1,
                JobStatus::Active,
                NOW - 10,
            ),
            job(
                3,
                11,
                Some(CRYSTALLINE_CARBONIDE),
                RIFTER_BLUEPRINT,
                5,
                JobStatus::Ready,
                NOW - 3_600,
            ),
            job(
                4,
                5,
                None,
                RIFTER_BLUEPRINT,
                1,
                JobStatus::Paused,
                NOW + 20_000,
            ),
            job(
                5,
                1,
                Some(PAU),
                RIFTER_BLUEPRINT,
                2,
                JobStatus::Delivered,
                900_000,
            ),
        ]
    }

    fn save_jobs(env: &Env, jobs: Vec<Job>) {
        let data = CharacterData {
            character_id: MAIN,
            assets: None,
            orders: None,
            jobs: Some(Fetched {
                data: jobs,
                fetched_at: NOW,
                last_modified: None,
                expires: None,
            }),
            wallet: None,
        };
        saved::save_character(&esi_dir(env), &data).expect("Main's jobs saved");
    }

    fn start(env: &Env, characters: Vec<Character>) -> Harness<'static, App> {
        let config = AppConfig {
            data_dir: env.data.clone(),
            cache_dir: env.cache.clone(),
            fetch: Box::new(NoNetwork),
            clock: Box::new(|| NOW),
            eve: Box::new(FakeEve { characters }),
            market: Box::new(crate::offline::Offline),
        };
        let mut harness = Harness::builder()
            .with_size(vec2(1280.0, 800.0))
            .with_max_steps(crate::SETTLE_STEPS)
            .build_eframe(move |cc| App::new(&cc.egui_ctx, config).expect("the app starts"));
        harness
            .state_mut()
            .show_page(eve_means_of_profit::rail::Page::Build);
        harness
    }

    fn store(env: &Env) -> Store {
        Store::open(&env.data.join("user.sqlite"), NOW)
            .expect("the store opens")
            .store
    }

    /// A Hobgoblin II plan that builds Hobgoblin I, with stock from Main at every place.
    fn make_plan(env: &Env, name: &str) -> i64 {
        let record = PlanRecord {
            id: 0,
            name: name.to_owned(),
            product: HOBGOBLIN_II,
            quantity: 1,
            build: BTreeSet::from([HOBGOBLIN_I]),
            me: BTreeMap::new(),
            manufacturing: None,
            reactions: None,
            price_rule: PriceRule::WalkSell,
            route: None,
            service: None,
            packing: Packing::default(),
            pasted_stock: String::new(),
            created_at: NOW,
            updated_at: NOW,
        };
        let id = store(env)
            .create_plan(&record, NOW)
            .expect("the plan is saved");
        let scope = StockScope {
            characters: BTreeSet::from([MAIN_ID]),
            places: Places::All,
        };
        store(env)
            .set_plan_stock(id, &PlanStock::Characters(scope))
            .expect("the plan's stock is saved");
        id
    }

    fn assign(env: &Env, job_id: i64, plan_id: i64) {
        store(env)
            .assign_job(job_id, plan_id, NOW)
            .expect("the job is assigned");
    }

    fn assignments(env: &Env) -> BTreeMap<i64, i64> {
        store(env)
            .job_assignments()
            .expect("the assignments are read")
    }

    /// Main's jobs, all still going: two manufacturing runs of a Hobgoblin I, an ME research job,
    /// and a Rifter run.
    fn assigning_jobs() -> Vec<Job> {
        vec![
            job(
                501,
                1,
                Some(HOBGOBLIN_I),
                HOBGOBLIN_I_BLUEPRINT,
                1,
                JobStatus::Active,
                NOW + 3_600,
            ),
            job(
                503,
                1,
                Some(HOBGOBLIN_I),
                HOBGOBLIN_I_BLUEPRINT,
                5,
                JobStatus::Active,
                NOW + 7_200,
            ),
            job(
                502,
                4,
                None,
                HOBGOBLIN_I_BLUEPRINT,
                1,
                JobStatus::Active,
                NOW + 7_200,
            ),
            job(
                504,
                1,
                Some(RIFTER),
                RIFTER_BLUEPRINT,
                1,
                JobStatus::Active,
                NOW + 7_200,
            ),
        ]
    }

    /// Whether a node showing `value` (a dropdown keeps its text as a value) sits on a row that
    /// some node labelled `anchor` is on.
    fn on_row_value(harness: &Harness<'_, App>, anchor: &str, value: &str) -> bool {
        let rows: Vec<f32> = harness
            .query_all_by_label(anchor)
            .map(|node| node.rect().center().y)
            .collect();
        !rows.is_empty()
            && harness.query_all_by_value(value).any(|node| {
                let y = node.rect().center().y;
                rows.iter().any(|row| (y - row).abs() <= 3.0)
            })
    }

    /// The top of the first node showing `text`, as its label or as its value.
    fn first_top(harness: &Harness<'_, App>, text: &str) -> f32 {
        harness
            .query_all_by_label(text)
            .next()
            .or_else(|| harness.query_all_by_value(text).next())
            .unwrap_or_else(|| panic!("no node showing {text:?}"))
            .rect()
            .min
            .y
    }

    /// Runs frames until the start-up read has arrived: the page's loading line is gone.
    fn loaded(harness: &mut Harness<'_, App>) {
        until(harness, "the saved data", |h| {
            !shown(h, "Loading character data…")
        });
    }

    #[test]
    fn the_default_rows_are_the_jobs_still_going_by_end_time() {
        let env = env();
        save_jobs(&env, main_jobs());
        let mut harness = start(&env, vec![character(MAIN, "Main")]);
        loaded(&mut harness);
        click(&mut harness, "Industry jobs");

        assert!(shown(&harness, "4 jobs · 2 ready to deliver"));
        assert!(shown(&harness, "Reactions"));
        assert!(shown(&harness, "Crystalline Carbonide"));
        assert!(shown(&harness, "Invention"));
        assert!(shown(&harness, "Hobgoblin II Blueprint"));
        assert!(shown(&harness, "Manufacturing"));
        assert!(shown(&harness, "in 2h"));
        assert!(shown(&harness, "Copying"));
        assert!(shown(&harness, "Rifter Blueprint"));
        assert!(shown(&harness, "paused"));
        assert!(on_row(&harness, "Manufacturing", &["10"]));
        assert!(on_row(&harness, "Manufacturing", &["Main"]));
        assert!(on_row(&harness, "Manufacturing", &["Home Fort"]));
        assert!(on_row(&harness, "Manufacturing", &["in 2h"]));
        assert!(on_row(&harness, "Reactions", &["ready to deliver"]));
        assert!(on_row(&harness, "Copying", &["paused"]));
        assert!(shown(&harness, "ready to deliver"));
        assert!(!shown(&harness, "1970-01-11 10:00"));
    }

    #[test]
    fn the_jobs_still_going_are_listed_by_end_time() {
        let env = env();
        save_jobs(&env, main_jobs());
        let mut harness = start(&env, vec![character(MAIN, "Main")]);
        loaded(&mut harness);
        click(&mut harness, "Industry jobs");

        assert!(top(&harness, "Reactions") < top(&harness, "Invention"));
        assert!(top(&harness, "Invention") < top(&harness, "Manufacturing"));
        assert!(top(&harness, "Manufacturing") < top(&harness, "Copying"));
        assert!(on_row(&harness, "Invention", &["Hobgoblin II Blueprint"]));
        assert!(on_row(&harness, "Invention", &["ready to deliver"]));
    }

    #[test]
    fn show_finished_adds_the_delivered_job_last() {
        let env = env();
        save_jobs(&env, main_jobs());
        let mut harness = start(&env, vec![character(MAIN, "Main")]);
        loaded(&mut harness);
        click(&mut harness, "Industry jobs");
        harness
            .get_all_by_label("Show finished")
            .next()
            .expect("the Show finished chip")
            .click();
        harness.run();

        assert!(shown(&harness, "5 jobs · 2 ready to deliver"));
        assert!(shown(&harness, "delivered"));
        assert!(top(&harness, "delivered") > top(&harness, "Copying"));
        assert!(on_row(&harness, "delivered", &["1970-01-11 10:00"]));
        assert!(shown(&harness, "1970-01-11 10:00"));
    }

    #[test]
    fn an_activity_without_a_name_shows_its_number() {
        let env = env();
        save_jobs(
            &env,
            vec![job(
                9,
                2,
                None,
                RIFTER_BLUEPRINT,
                1,
                JobStatus::Active,
                NOW + 7_500,
            )],
        );
        let mut harness = start(&env, vec![character(MAIN, "Main")]);
        loaded(&mut harness);
        click(&mut harness, "Industry jobs");

        assert!(shown(&harness, "Activity 2"));
        assert!(shown(&harness, "Rifter Blueprint"));
    }

    #[test]
    fn with_no_characters_the_page_says_so() {
        let env = env();
        let mut harness = start(&env, Vec::new());
        loaded(&mut harness);
        click(&mut harness, "Industry jobs");

        assert!(shown(&harness, EMPTY));
    }

    #[test]
    fn a_job_is_assigned_to_a_plan_and_taken_off_it_again() {
        let env = env();
        let a = make_plan(&env, "Hobgoblin A");
        let b = make_plan(&env, "Hobgoblin B");
        assign(&env, 501, b);
        save_jobs(&env, assigning_jobs());
        let mut harness = start(&env, vec![character(MAIN, "Main")]);
        loaded(&mut harness);
        click(&mut harness, "Industry jobs");

        // 501 is assigned to B; 503 offers the plans, in list order.
        assert!(on_row(&harness, "Hobgoblin B", &["×"]));
        assert!(on_row_value(&harness, "5", "Assign…"));
        click(&mut harness, "Assign…");
        let a_top = first_top(&harness, "Hobgoblin A");
        // The row of 501 also shows B, so look for the dropdown's B below A.
        let b_below = harness
            .query_all_by_label("Hobgoblin B")
            .any(|node| node.rect().min.y > a_top)
            || harness
                .query_all_by_value("Hobgoblin B")
                .any(|node| node.rect().min.y > a_top);
        assert!(shown(&harness, "Hobgoblin A"));
        assert!(b_below);

        click(&mut harness, "Hobgoblin A");
        assert_eq!(assignments(&env), BTreeMap::from([(501, b), (503, a)]));
        assert!(on_row(&harness, "Hobgoblin A", &["×"]));

        // The × on 501 takes it off B, and its cell offers the plans again.
        click(&mut harness, "×");
        assert_eq!(assignments(&env), BTreeMap::from([(503, a)]));
        assert!(on_row_value(&harness, "in 1h", "Assign…"));
        click(&mut harness, "Assign…");
        assert!(shown(&harness, "Hobgoblin A"));
        assert!(shown(&harness, "Hobgoblin B"));
        assert!(first_top(&harness, "Hobgoblin A") < first_top(&harness, "Hobgoblin B"));
    }

    #[test]
    fn a_plan_cell_is_empty_without_a_suggestion_and_mute_when_no_plan_builds_it() {
        let env = env();
        make_plan(&env, "Hobgoblin A");
        save_jobs(&env, assigning_jobs());
        let mut harness = start(&env, vec![character(MAIN, "Main")]);
        loaded(&mut harness);
        click(&mut harness, "Industry jobs");

        // 502 is research: no plan applies to it, so its cell is empty.
        assert!(shown(&harness, "ME research"));
        assert!(!on_row_value(&harness, "ME research", "Assign…"));
        assert!(!on_row(&harness, "ME research", &["no plan builds this"]));
        // 504 builds a Rifter, which no plan builds.
        assert!(on_row(&harness, "no plan builds this", &["Rifter"]));
        assert!(!on_row_value(&harness, "Rifter", "Assign…"));
    }

    #[test]
    fn the_standing_jobs_show_no_plan_builds_this_where_a_plan_could_take_them() {
        let env = env();
        save_jobs(&env, main_jobs());
        let mut harness = start(&env, vec![character(MAIN, "Main")]);
        loaded(&mut harness);
        click(&mut harness, "Industry jobs");

        assert!(on_row(&harness, "Manufacturing", &["no plan builds this"]));
        assert!(on_row(&harness, "Reactions", &["no plan builds this"]));
        assert!(!on_row(&harness, "Invention", &["no plan builds this"]));
        assert!(!on_row(&harness, "Copying", &["no plan builds this"]));
    }

    #[test]
    fn the_jobs_page_matches_its_snapshot() {
        let env = env();
        save_jobs(&env, main_jobs());
        let mut harness = start(&env, vec![character(MAIN, "Main")]);
        loaded(&mut harness);
        click(&mut harness, "Industry jobs");
        harness.run();
        snapshot(&mut harness, "jobs-page");
    }

    #[test]
    fn the_assign_dropdown_matches_its_snapshot() {
        let env = env();
        make_plan(&env, "Hobgoblin A");
        let b = make_plan(&env, "Hobgoblin B");
        assign(&env, 501, b);
        save_jobs(&env, assigning_jobs());
        let mut harness = start(&env, vec![character(MAIN, "Main")]);
        loaded(&mut harness);
        click(&mut harness, "Industry jobs");
        click(&mut harness, "Assign…");
        snapshot(&mut harness, "jobs_assign");
    }
}

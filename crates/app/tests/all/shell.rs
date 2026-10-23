//! Acceptance tests for the app shell. The network is a fake that serves the build 3607705
//! fixture as a zip, built in the test. The clock is fixed at 1,000,000.
#[cfg(test)]
mod tests {

    use std::fs;
    use std::io::{Cursor, Write};
    use std::path::{Path, PathBuf};
    use std::sync::mpsc::{self, Receiver};
    use std::sync::{Arc, Mutex, PoisonError};
    use std::thread;
    use std::time::{Duration, Instant};

    use egui::{Event, Key, Modifiers, PointerButton, Pos2, vec2};
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;
    use eve_means_of_profit::theme::{Style, contrast};
    use eve_means_of_profit::{App, AppConfig};
    use mop_sde::{Fetch, FetchError, LATEST_URL, import};
    use mop_store::{Density, Store};
    use tempfile::TempDir;

    const NOW: i64 = 1_000_000;
    const BUILD: u64 = 3_607_705;
    /// How long the fake takes to answer the latest-build address, so the checking screen is seen.
    const LATENCY: Duration = Duration::from_millis(200);
    const FIXTURE: &str = "../sde/tests/fixtures/sde-3607705";

    fn latest_body(build: u64) -> String {
        format!(
            "{{\"_key\":\"sde\",\"buildNumber\":{build},\"releaseDate\":\"2026-10-01T00:00:00Z\"}}\n"
        )
    }

    fn fixture() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE)
    }

    /// The fixture files, zipped with them at the root, as the server sends them.
    fn fixture_zip() -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for entry in fs::read_dir(fixture()).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name().to_string_lossy().into_owned();
            writer
                .start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(&fs::read(entry.path()).unwrap()).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    /// What the fake answers to the latest-build address.
    #[derive(Clone)]
    enum Latest {
        Body(String),
        Status(u16),
    }

    /// A network that serves the fixture zip in four chunks, `pause` apart. Each request is recorded.
    /// With `hold`, the download stops before chunk `hold.0` until the test sends on `hold.1`.
    struct Fake {
        latest: Latest,
        zip: Vec<u8>,
        pause: Duration,
        asked: Arc<Mutex<Vec<String>>>,
        hold: Option<(usize, Mutex<Receiver<()>>)>,
        /// When set, the download fails with this network error before sending anything.
        fail_download: Option<String>,
    }

    impl Fake {
        fn new(latest: Latest) -> Self {
            Self {
                latest,
                zip: fixture_zip(),
                pause: Duration::from_millis(20),
                asked: Arc::new(Mutex::new(Vec::new())),
                hold: None,
                fail_download: None,
            }
        }

        fn failing_download(mut self, reason: &str) -> Self {
            self.fail_download = Some(reason.to_owned());
            self
        }

        fn paused(mut self, pause: Duration) -> Self {
            self.pause = pause;
            self
        }

        fn held(mut self, chunk: usize, release: Receiver<()>) -> Self {
            self.hold = Some((chunk, Mutex::new(release)));
            self
        }

        fn asked(&self) -> Arc<Mutex<Vec<String>>> {
            Arc::clone(&self.asked)
        }
    }

    impl Fetch for Fake {
        fn get_text(&self, url: &str) -> Result<String, FetchError> {
            self.asked
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(url.to_owned());
            thread::sleep(LATENCY);
            match &self.latest {
                Latest::Body(text) => Ok(text.clone()),
                Latest::Status(status) => Err(FetchError::Http { status: *status }),
            }
        }

        fn download(
            &self,
            url: &str,
            dest: &mut dyn Write,
            progress: &mut dyn FnMut(u64, Option<u64>) -> std::ops::ControlFlow<()>,
        ) -> Result<u64, FetchError> {
            self.asked
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(url.to_owned());
            if let Some(reason) = &self.fail_download {
                return Err(FetchError::Network(reason.clone()));
            }
            let total = u64::try_from(self.zip.len()).unwrap_or(u64::MAX);
            let piece_len = self.zip.len().div_ceil(4);
            let mut done = 0;
            for (index, piece) in self.zip.chunks(piece_len).enumerate() {
                if let Some((chunk, release)) = &self.hold
                    && *chunk == index
                    && release
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .recv()
                        .is_err()
                {
                    return Err(FetchError::Cancelled);
                }
                thread::sleep(self.pause);
                dest.write_all(piece)
                    .map_err(|error| FetchError::Network(error.to_string()))?;
                done += u64::try_from(piece.len()).unwrap_or(u64::MAX);
                if progress(done, Some(total)).is_break() {
                    return Err(FetchError::Cancelled);
                }
            }
            // A hold on the chunk after the last one stops the download between the last byte and
            // the end of the reply, which is where the start-up shows "Importing game data…".
            if let Some((chunk, release)) = &self.hold
                && *chunk == self.zip.chunks(piece_len).count()
                && release
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .recv()
                    .is_err()
            {
                return Err(FetchError::Cancelled);
            }
            Ok(done)
        }
    }

    /// Where one test keeps its files.
    struct Folders {
        _dir: Option<TempDir>,
        data: PathBuf,
        cache: PathBuf,
    }

    impl Folders {
        fn new() -> Self {
            let dir = TempDir::new().unwrap();
            let data = dir.path().join("data");
            let cache = dir.path().join("cache");
            Self {
                _dir: Some(dir),
                data,
                cache,
            }
        }

        /// A folder with the same name every run, for the screenshots: the data folder shows in the
        /// settings window, so a random name would change the picture.
        fn fixed() -> Self {
            let root = std::env::temp_dir().join("eve-means-of-profit-shell-snapshots");
            drop(fs::remove_dir_all(&root));
            Self {
                _dir: None,
                data: root.join("data"),
                cache: root.join("cache"),
            }
        }

        fn user_db(&self) -> PathBuf {
            self.data.join("user.sqlite")
        }

        fn sde_db(&self) -> PathBuf {
            self.data.join("sde.sqlite")
        }

        /// Imports the fixture as the installed game data.
        fn install_game_data(&self) {
            fs::create_dir_all(&self.data).unwrap();
            import(&fixture(), &self.sde_db()).unwrap();
        }

        /// Stores the time of the last check, so the daily check is due or not.
        fn set_last_check(&self, last: i64) {
            fs::create_dir_all(&self.data).unwrap();
            let mut store = Store::open(&self.user_db(), NOW).unwrap().store;
            let mut settings = store.settings().unwrap();
            settings.last_sde_check = Some(last);
            store.save_settings(&settings).unwrap();
        }
    }

    fn start(fake: Fake, folders: &Folders) -> Harness<'static, App> {
        let config = AppConfig {
            data_dir: folders.data.clone(),
            cache_dir: folders.cache.clone(),
            fetch: Box::new(fake),
            clock: Box::new(|| NOW),
        };
        Harness::builder()
            .with_size(vec2(1280.0, 800.0))
            .build_eframe(move |cc| App::new(&cc.egui_ctx, config).expect("the app starts"))
    }

    /// Whether some node shows `text`. The error text is on both the first-run screen and the status line.
    fn shown(harness: &Harness<'_, App>, text: &str) -> bool {
        harness.query_all_by_label(text).next().is_some()
    }

    /// Runs frames until `check` holds, failing after 20 seconds.
    fn until(
        harness: &mut Harness<'_, App>,
        what: &str,
        check: impl Fn(&Harness<'_, App>) -> bool,
    ) {
        let started = Instant::now();
        while !check(harness) {
            assert!(
                started.elapsed() < Duration::from_secs(20),
                "timed out waiting for {what}"
            );
            harness.run_steps(1);
            thread::sleep(Duration::from_millis(5));
        }
    }

    /// Runs frames for a short while, so that background results are read.
    fn settle(harness: &mut Harness<'_, App>) {
        for _ in 0..20 {
            harness.run_steps(1);
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn build_page_shown(harness: &Harness<'_, App>) -> bool {
        ["Plans", "Pasted stock", "Plan", "Steps", "Buy list"]
            .iter()
            .all(|panel| shown(harness, panel))
    }

    #[test]
    fn the_colour_tokens_meet_the_contrast_values_of_the_design_note() {
        let style = Style::eve_dense();
        let ratio = |a, b| format!("{:.2}", contrast(a, b));
        assert_eq!(ratio(style.text, style.bg), "11.96");
        assert_eq!(ratio(style.text, style.panel), "11.17");
        assert_eq!(ratio(style.text, style.row), "10.21");
        assert_eq!(ratio(style.mute, style.bg), "5.54");
        assert_eq!(ratio(style.mute, style.row), "4.73");
        assert_eq!(ratio(style.on_accent, style.accent), "7.96");
        assert_eq!(ratio(style.stale, style.bg), "5.78");
        assert_eq!(ratio(style.pos, style.bg), "7.03");
        assert_eq!(ratio(style.neg, style.bg), "5.43");
        assert_eq!(ratio(style.row, style.line), "1.16");
        assert_eq!(ratio(style.bg, style.line), "1.36");
        assert_eq!(ratio(style.bg, style.row), "1.17");
        assert_eq!(ratio(style.bg, style.sel), "1.20");
    }

    #[test]
    fn a_row_is_selected_then_hovered_then_zebra_then_plain() {
        let style = Style::eve_dense();
        assert_eq!(style.row_fill(true, true, true), style.sel);
        assert_eq!(style.row_fill(true, true, false), style.line);
        assert_eq!(style.row_fill(true, false, false), style.row);
        assert_eq!(style.row_fill(false, false, false), style.bg);
    }

    #[test]
    fn the_first_run_downloads_imports_and_then_shows_the_pages() {
        let folders = Folders::new();
        let fake = Fake::new(Latest::Body(latest_body(BUILD)));
        let mut harness = start(fake, &folders);
        harness.run_steps(1);
        assert!(shown(&harness, "Checking for game data…"));

        until(&mut harness, "the download", |h| {
            shown(h, "Getting the game data")
                && h.query_all_by_label_contains("Build 3607705 · ")
                    .next()
                    .is_some()
        });
        until(&mut harness, "the build pages", |h| {
            shown(h, "Build") && build_page_shown(h)
        });

        assert!(folders.sde_db().is_file());
        let mut store = Store::open(&folders.user_db(), NOW).unwrap().store;
        assert_eq!(store.settings().unwrap().last_sde_check, Some(NOW));
    }

    #[test]
    fn the_rail_offers_build_and_setup_once_the_pages_are_shown() {
        let folders = Folders::new();
        folders.install_game_data();
        folders.set_last_check(990_000);
        let mut harness = start(Fake::new(Latest::Body(latest_body(BUILD))), &folders);
        until(&mut harness, "the build pages", build_page_shown);
        assert_eq!(harness.query_all_by_label("Build").count(), 1);
        assert_eq!(harness.query_all_by_label("Setup").count(), 1);
        harness.get_by_label("Setup").click();
        until(&mut harness, "the setup panels", |h| shown(h, "Price rule"));
        harness.get_by_label("Build").click();
        until(&mut harness, "the build pages again", build_page_shown);
    }

    #[test]
    fn the_locked_pages_show_each_panel_title_once_and_no_tab_bar() {
        let folders = Folders::new();
        folders.install_game_data();
        folders.set_last_check(990_000);
        let mut harness = start(Fake::new(Latest::Body(latest_body(BUILD))), &folders);
        until(&mut harness, "the build pages", build_page_shown);
        for panel in ["Plans", "Pasted stock", "Plan", "Steps", "Buy list"] {
            assert_eq!(harness.query_all_by_label(panel).count(), 1, "{panel}");
        }
        harness.get_by_label("Setup").click();
        until(&mut harness, "the setup panels", |h| shown(h, "Price rule"));
        for panel in ["Facilities", "Freight services", "Price rule"] {
            assert_eq!(harness.query_all_by_label(panel).count(), 1, "{panel}");
        }
    }

    #[test]
    fn a_failed_first_download_shows_the_error_and_try_again() {
        let folders = Folders::new();
        let fake = Fake::new(Latest::Body(latest_body(BUILD))).failing_download("connection reset");
        let mut harness = start(fake, &folders);
        let text = "Could not get the game data: cannot reach the server: connection reset";
        until(&mut harness, "the error text", |h| shown(h, text));
        assert_eq!(harness.query_all_by_label(text).count(), 2);
        assert!(shown(&harness, "ERROR"));
        assert!(shown(&harness, "Try again"));
    }

    #[test]
    fn the_status_line_shows_the_update_progress_while_the_download_runs() {
        let folders = Folders::new();
        folders.install_game_data();
        folders.set_last_check(900_000);
        let (release, gate) = mpsc::channel();
        let fake = Fake::new(Latest::Body(latest_body(3_607_800))).held(1, gate);
        let mut harness = start(fake, &folders);
        until(&mut harness, "the notice", |h| {
            shown(h, "Game data build 3607800 is available.")
        });
        harness.get_by_label("Update").click();
        until(&mut harness, "the update progress", |h| {
            h.query_all_by_label_contains("Updating game data:")
                .next()
                .is_some()
        });
        release
            .send(())
            .expect("the download is waiting for the release");
        until(&mut harness, "the update error", |h| {
            h.query_all_by_label_contains("Could not update the game data: ")
                .next()
                .is_some()
        });
    }

    #[test]
    fn cancel_shows_the_cancelled_text_and_try_again_finishes() {
        let folders = Folders::new();
        let fake = Fake::new(Latest::Body(latest_body(BUILD))).paused(Duration::from_millis(200));
        let mut harness = start(fake, &folders);
        until(&mut harness, "the Cancel button", |h| shown(h, "Cancel"));
        harness.get_by_label("Cancel").click();
        until(&mut harness, "the cancelled text", |h| {
            shown(h, "Download cancelled.")
        });
        harness.get_by_label("Try again").click();
        until(&mut harness, "the build pages", build_page_shown);
        assert!(folders.sde_db().is_file());
    }

    #[test]
    fn a_server_error_shows_on_the_first_run_screen_and_in_the_status_line() {
        let folders = Folders::new();
        let fake = Fake::new(Latest::Status(503));
        let mut harness = start(fake, &folders);
        let text = "Could not get the game data: the server answered with status 503";
        until(&mut harness, "the error text", |h| shown(h, text));
        assert_eq!(harness.query_all_by_label(text).count(), 2);
        assert!(shown(&harness, "ERROR"));
        assert!(shown(&harness, "Try again"));
    }

    #[test]
    fn a_user_file_in_another_layout_is_set_aside_with_a_notice() {
        let folders = Folders::new();
        folders.install_game_data();
        import(&fixture(), &folders.user_db()).unwrap();
        let text = format!(
            "Your saved data came from an earlier test build and was set aside as {}.old-test. Starting fresh.",
            folders.user_db().display()
        );
        let mut harness = start(Fake::new(Latest::Status(503)), &folders);
        settle(&mut harness);
        assert!(shown(&harness, "NOTICE"));
        assert!(
            harness
                .query_all_by_label_contains("Your saved data came from an earlier test build")
                .next()
                .is_some()
        );
        let notices = &harness.state_mut().shared_mut().notices;
        assert_eq!(notices[0].text, text);
        assert!(folders.data.join("user.sqlite.old-test").is_file());
    }

    #[test]
    fn a_new_or_current_user_file_gives_no_start_notice() {
        let folders = Folders::new();
        folders.install_game_data();
        folders.set_last_check(990_000);
        for _ in 0..2 {
            let mut harness = start(Fake::new(Latest::Status(503)), &folders);
            settle(&mut harness);
            assert!(build_page_shown(&harness));
            assert!(!shown(&harness, "Starting fresh."));
            assert!(!folders.data.join("user.sqlite.old-test").exists());
        }
    }

    #[test]
    fn a_newer_build_gives_a_notice_when_the_check_is_due() {
        let folders = Folders::new();
        folders.install_game_data();
        folders.set_last_check(900_000);
        let fake = Fake::new(Latest::Body(latest_body(3_607_800)));
        let asked = fake.asked();
        let mut harness = start(fake, &folders);
        until(&mut harness, "the notice", |h| {
            shown(h, "Game data build 3607800 is available.")
        });
        assert!(shown(&harness, "Update"));
        assert_eq!(*asked.lock().unwrap(), vec![LATEST_URL.to_owned()]);
    }

    #[test]
    fn no_check_is_made_when_the_last_one_was_recent() {
        let folders = Folders::new();
        folders.install_game_data();
        folders.set_last_check(990_000);
        let fake = Fake::new(Latest::Body(latest_body(3_607_800)));
        let asked = fake.asked();
        let mut harness = start(fake, &folders);
        settle(&mut harness);
        assert!(asked.lock().unwrap().is_empty());
        assert!(shown(&harness, "No market data yet"));
    }

    #[test]
    fn clicking_setup_shows_its_panels_and_the_locked_pages_keep_their_layout() {
        let folders = Folders::new();
        folders.install_game_data();
        folders.set_last_check(990_000);
        let mut harness = start(Fake::new(Latest::Body(latest_body(BUILD))), &folders);
        until(&mut harness, "the build pages", build_page_shown);

        let plans = harness.get_by_label("Plans").rect();
        assert_eq!(harness.query_all_by_label("Plans").count(), 1);
        harness.event(Event::PointerMoved(plans.center()));
        harness.event(Event::PointerButton {
            pos: plans.center(),
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        });
        let elsewhere = Pos2::new(plans.center().x + 400.0, plans.center().y + 200.0);
        harness.event(Event::PointerMoved(elsewhere));
        harness.event(Event::PointerButton {
            pos: elsewhere,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        });
        settle(&mut harness);
        assert_eq!(harness.get_by_label("Plans").rect(), plans);

        harness.get_by_label("Setup").click();
        until(&mut harness, "the setup panels", |h| {
            shown(h, "Facilities") && shown(h, "Freight services") && shown(h, "Price rule")
        });
        harness.get_by_label("Build").click();
        until(&mut harness, "the build pages again", build_page_shown);
    }

    #[test]
    fn the_status_line_shows_the_first_notice_and_dismiss_shows_the_next() {
        let folders = Folders::new();
        folders.install_game_data();
        folders.set_last_check(990_000);
        let mut harness = start(Fake::new(Latest::Body(latest_body(BUILD))), &folders);
        settle(&mut harness);
        assert!(shown(&harness, "No market data yet"));

        let shared = harness.state_mut().shared_mut();
        shared
            .notices
            .push(eve_means_of_profit::status::Notice::info("First notice"));
        shared
            .notices
            .push(eve_means_of_profit::status::Notice::info("Second notice"));
        shared
            .notices
            .push(eve_means_of_profit::status::Notice::info("Third notice"));
        settle(&mut harness);
        assert!(shown(&harness, "First notice (1 of 3)"));

        harness.get_by_label("Dismiss").click();
        settle(&mut harness);
        assert!(shown(&harness, "Second notice (1 of 2)"));
    }

    #[test]
    fn ctrl_comma_opens_settings_and_a_density_is_saved() {
        let folders = Folders::new();
        folders.install_game_data();
        folders.set_last_check(990_000);
        let mut harness = start(Fake::new(Latest::Body(latest_body(BUILD))), &folders);
        until(&mut harness, "the build pages", build_page_shown);

        harness.key_press_modifiers(Modifiers::CTRL, Key::Comma);
        until(&mut harness, "the settings window", |h| {
            shown(h, "Settings")
        });
        harness.get_by_label("Dense").click();
        settle(&mut harness);

        let mut store = Store::open(&folders.user_db(), NOW).unwrap().store;
        assert_eq!(store.settings().unwrap().density, Density::Dense);
    }

    #[test]
    fn the_ui_keeps_drawing_while_a_download_is_pending() {
        let folders = Folders::new();
        let (release, gate) = mpsc::channel();
        let fake = Fake::new(Latest::Body(latest_body(BUILD))).held(0, gate);
        let mut harness = start(fake, &folders);
        until(&mut harness, "the download", |h| {
            shown(h, "Getting the game data")
        });

        let started = Instant::now();
        for _ in 0..30 {
            harness.run_steps(1);
        }
        assert!(started.elapsed() < Duration::from_secs(10));
        assert!(shown(&harness, "Getting the game data"));

        release
            .send(())
            .expect("the download is waiting for the release");
        until(&mut harness, "the build pages", build_page_shown);
    }

    #[test]
    fn the_snapshots_for_the_owner_are_taken() {
        let folders = Folders::fixed();
        let (release, gate) = mpsc::channel();
        let fake = Fake::new(Latest::Body(latest_body(BUILD))).held(2, gate);
        let mut harness = start(fake, &folders);
        until(&mut harness, "half the download", |h| {
            h.query_all_by_label_contains("Build 3607705 · ")
                .next()
                .is_some()
        });
        settle(&mut harness);
        harness.snapshot("shell-first-run");
        release
            .send(())
            .expect("the download is waiting for the release");
        until(&mut harness, "the build pages", build_page_shown);
        harness.snapshot("shell-build-empty");
        harness.get_by_label("Setup").click();
        until(&mut harness, "the setup panels", |h| shown(h, "Price rule"));
        harness.snapshot("shell-setup-empty");
        harness.get_by_label("Build").click();
        until(&mut harness, "the build pages again", build_page_shown);
        harness.key_press_modifiers(Modifiers::CTRL, Key::Comma);
        until(&mut harness, "the settings window", |h| {
            shown(h, "Settings")
        });
        settle(&mut harness);
        harness.snapshot("shell-settings");
    }

    #[test]
    fn the_error_snapshot_is_taken_on_the_status_line() {
        let folders = Folders::new();
        let mut harness = start(Fake::new(Latest::Status(503)), &folders);
        until(&mut harness, "the error text", |h| {
            shown(
                h,
                "Could not get the game data: the server answered with status 503",
            )
        });
        harness.snapshot("shell-status-error");
    }

    #[test]
    fn the_importing_screen_shows_until_the_import_ends() {
        let folders = Folders::new();
        let (release, gate) = mpsc::channel();
        let fake = Fake::new(Latest::Body(latest_body(BUILD))).held(4, gate);
        let mut harness = start(fake, &folders);
        until(&mut harness, "the importing text", |h| {
            shown(h, "Importing game data…")
        });
        release
            .send(())
            .expect("the download is waiting for the release");
        until(&mut harness, "the build pages", build_page_shown);
    }

    #[test]
    fn a_failed_daily_check_shows_an_error_notice() {
        let folders = Folders::new();
        folders.install_game_data();
        folders.set_last_check(900_000);
        let mut harness = start(Fake::new(Latest::Status(503)), &folders);
        until(&mut harness, "the check error", |h| {
            shown(
                h,
                "Could not check for new game data: the server answered with status 503",
            )
        });
        assert!(shown(&harness, "ERROR"));
    }

    #[test]
    fn update_downloads_in_the_background_and_a_bad_download_is_reported() {
        let folders = Folders::new();
        folders.install_game_data();
        folders.set_last_check(900_000);
        let mut harness = start(Fake::new(Latest::Body(latest_body(3_607_800))), &folders);
        until(&mut harness, "the notice", |h| {
            shown(h, "Game data build 3607800 is available.")
        });
        harness.get_by_label("Update").click();
        // The fixture zip is build 3607705, so the update is refused after the download.
        until(&mut harness, "the update error", |h| {
            shown(
                h,
                "Could not update the game data: the download is build 3607705, but build 3607800 was asked for",
            )
        });
        assert!(!shown(&harness, "Update"));
    }
}

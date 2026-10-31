//! Acceptance tests for the app shell. The network is a fake that serves the build 3607705
//! fixture as a zip, built in the test. The clock is fixed at 1,000,000.
#[cfg(test)]
mod tests {

    use std::fs;
    use std::io::{Cursor, Write};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicI64, Ordering};
    use std::sync::mpsc::{self, Receiver};
    use std::sync::{Arc, Mutex, PoisonError};
    use std::thread;
    use std::time::{Duration, Instant};

    use egui::{Event, Key, Modifiers, MouseWheelUnit, PointerButton, Pos2, TouchPhase, vec2};
    use egui_kittest::Harness;
    use egui_kittest::kittest::{NodeT, Queryable};
    use eve_means_of_profit::rail::Page;
    use eve_means_of_profit::session::NoEve;
    use eve_means_of_profit::theme::{Style, contrast};
    use eve_means_of_profit::{App, AppConfig};
    use mop_sde::{Fetch, FetchError, LATEST_URL, import};
    use mop_store::{Density, InterfaceSize, Store};
    use tempfile::TempDir;

    use crate::offline::Offline;

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
        /// settings page, so a random name would change the picture.
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

        /// Stores an interface size, as an earlier run of the app would have.
        fn set_interface_size(&self, size: InterfaceSize) {
            fs::create_dir_all(&self.data).unwrap();
            let mut store = Store::open(&self.user_db(), NOW).unwrap().store;
            let mut settings = store.settings().unwrap();
            settings.interface_size = size;
            store.save_settings(&settings).unwrap();
        }

        /// Stores whether the rail shows icons alone, as an earlier run of the app would have.
        fn set_rail_icons_only(&self, icons_only: bool) {
            fs::create_dir_all(&self.data).unwrap();
            let mut store = Store::open(&self.user_db(), NOW).unwrap().store;
            let mut settings = store.settings().unwrap();
            settings.rail_icons_only = icons_only;
            store.save_settings(&settings).unwrap();
        }

        /// Stores the keep-refreshing choice, as the Display panel's box would have saved it.
        fn set_refresh_while_open(&self, value: &str) {
            fs::create_dir_all(&self.data).unwrap();
            let mut store = Store::open(&self.user_db(), NOW).unwrap().store;
            store.set_setting("refresh_while_open", value).unwrap();
        }
    }

    fn start(fake: Fake, folders: &Folders) -> Harness<'static, App> {
        start_with_clock(fake, folders, Arc::new(AtomicI64::new(NOW)))
    }

    /// The app on `folders`' files, with a clock the test moves instead of a fixed one.
    fn start_with_clock(
        fake: Fake,
        folders: &Folders,
        clock: Arc<AtomicI64>,
    ) -> Harness<'static, App> {
        let config = AppConfig {
            data_dir: folders.data.clone(),
            cache_dir: folders.cache.clone(),
            fetch: Box::new(fake),
            clock: Box::new(move || clock.load(Ordering::SeqCst)),
            eve: Box::new(NoEve),
            market: Box::new(Offline),
        };
        let mut harness = Harness::builder()
            .with_size(vec2(1280.0, 800.0))
            .with_max_steps(crate::SETTLE_STEPS)
            .build_eframe(move |cc| App::new(&cc.egui_ctx, config).expect("the app starts"));
        harness.state_mut().show_page(Page::Build);
        harness
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

    /// The `explicit` marks of the price answers that arrived since the last call.
    fn take_prices(harness: &mut Harness<'_, App>) -> Vec<bool> {
        harness
            .state_mut()
            .shared_mut()
            .market
            .take_outcomes()
            .into_iter()
            .map(|outcome| outcome.explicit)
            .collect()
    }

    /// Runs frames until `wanted` price answers have arrived, and returns their marks.
    fn wait_for_prices(harness: &mut Harness<'_, App>, wanted: usize) -> Vec<bool> {
        let mut all = Vec::new();
        let started = Instant::now();
        while all.len() < wanted {
            assert!(
                started.elapsed() < Duration::from_secs(20),
                "only {} price answers arrived, wanted {wanted}",
                all.len()
            );
            harness.run_steps(1);
            thread::sleep(Duration::from_millis(5));
            all.extend(take_prices(harness));
        }
        all
    }

    /// Runs frames until no price request is queued or running, so that the next ask cannot be
    /// dropped as a duplicate of this one.
    fn prices_settled(harness: &mut Harness<'_, App>) {
        let started = Instant::now();
        while harness.state_mut().shared_mut().market.running() {
            assert!(
                started.elapsed() < Duration::from_secs(20),
                "the price request never finished"
            );
            harness.run_steps(1);
            thread::sleep(Duration::from_millis(5));
        }
    }

    /// Scrolls the Display panel with the pointer held at `at`, by `delta` points down (negative)
    /// or up (positive). The controls at the bottom of the panel need it.
    fn scroll_display(harness: &mut Harness<'_, App>, at: Pos2, delta: f32) {
        harness.hover_at(at);
        harness.event(Event::MouseWheel {
            unit: MouseWheelUnit::Point,
            delta: vec2(0.0, delta),
            phase: TouchPhase::Move,
            modifiers: Modifiers::NONE,
        });
        harness.run_steps(20);
    }

    fn build_page_shown(harness: &Harness<'_, App>) -> bool {
        ["Plans", "Plan", "Steps", "Buy list"]
            .iter()
            .all(|panel| shown(harness, panel))
    }

    /// Finishes a first run: the app starts on Today, waits for the welcome, then takes
    /// `Try without logging in`.
    fn finish_first_run(harness: &mut Harness<'_, App>) {
        harness.state_mut().show_page(Page::Today);
        until(harness, "the welcome", |h| {
            shown(h, "Welcome to EVE Means of Profit")
        });
        harness.get_by_label("Try without logging in").click();
    }

    /// The app's current zoom factor, which shows the interface size.
    fn zoom(harness: &Harness<'_, App>) -> f32 {
        harness.ctx.zoom_factor()
    }

    /// The interface size last saved in the test's data folder.
    fn saved_size(folders: &Folders) -> InterfaceSize {
        let mut store = Store::open(&folders.user_db(), NOW).unwrap().store;
        store.settings().unwrap().interface_size
    }

    /// Whether the rail shows icons alone in the test's data folder.
    fn saved_rail_icons_only(folders: &Folders) -> bool {
        let mut store = Store::open(&folders.user_db(), NOW).unwrap().store;
        store.settings().unwrap().rail_icons_only
    }

    /// The keep-refreshing choice as the test's data folder holds it, `None` when never saved.
    fn saved_refresh_while_open(folders: &Folders) -> Option<String> {
        let store = Store::open(&folders.user_db(), NOW).unwrap().store;
        store.setting("refresh_while_open").unwrap()
    }

    /// The text put on the clipboard by the frame just drawn, if any.
    fn copied_text(harness: &Harness<'_, App>) -> Option<String> {
        harness
            .output()
            .platform_output
            .commands
            .iter()
            .find_map(|command| {
                if let egui::OutputCommand::CopyText(text) = command {
                    Some(text.clone())
                } else {
                    None
                }
            })
    }

    /// The top edge of the first node labelled `label`, to check what is above what.
    fn top_of(harness: &Harness<'_, App>, label: &str) -> f32 {
        harness
            .query_all_by_label(label)
            .next()
            .unwrap_or_else(|| panic!("no node labelled {label}"))
            .rect()
            .min
            .y
    }

    /// Whether the radio button labelled `label` is the selected one.
    fn selected(harness: &Harness<'_, App>, label: &str) -> bool {
        harness
            .query_all_by_label(label)
            .next()
            .unwrap_or_else(|| panic!("no node labelled {label}"))
            .accesskit_node()
            .toggled()
            == Some(egui::accesskit::Toggled::True)
    }

    /// Whether the checkbox labelled `label` is ticked.
    fn ticked(harness: &Harness<'_, App>, label: &str) -> bool {
        harness
            .query_all_by_label(label)
            .find(|node| node.accesskit_node().role() == egui::accesskit::Role::CheckBox)
            .unwrap_or_else(|| panic!("no checkbox labelled {label}"))
            .accesskit_node()
            .toggled()
            == Some(egui::accesskit::Toggled::True)
    }

    /// Ticks or unticks the checkbox labelled `label`, scrolling it into view first. The pick goes
    /// through accesskit, which reaches the box even where the panel has it below its fold.
    fn click_checkbox(harness: &mut Harness<'_, App>, label: &str) {
        {
            let node = harness
                .query_all_by_label(label)
                .find(|node| node.accesskit_node().role() == egui::accesskit::Role::CheckBox)
                .unwrap_or_else(|| panic!("no checkbox labelled {label}"));
            node.scroll_to_me();
        }
        harness.run_steps(2);
        harness
            .query_all_by_label(label)
            .find(|node| node.accesskit_node().role() == egui::accesskit::Role::CheckBox)
            .unwrap_or_else(|| panic!("no checkbox labelled {label}"))
            .click_accesskit();
        harness.run_steps(3);
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
    fn the_first_run_downloads_imports_and_then_shows_the_welcome() {
        let folders = Folders::new();
        let (release, gate) = mpsc::channel();
        let fake = Fake::new(Latest::Body(latest_body(BUILD))).held(1, gate);
        let mut harness = start(fake, &folders);
        harness.run_steps(1);
        assert!(shown(&harness, "Checking for game data…"));
        assert!(shown(
            &harness,
            "When the game data is ready, try without logging in or import from jEve."
        ));

        until(&mut harness, "the download", |h| {
            shown(h, "Getting the game data")
                && h.query_all_by_label_contains("Build 3607705 · ")
                    .next()
                    .is_some()
        });
        assert!(shown(
            &harness,
            "When the game data is ready, try without logging in or import from jEve."
        ));
        release
            .send(())
            .expect("the download is waiting for the release");

        until(&mut harness, "the welcome", |h| {
            shown(h, "Welcome to EVE Means of Profit")
        });
        for text in [
            "Welcome to EVE Means of Profit",
            "No EVE login is needed.",
            "Try without logging in",
            "Product, quantity, paste what you own, buy list.",
            "Import from jEve",
            "jEve's characters now; import its history on Net worth and its stockpiles on Stockpiles.",
            "Next: log in on Characters for live data, paste your stock on Build, and pick your facility in Settings.",
        ] {
            assert!(shown(&harness, text), "{text}");
        }
        assert!(!shown(
            &harness,
            "When the game data is ready, try without logging in or import from jEve."
        ));

        assert!(folders.sde_db().is_file());
        let mut store = Store::open(&folders.user_db(), NOW).unwrap().store;
        assert_eq!(store.settings().unwrap().last_sde_check, Some(NOW));
    }

    #[test]
    fn try_without_logging_in_from_the_welcome_opens_build() {
        let folders = Folders::new();
        let mut harness = start(Fake::new(Latest::Body(latest_body(BUILD))), &folders);
        until(&mut harness, "the welcome", |h| {
            shown(h, "Welcome to EVE Means of Profit")
        });
        finish_first_run(&mut harness);
        until(&mut harness, "the build pages", build_page_shown);
        assert!(!shown(&harness, "Welcome to EVE Means of Profit"));
    }

    #[test]
    fn import_from_jeve_from_the_welcome_opens_characters() {
        let folders = Folders::new();
        let mut harness = start(Fake::new(Latest::Body(latest_body(BUILD))), &folders);
        until(&mut harness, "the welcome", |h| {
            shown(h, "Welcome to EVE Means of Profit")
        });
        harness.state_mut().show_page(Page::Today);
        harness.get_by_label("Import from jEve").click();
        until(&mut harness, "the characters page", |h| {
            shown(h, "jEve data") && shown(h, "No jEve data file found.")
        });
        assert!(!shown(&harness, "Welcome to EVE Means of Profit"));
    }

    #[test]
    fn a_later_start_shows_the_build_pages_without_the_welcome() {
        let folders = Folders::new();
        folders.install_game_data();
        folders.set_last_check(990_000);
        let mut harness = start(Fake::new(Latest::Body(latest_body(BUILD))), &folders);
        until(&mut harness, "the build pages", build_page_shown);
        assert!(!shown(&harness, "Welcome to EVE Means of Profit"));
    }

    #[test]
    fn the_rail_offers_build_and_setup_once_the_pages_are_shown() {
        let folders = Folders::new();
        folders.install_game_data();
        folders.set_last_check(990_000);
        let mut harness = start(Fake::new(Latest::Body(latest_body(BUILD))), &folders);
        until(&mut harness, "the build pages", build_page_shown);
        assert_eq!(harness.query_all_by_label("Build").count(), 1);
        assert_eq!(harness.query_all_by_label(Page::Setup.label()).count(), 1);
        harness.get_by_label(Page::Setup.label()).click();
        until(&mut harness, "the setup panels", |h| shown(h, "Price rule"));
        harness.get_by_label("Build").click();
        until(&mut harness, "the build pages again", build_page_shown);
    }

    #[test]
    fn a_saved_icons_only_rail_starts_narrow_with_the_build_entry_short() {
        let folders = Folders::new();
        folders.install_game_data();
        folders.set_last_check(990_000);
        folders.set_rail_icons_only(true);
        let mut harness = start(Fake::new(Latest::Body(latest_body(BUILD))), &folders);
        until(&mut harness, "the build pages", build_page_shown);
        assert!(
            harness.get_by_label("Build").rect().width() <= 48.0,
            "the icons-only Build entry is at most 48 px wide"
        );
    }

    #[test]
    fn a_rail_with_nothing_saved_shows_the_build_entry_wider_than_the_narrow_width() {
        let folders = Folders::new();
        folders.install_game_data();
        folders.set_last_check(990_000);
        let mut harness = start(Fake::new(Latest::Body(latest_body(BUILD))), &folders);
        until(&mut harness, "the build pages", build_page_shown);
        assert!(
            harness.get_by_label("Build").rect().width() > 48.0,
            "the labelled Build entry is wider than 48 px"
        );
    }

    #[test]
    fn the_locked_pages_show_each_panel_title_once_and_no_tab_bar() {
        let folders = Folders::new();
        folders.install_game_data();
        folders.set_last_check(990_000);
        let mut harness = start(Fake::new(Latest::Body(latest_body(BUILD))), &folders);
        until(&mut harness, "the build pages", build_page_shown);
        for panel in ["Plans", "Plan", "Steps", "Buy list"] {
            assert_eq!(harness.query_all_by_label(panel).count(), 1, "{panel}");
        }
        harness.get_by_label(Page::Setup.label()).click();
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
        until(&mut harness, "the welcome", |h| {
            shown(h, "Welcome to EVE Means of Profit")
        });
        finish_first_run(&mut harness);
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

        harness.get_by_label(Page::Setup.label()).click();
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
    fn refresh_all_keeps_a_notice_and_dismiss_or_its_action_removes_it() {
        use eve_means_of_profit::status::{self, Action, Notice};

        struct State {
            notices: Vec<Notice>,
            action: Option<Action>,
        }
        let mut harness = Harness::new_ui_state(
            |ui, state: &mut State| {
                state.action = status::show(
                    ui,
                    &Style::eve_dense(),
                    &mut state.notices,
                    None,
                    "idle",
                    "",
                );
            },
            State {
                notices: vec![Notice::info("First notice")],
                action: None,
            },
        );
        harness.run();
        assert!(harness.query_all_by_label("First notice").next().is_some());

        harness.get_by_label("Refresh all").click();
        harness.step();
        assert_eq!(harness.state().notices.len(), 1);
        assert_eq!(harness.state().action, Some(Action::RefreshAll));
        assert!(harness.query_all_by_label("First notice").next().is_some());

        harness.get_by_label("Dismiss").click();
        harness.step();
        assert!(harness.state().notices.is_empty());

        harness.state_mut().notices =
            vec![Notice::info("Update ready").with_action(Action::Update { build: 42 })];
        harness.run();
        harness.get_by_label("Update").click();
        harness.step();
        assert_eq!(harness.state().action, Some(Action::Update { build: 42 }));
        assert!(harness.state().notices.is_empty());
    }

    #[test]
    fn ctrl_comma_opens_the_settings_page_and_a_density_is_saved() {
        let folders = Folders::new();
        folders.install_game_data();
        folders.set_last_check(990_000);
        let mut harness = start(Fake::new(Latest::Body(latest_body(BUILD))), &folders);
        until(&mut harness, "the build pages", build_page_shown);

        harness.key_press_modifiers(Modifiers::CTRL, Key::Comma);
        until(&mut harness, "the settings page", |h| {
            shown(h, "Interface size")
        });
        harness.get_by_label("Dense").click();
        settle(&mut harness);

        let mut store = Store::open(&folders.user_db(), NOW).unwrap().store;
        assert_eq!(store.settings().unwrap().density, Density::Dense);
    }

    #[test]
    fn ctrl_comma_and_the_view_menu_open_the_settings_page() {
        let folders = Folders::new();
        folders.install_game_data();
        folders.set_last_check(990_000);
        let mut harness = start(Fake::new(Latest::Body(latest_body(BUILD))), &folders);
        until(&mut harness, "the build pages", build_page_shown);

        harness.key_press_modifiers(Modifiers::CTRL, Key::Comma);
        until(&mut harness, "the settings page", |h| {
            shown(h, "Facilities") && shown(h, "Price history") && shown(h, "Display")
        });
        assert!(
            top_of(&harness, "Facilities") < top_of(&harness, "Price history")
                && top_of(&harness, "Price history") < top_of(&harness, "Display"),
            "the three panels sit down the left in that order"
        );
        assert_eq!(
            harness
                .query_all_by_role_and_label(egui::accesskit::Role::Window, "Settings")
                .count(),
            0,
            "no Settings window opens"
        );

        harness.get_by_label("Build").click();
        until(&mut harness, "the build pages again", build_page_shown);
        harness.get_by_label("View").click();
        until(&mut harness, "the View menu", |h| shown(h, "Settings…"));
        harness.get_by_label("Settings…").click();
        until(&mut harness, "the settings page from the menu", |h| {
            shown(h, "Facilities") && shown(h, "Price history") && shown(h, "Display")
        });
        assert!(
            top_of(&harness, "Facilities") < top_of(&harness, "Price history")
                && top_of(&harness, "Price history") < top_of(&harness, "Display"),
            "the menu opens the same page with the panels in that order"
        );
        assert_eq!(
            harness
                .query_all_by_role_and_label(egui::accesskit::Role::Window, "Settings")
                .count(),
            0,
            "the menu opens no Settings window"
        );
    }

    #[test]
    fn ctrl_comma_on_the_first_run_screen_opens_no_settings_page_or_window() {
        let folders = Folders::new();
        let mut harness = start(Fake::new(Latest::Status(503)), &folders);
        until(&mut harness, "the error text", |h| {
            shown(
                h,
                "Could not get the game data: the server answered with status 503",
            )
        });

        harness.key_press_modifiers(Modifiers::CTRL, Key::Comma);
        settle(&mut harness);
        assert!(
            !shown(&harness, "Facilities"),
            "the Settings page stays away"
        );
        assert!(!shown(&harness, "Price history"));
        assert_eq!(
            harness
                .query_all_by_role_and_label(egui::accesskit::Role::Window, "Settings")
                .count(),
            0,
            "no Settings window opens"
        );
    }

    #[test]
    fn the_settings_page_shows_the_interface_sizes_and_the_density_names() {
        let folders = Folders::new();
        folders.install_game_data();
        folders.set_last_check(990_000);
        let mut harness = start(Fake::new(Latest::Body(latest_body(BUILD))), &folders);
        until(&mut harness, "the build pages", build_page_shown);
        harness.key_press_modifiers(Modifiers::CTRL, Key::Comma);
        until(&mut harness, "the settings page", |h| {
            shown(h, "Interface size")
        });

        for label in [
            "100%", "125%", "150%", "175%", "200%", "Roomy", "Normal", "Dense",
        ] {
            assert!(shown(&harness, label), "{label} is listed");
        }
        assert_eq!(
            harness.query_all_by_label("Sparse").count(),
            0,
            "Sparse is named Roomy"
        );
        assert!(
            top_of(&harness, "Interface size") < top_of(&harness, "Density"),
            "the interface size is above the density"
        );
        assert!(
            top_of(&harness, "Roomy") < top_of(&harness, "Normal")
                && top_of(&harness, "Normal") < top_of(&harness, "Dense"),
            "the density choices read Roomy, Normal, Dense"
        );
    }

    #[test]
    fn a_settings_interface_size_click_is_saved_and_sets_the_zoom() {
        let folders = Folders::new();
        folders.install_game_data();
        folders.set_last_check(990_000);
        let mut harness = start(Fake::new(Latest::Body(latest_body(BUILD))), &folders);
        until(&mut harness, "the build pages", build_page_shown);
        harness.key_press_modifiers(Modifiers::CTRL, Key::Comma);
        until(&mut harness, "the settings page", |h| {
            shown(h, "Interface size")
        });

        harness.get_by_label("175%").click();
        settle(&mut harness);
        assert!(
            (zoom(&harness) - 1.75).abs() < 0.001,
            "the click zooms to 175%"
        );
        assert_eq!(saved_size(&folders), InterfaceSize::P175);
        assert!(selected(&harness, "175%"), "175% is the selected one");
    }

    #[test]
    fn the_settings_page_saves_150_percent_and_copies_the_data_folder_path() {
        let folders = Folders::new();
        folders.install_game_data();
        folders.set_last_check(990_000);
        let mut harness = start(Fake::new(Latest::Body(latest_body(BUILD))), &folders);
        until(&mut harness, "the build pages", build_page_shown);
        harness.key_press_modifiers(Modifiers::CTRL, Key::Comma);
        until(&mut harness, "the settings page", |h| {
            shown(h, "Interface size")
        });

        harness.get_by_label("150%").click();
        settle(&mut harness);
        assert!(
            (zoom(&harness) - 1.5).abs() < 0.001,
            "the click zooms to 150%"
        );
        assert_eq!(saved_size(&folders), InterfaceSize::P150);

        // The Display panel is scrollable and Copy path sits at its bottom.
        let at = harness.get_by_label("Interface size").rect().center();
        scroll_display(&mut harness, at, -400.0);
        harness.get_by_label("Copy path").click();
        harness.run_steps(1);
        assert_eq!(
            copied_text(&harness),
            Some(folders.data.display().to_string()),
            "Copy path copies the data folder"
        );
    }

    #[test]
    fn a_saved_interface_size_is_the_selected_settings_choice() {
        let folders = Folders::new();
        folders.install_game_data();
        folders.set_last_check(990_000);
        folders.set_interface_size(InterfaceSize::P125);
        let mut harness = start(Fake::new(Latest::Body(latest_body(BUILD))), &folders);
        until(&mut harness, "the build pages", build_page_shown);
        harness.key_press_modifiers(Modifiers::CTRL, Key::Comma);
        until(&mut harness, "the settings page", |h| {
            shown(h, "Interface size")
        });

        assert!(selected(&harness, "125%"), "125% is the selected one");
        assert!(!selected(&harness, "100%"));
    }

    #[test]
    fn the_settings_page_shows_the_rail_choices_below_the_density() {
        let folders = Folders::new();
        folders.install_game_data();
        folders.set_last_check(990_000);
        let mut harness = start(Fake::new(Latest::Body(latest_body(BUILD))), &folders);
        until(&mut harness, "the build pages", build_page_shown);
        harness.key_press_modifiers(Modifiers::CTRL, Key::Comma);
        until(&mut harness, "the settings page", |h| {
            shown(h, "Interface size")
        });

        assert!(shown(&harness, "Rail"), "Rail is listed");
        assert!(
            shown(&harness, "Icons and labels"),
            "the labelled choice is listed"
        );
        assert!(
            shown(&harness, "Icons only"),
            "the icons-only choice is listed"
        );
        assert!(
            selected(&harness, "Icons and labels"),
            "a new data folder shows icons and labels"
        );
        assert!(!selected(&harness, "Icons only"));
        assert!(
            top_of(&harness, "Density") < top_of(&harness, "Rail"),
            "the rail choice is below the density"
        );
    }

    #[test]
    fn a_rail_choice_is_saved_at_once_and_resizes_the_build_entry() {
        let folders = Folders::new();
        folders.install_game_data();
        folders.set_last_check(990_000);
        let mut harness = start(Fake::new(Latest::Body(latest_body(BUILD))), &folders);
        until(&mut harness, "the build pages", build_page_shown);
        harness.key_press_modifiers(Modifiers::CTRL, Key::Comma);
        until(&mut harness, "the settings page", |h| {
            shown(h, "Interface size")
        });
        assert!(harness.get_by_label("Build").rect().width() > 48.0);

        let at = harness.get_by_label("Interface size").rect().center();
        scroll_display(&mut harness, at, -250.0);
        harness.get_by_label("Icons only").click();
        settle(&mut harness);
        assert!(
            saved_rail_icons_only(&folders),
            "the choice is saved at once"
        );
        assert!(harness.state_mut().shared_mut().rail_icons_only);
        assert!(
            harness.get_by_label("Build").rect().width() <= 48.0,
            "the icons-only Build entry is at most 48 px wide"
        );

        let at = harness.get_by_label("Copy path").rect().center();
        scroll_display(&mut harness, at, 250.0);
        harness.get_by_label("Icons and labels").click_accesskit();
        settle(&mut harness);
        assert!(
            !saved_rail_icons_only(&folders),
            "the other choice is saved at once"
        );
        assert!(!harness.state_mut().shared_mut().rail_icons_only);
        assert!(
            harness.get_by_label("Build").rect().width() > 48.0,
            "the labelled Build entry is wider again"
        );
    }

    #[test]
    fn the_keep_refreshing_box_saves_a_1_and_a_0_and_a_new_app_reads_them() {
        let folders = Folders::new();
        folders.install_game_data();
        folders.set_last_check(990_000);
        let mut harness = start(Fake::new(Latest::Body(latest_body(BUILD))), &folders);
        until(&mut harness, "the build pages", build_page_shown);
        harness.key_press_modifiers(Modifiers::CTRL, Key::Comma);
        until(&mut harness, "the settings page", |h| {
            shown(h, "Interface size")
        });

        assert!(shown(&harness, "Refresh"), "the refresh section is listed");
        assert!(
            shown(&harness, "Keep refreshing while open"),
            "the keep-refreshing choice is listed"
        );
        assert!(
            !ticked(&harness, "Keep refreshing while open"),
            "a fresh start has the choice off"
        );
        assert_eq!(saved_refresh_while_open(&folders).as_deref(), None);

        click_checkbox(&mut harness, "Keep refreshing while open");
        settle(&mut harness);
        assert!(ticked(&harness, "Keep refreshing while open"));
        assert!(harness.state_mut().shared_mut().refresh_while_open);
        assert_eq!(saved_refresh_while_open(&folders).as_deref(), Some("1"));

        // A new app on the same files reads the saved 1 as on.
        drop(harness);
        let mut reopened = start(Fake::new(Latest::Status(503)), &folders);
        until(&mut reopened, "the build pages", build_page_shown);
        assert!(
            reopened.state_mut().shared_mut().refresh_while_open,
            "the saved 1 reads as on"
        );
        drop(reopened);

        // The box unticks to a saved 0.
        let mut harness = start(Fake::new(Latest::Status(503)), &folders);
        until(&mut harness, "the build pages", build_page_shown);
        harness.key_press_modifiers(Modifiers::CTRL, Key::Comma);
        until(&mut harness, "the settings page", |h| {
            shown(h, "Interface size")
        });
        assert!(ticked(&harness, "Keep refreshing while open"));
        click_checkbox(&mut harness, "Keep refreshing while open");
        settle(&mut harness);
        assert!(!ticked(&harness, "Keep refreshing while open"));
        assert!(!harness.state_mut().shared_mut().refresh_while_open);
        assert_eq!(saved_refresh_while_open(&folders).as_deref(), Some("0"));
        drop(harness);

        // A fresh data folder with no key at all reads as off.
        let fresh = Folders::new();
        fresh.install_game_data();
        fresh.set_last_check(990_000);
        let mut harness = start(Fake::new(Latest::Status(503)), &fresh);
        assert!(!harness.state_mut().shared_mut().refresh_while_open);
    }

    #[test]
    fn with_the_keep_refreshing_choice_off_no_price_is_asked_again() {
        let folders = Folders::new();
        folders.install_game_data();
        folders.set_last_check(990_000);
        let clock = Arc::new(AtomicI64::new(NOW));
        let mut harness = start_with_clock(
            Fake::new(Latest::Body(latest_body(BUILD))),
            &folders,
            Arc::clone(&clock),
        );
        until(&mut harness, "the build pages", build_page_shown);

        assert_eq!(
            wait_for_prices(&mut harness, 1),
            vec![false],
            "the start-up ask is quiet"
        );
        prices_settled(&mut harness);
        assert!(
            take_prices(&mut harness).is_empty(),
            "the start-up ask gives one answer"
        );

        clock.store(NOW + 3_600, Ordering::SeqCst);
        settle(&mut harness);
        assert!(
            take_prices(&mut harness).is_empty(),
            "twenty frames an hour on ask for nothing"
        );
    }

    #[test]
    fn with_the_keep_refreshing_choice_on_the_prices_are_asked_again_every_fifteen_minutes() {
        let folders = Folders::new();
        folders.install_game_data();
        folders.set_last_check(990_000);
        folders.set_refresh_while_open("1");
        let clock = Arc::new(AtomicI64::new(NOW));
        let mut harness = start_with_clock(
            Fake::new(Latest::Body(latest_body(BUILD))),
            &folders,
            Arc::clone(&clock),
        );
        until(&mut harness, "the build pages", build_page_shown);

        assert_eq!(
            wait_for_prices(&mut harness, 1),
            vec![false],
            "the start-up ask is quiet"
        );
        prices_settled(&mut harness);
        assert!(
            take_prices(&mut harness).is_empty(),
            "exactly one answer at start"
        );

        clock.store(NOW + 899, Ordering::SeqCst);
        settle(&mut harness);
        assert!(
            take_prices(&mut harness).is_empty(),
            "at +899 the gap has not run out"
        );

        clock.store(NOW + 900, Ordering::SeqCst);
        assert_eq!(
            wait_for_prices(&mut harness, 1),
            vec![false],
            "at +900 one quiet answer"
        );
        prices_settled(&mut harness);
        settle(&mut harness);
        assert!(
            take_prices(&mut harness).is_empty(),
            "at +900 one answer, not two"
        );

        clock.store(NOW + 1_800, Ordering::SeqCst);
        assert_eq!(
            wait_for_prices(&mut harness, 1),
            vec![false],
            "at +1800 one quiet answer"
        );
        prices_settled(&mut harness);
        settle(&mut harness);
        assert!(
            take_prices(&mut harness).is_empty(),
            "at +1800 one answer, not two"
        );
    }

    #[test]
    fn turning_the_keep_refreshing_choice_on_asks_once_then_waits_its_gap() {
        let folders = Folders::new();
        folders.install_game_data();
        folders.set_last_check(990_000);
        let clock = Arc::new(AtomicI64::new(NOW));
        let mut harness = start_with_clock(
            Fake::new(Latest::Body(latest_body(BUILD))),
            &folders,
            Arc::clone(&clock),
        );
        until(&mut harness, "the build pages", build_page_shown);

        assert_eq!(
            wait_for_prices(&mut harness, 1),
            vec![false],
            "the start-up ask is quiet"
        );
        prices_settled(&mut harness);
        assert!(
            take_prices(&mut harness).is_empty(),
            "the start-up ask gives one answer"
        );

        // The choice was off at start, so nothing was timed: ticking it makes the next frame due.
        harness.key_press_modifiers(Modifiers::CTRL, Key::Comma);
        until(&mut harness, "the settings page", |h| {
            shown(h, "Interface size")
        });
        click_checkbox(&mut harness, "Keep refreshing while open");
        settle(&mut harness);
        assert!(
            harness.state_mut().shared_mut().refresh_while_open,
            "the choice is on"
        );

        assert_eq!(
            wait_for_prices(&mut harness, 1),
            vec![false],
            "the frame after the tick asks once, quietly"
        );
        prices_settled(&mut harness);
        settle(&mut harness);
        assert!(
            take_prices(&mut harness).is_empty(),
            "the rest of the minute asks nothing"
        );

        clock.store(NOW + 899, Ordering::SeqCst);
        settle(&mut harness);
        assert!(
            take_prices(&mut harness).is_empty(),
            "the gap from the tick has not run out"
        );

        clock.store(NOW + 900, Ordering::SeqCst);
        assert_eq!(
            wait_for_prices(&mut harness, 1),
            vec![false],
            "the gap from the tick runs out at +900"
        );
    }

    #[test]
    fn turning_the_keep_refreshing_choice_off_and_on_again_asks_at_once() {
        let folders = Folders::new();
        folders.install_game_data();
        folders.set_last_check(990_000);
        folders.set_refresh_while_open("1");
        let clock = Arc::new(AtomicI64::new(NOW));
        let mut harness = start_with_clock(
            Fake::new(Latest::Body(latest_body(BUILD))),
            &folders,
            Arc::clone(&clock),
        );
        until(&mut harness, "the build pages", build_page_shown);

        assert_eq!(
            wait_for_prices(&mut harness, 1),
            vec![false],
            "the start-up ask is quiet"
        );
        prices_settled(&mut harness);
        assert!(
            take_prices(&mut harness).is_empty(),
            "the start-up ask gives one answer"
        );

        // Off while the start-up gap still has time left: nothing is asked, and the time the
        // gap had left is dropped with the choice.
        clock.store(NOW + 120, Ordering::SeqCst);
        harness.key_press_modifiers(Modifiers::CTRL, Key::Comma);
        until(&mut harness, "the settings page", |h| {
            shown(h, "Interface size")
        });
        click_checkbox(&mut harness, "Keep refreshing while open");
        settle(&mut harness);
        assert!(
            !harness.state_mut().shared_mut().refresh_while_open,
            "the choice is off"
        );
        assert_eq!(saved_refresh_while_open(&folders).as_deref(), Some("0"));
        settle(&mut harness);
        assert!(
            take_prices(&mut harness).is_empty(),
            "the choice off asks for nothing"
        );

        // Back on before the gap's old time (NOW + 900) has come: the next frame asks, it does
        // not wait out what the choice had left when it went off.
        clock.store(NOW + 600, Ordering::SeqCst);
        click_checkbox(&mut harness, "Keep refreshing while open");
        settle(&mut harness);
        assert!(
            harness.state_mut().shared_mut().refresh_while_open,
            "the choice is on again"
        );
        assert_eq!(saved_refresh_while_open(&folders).as_deref(), Some("1"));

        assert_eq!(
            wait_for_prices(&mut harness, 1),
            vec![false],
            "the frame after the choice comes back on asks at once, quietly"
        );
        prices_settled(&mut harness);
        settle(&mut harness);
        assert!(
            take_prices(&mut harness).is_empty(),
            "the ask comes once, not on every frame"
        );
    }

    #[test]
    fn ctrl_plus_minus_and_zero_change_and_save_the_interface_size() {
        let folders = Folders::new();
        folders.install_game_data();
        folders.set_last_check(990_000);
        let mut harness = start(Fake::new(Latest::Body(latest_body(BUILD))), &folders);
        until(&mut harness, "the build pages", build_page_shown);
        assert!(
            (zoom(&harness) - 1.0).abs() < 0.001,
            "a new folder starts at 100%"
        );

        harness.key_press_modifiers(Modifiers::CTRL, Key::Equals);
        settle(&mut harness);
        assert!(
            (zoom(&harness) - 1.25).abs() < 0.001,
            "Ctrl + goes one size up"
        );
        assert_eq!(saved_size(&folders), InterfaceSize::P125);

        harness.key_press_modifiers(Modifiers::CTRL, Key::Plus);
        settle(&mut harness);
        assert!((zoom(&harness) - 1.5).abs() < 0.001, "Ctrl + goes on up");
        assert_eq!(saved_size(&folders), InterfaceSize::P150);

        harness.key_press_modifiers(Modifiers::CTRL, Key::Minus);
        settle(&mut harness);
        assert!(
            (zoom(&harness) - 1.25).abs() < 0.001,
            "Ctrl − goes one size down"
        );
        assert_eq!(saved_size(&folders), InterfaceSize::P125);

        harness.key_press_modifiers(Modifiers::CTRL, Key::Num0);
        settle(&mut harness);
        assert!(
            (zoom(&harness) - 1.0).abs() < 0.001,
            "Ctrl 0 goes back to 100%"
        );
        assert_eq!(saved_size(&folders), InterfaceSize::P100);
    }

    #[test]
    fn the_interface_size_stops_at_each_end_without_a_notice() {
        for (size, key) in [
            (InterfaceSize::P200, Key::Plus),
            (InterfaceSize::P100, Key::Minus),
        ] {
            let folders = Folders::new();
            folders.install_game_data();
            folders.set_last_check(990_000);
            folders.set_interface_size(size);
            let mut harness = start(Fake::new(Latest::Body(latest_body(BUILD))), &folders);
            until(&mut harness, "the build pages", build_page_shown);
            assert!((zoom(&harness) - (f32::from(size.percent()) / 100.0)).abs() < 0.001);

            let notices = harness.state_mut().shared_mut().notices.len();
            harness.key_press_modifiers(Modifiers::CTRL, key);
            settle(&mut harness);
            assert!((zoom(&harness) - (f32::from(size.percent()) / 100.0)).abs() < 0.001);
            assert_eq!(harness.state_mut().shared_mut().notices.len(), notices);
            assert_eq!(saved_size(&folders), size);
        }
    }

    #[test]
    fn a_saved_interface_size_sets_the_zoom_at_start() {
        let folders = Folders::new();
        folders.install_game_data();
        folders.set_last_check(990_000);
        folders.set_interface_size(InterfaceSize::P150);
        let mut harness = start(Fake::new(Latest::Body(latest_body(BUILD))), &folders);
        until(&mut harness, "the build pages", build_page_shown);
        assert!(
            (zoom(&harness) - 1.5).abs() < 0.001,
            "the app starts at 150%"
        );
        assert!(!harness.ctx.options(|options| options.zoom_with_keyboard));
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
        until(&mut harness, "the welcome", |h| {
            shown(h, "Welcome to EVE Means of Profit")
        });
        finish_first_run(&mut harness);
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
        until(&mut harness, "the welcome", |h| {
            shown(h, "Welcome to EVE Means of Profit")
        });
        harness.snapshot("shell-first-run-welcome");
        finish_first_run(&mut harness);
        until(&mut harness, "the build pages", build_page_shown);
        harness.remove_cursor();
        harness.run();
        harness.snapshot("shell-build-empty");
        harness.get_by_label(Page::Setup.label()).click();
        until(&mut harness, "the setup panels", |h| shown(h, "Price rule"));
        harness.remove_cursor();
        harness.run();
        crate::ui::picture(&mut harness, "shell-setup-empty");
        harness.get_by_label("Build").click();
        until(&mut harness, "the build pages again", build_page_shown);
        harness.key_press_modifiers(Modifiers::CTRL, Key::Comma);
        until(&mut harness, "the settings page", |h| {
            shown(h, "Facilities") && shown(h, "Price history") && shown(h, "Display")
        });
        settle(&mut harness);
        assert_eq!(
            harness
                .query_all_by_role_and_label(egui::accesskit::Role::Window, "Settings")
                .count(),
            0,
            "the Settings page stands in for the window"
        );
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
        until(&mut harness, "the welcome", |h| {
            shown(h, "Welcome to EVE Means of Profit")
        });
        finish_first_run(&mut harness);
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

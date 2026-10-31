#![cfg(test)]

//! Acceptance tests for the app shell. The network is a fake that serves the build 3607705
//! fixture as a zip, built in the test. The clock is fixed at 1,000,000.
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

mod first_run;
mod notices;
mod refresh;
mod settings;
mod snapshots;
mod theme;
mod zoom;

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
fn start_with_clock(fake: Fake, folders: &Folders, clock: Arc<AtomicI64>) -> Harness<'static, App> {
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
fn until(harness: &mut Harness<'_, App>, what: &str, check: impl Fn(&Harness<'_, App>) -> bool) {
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

/// The keep-refreshing choice as the test's data folder holds it, `None` when never saved.
fn saved_refresh_while_open(folders: &Folders) -> Option<String> {
    let store = Store::open(&folders.user_db(), NOW).unwrap().store;
    store.setting("refresh_while_open").unwrap()
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

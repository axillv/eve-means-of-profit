//! The app: start-up, the results the worker sends back, and one frame of the window.
//!
//! [`App::new`] opens the two databases, then starts the first-run download or the daily check.
//! Each frame, [`App::show`] takes the results the worker has sent, then draws the menu bar, the
//! status line, and either the first-run screen or the rail and the page.

pub mod build;
pub mod dock;
pub mod first_run;
pub mod layout;
pub mod rail;
pub mod settings_window;
pub mod setup;
pub mod status;
pub mod table;
pub mod theme;
pub mod worker;

use std::error::Error;
use std::fs;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};

use egui::{Context, Key, Modifiers, Ui, ViewportCommand};
use egui_dock::DockState;
use profit_sde::{
    Fetch, FetchError, ImportReport, Sde, SdeBuild, SdeError, download_and_import, latest_build,
    needs_update, update_due,
};
use profit_store::{Density, Settings, Store};

use build::{BuildPage, BuildPanel};
use first_run::{Reply, Startup};
use rail::Page;
use setup::{SetupPage, SetupPanel};
use status::{Action, Notice};
use theme::Style;
use worker::{Job, Worker, lock};

const USER_DB: &str = "user.sqlite";
const SDE_DB: &str = "sde.sqlite";
/// Unix seconds, from the system or a fixed value in tests.
pub type Clock = Box<dyn Fn() -> i64 + Send>;

/// Where the app keeps its files, and the network and the clock it uses.
pub struct AppConfig {
    /// Holds `sde.sqlite` (the game data) and `user.sqlite` (the user's own data).
    pub data_dir: PathBuf,
    /// Holds the download while it is checked and imported.
    pub cache_dir: PathBuf,
    pub fetch: Box<dyn Fetch + Send>,
    pub clock: Clock,
}

/// What a job reports back to the UI thread.
pub enum Event {
    /// The latest-build check finished. `first_run` is true when no game data exists yet.
    Checked {
        first_run: bool,
        result: Result<SdeBuild, FetchError>,
    },
    /// Bytes of a download so far, and the total when the server announced it.
    Progress {
        update: bool,
        done: u64,
        total: Option<u64>,
    },
    /// A download, check and import finished, or failed.
    Finished {
        update: bool,
        build: u64,
        result: Result<ImportReport, FetchError>,
    },
}

/// State the pages and the windows share.
pub struct Shared {
    /// The game data, `None` until the first download has been imported.
    pub sde: Option<Sde>,
    pub store: Store,
    pub worker: Worker,
    /// The Forge prices the Build page reads, shared with the Items and the Orders pages.
    pub market: Arc<build::prices::Prices>,
    pub style: Style,
    pub density: Density,
    pub notices: Vec<Notice>,
    pub clock: Clock,
}

impl Shared {
    /// Changes the saved settings. Returns whether the change was saved; a save that fails
    /// becomes an error notice.
    pub fn change_settings(&mut self, change: impl FnOnce(&mut Settings)) -> bool {
        let saved = self.store.settings().and_then(|mut settings| {
            change(&mut settings);
            self.store.save_settings(&settings)
        });
        match saved {
            Ok(()) => true,
            Err(error) => {
                self.notices.push(Notice::error(format!(
                    "Could not save the settings: {error}"
                )));
                false
            }
        }
    }
}

pub struct App {
    ctx: Context,
    shared: Shared,
    data_dir: PathBuf,
    sde_path: PathBuf,
    cache_dir: PathBuf,
    startup: Startup,
    /// Set by the Cancel button; the download stops at its next chunk.
    cancel: Arc<AtomicBool>,
    /// The build of the game data in use.
    installed: Option<u64>,
    /// Progress of a background update: bytes done and the total when known.
    update: Option<(u64, Option<u64>)>,
    events: Receiver<Event>,
    events_tx: Sender<Event>,
    page: Page,
    build_page: BuildPage,
    setup_page: SetupPage,
    build_dock: DockState<BuildPanel>,
    setup_dock: DockState<SetupPanel>,
    settings_open: bool,
}

impl App {
    /// Opens the databases and starts the first-run download, or the daily check when the game
    /// data is due for one. Fails only when a database cannot be opened or read.
    pub fn new(ctx: &Context, config: AppConfig) -> Result<Self, Box<dyn Error + Send + Sync>> {
        fs::create_dir_all(&config.data_dir)?;
        fs::create_dir_all(&config.cache_dir)?;
        let now = (config.clock)();
        let opened = Store::open(&config.data_dir.join(USER_DB), now)?;
        let mut notices = Vec::new();
        if let Some(old) = &opened.started_fresh {
            notices.push(Notice::info(format!(
                "Your saved data came from an earlier test build and was set aside as {}. Starting fresh.",
                old.display()
            )));
        }
        let mut store = opened.store;
        let settings = store.settings()?;
        let sde_path = config.data_dir.join(SDE_DB);
        let sde = open_sde(&sde_path)?;
        let installed = sde
            .as_ref()
            .map(Sde::build)
            .transpose()?
            .map(|build| build.number);
        let style = Style::eve_dense();
        style.apply(ctx);
        let (events_tx, events) = mpsc::channel();
        let mut app = Self {
            ctx: ctx.clone(),
            shared: Shared {
                sde,
                store,
                worker: Worker::spawn(ctx.clone(), config.fetch),
                market: Arc::new(build::prices::Prices::new(Box::new(
                    profit_market::http::UreqHttp::new(),
                ))),
                style,
                density: settings.density,
                notices,
                clock: config.clock,
            },
            data_dir: config.data_dir,
            sde_path,
            cache_dir: config.cache_dir,
            startup: Startup::Ready,
            cancel: Arc::new(AtomicBool::new(false)),
            installed,
            update: None,
            events,
            events_tx,
            page: Page::Build,
            build_page: BuildPage::default(),
            setup_page: SetupPage,
            build_dock: dock::build_layout(),
            setup_dock: dock::setup_layout(),
            settings_open: false,
        };
        if app.shared.sde.is_none() {
            app.start_first_run();
        } else if update_due(settings.last_sde_check, now) {
            app.spawn_check(false);
        }
        Ok(app)
    }

    /// The state the pages and windows share, for code that drives the app from outside.
    pub fn shared_mut(&mut self) -> &mut Shared {
        &mut self.shared
    }

    /// One frame: take the worker's results, then draw.
    pub fn show(&mut self, ui: &mut Ui) {
        self.drain_events();
        if ui.input_mut(|input| input.consume_key(Modifiers::CTRL, Key::Comma)) {
            self.settings_open = true;
        }
        self.menu_bar(ui);
        let activity = self.activity();
        let idle = self.idle_text();
        let clicked = status::show(
            ui,
            &self.shared.style,
            &mut self.shared.notices,
            activity.as_deref(),
            &idle,
        );
        if let Some(Action::Update { build }) = clicked {
            self.start_update(build);
        }
        match self.startup {
            Startup::Ready => self.pages(ui),
            Startup::Checking
            | Startup::Downloading { .. }
            | Startup::Importing { .. }
            | Startup::Cancelled
            | Startup::Failed(_) => self.first_run_screen(ui),
        }
        let ctx = ui.ctx().clone();
        settings_window::show(
            &ctx,
            &mut self.settings_open,
            &mut self.shared,
            &self.data_dir,
        );
    }

    fn menu_bar(&mut self, ui: &mut Ui) {
        egui::Panel::top("menu").show(ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("Quit").clicked() {
                        ui.ctx().send_viewport_cmd(ViewportCommand::Close);
                        ui.close();
                    }
                });
                ui.menu_button("View", |ui| {
                    if ui.button("Settings…").clicked() {
                        self.settings_open = true;
                        ui.close();
                    }
                });
            });
        });
    }

    fn pages(&mut self, ui: &mut Ui) {
        let style = self.shared.style;
        rail::show(ui, &style, &mut self.page);
        egui::CentralPanel::default().show(ui, |ui| match self.page {
            Page::Build => dock::show_locked(
                ui,
                &style,
                &mut self.build_dock,
                BuildPanel::title,
                |ui, panel| build::show(&mut self.build_page, panel, ui, &mut self.shared),
            ),
            Page::Setup => dock::show_locked(
                ui,
                &style,
                &mut self.setup_dock,
                SetupPanel::title,
                |ui, panel| setup::show(&mut self.setup_page, panel, ui, &mut self.shared),
            ),
        });
    }

    fn first_run_screen(&mut self, ui: &mut Ui) {
        let style = self.shared.style;
        let reply = egui::CentralPanel::default()
            .show(ui, |ui| first_run::show(ui, &style, &self.startup))
            .inner;
        match reply {
            Some(Reply::Cancel) => self.cancel.store(true, Ordering::Relaxed),
            Some(Reply::TryAgain) => self.start_first_run(),
            None => {}
        }
    }

    /// The activity text shown in the status line while a background update runs.
    fn activity(&self) -> Option<String> {
        self.update.map(|(done, total)| {
            format!(
                "Updating game data: {}",
                first_run::progress_text(done, total)
            )
        })
    }

    /// The idle text of the status line: the age of the oldest market price held, when there is one.
    fn idle_text(&self) -> String {
        match self.shared.market.published_at() {
            Some(published) => {
                let now = (self.shared.clock)();
                format!(
                    "Market data: oldest {}",
                    build::prices::age_text(now.saturating_sub(published))
                )
            }
            None => status::IDLE_TEXT.to_owned(),
        }
    }

    fn drain_events(&mut self) {
        while let Ok(event) = self.events.try_recv() {
            self.apply(event);
        }
    }

    fn apply(&mut self, event: Event) {
        match event {
            Event::Checked { first_run, result } => self.checked(first_run, result),
            Event::Progress {
                update,
                done,
                total,
            } => self.progress(update, done, total),
            Event::Finished {
                update,
                build,
                result,
            } => self.finished(update, build, result),
        }
    }

    fn checked(&mut self, first_run: bool, result: Result<SdeBuild, FetchError>) {
        match result {
            Ok(latest) => {
                self.record_check();
                if first_run {
                    self.startup = Startup::Downloading {
                        build: latest.number,
                        done: 0,
                        total: None,
                    };
                    self.spawn_download(latest.number, false, Arc::clone(&self.cancel));
                } else if needs_update(self.installed, latest.number) {
                    self.shared.notices.push(
                        Notice::info(format!("Game data build {} is available.", latest.number))
                            .with_action(Action::Update {
                                build: latest.number,
                            }),
                    );
                }
            }
            Err(error) if first_run => {
                self.fail(format!("Could not get the game data: {error}"));
            }
            Err(error) => {
                self.shared.notices.push(Notice::error(format!(
                    "Could not check for new game data: {error}"
                )));
            }
        }
    }

    /// Saves the time of the check, so the next one waits a day.
    fn record_check(&mut self) {
        let now = (self.shared.clock)();
        self.shared
            .change_settings(|settings| settings.last_sde_check = Some(now));
    }

    fn progress(&mut self, update: bool, done: u64, total: Option<u64>) {
        if update {
            self.update = Some((done, total));
        } else if let Startup::Downloading { build, .. } = self.startup {
            self.startup = if total.is_some_and(|total| done >= total) {
                Startup::Importing { build }
            } else {
                Startup::Downloading { build, done, total }
            };
        }
    }

    fn finished(&mut self, update: bool, build: u64, result: Result<ImportReport, FetchError>) {
        if update {
            self.update = None;
        }
        match result {
            Ok(_) => match Sde::open(&self.sde_path) {
                Ok(sde) => {
                    self.shared.sde = Some(sde);
                    self.installed = Some(build);
                    if update {
                        self.shared
                            .notices
                            .push(Notice::info(format!("Game data updated to build {build}.")));
                    } else {
                        self.startup = Startup::Ready;
                    }
                }
                Err(error) if update => {
                    self.shared.notices.push(Notice::error(format!(
                        "Could not open the game data: {error}"
                    )));
                }
                Err(error) => {
                    self.fail(format!("Could not open the game data: {error}"));
                }
            },
            Err(FetchError::Cancelled) if !update => self.startup = Startup::Cancelled,
            Err(error) if update => {
                self.shared.notices.push(Notice::error(format!(
                    "Could not update the game data: {error}"
                )));
            }
            Err(error) => self.fail(format!("Could not get the game data: {error}")),
        }
    }

    /// The first-run screen shows the error, and the status line shows it as an error too.
    fn fail(&mut self, text: String) {
        self.shared.notices.push(Notice::error(text.clone()));
        self.startup = Startup::Failed(text);
    }

    fn start_first_run(&mut self) {
        self.cancel = Arc::new(AtomicBool::new(false));
        self.startup = Startup::Checking;
        self.spawn_check(true);
    }

    fn start_update(&mut self, build: u64) {
        self.update = Some((0, None));
        self.spawn_download(build, true, Arc::new(AtomicBool::new(false)));
    }

    fn spawn_check(&self, first_run: bool) {
        let fetch = self.shared.worker.fetch();
        let (tx, ctx) = (self.events_tx.clone(), self.ctx.clone());
        let job: Job = Box::new(move || {
            let result = latest_build(&**lock(&fetch));
            notify(&tx, &ctx, Event::Checked { first_run, result });
        });
        self.shared.worker.run(job);
    }

    /// Downloads and imports `build`. The progress reports go to the UI; `cancel` stops the
    /// download when it is set.
    fn spawn_download(&self, build: u64, update: bool, cancel: Arc<AtomicBool>) {
        let fetch = self.shared.worker.fetch();
        let (tx, ctx) = (self.events_tx.clone(), self.ctx.clone());
        let (work, db) = (self.cache_dir.clone(), self.sde_path.clone());
        let job: Job = Box::new(move || {
            let guard = lock(&fetch);
            let mut progress = |done: u64, total: Option<u64>| {
                notify(
                    &tx,
                    &ctx,
                    Event::Progress {
                        update,
                        done,
                        total,
                    },
                );
                if cancel.load(Ordering::Relaxed) {
                    ControlFlow::Break(())
                } else {
                    ControlFlow::Continue(())
                }
            };
            let result = download_and_import(&**guard, build, &work, &db, &mut progress);
            notify(
                &tx,
                &ctx,
                Event::Finished {
                    update,
                    build,
                    result,
                },
            );
        });
        self.shared.worker.run(job);
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.show(ui);
    }
}

/// Sends a job's result to the UI and wakes the UI up to read it.
fn notify(tx: &Sender<Event>, ctx: &Context, event: Event) {
    // The receiver is gone only when the app is closing, and then nobody needs the event.
    drop(tx.send(event));
    ctx.request_repaint();
}

/// The game data, or `None` when none has been imported yet.
fn open_sde(path: &Path) -> Result<Option<Sde>, SdeError> {
    match Sde::open(path) {
        Ok(sde) => Ok(Some(sde)),
        Err(SdeError::NotImported) => Ok(None),
        Err(error) => Err(error),
    }
}

//! The app: start-up, the results the worker sends back, and one frame of the window.
//!
//! [`App::new`] opens the two databases, then starts the first-run download or the daily check.
//! Each frame, [`App::show`] takes the results the worker has sent, then draws the menu bar, the
//! status line, and either the first-run screen or the rail and the page.

pub mod assets;
pub mod build;
pub mod characters;
pub mod claims;
pub mod dock;
pub mod empty;
pub mod first_run;
pub mod history;
pub mod item_panel;
pub mod jobs;
pub mod layout;
pub mod net_worth;
pub mod orders;
pub mod rail;
pub mod selection;
pub mod session;
pub mod settings_window;
pub mod setup;
pub mod status;
pub mod stockpiles;
pub mod table;
pub mod theme;
pub mod today;
pub mod worker;

use std::collections::BTreeMap;
use std::error::Error;
use std::fs;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use egui::{Context, Key, Modifiers, Ui, ViewportCommand};
use egui_dock::DockState;
use mop_esi::character_data::Refreshed;
use mop_esi::error::EsiError;
use mop_esi::industry::{self as esi_industry, IndustryData, IndustryKind};
use mop_market::cache::Http;
use mop_sde::{
    Fetch, FetchError, ImportReport, Sde, SdeBuild, SdeError, download_and_import, latest_build,
    needs_update, update_due,
};
use mop_store::{Density, InterfaceSize, Settings, Store};

use assets::{AssetsPage, AssetsPanel};
use build::{BuildPage, BuildPanel};
use characters::{CharactersPage, CharactersPanel};
use claims::ClaimsState;
use first_run::{Reply, Startup};
use jobs::{JobsPage, JobsPanel};
use net_worth::{NetWorthPage, NetWorthPanel};
use orders::{OrdersPage, OrdersPanel};
use rail::Page;
use selection::Selection;
use session::{EveAccess, Loaded, Session};
use setup::{SetupPage, SetupPanel};
use status::{Action, Notice};
use stockpiles::{StockpilesPage, StockpilesPanel};
use theme::Style;
use worker::{Job, Worker, lock};

const USER_DB: &str = "user.sqlite";
const SDE_DB: &str = "sde.sqlite";
/// Seconds before a kind of industry data is fetched again after a fetch.
const INDUSTRY_RETRY_SECS: i64 = 60;
/// Seconds before a kind of industry data is fetched again after a failed fetch.
const INDUSTRY_ERROR_RETRY_SECS: i64 = 300;
/// The age a fetched kind of industry data is treated as fresh for, when ESI gave no expiry. The
/// Steps total line shows an older age in amber.
pub(crate) const INDUSTRY_AGE_SECS: i64 = 3_600;
/// Character data older than this (one day, in seconds) is worth a warning at start.
const STALE_DATA_SECS: i64 = 86_400;
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
    /// The EVE login and the files next to it: the real one in the app, a fake in tests.
    pub eve: Box<dyn EveAccess>,
    /// The price file's HTTP client: the real one in the app, a fake in tests.
    pub market: Box<dyn mop_market::fuzzwork::BytesHttp + Send + Sync>,
}

/// What a job reports back to the UI thread.
pub enum Event {
    /// The saved character data and jEve's profile, read at start.
    Loaded(Loaded),
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
    /// The saved industry data, read at start; the error text when it could not be read.
    IndustryLoaded(Result<IndustryData, String>),
    /// A refresh of one kind of industry data finished: the copy it worked on, its outcome, and
    /// whether the saved file could not be read before the refresh (so the refresh replaces it).
    Industry {
        kind: IndustryKind,
        data: IndustryData,
        result: Result<Refreshed, EsiError>,
        unreadable: bool,
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
    /// The client for the one global PLEX market, and the notices the price-copy jobs leave for
    /// the next frame, which `net_worth::record::tick` moves into `notices`.
    pub plex_http: Arc<dyn Http + Send + Sync>,
    pub copy_notices: Arc<Mutex<Vec<Notice>>>,
    pub style: Style,
    pub density: Density,
    /// Whether the rail shows icons alone, saved by an earlier run.
    pub rail_icons_only: bool,
    /// The data folder, shown on the Settings page.
    pub data_dir: PathBuf,
    pub notices: Vec<Notice>,
    pub clock: Clock,
    /// The characters and their saved data, jEve's profile, and the structure names.
    pub session: Session,
    /// The item, place and character picked on the pages. Not saved.
    pub selection: Selection,
    /// The EVE industry data the Steps table reads: cost indices and adjusted prices.
    pub industry: IndustryData,
    /// The live plan reservations and the running jobs, read by every page.
    pub claims: ClaimsState,
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

    /// Saves the hidden columns of the table named `table` under `columns.<table>`. An empty list
    /// clears the key. Returns whether the change was saved; a save that fails becomes an error
    /// notice.
    pub fn save_hidden_columns(&mut self, table: &str, hidden: &[String]) -> bool {
        let key = format!("columns.{table}");
        let text = table::hidden_to_text(hidden);
        let saved = if text.is_empty() {
            self.store.clear_setting(&key)
        } else {
            self.store.set_setting(&key, &text)
        };
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

/// Saves the open panels of `page` under `panels.<page>`; `None` is no change and saves nothing. A
/// save that fails becomes an error notice.
fn save_panels(cx: &mut Shared, page: &str, text: Option<String>) {
    let Some(text) = text else {
        return;
    };
    if let Err(error) = cx.store.set_setting(&format!("panels.{page}"), &text) {
        cx.notices.push(Notice::error(format!(
            "Could not save the settings: {error}"
        )));
    }
}

/// The eight page docks, each shaped by its saved panel list.
struct Docks {
    build: DockState<BuildPanel>,
    assets: DockState<AssetsPanel>,
    net_worth: DockState<NetWorthPanel>,
    orders: DockState<OrdersPanel>,
    jobs: DockState<JobsPanel>,
    characters: DockState<CharactersPanel>,
    stockpiles: DockState<StockpilesPanel>,
    setup: DockState<SetupPanel>,
}

/// Builds every page's dock from its default layout and the panel list saved for it.
fn saved_docks(store: &Store) -> Result<Docks, mop_store::StoreError> {
    Ok(Docks {
        build: dock::with_saved(
            store,
            "build",
            dock::build_layout(),
            &dock::BUILD_PANELS,
            BuildPanel::title,
        )?,
        assets: dock::with_saved(
            store,
            "assets",
            dock::assets_layout(),
            &dock::ASSETS_PANELS,
            AssetsPanel::title,
        )?,
        net_worth: dock::with_saved(
            store,
            "net_worth",
            dock::net_worth_layout(),
            &dock::NET_WORTH_PANELS,
            NetWorthPanel::title,
        )?,
        orders: dock::with_saved(
            store,
            "orders",
            dock::orders_layout(),
            &[OrdersPanel::Orders],
            OrdersPanel::title,
        )?,
        jobs: dock::with_saved(
            store,
            "jobs",
            dock::jobs_layout(),
            &[JobsPanel::IndustryJobs],
            JobsPanel::title,
        )?,
        characters: dock::with_saved(
            store,
            "characters",
            dock::characters_layout(),
            &dock::CHARACTERS_PANELS,
            CharactersPanel::title,
        )?,
        stockpiles: dock::with_saved(
            store,
            "stockpiles",
            dock::stockpiles_layout(),
            &dock::STOCKPILES_PANELS,
            StockpilesPanel::title,
        )?,
        setup: dock::with_saved(
            store,
            "setup",
            dock::setup_layout(),
            &dock::SETUP_PANELS,
            SetupPanel::title,
        )?,
    })
}

pub struct App {
    ctx: Context,
    shared: Shared,
    sde_path: PathBuf,
    cache_dir: PathBuf,
    startup: Startup,
    /// Set by the Cancel button; the download stops at its next chunk.
    cancel: Arc<AtomicBool>,
    /// The build of the game data in use.
    installed: Option<u64>,
    /// Progress of a background update: bytes done and the total when known.
    update: Option<(u64, Option<u64>)>,
    /// Saves the readings of the downloads; the Setup panel reads the history through the shared store.
    recorder: Arc<history::Recorder>,
    /// When the background refresh last checked, in seconds.
    history_checked: Option<i64>,
    events: Receiver<Event>,
    events_tx: Sender<Event>,
    page: Page,
    build_page: BuildPage,
    build_dock: DockState<BuildPanel>,
    assets_page: AssetsPage,
    assets_dock: DockState<AssetsPanel>,
    net_worth_page: NetWorthPage,
    net_worth_dock: DockState<NetWorthPanel>,
    net_worth_seen: BTreeMap<u64, u64>,
    net_worth_started: bool,
    orders_page: OrdersPage,
    orders_dock: DockState<OrdersPanel>,
    jobs_page: JobsPage,
    jobs_dock: DockState<JobsPanel>,
    characters_page: CharactersPage,
    characters_dock: DockState<CharactersPanel>,
    stockpiles_page: StockpilesPage,
    stockpiles_dock: DockState<StockpilesPanel>,
    setup_page: SetupPage,
    setup_dock: DockState<SetupPanel>,
    /// The saved interface size, shown as the zoom factor.
    interface_size: InterfaceSize,
    /// Whether the saved industry data has been read.
    industry_loaded: bool,
    /// Per kind of industry data: whether a refresh is running and when it may run again.
    industry_timing: IndustryTimings,
}

/// When one kind of industry data may be fetched again, and whether a request is running.
#[derive(Debug, Default, Clone, Copy)]
struct IndustryTiming {
    busy: bool,
    next: i64,
}

#[derive(Debug, Default)]
struct IndustryTimings {
    systems: IndustryTiming,
    prices: IndustryTiming,
}

impl IndustryTimings {
    fn get_mut(&mut self, kind: IndustryKind) -> &mut IndustryTiming {
        match kind {
            IndustryKind::Systems => &mut self.systems,
            IndustryKind::Prices => &mut self.prices,
        }
    }
}

impl App {
    /// Opens the databases and starts the first-run download, or the daily check when the game
    /// data is due for one. Fails only when a database cannot be opened or read.
    pub fn new(ctx: &Context, config: AppConfig) -> Result<Self, Box<dyn Error + Send + Sync>> {
        fs::create_dir_all(&config.data_dir)?;
        fs::create_dir_all(&config.cache_dir)?;
        let now = (config.clock)();
        let opened = Store::open(&config.data_dir.join(USER_DB), now)?;
        let notices = fresh_start_notice(opened.started_fresh.as_ref());
        let mut store = opened.store;
        let recorder = Arc::new(history::Recorder::open(
            &config.data_dir.join(USER_DB),
            now,
        )?);
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
        ctx.options_mut(|options| options.zoom_with_keyboard = false);
        ctx.set_zoom_factor(f32::from(settings.interface_size.percent()) / 100.0);
        let docks = saved_docks(&store)?;
        let (events_tx, events) = mpsc::channel();
        let mut app = Self {
            ctx: ctx.clone(),
            shared: Shared {
                sde,
                store,
                worker: Worker::spawn(ctx.clone(), config.fetch),
                market: Arc::new(
                    build::prices::Prices::new(config.market).recording(Arc::clone(&recorder)),
                ),
                plex_http: Arc::new(mop_market::http::UreqHttp::new()),
                copy_notices: Arc::new(Mutex::new(Vec::new())),
                style,
                density: settings.density,
                rail_icons_only: settings.rail_icons_only,
                data_dir: config.data_dir,
                notices,
                clock: config.clock,
                session: Session::new(config.eve),
                selection: Selection::default(),
                industry: IndustryData::default(),
                claims: ClaimsState::default(),
            },
            sde_path,
            cache_dir: config.cache_dir,
            startup: Startup::Ready,
            cancel: Arc::new(AtomicBool::new(false)),
            installed,
            update: None,
            recorder,
            history_checked: None,
            events,
            events_tx,
            page: Page::Today,
            build_page: BuildPage::default(),
            build_dock: docks.build,
            assets_page: AssetsPage,
            assets_dock: docks.assets,
            net_worth_page: NetWorthPage::default(),
            net_worth_dock: docks.net_worth,
            net_worth_seen: BTreeMap::new(),
            net_worth_started: false,
            orders_page: OrdersPage,
            orders_dock: docks.orders,
            jobs_page: JobsPage,
            jobs_dock: docks.jobs,
            characters_page: CharactersPage,
            characters_dock: docks.characters,
            stockpiles_page: StockpilesPage::default(),
            stockpiles_dock: docks.stockpiles,
            setup_page: SetupPage,
            setup_dock: docks.setup,
            interface_size: settings.interface_size,
            industry_loaded: false,
            industry_timing: IndustryTimings::default(),
        };
        app.spawn_session_load();
        app.spawn_industry_load();
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

    /// Makes `page` the page on show, for code that drives the app from outside.
    pub fn show_page(&mut self, page: Page) {
        self.page = page;
    }

    /// One frame: take the worker's results, then draw.
    pub fn show(&mut self, ui: &mut Ui) {
        self.drain_events();
        history::background_refresh(&mut self.shared, &mut self.history_checked);
        self.refresh_industry_when_due();
        net_worth::record::tick(
            &mut self.shared,
            &mut self.net_worth_seen,
            &mut self.net_worth_started,
        );
        for error in self.recorder.take_errors() {
            self.shared.notices.push(Notice::error(error));
        }
        if ui.input_mut(|input| input.consume_key(Modifiers::CTRL, Key::Comma)) {
            self.page = Page::Setup;
        }
        self.zoom_with_keys(ui);
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
        table::sync_sorts(&ui.ctx().clone(), &mut self.shared);
    }

    /// Steps the interface size with Ctrl +, Ctrl − and Ctrl 0, then saves it. At 200% Ctrl +
    /// and at 100% Ctrl − and Ctrl 0 do nothing: no notice and no save.
    fn zoom_with_keys(&mut self, ui: &mut Ui) {
        let size = if ui.input_mut(|input| {
            input.consume_key(Modifiers::CTRL, Key::Plus)
                || input.consume_key(Modifiers::CTRL, Key::Equals)
        }) {
            stepped_size(self.interface_size, 1)
        } else if ui.input_mut(|input| input.consume_key(Modifiers::CTRL, Key::Minus)) {
            stepped_size(self.interface_size, -1)
        } else if ui.input_mut(|input| input.consume_key(Modifiers::CTRL, Key::Num0)) {
            (self.interface_size != InterfaceSize::P100).then_some(InterfaceSize::P100)
        } else {
            None
        };
        let Some(size) = size else {
            return;
        };
        // The zoom stays for this run even when the save fails; the notice says it was not saved.
        self.interface_size = size;
        ui.ctx().set_zoom_factor(f32::from(size.percent()) / 100.0);
        self.shared
            .change_settings(|settings| settings.interface_size = size);
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
                        self.page = Page::Setup;
                        ui.close();
                    }
                });
            });
        });
    }

    fn pages(&mut self, ui: &mut Ui) {
        let style = self.shared.style;
        rail::show_with(ui, &style, &mut self.page, self.shared.rail_icons_only);
        egui::CentralPanel::default().show(ui, |ui| match self.page {
            Page::Today => {
                if let Some(page) = today::show(ui, &mut self.shared) {
                    self.page = page;
                }
            }
            Page::Build => {
                let text = dock::show_page(
                    ui,
                    &style,
                    "build",
                    &dock::BUILD_PANELS,
                    &mut self.build_dock,
                    BuildPanel::title,
                    |ui, panel| build::show(&mut self.build_page, panel, ui, &mut self.shared),
                );
                save_panels(&mut self.shared, "build", text);
            }
            Page::Assets => {
                let text = dock::show_page(
                    ui,
                    &style,
                    "assets",
                    &dock::ASSETS_PANELS,
                    &mut self.assets_dock,
                    AssetsPanel::title,
                    |ui, panel| assets::show(&mut self.assets_page, panel, ui, &mut self.shared),
                );
                save_panels(&mut self.shared, "assets", text);
            }
            Page::NetWorth => {
                let text = dock::show_page(
                    ui,
                    &style,
                    "net_worth",
                    &dock::NET_WORTH_PANELS,
                    &mut self.net_worth_dock,
                    NetWorthPanel::title,
                    |ui, panel| {
                        net_worth::show(&mut self.net_worth_page, panel, ui, &mut self.shared);
                    },
                );
                save_panels(&mut self.shared, "net_worth", text);
            }
            Page::Orders => dock::show_locked(
                ui,
                &style,
                &mut self.orders_dock,
                OrdersPanel::title,
                |ui, panel| orders::show(&mut self.orders_page, panel, ui, &mut self.shared),
            ),
            Page::Jobs => dock::show_locked(
                ui,
                &style,
                &mut self.jobs_dock,
                JobsPanel::title,
                |ui, panel| jobs::show(&mut self.jobs_page, panel, ui, &mut self.shared),
            ),
            Page::Characters => {
                let text = dock::show_page(
                    ui,
                    &style,
                    "characters",
                    &dock::CHARACTERS_PANELS,
                    &mut self.characters_dock,
                    CharactersPanel::title,
                    |ui, panel| {
                        characters::show(&mut self.characters_page, panel, ui, &mut self.shared);
                    },
                );
                save_panels(&mut self.shared, "characters", text);
            }
            Page::Stockpiles => self.show_stockpiles(ui, &style),
            Page::Setup => {
                let text = dock::show_page(
                    ui,
                    &style,
                    "setup",
                    &dock::SETUP_PANELS,
                    &mut self.setup_dock,
                    SetupPanel::title,
                    |ui, panel| setup::show(&mut self.setup_page, panel, ui, &mut self.shared),
                );
                save_panels(&mut self.shared, "setup", text);
            }
        });
        // The inspector floats over the page, so it is drawn after the panels that fill it.
        if matches!(self.page, Page::Build | Page::Assets | Page::Stockpiles) {
            item_panel::inspector(ui, &mut self.shared);
        }
    }

    fn show_stockpiles(&mut self, ui: &mut Ui, style: &Style) {
        let text = dock::show_page(
            ui,
            style,
            "stockpiles",
            &dock::STOCKPILES_PANELS,
            &mut self.stockpiles_dock,
            StockpilesPanel::title,
            |ui, panel| {
                self.stockpiles_page.show(panel, ui, &mut self.shared);
            },
        );
        save_panels(&mut self.shared, "stockpiles", text);
        self.stockpiles_page.show_windows(ui, &mut self.shared);
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
                    // A floor at 0 is meant here: a publish time in the future shows as an age of zero.
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
            Event::Loaded(loaded) => self.loaded(loaded),
            Event::Checked { first_run, result } => self.checked(first_run, result),
            Event::IndustryLoaded(result) => self.industry_loaded(result),
            Event::Industry {
                kind,
                data,
                result,
                unreadable,
            } => self.industry_refreshed(kind, &data, result, unreadable),
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

    /// Takes the start-up read: the login characters with their data, jEve's profile, and the
    /// typed names. Warns once when the oldest data is a day old or more. Fetches nothing.
    fn loaded(&mut self, loaded: Loaded) {
        let saved_ok = loaded.saved.is_ok();
        match loaded.saved {
            Ok(saved) => {
                self.shared.session.set_saved(saved);
                let now = (self.shared.clock)();
                if let Some(oldest) = self.shared.session.oldest_fetch() {
                    // A floor at 0 is meant here: a fetch time in the future (clock set back) gives a
                    // very small or zero age, which is the right answer, not an error.
                    let age = now.saturating_sub(oldest);
                    if age > STALE_DATA_SECS {
                        self.shared.notices.push(Notice::info(format!(
                            "Character data is {} old. Refresh it on the Characters page.",
                            build::prices::age_text(age)
                        )));
                    }
                }
            }
            Err(error) => {
                self.shared.session.clear();
                self.shared.notices.push(Notice::error(format!(
                    "Could not read the saved character data: {error}"
                )));
            }
        }
        match loaded.jeve {
            // With the saved data unreadable the pages show no characters at all, jEve's included.
            Ok(jeve) if saved_ok => self.shared.session.set_jeve(jeve),
            Ok(_) => {}
            Err(error) => self.shared.notices.push(Notice::error(format!(
                "Could not read jEve's data file: {error}"
            ))),
        }
        match self.shared.store.structure_names() {
            Ok(typed) => self.shared.session.set_typed_names(typed),
            Err(error) => self.shared.notices.push(Notice::error(format!(
                "Could not read the typed structure names: {error}"
            ))),
        }
        self.shared.session.mark_loaded();
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

    /// Reads the saved character data and jEve's profile on the worker.
    fn spawn_session_load(&self) {
        let eve = self.shared.session.eve();
        let (tx, ctx) = (self.events_tx.clone(), self.ctx.clone());
        let data_dir = self.shared.data_dir.clone();
        let job: Job = Box::new(move || {
            let loaded = session::load(&**session::lock_eve(&eve), &data_dir);
            notify(&tx, &ctx, Event::Loaded(loaded));
        });
        self.shared.worker.run(job);
    }

    /// Reads the saved industry data on the worker. Refreshes wait for it.
    fn spawn_industry_load(&self) {
        let (tx, ctx) = (self.events_tx.clone(), self.ctx.clone());
        let dir = self.shared.data_dir.join("esi");
        let job: Job = Box::new(move || {
            let loaded = esi_industry::load_industry(&dir).map_err(|error| error.to_string());
            notify(&tx, &ctx, Event::IndustryLoaded(loaded));
        });
        self.shared.worker.run(job);
    }

    /// Starts a refresh of the industry kinds that are due, while a plan is shown on the Build
    /// page. One refresh per kind at a time; nothing is requested before the saved data is read.
    fn refresh_industry_when_due(&mut self) {
        if !self.industry_loaded
            || !matches!(self.page, Page::Build)
            || self.build_page.record.is_none()
        {
            return;
        }
        let now = (self.shared.clock)();
        for kind in [IndustryKind::Systems, IndustryKind::Prices] {
            let timing = *self.industry_timing.get_mut(kind);
            if timing.busy || now < timing.next {
                continue;
            }
            let expiry = match kind {
                IndustryKind::Systems => self
                    .shared
                    .industry
                    .systems
                    .as_ref()
                    .map(|fetched| fetched.next_refresh(INDUSTRY_AGE_SECS)),
                IndustryKind::Prices => self
                    .shared
                    .industry
                    .prices
                    .as_ref()
                    .map(|fetched| fetched.next_refresh(INDUSTRY_AGE_SECS)),
            };
            let due = match expiry {
                None | Some(Err(_)) => true,
                Some(Ok(at)) => at <= now,
            };
            if due {
                self.spawn_industry_refresh(kind, now);
            }
        }
    }

    /// One job for one kind of industry data.
    fn spawn_industry_refresh(&mut self, kind: IndustryKind, now: i64) {
        self.industry_timing.get_mut(kind).busy = true;
        let eve = self.shared.session.eve();
        let mut data = self.shared.industry.clone();
        let (tx, ctx) = (self.events_tx.clone(), self.ctx.clone());
        let dir = self.shared.data_dir.join("esi");
        let job: Job = Box::new(move || {
            // Read before the refresh saves over the file, so a corrupt file can be reported.
            let unreadable = esi_industry::load_industry(&dir).is_err();
            let result = session::lock_eve(&eve).refresh_industry(&mut data, kind, now);
            notify(
                &tx,
                &ctx,
                Event::Industry {
                    kind,
                    data,
                    result,
                    unreadable,
                },
            );
        });
        self.shared.worker.run(job);
    }

    fn industry_loaded(&mut self, result: Result<IndustryData, String>) {
        self.industry_loaded = true;
        match result {
            Ok(data) => self.shared.industry = data,
            Err(error) => self.shared.notices.push(Notice::error(format!(
                "Could not read the saved industry data: {error}"
            ))),
        }
    }

    fn industry_refreshed(
        &mut self,
        kind: IndustryKind,
        data: &IndustryData,
        result: Result<Refreshed, EsiError>,
        unreadable: bool,
    ) {
        let now = (self.shared.clock)();
        let timing = self.industry_timing.get_mut(kind);
        timing.busy = false;
        match result {
            Ok(Refreshed::Fetched) => {
                match kind {
                    IndustryKind::Systems => self.shared.industry.systems.clone_from(&data.systems),
                    IndustryKind::Prices => self.shared.industry.prices.clone_from(&data.prices),
                }
                if unreadable {
                    self.shared.notices.push(Notice::error(
                        "The saved industry data could not be read and was replaced with fresh data.",
                    ));
                }
                // Cannot saturate: `now` is a Unix time in seconds, and adding 60 cannot reach i64::MAX.
                timing.next = now.saturating_add(INDUSTRY_RETRY_SECS);
            }
            // Never ask again before the retry interval, even when the answer says `now`: a
            // request answered with `now` would otherwise be asked again on every frame.
            Ok(Refreshed::NotYet { until }) => {
                // Cannot saturate: `now` is a Unix time in seconds, and adding 60 cannot reach i64::MAX.
                timing.next = until.max(now.saturating_add(INDUSTRY_RETRY_SECS));
            }
            Err(EsiError::RateLimited {
                retry_after_seconds,
            }) => {
                self.shared.notices.push(Notice::error(format!(
                    "EVE industry data paused for {retry_after_seconds} s (ESI rate limit)."
                )));
                // Cannot saturate: `now` is a Unix time in seconds, and adding 300 cannot reach i64::MAX.
                timing.next = now.saturating_add(INDUSTRY_ERROR_RETRY_SECS);
            }
            Err(error) => {
                self.shared.notices.push(Notice::error(format!(
                    "Could not get EVE industry data: {error}"
                )));
                // Cannot saturate: `now` is a Unix time in seconds, and adding 300 cannot reach i64::MAX.
                timing.next = now.saturating_add(INDUSTRY_ERROR_RETRY_SECS);
            }
        }
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

/// The interface size `steps` places from `size` in [`InterfaceSize::ALL`], or `None` past either
/// end.
pub(crate) fn stepped_size(size: InterfaceSize, steps: isize) -> Option<InterfaceSize> {
    let index = InterfaceSize::ALL.iter().position(|&each| each == size)?;
    let index = index.checked_add_signed(steps)?;
    InterfaceSize::ALL.get(index).copied()
}

/// The notice for saved data that an earlier test build left behind and that was set aside.
fn fresh_start_notice(started_fresh: Option<&PathBuf>) -> Vec<Notice> {
    started_fresh
        .map(|old| {
            Notice::info(format!(
                "Your saved data came from an earlier test build and was set aside as {}. Starting fresh.",
                old.display()
            ))
        })
        .into_iter()
        .collect()
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

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::ops::ControlFlow;

    use mop_market::cache::HttpResponse;
    use mop_market::error::MarketError;
    use mop_market::fuzzwork::{BytesHttp, BytesResponse};
    use mop_sde::{Fetch, FetchError};
    use tempfile::TempDir;

    use super::*;
    use crate::session::NoEve;

    const NOW: i64 = 1_000_000;

    /// Nothing is fetched in these tests.
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

    /// A market that never answers: no price is read in these tests.
    struct NoHttp;

    impl BytesHttp for NoHttp {
        fn get_bytes(
            &self,
            _url: &str,
            _if_none_match: Option<&str>,
        ) -> Result<BytesResponse, MarketError> {
            Err(MarketError::Network("not in these tests".to_owned()))
        }
    }

    /// The PLEX market that never answers either.
    struct NoPlex;

    impl Http for NoPlex {
        fn get(
            &self,
            _url: &str,
            _if_none_match: Option<&str>,
        ) -> Result<HttpResponse, MarketError> {
            Err(MarketError::Network("not in these tests".to_owned()))
        }
    }

    fn shared(dir: &TempDir) -> Shared {
        let store = Store::open(&dir.path().join("user.sqlite"), NOW)
            .expect("the store opens")
            .store;
        Shared {
            sde: None,
            store,
            worker: Worker::spawn(Context::default(), Box::new(NoNetwork)),
            market: Arc::new(build::prices::Prices::new(Box::new(NoHttp))),
            plex_http: Arc::new(NoPlex),
            copy_notices: Arc::new(Mutex::new(Vec::new())),
            style: Style::eve_dense(),
            density: Density::Normal,
            rail_icons_only: false,
            data_dir: dir.path().to_path_buf(),
            notices: Vec::new(),
            clock: Box::new(|| NOW),
            session: Session::new(Box::new(NoEve)),
            selection: Selection::default(),
            industry: IndustryData::default(),
            claims: ClaimsState::default(),
        }
    }

    fn config(data: PathBuf, cache: PathBuf) -> AppConfig {
        AppConfig {
            data_dir: data,
            cache_dir: cache,
            fetch: Box::new(NoNetwork),
            clock: Box::new(|| NOW),
            eve: Box::new(NoEve),
            market: Box::new(NoHttp),
        }
    }

    fn tabs<T: Copy>(state: &DockState<T>) -> Vec<T> {
        state.iter_all_tabs().map(|(_, tab)| *tab).collect()
    }

    #[test]
    fn a_tick_saves_the_open_panels_under_the_page_key() {
        let dir = TempDir::new().expect("a temp folder");
        let mut cx = shared(&dir);
        save_panels(&mut cx, "build", Some("Plans,Buy list".to_owned()));
        assert_eq!(
            cx.store
                .setting("panels.build")
                .expect("the setting reads")
                .as_deref(),
            Some("Plans,Buy list")
        );
        save_panels(&mut cx, "build", None);
        assert_eq!(
            cx.store
                .setting("panels.build")
                .expect("the setting reads")
                .as_deref(),
            Some("Plans,Buy list"),
            "no tick saves nothing"
        );
    }

    #[test]
    fn a_failed_panel_save_becomes_an_error_notice() {
        let dir = TempDir::new().expect("a temp folder");
        let mut cx = shared(&dir);
        // A space is not allowed in a setting name, so the store refuses the key.
        save_panels(&mut cx, "Build page", Some("Plans".to_owned()));
        assert_eq!(cx.notices.len(), 1);
        assert!(
            cx.notices[0]
                .text
                .starts_with("Could not save the settings:"),
            "the notice names the save failure"
        );
    }

    #[test]
    fn the_app_starts_the_characters_dock_from_its_saved_panel_list() {
        let dir = TempDir::new().expect("a temp folder");
        let data = dir.path().join("data");
        let cache = dir.path().join("cache");
        fs::create_dir_all(&data).expect("the data folder");
        let mut store = Store::open(&data.join(USER_DB), NOW)
            .expect("the store opens")
            .store;
        store
            .set_setting("panels.characters", "Characters")
            .expect("the panel list saves");
        drop(store);

        let app = App::new(&Context::default(), config(data, cache)).expect("the app starts");
        assert_eq!(
            tabs(&app.characters_dock),
            [CharactersPanel::Characters],
            "the saved list replaces the default layout"
        );
    }
}

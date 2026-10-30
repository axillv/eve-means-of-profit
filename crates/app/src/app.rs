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
pub mod images;
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
pub mod what_to_build;
pub mod worker;

use std::collections::{BTreeMap, HashMap};
use std::error::Error;
use std::fs;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use egui::{
    Button, Context, Key, Modifiers, RichText, Sense, Stroke, TextWrapMode, Ui, ViewportCommand,
    WidgetInfo, WidgetType, vec2,
};
use egui_dock::{DockState, NodeIndex};
use egui_phosphor::regular;
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
use dock::UserNames;
use first_run::{Reply, Startup};
use jobs::{JobsPage, JobsPanel};
use layout::add_to::AddTo;
use layout::files::Files;
use layout::model::{Binding, Layout, PanelRef, Slot};
use layout::user::UserLayouts;
use net_worth::{NetWorthPage, NetWorthPanel};
use orders::{OrdersPage, OrdersPanel};
use rail::{Page, RailAction, Shown};
use selection::Selection;
use session::{EveAccess, Loaded, Session};
use setup::{SetupPage, SetupPanel};
use status::{Action, Notice};
use stockpiles::{StockpilesPage, StockpilesPanel};
use theme::Style;
use what_to_build::WhatToBuildPage;
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

/// The egui temp id that holds the text in the `Paste layout` window while it is open.
fn paste_layout_id() -> egui::Id {
    egui::Id::new("paste layout window text")
}

/// The arrangement of a shipped page's dock as a player's layout: every tab by its panel's title,
/// a panel this version knows by its key, and one it does not by the title as written. Every slot
/// follows the selection.
fn page_arrangement<P: Copy>(dock: &DockState<P>, title: fn(P) -> &'static str) -> Layout {
    let slots = dock.map_tabs(|panel| {
        let name = title(*panel);
        let panel = layout::panels::panels()
            .iter()
            .find(|info| info.name == name)
            .map_or_else(
                || PanelRef::Unknown(name.to_owned()),
                |info| PanelRef::Known(info.key),
            );
        Slot {
            panel,
            binding: Binding::Follow,
        }
    });
    layout::dock_convert::from_dock(&slots, "")
}

/// The names a player's layout needs for its headers: what is selected to follow, and the name of
/// every pin the layout holds.
struct LayoutNames {
    selected_plan: Option<(i64, String)>,
    selected_stockpile: Option<(i64, String)>,
    pins: Vec<(Binding, String)>,
}

impl LayoutNames {
    /// Reads the selected plan from the Build page, the selected stockpile from the selection, and
    /// the name of every pin in `layout` from the store. A lookup that fails is treated as a missing
    /// plan or stockpile, so a damaged database shows "pinned: deleted" rather than no page.
    fn read(
        layout: &Layout,
        build_page: &BuildPage,
        selection: &Selection,
        store: &mut Store,
    ) -> Self {
        let selected_plan = build_page.selected.and_then(|id| {
            build_page
                .record
                .as_ref()
                .map(|record| (id, record.name.clone()))
        });
        let selected_stockpile = selection
            .stockpile
            .and_then(|id| stockpile_name(store, id).map(|name| (id, name)));
        let mut pins: Vec<(Binding, String)> = Vec::new();
        if let Some(root) = &layout.root {
            for slot in root.slots() {
                if matches!(slot.binding, Binding::Follow)
                    || pins.iter().any(|(pin, _)| *pin == slot.binding)
                {
                    continue;
                }
                let name = match &slot.binding {
                    Binding::Follow => None,
                    Binding::PinPlan(id) => plan_name(store, *id),
                    Binding::PinStockpile(id) => stockpile_name(store, *id),
                };
                if let Some(name) = name {
                    pins.push((slot.binding.clone(), name));
                }
            }
        }
        Self {
            selected_plan,
            selected_stockpile,
            pins,
        }
    }
}

/// The saved plan `id`'s name, or `None` when it is gone or cannot be read.
fn plan_name(store: &mut Store, id: i64) -> Option<String> {
    match store.plan(id) {
        Ok(plan) => plan.map(|plan| plan.name),
        Err(_) => None,
    }
}

/// The saved stockpile `id`'s name, or `None` when it is gone or cannot be read.
fn stockpile_name(store: &mut Store, id: i64) -> Option<String> {
    match store.stockpile(id) {
        Ok(stockpile) => stockpile.map(|stockpile| stockpile.name),
        Err(_) => None,
    }
}

/// The page state a player's layout panel reads, one mutable reference per shipped page that a
/// panel cannot pin. The Build and Stockpiles pages are drawn by their own code, because a pinned
/// panel reads a page of its own.
struct PageStates<'a> {
    assets: &'a mut AssetsPage,
    net_worth: &'a mut NetWorthPage,
    orders: &'a mut OrdersPage,
    jobs: &'a mut JobsPage,
    characters: &'a mut CharactersPage,
    setup: &'a mut SetupPage,
}

/// Draws the panel of a player's layout named by `key`. Only a key in the panel list reaches here,
/// and a key with no shipped panel draws nothing. The Build and Stockpiles panels are drawn by the
/// caller, which picks the page the slot is bound to.
fn show_panel(key: &str, ui: &mut Ui, pages: &mut PageStates, cx: &mut Shared) {
    match key.split_once('.') {
        Some(("assets", _)) => show_assets_panel(key, ui, &mut *pages.assets, cx),
        Some(("net_worth", _)) => show_net_worth_panel(key, ui, &mut *pages.net_worth, cx),
        Some(("orders", _)) => {
            if key == "orders.orders" {
                orders::show(&mut *pages.orders, OrdersPanel::Orders, ui, cx);
            }
        }
        Some(("jobs", _)) => {
            if key == "jobs.industry_jobs" {
                jobs::show(&mut *pages.jobs, JobsPanel::IndustryJobs, ui, cx);
            }
        }
        Some(("characters", _)) => show_characters_panel(key, ui, &mut *pages.characters, cx),
        Some(("setup", _)) => show_setup_panel(key, ui, &mut *pages.setup, cx),
        _ if key == "item" => item_panel::show(ui, cx),
        _ => {}
    }
}

/// The Build page a slot pinned to plan `id` draws: made and loaded the first time the slot is
/// shown, then kept for the session.
fn pinned_plan<'a>(
    pinned: &'a mut HashMap<i64, BuildPage>,
    id: i64,
    cx: &mut Shared,
) -> &'a mut BuildPage {
    pinned.entry(id).or_insert_with(|| {
        let mut page = BuildPage::default();
        page.load(id, cx);
        page
    })
}

/// The Stockpiles page a slot pinned to stockpile `id` draws: made with `open` the first time the
/// slot is shown, then kept for the session. `open` selects the stockpile for the page on show, so
/// the selection it was given is put back.
fn pinned_stockpile<'a>(
    pinned: &'a mut HashMap<i64, StockpilesPage>,
    id: i64,
    cx: &mut Shared,
) -> &'a mut StockpilesPage {
    pinned.entry(id).or_insert_with(|| {
        let mut page = StockpilesPage::default();
        let selected = cx.selection.stockpile;
        page.open(id, cx);
        cx.selection.stockpile = selected;
        page
    })
}

fn show_build_panel(key: &str, ui: &mut Ui, page: &mut BuildPage, cx: &mut Shared) {
    let panel = match key {
        "build.plans" => BuildPanel::Plans,
        "build.steps" => BuildPanel::Steps,
        "build.buy_list" => BuildPanel::BuyList,
        // The pasted stock is a window of the Plan panel, so that panel shows its page.
        "build.plan" | "build.pasted_stock" => BuildPanel::Plan,
        _ => return,
    };
    build::show(page, panel, ui, cx);
}

fn show_assets_panel(key: &str, ui: &mut Ui, page: &mut AssetsPage, cx: &mut Shared) {
    let panel = match key {
        "assets.places" => AssetsPanel::Places,
        "assets.assets" => AssetsPanel::Assets,
        _ => return,
    };
    assets::show(page, panel, ui, cx);
}

fn show_net_worth_panel(key: &str, ui: &mut Ui, page: &mut NetWorthPage, cx: &mut Shared) {
    let panel = match key {
        "net_worth.net_worth" => NetWorthPanel::NetWorth,
        "net_worth.by_character" => NetWorthPanel::ByCharacter,
        "net_worth.right_now" => NetWorthPanel::RightNow,
        _ => return,
    };
    net_worth::show(page, panel, ui, cx);
}

fn show_characters_panel(key: &str, ui: &mut Ui, page: &mut CharactersPage, cx: &mut Shared) {
    let panel = match key {
        "characters.characters" => CharactersPanel::Characters,
        "characters.jeve_data" => CharactersPanel::JeveData,
        _ => return,
    };
    characters::show(page, panel, ui, cx);
}

fn show_setup_panel(key: &str, ui: &mut Ui, page: &mut SetupPage, cx: &mut Shared) {
    let panel = match key {
        "setup.facilities" => SetupPanel::Facilities,
        "setup.freight_services" => SetupPanel::FreightServices,
        "setup.price_history" => SetupPanel::PriceHistory,
        "setup.price_rule" => SetupPanel::PriceRule,
        "setup.structures" => SetupPanel::Structures,
        "setup.fees" => SetupPanel::Fees,
        "setup.invention" => SetupPanel::Invention,
        "setup.display" => SetupPanel::Display,
        _ => return,
    };
    setup::show(page, panel, ui, cx);
}

fn show_stockpiles_panel(key: &str, ui: &mut Ui, page: &mut StockpilesPage, cx: &mut Shared) {
    let panel = match key {
        "stockpiles.stockpiles" => StockpilesPanel::Stockpiles,
        "stockpiles.contents" => StockpilesPanel::Contents,
        _ => return,
    };
    page.show(panel, ui, cx);
}

/// The icon a page entry shows in the icons-only rail.
fn page_icon(page: Page) -> &'static str {
    match page {
        Page::Today => regular::SUN,
        Page::WhatToBuild => regular::COMPASS,
        Page::Build => regular::HAMMER,
        Page::Stockpiles => regular::STACK,
        Page::Assets => regular::PACKAGE,
        Page::NetWorth => regular::CHART_LINE_UP,
        Page::Orders => regular::STOREFRONT,
        Page::Jobs => regular::FACTORY,
        Page::Characters => regular::USERS,
        Page::Setup => regular::GEAR,
    }
}

/// Whether a page entry starts a group, and so has a line above it.
fn page_starts_group(page: Page) -> bool {
    matches!(page, Page::WhatToBuild | Page::Assets | Page::Characters)
}

/// Draws the line between groups of rail entries, with space above and below.
fn rail_divider(ui: &mut Ui, style: &Style) {
    ui.add_space(8.0);
    let (line, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().hline(
        line.x_range(),
        line.center().y,
        Stroke::new(1.0, style.line),
    );
    ui.add_space(8.0);
}

/// Draws one icon-only rail entry, accented when it is the one on show, with its label as hover
/// text and as its widget label.
fn icon_entry(
    ui: &mut Ui,
    style: &Style,
    content_width: f32,
    icon: &str,
    label: &str,
    hover: &str,
    selected: bool,
) -> egui::Response {
    let (fill, text) = if selected {
        (style.accent, style.on_accent)
    } else {
        (style.row, style.text)
    };
    let button = Button::new(RichText::new(icon).color(text))
        .fill(fill)
        .corner_radius(style.control_radius)
        .wrap_mode(TextWrapMode::Extend)
        .min_size(vec2(content_width, 40.0));
    let response = ui.add(button);
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), label));
    response.on_hover_text(hover)
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
    events: Receiver<Event>,
    events_tx: Sender<Event>,
    page: Page,
    /// The player's layouts and their file.
    user: UserLayouts,
    /// The layout on show, `None` when a shipped page is.
    layout_shown: Option<usize>,
    /// The `Add to layout…` window and the layout it last used.
    add_to: AddTo,
    /// The plan each pinned plan panel shows, made when the panel is first shown.
    pinned_plans: HashMap<i64, BuildPage>,
    /// The stockpile each pinned stockpile panel shows, made when the panel is first shown.
    pinned_stockpiles: HashMap<i64, StockpilesPage>,
    what_to_build_page: WhatToBuildPage,
    /// The open and save dialogs, and where their layouts come from and go to.
    files: Files,
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
        images::init(
            ctx,
            config.data_dir.join("images"),
            Arc::new(images::Ureq::new()),
        );
        let now = (config.clock)();
        let (user, layout_notices) = UserLayouts::load(
            config.data_dir.join("layouts.txt"),
            layout::panels::panels(),
            now,
        );
        let opened = Store::open(&config.data_dir.join(USER_DB), now)?;
        let mut notices = fresh_start_notice(opened.started_fresh.as_ref());
        notices.extend(layout_notices);
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
        let (docks, (events_tx, events)) = (saved_docks(&store)?, mpsc::channel());
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
            events,
            events_tx,
            page: Page::Today,
            user,
            layout_shown: None,
            add_to: AddTo::default(),
            pinned_plans: HashMap::new(),
            pinned_stockpiles: HashMap::new(),
            what_to_build_page: WhatToBuildPage::default(),
            files: Files::native(),
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
        app.begin(settings.last_sde_check, now);
        Ok(app)
    }

    /// Starts the session and industry loads, then the first-run download, or the daily check when
    /// the game data is due for one.
    fn begin(&mut self, last_sde_check: Option<i64>, now: i64) {
        self.spawn_session_load();
        self.spawn_industry_load();
        if self.shared.sde.is_none() {
            self.start_first_run();
        } else if update_due(last_sde_check, now) {
            self.spawn_check(false);
        }
    }

    /// The state the pages and windows share, for code that drives the app from outside.
    pub fn shared_mut(&mut self) -> &mut Shared {
        &mut self.shared
    }

    /// Makes `page` the page on show, for code that drives the app from outside.
    pub fn show_page(&mut self, page: Page) {
        self.go_to(page);
    }

    /// Replaces the open and save dialogs, for code that drives the app from outside.
    pub fn set_files(&mut self, files: Files) {
        self.files = files;
    }

    /// Makes `page` the page on show, and takes any player's layout off the screen.
    fn go_to(&mut self, page: Page) {
        self.page = page;
        self.layout_shown = None;
    }

    /// One frame: take the worker's results, then draw.
    pub fn show(&mut self, ui: &mut Ui) {
        self.drain_events();
        characters::tick(&mut self.shared, &self.ctx);
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
            self.go_to(Page::Setup);
        }
        self.zoom_with_keys(ui);
        self.menu_bar(ui);
        self.paste_window(ui);
        self.add_to_layout();
        let activity = self.activity();
        let idle = self.idle_text();
        let clicked = status::show(
            ui,
            &self.shared.style,
            &mut self.shared.notices,
            activity.as_deref(),
            &idle,
        );
        match clicked {
            Some(Action::Update { build }) => self.start_update(build),
            Some(Action::RefreshAll) => {
                characters::refresh_all(&mut self.shared, &self.ctx, true);
            }
            None => {}
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
                    if ui.button("Open layout file…").clicked() {
                        self.open_layout_file();
                        ui.close();
                    }
                    if ui.button("Paste layout").clicked() {
                        self.ctx.data_mut(|data| {
                            data.insert_temp(paste_layout_id(), String::new());
                        });
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("Quit").clicked() {
                        ui.ctx().send_viewport_cmd(ViewportCommand::Close);
                        ui.close();
                    }
                });
                ui.menu_button("View", |ui| {
                    if ui.button("Settings…").clicked() {
                        self.go_to(Page::Setup);
                        ui.close();
                    }
                });
            });
        });
    }

    /// Draws the `Paste layout` window while it is open. `Add` reads the text and closes the
    /// window; `Cancel` closes it and reads nothing.
    fn paste_window(&mut self, ui: &mut Ui) {
        let id = paste_layout_id();
        let Some(mut text) = ui.ctx().data(|data| data.get_temp::<String>(id)) else {
            return;
        };
        let mut add = false;
        let mut cancel = false;
        egui::Window::new("Paste layout")
            .id(egui::Id::new("paste layout window"))
            .collapsible(false)
            .resizable(true)
            .show(ui.ctx(), |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut text)
                        .desired_rows(12)
                        .desired_width(520.0),
                );
                ui.horizontal(|ui| {
                    add = ui.button("Add").clicked();
                    cancel = ui.button("Cancel").clicked();
                });
            });
        if add {
            self.add_pasted(&text);
            ui.ctx().data_mut(|data| data.remove::<String>(id));
        } else if cancel {
            ui.ctx().data_mut(|data| data.remove::<String>(id));
        } else {
            ui.ctx().data_mut(|data| data.insert_temp(id, text));
        }
    }

    /// Opens the `Add to layout…` window when a panel asked for it, and adds the panel when the
    /// window answers. The page on show and the rail selection stay as they are.
    fn add_to_layout(&mut self) {
        if let Some(name) = dock::take_add_request(&self.ctx) {
            self.add_to.open(&name, layout::panels::panels());
        }
        let selected_plan = self.build_page.selected.and_then(|id| {
            self.build_page
                .record
                .as_ref()
                .map(|record| (id, record.name.as_str()))
        });
        let selected_stockpile = self
            .shared
            .selection
            .stockpile
            .and_then(|id| stockpile_name(&mut self.shared.store, id).map(|name| (id, name)));
        let selected_stockpile = selected_stockpile
            .as_ref()
            .map(|(id, name)| (*id, name.as_str()));
        let Some(outcome) = self.add_to.show(
            &self.ctx,
            &self.user.layouts,
            selected_plan,
            selected_stockpile,
        ) else {
            return;
        };
        let Some(panel) = layout::panels::panels()
            .iter()
            .find(|info| outcome.slot.panel == PanelRef::Known(info.key))
            .map(|info| info.name)
        else {
            return;
        };
        let (_, notice) = layout::add_to::add(&mut self.user.layouts, outcome, panel);
        self.shared.notices.push(notice);
        if let Some(notice) = self.user.save() {
            self.shared.notices.push(notice);
        }
    }

    /// Reads the file the player picked and adds its layout. A file dialog the player cancelled
    /// changes nothing.
    fn open_layout_file(&mut self) {
        let Some(path) = (self.files.open)() else {
            return;
        };
        let taken = self.taken_names();
        let taken: Vec<&str> = taken.iter().map(String::as_str).collect();
        match layout::files::import_file(&path, layout::panels::panels(), &taken) {
            Ok(imported) => self.add_imported(imported),
            Err(notice) => build::state::push_error(&mut self.shared.notices, notice.text),
        }
    }

    /// Reads a layout pasted into the `Paste layout` window and adds it.
    fn add_pasted(&mut self, text: &str) {
        let taken = self.taken_names();
        let taken: Vec<&str> = taken.iter().map(String::as_str).collect();
        match layout::files::import_text(text, layout::panels::panels(), &taken) {
            Ok(imported) => self.add_imported(imported),
            Err(notice) => build::state::push_error(&mut self.shared.notices, notice.text),
        }
    }

    /// Adds an imported layout, shows it, and reports what came in and what was left out.
    fn add_imported(&mut self, imported: layout::files::Imported) {
        let (index, saved) = self.user.push(imported.layout);
        self.layout_shown = Some(index);
        self.shared.notices.extend(imported.notices);
        if let Some(notice) = saved {
            build::state::push_error(&mut self.shared.notices, notice.text);
        }
    }

    /// Every name a new layout may not take: every page label and every layout name.
    fn taken_names(&self) -> Vec<String> {
        let mut taken: Vec<String> = Page::ALL
            .iter()
            .map(|page| page.label().to_owned())
            .collect();
        taken.extend(self.user.names());
        taken
    }

    /// The arrangement of a shipped page's dock as a player's layout.
    fn page_layout(&self, page: Page) -> Layout {
        match page {
            Page::Today | Page::WhatToBuild => Layout {
                name: String::new(),
                root: None,
            },
            Page::Build => page_arrangement(&self.build_dock, BuildPanel::title),
            Page::Assets => page_arrangement(&self.assets_dock, AssetsPanel::title),
            Page::NetWorth => page_arrangement(&self.net_worth_dock, NetWorthPanel::title),
            Page::Orders => page_arrangement(&self.orders_dock, OrdersPanel::title),
            Page::Jobs => page_arrangement(&self.jobs_dock, JobsPanel::title),
            Page::Characters => page_arrangement(&self.characters_dock, CharactersPanel::title),
            Page::Stockpiles => page_arrangement(&self.stockpiles_dock, StockpilesPanel::title),
            Page::Setup => page_arrangement(&self.setup_dock, SetupPanel::title),
        }
    }

    /// Does what a right-click on the rail asked for.
    fn rail_action(&mut self, action: RailAction) {
        match action {
            RailAction::SaveCopy(page) => {
                let arrangement = self.page_layout(page);
                let (index, notice) = self.user.save_copy(page.label(), arrangement);
                self.layout_shown = Some(index);
                if let Some(notice) = notice {
                    build::state::push_error(&mut self.shared.notices, notice.text);
                }
            }
            RailAction::Copy(index) => {
                self.ctx.copy_text(self.user.copy_text(index));
                self.shared.notices.push(Notice::info(
                    "Layout copied. Paste it in Discord or in the app (File > Paste layout).",
                ));
            }
            RailAction::SaveToFile(index) => {
                let (name, text) = self.user.file_text(index);
                if let Some(path) = (self.files.save)(&name) {
                    let notice = layout::files::write_text(&path, &text);
                    self.shared.notices.push(notice);
                }
            }
            RailAction::Rename { index, name } => {
                if let Some(notice) = self.user.rename(index, name) {
                    build::state::push_error(&mut self.shared.notices, notice.text);
                }
            }
            RailAction::Delete(index) => {
                let shown = self.layout_shown;
                if let Some(notice) = self.user.delete(index) {
                    build::state::push_error(&mut self.shared.notices, notice.text);
                }
                self.layout_shown = match shown {
                    Some(shown) if shown == index => {
                        self.page = Page::ALL[0];
                        None
                    }
                    Some(shown) if shown > index => Some(shown - 1),
                    shown => shown,
                };
            }
        }
    }

    /// Draws the rail and takes the entry the player picked, page or layout.
    fn rail(&mut self, ui: &mut Ui, style: &Style) {
        if self.shared.rail_icons_only {
            self.icons_only_rail(ui, style);
            return;
        }
        let names = self.user.names();
        let mut shown = match self.layout_shown {
            Some(index) => Shown::Layout(index),
            None => Shown::Page(self.page),
        };
        let action = rail::show_with_layouts(ui, style, &mut shown, &names);
        match shown {
            Shown::Page(page) => self.go_to(page),
            Shown::Layout(index) => self.layout_shown = Some(index),
        }
        if let Some(action) = action {
            self.rail_action(action);
        }
    }

    /// Draws the icons-only rail: every page, then the player's layouts, as icons with their labels
    /// on hover. The shipped icons-only rail draws the pages alone, so this one draws the layout
    /// entries too rather than hide them from a player who has that setting on.
    fn icons_only_rail(&mut self, ui: &mut Ui, style: &Style) {
        if let Some(page) = empty::take(ui.ctx()) {
            self.go_to(page);
        }
        let names = self.user.names();
        let mut shown = match self.layout_shown {
            Some(index) => Shown::Layout(index),
            None => Shown::Page(self.page),
        };
        let width = style.rail_narrow;
        let content_width = width
            - egui::Frame::side_top_panel(ui.style())
                .total_margin()
                .sum()
                .x;
        let mut action = None;
        egui::Panel::left("rail")
            .exact_size(width)
            .resizable(false)
            .show(ui, |ui| {
                ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
                    for entry in Page::ALL {
                        if page_starts_group(entry) {
                            rail_divider(ui, style);
                        }
                        let response = icon_entry(
                            ui,
                            style,
                            content_width,
                            page_icon(entry),
                            entry.label(),
                            entry.title(),
                            shown == Shown::Page(entry),
                        );
                        if response.clicked() {
                            shown = Shown::Page(entry);
                        }
                        response.context_menu(|menu| {
                            if menu.button("Save a copy").clicked() {
                                action = Some(RailAction::SaveCopy(entry));
                                menu.close();
                            }
                        });
                    }
                    if names.is_empty() {
                        return;
                    }
                    rail_divider(ui, style);
                    for (index, name) in names.iter().enumerate() {
                        let response = icon_entry(
                            ui,
                            style,
                            content_width,
                            regular::FOLDER,
                            name,
                            name,
                            shown == Shown::Layout(index),
                        );
                        if response.clicked() {
                            shown = Shown::Layout(index);
                        }
                    }
                });
            });
        match shown {
            Shown::Page(page) => self.go_to(page),
            Shown::Layout(index) => self.layout_shown = Some(index),
        }
        if let Some(action) = action {
            self.rail_action(action);
        }
    }

    fn pages(&mut self, ui: &mut Ui) {
        let style = self.shared.style;
        self.rail(ui, &style);
        if self.show_layout(ui, &style) {
            return;
        }
        egui::CentralPanel::default().show(ui, |ui| match self.page {
            Page::Today => {
                if let Some(page) = today::show(ui, &mut self.shared) {
                    self.go_to(page);
                }
            }
            Page::WhatToBuild => self.show_what_to_build(ui),
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
            Page::Stockpiles => {
                self.show_stockpiles(ui, &style);
                self.open_built_plan();
            }
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

    /// Draws the What could I build? page and follows a page its buttons ask for.
    fn show_what_to_build(&mut self, ui: &mut Ui) {
        if let Some(page) = what_to_build::show(ui, &mut self.what_to_build_page, &mut self.shared)
        {
            self.go_to(page);
        }
    }

    /// Draws the player's layout on show, when there is one that still exists, and says whether it
    /// did. A layout the rail no longer lists shows the page it was hiding.
    fn show_layout(&mut self, ui: &mut Ui, style: &Style) -> bool {
        let Some(index) = self.layout_shown else {
            return false;
        };
        if index >= self.user.layouts.len() {
            self.layout_shown = None;
            return false;
        }
        self.user_layout(ui, index, style);
        true
    }

    /// Draws the player's layout at `index`. A panel that follows the selection shows its shipped
    /// page's state; a panel pinned to a plan or stockpile shows a page of its own, kept for the
    /// session.
    fn user_layout(&mut self, ui: &mut Ui, index: usize, style: &Style) {
        let Some(layout) = self.user.layouts.get(index) else {
            return;
        };
        let names = LayoutNames::read(
            layout,
            &self.build_page,
            &self.shared.selection,
            &mut self.shared.store,
        );
        let pin_name = |binding: &Binding| {
            names
                .pins
                .iter()
                .find(|(pin, _)| pin == binding)
                .map(|(_, name)| name.clone())
        };
        let user_names = UserNames {
            selected_plan: names
                .selected_plan
                .as_ref()
                .map(|(id, name)| (*id, name.as_str())),
            selected_stockpile: names
                .selected_stockpile
                .as_ref()
                .map(|(id, name)| (*id, name.as_str())),
            pin_name: &pin_name,
        };
        if layout.root.is_none() {
            // The conversion gives a layout with no panel a leaf with no tabs, which the dock draws
            // as a blank leaf. The empty-layout text needs a dock with no node at all.
            let mut dock = DockState::new(Vec::new());
            dock.main_surface_mut().remove_leaf(NodeIndex::root());
            dock::show_user(
                ui,
                style,
                &mut dock,
                layout::panels::panels(),
                &user_names,
                |_, _| {},
            );
            return;
        }
        let mut pages = PageStates {
            assets: &mut self.assets_page,
            net_worth: &mut self.net_worth_page,
            orders: &mut self.orders_page,
            jobs: &mut self.jobs_page,
            characters: &mut self.characters_page,
            setup: &mut self.setup_page,
        };
        let notice = self.user.show(index, ui, style, &user_names, |ui, slot| {
            let PanelRef::Known(key) = &slot.panel else {
                return;
            };
            let key = *key;
            match &slot.binding {
                Binding::PinPlan(id) => {
                    let page = pinned_plan(&mut self.pinned_plans, *id, &mut self.shared);
                    show_build_panel(key, ui, page, &mut self.shared);
                }
                Binding::PinStockpile(id) => {
                    let page = pinned_stockpile(&mut self.pinned_stockpiles, *id, &mut self.shared);
                    // The Contents panel reads the selection, so a pinned page lends it the
                    // stockpile it shows and the page on show gets the selection back.
                    let selected = self.shared.selection.stockpile;
                    self.shared.selection.stockpile = Some(*id);
                    show_stockpiles_panel(key, ui, page, &mut self.shared);
                    self.shared.selection.stockpile = selected;
                }
                Binding::Follow => match key.split_once('.') {
                    Some(("build", _)) => {
                        show_build_panel(key, ui, &mut self.build_page, &mut self.shared);
                    }
                    Some(("stockpiles", _)) => {
                        show_stockpiles_panel(key, ui, &mut self.stockpiles_page, &mut self.shared);
                    }
                    _ => show_panel(key, ui, &mut pages, &mut self.shared),
                },
            }
        });
        if let Some(notice) = notice {
            build::state::push_error(&mut self.shared.notices, notice.text);
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

    /// Opens the plan a `Build what is short` click made, once the Stockpiles page has been drawn.
    /// A request skips a plan already on screen, so that one is loaded directly; a plan with
    /// unsaved edits asks about leaving first.
    fn open_built_plan(&mut self) {
        let Some(id) = self.stockpiles_page.open_plan.take() else {
            return;
        };
        self.go_to(Page::Build);
        if self.build_page.selected == Some(id) {
            if self.build_page.dirty {
                self.build_page.dialog =
                    Some(build::state::Dialog::Leave(build::state::Next::Select(id)));
            } else {
                self.build_page.load(id, &mut self.shared);
            }
        } else {
            self.build_page
                .request(build::state::Next::Select(id), &mut self.shared);
        }
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
        // The one automatic refresh: characters and prices are asked once, when the saved session
        // has loaded. After this only a player's button or a finished login asks again.
        characters::refresh_all(&mut self.shared, &self.ctx, false);
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

    /// A saved plan of one Rifter, with everything else at its defaults.
    fn record(name: &str) -> mop_store::PlanRecord {
        mop_store::PlanRecord {
            id: 0,
            name: name.to_owned(),
            product: 587,
            quantity: 1,
            build: std::collections::BTreeSet::new(),
            me: BTreeMap::new(),
            manufacturing: None,
            reactions: None,
            price_rule: Default::default(),
            route: None,
            service: None,
            packing: Default::default(),
            pasted_stock: String::new(),
            created_at: NOW,
            updated_at: NOW,
        }
    }

    #[test]
    fn a_plan_made_on_the_stockpiles_page_opens_on_build() {
        let dir = TempDir::new().expect("a temp folder");
        let data = dir.path().join("data");
        let cache = dir.path().join("cache");
        fs::create_dir_all(&data).expect("the data folder");
        let mut app = App::new(&Context::default(), config(data, cache)).expect("the app starts");
        let id = app
            .shared
            .store
            .create_plan(&record("Kit (short)"), NOW)
            .expect("the plan is saved");
        app.page = Page::Stockpiles;
        app.stockpiles_page.open_plan = Some(id);

        app.open_built_plan();

        assert_eq!(app.page, Page::Build);
        assert_eq!(app.build_page.selected, Some(id));
        assert_eq!(app.stockpiles_page.open_plan, None);

        // The plan is on screen with an unsaved edit: the leave dialog asks first.
        app.build_page.dirty = true;
        app.page = Page::Stockpiles;
        app.stockpiles_page.open_plan = Some(id);
        app.open_built_plan();
        assert_eq!(app.page, Page::Build);
        assert_eq!(
            app.build_page.dialog,
            Some(build::state::Dialog::Leave(build::state::Next::Select(id)))
        );

        // Leaving loads the plan again and drops the edit.
        app.build_page
            .go(build::state::Next::Select(id), &mut app.shared);
        assert!(!app.build_page.dirty);
        assert_eq!(app.build_page.selected, Some(id));

        // A different plan on screen with unsaved edits is asked about through the usual request.
        let other = app
            .shared
            .store
            .create_plan(&record("Kit (short) 2"), NOW)
            .expect("the other plan is saved");
        app.build_page.dirty = true;
        app.page = Page::Stockpiles;
        app.stockpiles_page.open_plan = Some(other);
        app.open_built_plan();
        assert_eq!(app.page, Page::Build);
        assert_eq!(
            app.build_page.dialog,
            Some(build::state::Dialog::Leave(build::state::Next::Select(
                other
            )))
        );
        assert_eq!(
            app.build_page.selected,
            Some(id),
            "the plan on screen waits for the answer"
        );
    }
}

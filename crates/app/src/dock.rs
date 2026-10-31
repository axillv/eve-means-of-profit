//! Pages as locked egui_dock layouts, and a user's layout as a dock of its own. The lock recipe is
//! in the design note (section 5.1): tab bars hidden, tabs not draggable, no close buttons, no tab
//! menus, and the show-tab-bar button kept at size 0. Each panel draws its own header with its name
//! and, when its body has a table, a button that copies the table. A user layout instead shows its
//! tab bar, drags and closes its tabs, and lets each panel follow the selection or be pinned.

use egui::{
    Align, CornerRadius, FontId, Id, Layout, Margin, Rect, RichText, Sense, Stroke, Ui, UiBuilder,
    WidgetText, vec2,
};
use egui_dock::{DockArea, DockState, NodeIndex, OverlayType, TabViewer};
use mop_store::Store;
use mop_store::StoreError;

use crate::Shared;
use crate::assets::AssetsPanel;
use crate::build::BuildPanel;
use crate::characters::CharactersPanel;
use crate::jobs::JobsPanel;
use crate::kit::{self, ButtonKind, SheetHeader, add_small_button};
use crate::layout::model::{Binding, Follows, PanelInfo, PanelRef, Slot, display_name};
use crate::net_worth::NetWorthPanel;
use crate::orders::OrdersPanel;
use crate::setup::SetupPanel;
use crate::stockpiles::StockpilesPanel;
use crate::table::{copy_request, has_table, resort_request, rows_moved};
use crate::theme::Style;

/// The panel name last asked for by an `Add to layout…` item.
pub fn take_add_request(ctx: &egui::Context) -> Option<String> {
    ctx.data_mut(|data| data.remove_temp::<String>(Id::new("add to layout")))
}

/// The `Add to layout…` item of a panel's `Layout` menu.
fn add_to_layout(ui: &mut Ui, name: &str) {
    if ui.button("Add to layout…").clicked() {
        ui.data_mut(|data| data.insert_temp(Id::new("add to layout"), name.to_owned()));
        ui.close();
    }
}

/// The Build panels the Panels menu lists. The Item inspector floats outside the dock.
pub const BUILD_PANELS: [BuildPanel; 4] = [
    BuildPanel::Plans,
    BuildPanel::Plan,
    BuildPanel::Steps,
    BuildPanel::BuyList,
];

/// The Assets panels the Panels menu lists. The Item inspector floats outside the dock.
pub const ASSETS_PANELS: [AssetsPanel; 2] = [AssetsPanel::Places, AssetsPanel::Assets];

/// The Net worth panels the Panels menu lists.
pub const NET_WORTH_PANELS: [NetWorthPanel; 3] = [
    NetWorthPanel::NetWorth,
    NetWorthPanel::ByCharacter,
    NetWorthPanel::RightNow,
];

/// The Characters panels the Panels menu lists.
pub const CHARACTERS_PANELS: [CharactersPanel; 2] =
    [CharactersPanel::Characters, CharactersPanel::JeveData];

/// The Stockpiles panels the Panels menu lists. Item is the inspector and is never listed.
pub const STOCKPILES_PANELS: [StockpilesPanel; 2] =
    [StockpilesPanel::Stockpiles, StockpilesPanel::Contents];

/// The Settings panels the Panels menu lists.
pub const SETUP_PANELS: [SetupPanel; 8] = [
    SetupPanel::Facilities,
    SetupPanel::FreightServices,
    SetupPanel::PriceHistory,
    SetupPanel::PriceRule,
    SetupPanel::Structures,
    SetupPanel::Fees,
    SetupPanel::Invention,
    SetupPanel::Display,
];

/// The Build page: a narrow Plans list on the left; Plan on top, then Steps and Buy list side by
/// side below it.
pub fn build_layout() -> DockState<BuildPanel> {
    let mut dock = DockState::new(vec![BuildPanel::Plans]);
    let surface = dock.main_surface_mut();
    let [_plans, plan] = surface.split_right(NodeIndex::root(), 0.2, vec![BuildPanel::Plan]);
    let [_plan, lower] = surface.split_below(plan, 0.35, vec![BuildPanel::Steps]);
    surface.split_right(lower, 0.5, vec![BuildPanel::BuyList]);
    dock
}

/// The Assets page: Places at 25%, Assets at 75%.
pub fn assets_layout() -> DockState<AssetsPanel> {
    let mut dock = DockState::new(vec![AssetsPanel::Places]);
    let surface = dock.main_surface_mut();
    surface.split_right(NodeIndex::root(), 0.25, vec![AssetsPanel::Assets]);
    dock
}

/// The Net worth page: Net worth on top at 60%, By character and Right now side by side below it.
pub fn net_worth_layout() -> DockState<NetWorthPanel> {
    let mut dock = DockState::new(vec![NetWorthPanel::NetWorth]);
    let surface = dock.main_surface_mut();
    let [_net_worth, lower] =
        surface.split_below(NodeIndex::root(), 0.6, vec![NetWorthPanel::ByCharacter]);
    surface.split_right(lower, 0.5, vec![NetWorthPanel::RightNow]);
    dock
}

/// The Orders page: one panel.
pub fn orders_layout() -> DockState<OrdersPanel> {
    DockState::new(vec![OrdersPanel::Orders])
}

/// The Industry jobs page: one panel.
pub fn jobs_layout() -> DockState<JobsPanel> {
    DockState::new(vec![JobsPanel::IndustryJobs])
}

/// The Characters page: Characters above jEve data.
pub fn characters_layout() -> DockState<CharactersPanel> {
    let mut dock = DockState::new(vec![CharactersPanel::Characters]);
    dock.main_surface_mut()
        .split_below(NodeIndex::root(), 0.5, vec![CharactersPanel::JeveData]);
    dock
}

/// The Stockpiles page: Stockpiles at 25%, Contents at 75%.
pub fn stockpiles_layout() -> DockState<StockpilesPanel> {
    let mut dock = DockState::new(vec![StockpilesPanel::Stockpiles]);
    dock.main_surface_mut()
        .split_right(NodeIndex::root(), 0.25, vec![StockpilesPanel::Contents]);
    dock
}

/// The Setup page: Facilities, Price history and Display stacked on the left; Freight services over
/// Price rule and Value prices over Structures, Fees and Invention on the right.
pub fn setup_layout() -> DockState<SetupPanel> {
    let mut dock = DockState::new(vec![SetupPanel::Facilities]);
    let surface = dock.main_surface_mut();
    let [facilities, freight] =
        surface.split_right(NodeIndex::root(), 0.5, vec![SetupPanel::FreightServices]);
    let [_facilities, history] =
        surface.split_below(facilities, 0.4, vec![SetupPanel::PriceHistory]);
    surface.split_below(history, 0.5, vec![SetupPanel::Display]);
    let [_freight, price] = surface.split_below(
        freight,
        0.5,
        vec![SetupPanel::PriceRule, SetupPanel::ValuePrices],
    );
    surface.split_below(
        price,
        0.5,
        vec![
            SetupPanel::Structures,
            SetupPanel::Fees,
            SetupPanel::Invention,
        ],
    );
    dock
}

/// The body of a panel whose page is not built yet. The header already names the panel, so the body
/// shows only the loading text until the saved character data has arrived.
pub fn placeholder(ui: &mut Ui, cx: &Shared) {
    if !cx.session.loaded() {
        ui.label(RichText::new("Loading character data…").color(cx.style.mute));
    }
}

/// Draws a locked page with its Panels menu. `all` is the page's tickable panels in menu order;
/// `title` names a panel. Returns the open panels as their titles joined with commas when a tick
/// changed the page, so the caller can save them, and `None` when no tick happened. A page with
/// fewer than two tickable panels draws no menu.
pub fn show_page<T: Copy + PartialEq>(
    ui: &mut Ui,
    theme: &Style,
    page: &str,
    all: &[T],
    state: &mut DockState<T>,
    title: fn(T) -> &'static str,
    body: impl FnMut(&mut Ui, T),
) -> Option<String> {
    let mut changed = false;
    if all.len() >= 2 {
        // The strip sits on the dock's top edge rather than above it: a row of its own would take
        // height from the panels, and the tables are laid out to fit the window.
        // A compact strip: the panels size their tables to the window, so every point the menu
        // takes from the dock would hide a row.
        ui.push_id(page, |ui| {
            ui.scope(|ui| {
                ui.spacing_mut().interact_size.y = 12.0;
                ui.spacing_mut().button_padding = vec2(6.0, 0.0);
                ui.horizontal(|ui| {
                    ui.menu_button(RichText::new("Panels").size(11.0), |ui| {
                        for &panel in all {
                            let mut open = state.find_tab(&panel).is_some();
                            if ui.checkbox(&mut open, title(panel)).changed()
                                && toggle(state, panel)
                            {
                                changed = true;
                            }
                        }
                    });
                });
            });
        });
    }
    show_locked(ui, theme, state, title, body);
    if changed {
        Some(open_text(state, all, title))
    } else {
        None
    }
}

/// Opens or closes `panel`. Returns whether the page changed; closing the last open tab is
/// refused, so a page never ends up with no panel at all.
fn toggle<T: Copy + PartialEq>(state: &mut DockState<T>, panel: T) -> bool {
    match state.find_tab(&panel) {
        Some(path) => {
            if state.iter_all_tabs().count() <= 1 {
                return false;
            }
            state.remove_tab(path).is_some()
        }
        None => {
            state
                .main_surface_mut()
                .split_right(NodeIndex::root(), 0.5, vec![panel]);
            true
        }
    }
}

/// The panels of `all` that are open, in list order, as their titles joined with commas.
fn open_text<T: Copy + PartialEq>(
    state: &DockState<T>,
    all: &[T],
    title: fn(T) -> &'static str,
) -> String {
    all.iter()
        .filter(|panel| state.find_tab(panel).is_some())
        .map(|&panel| title(panel))
        .collect::<Vec<_>>()
        .join(",")
}

/// Makes the page show the panels named in `text`: closes the listed panels it does not name and
/// opens the named ones that are missing, right of the whole layout at 50%, in list order. Empty
/// parts, names outside `all` and unknown names are ignored; a text with no valid name leaves the
/// page as it is.
fn apply_open<T: Copy + PartialEq>(
    state: &mut DockState<T>,
    text: &str,
    all: &[T],
    title: fn(T) -> &'static str,
) {
    // An empty text closes every listed panel but the last, so the page keeps one panel at
    // least. A text with names that all fail to match is corrupt instead, and leaves the page as
    // it is.
    if text.trim().is_empty() {
        for &panel in all {
            let Some(path) = state.find_tab(&panel) else {
                continue;
            };
            if state.iter_all_tabs().count() > 1 {
                state.remove_tab(path);
            }
        }
        return;
    }
    let named: Vec<T> = text
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .filter_map(|name| all.iter().copied().find(|panel| title(*panel) == name))
        .collect();
    if named.is_empty() {
        return;
    }
    for &panel in all {
        if named.contains(&panel) {
            continue;
        }
        let Some(path) = state.find_tab(&panel) else {
            continue;
        };
        if state.iter_all_tabs().count() > 1 {
            state.remove_tab(path);
        }
    }
    for &panel in all {
        if named.contains(&panel) && state.find_tab(&panel).is_none() {
            state
                .main_surface_mut()
                .split_right(NodeIndex::root(), 0.5, vec![panel]);
        }
    }
}

/// The default layout for `page`, shaped by its saved panel list when there is one. A read error
/// fails the start, and no saved setting means the default layout.
pub fn with_saved<T: Copy + PartialEq>(
    store: &Store,
    page: &str,
    mut state: DockState<T>,
    all: &[T],
    title: fn(T) -> &'static str,
) -> Result<DockState<T>, StoreError> {
    if let Some(text) = store.setting(&format!("panels.{page}"))? {
        apply_open(&mut state, &text, all, title);
    }
    Ok(state)
}

/// Draws a locked page. `title` names a panel; `body` draws the inside of one panel.
pub fn show_locked<T: Copy>(
    ui: &mut Ui,
    theme: &Style,
    state: &mut DockState<T>,
    title: fn(T) -> &'static str,
    body: impl FnMut(&mut Ui, T),
) {
    for (_, node) in state.iter_all_nodes_mut() {
        // A node with one panel hides its tab bar; a node with several keeps it so its tabs can be picked.
        if let Some(leaf) = node.get_leaf_mut() {
            leaf.tab_bar_hidden = leaf.tabs.len() == 1;
        }
    }
    let mut viewer = Locked { theme, title, body };
    DockArea::new(state)
        .style(style(ui, theme))
        .hidable_tab_bars(true)
        .draggable_tabs(false)
        .show_close_buttons(false)
        .show_leaf_close_all_buttons(false)
        .show_leaf_collapse_buttons(false)
        .tab_context_menus(false)
        .show_inside(ui, &mut viewer);
}

/// The text a user layout with no panel left shows.
const EMPTY_LAYOUT: &str =
    "This layout is empty. Add panels with \"Add to layout…\" in any panel's Layout menu.";

/// The names a user layout needs: what is selected to pin to, and the name of each existing pin.
pub struct UserNames<'a> {
    pub selected_plan: Option<(i64, &'a str)>,
    pub selected_stockpile: Option<(i64, &'a str)>,
    /// The plan or stockpile name a pin points at, or `None` when it is gone.
    pub pin_name: &'a dyn Fn(&Binding) -> Option<String>,
}

/// Draws a user's layout: every leaf shows its tab bar, tabs drag and close, and each panel follows
/// the selection or is pinned to one plan or stockpile. An empty layout draws no dock, only the
/// text that says how to add panels. `body` draws the inside of a panel that can show one.
pub fn show_user(
    ui: &mut Ui,
    theme: &Style,
    state: &mut DockState<Slot>,
    panels: &[PanelInfo],
    names: &UserNames,
    body: impl FnMut(&mut Ui, &Slot),
) {
    if state.main_surface().is_empty() {
        ui.label(EMPTY_LAYOUT);
        return;
    }
    for (_, node) in state.iter_all_nodes_mut() {
        // A user layout always shows its tabs, even when a leaf holds only one panel.
        if let Some(leaf) = node.get_leaf_mut() {
            leaf.tab_bar_hidden = false;
        }
    }
    let mut viewer = UserViewer {
        panels,
        selected_plan: names.selected_plan,
        selected_stockpile: names.selected_stockpile,
        pin_name: names.pin_name,
        theme,
        body,
    };
    // A user drops a tab on the edge of the panel it should split, not on a button in the middle.
    let mut style = style(ui, theme);
    style.overlay.overlay_type = OverlayType::HighlightedAreas;
    DockArea::new(state)
        .style(style)
        .draggable_tabs(true)
        .show_close_buttons(true)
        .show_leaf_close_all_buttons(false)
        .show_leaf_collapse_buttons(false)
        .tab_context_menus(false)
        .show_inside(ui, &mut viewer);
}

/// The follows kind of a known panel, or `None` for a panel this version does not know.
fn follows_of<'a>(panel: &PanelRef, panels: &'a [PanelInfo]) -> Option<&'a Follows> {
    match panel {
        PanelRef::Known(key) => panels
            .iter()
            .find(|info| info.key == *key)
            .map(|info| &info.follows),
        PanelRef::Unknown(_) => None,
    }
}

/// The muted words a following panel shows after its name; a panel that follows nothing has none.
fn follows_text(follows: &Follows) -> Option<&'static str> {
    match follows {
        Follows::Nothing => None,
        Follows::Plan => Some("follows plan"),
        Follows::Stockpile => Some("follows stockpile"),
        Follows::Item => Some("follows item"),
        Follows::Place => Some("follows place"),
        Follows::Character => Some("follows character"),
    }
}

/// The stable part of a tab id that tells two bindings of the same panel apart. `Binding` itself
/// does not hash, so its kind and id stand in for it.
fn binding_key(binding: &Binding) -> (&'static str, i64) {
    match binding {
        Binding::Follow => ("follow", 0),
        Binding::PinPlan(id) => ("plan", *id),
        Binding::PinStockpile(id) => ("stockpile", *id),
    }
}

/// The `Layout` menu of a panel: always `Add to layout…`, plus follow or pin for a plan or
/// stockpile panel.
fn layout_menu(
    ui: &mut Ui,
    theme: &Style,
    name: &str,
    tab: &mut Slot,
    follows: Option<&Follows>,
    selected_plan: Option<(i64, &str)>,
    selected_stockpile: Option<(i64, &str)>,
) {
    let stockpile = matches!(follows, Some(Follows::Stockpile));
    let follows = follows.filter(|follows| matches!(follows, Follows::Plan | Follows::Stockpile));
    layout_button(ui, theme, |ui| {
        add_to_layout(ui, name);
        if follows.is_none() {
            return;
        }
        ui.separator();
        if tab.binding == Binding::Follow {
            let selected = if stockpile {
                selected_stockpile
            } else {
                selected_plan
            };
            match selected {
                Some((id, name)) => {
                    if ui.button(format!("Pin to \"{name}\"")).clicked() {
                        tab.binding = if stockpile {
                            Binding::PinStockpile(id)
                        } else {
                            Binding::PinPlan(id)
                        };
                        ui.close();
                    }
                }
                None => {
                    let label = if stockpile {
                        "Pin to the selected stockpile"
                    } else {
                        "Pin to the selected plan"
                    };
                    let hover = if stockpile {
                        "No stockpile selected"
                    } else {
                        "No plan selected"
                    };
                    ui.add_enabled(false, egui::Button::new(label))
                        .on_disabled_hover_text(hover);
                }
            }
        } else {
            let follow = if stockpile {
                "Follow the selected stockpile"
            } else {
                "Follow the selected plan"
            };
            if ui.button(follow).clicked() {
                tab.binding = Binding::Follow;
                ui.close();
            }
        }
    });
}

/// Draws the `Layout` menu as a small Plain button, so it matches the buttons beside it.
fn layout_button(ui: &mut Ui, theme: &Style, add: impl FnOnce(&mut Ui)) -> egui::Response {
    let colors = kit::button_colors(theme, ButtonKind::Plain);
    let stroke = Stroke::new(1.0, colors.stroke);
    let text = Stroke::new(1.0, colors.text);
    let radius = CornerRadius::same(theme.control_radius);
    ui.scope(|ui| {
        ui.spacing_mut().interact_size.y = theme.sizes.small_button;
        ui.spacing_mut().button_padding = vec2(9.0, 0.0);
        let widgets = &mut ui.visuals_mut().widgets;
        widgets.inactive.weak_bg_fill = colors.fill;
        widgets.inactive.bg_fill = colors.fill;
        widgets.inactive.bg_stroke = stroke;
        widgets.inactive.fg_stroke = text;
        widgets.inactive.corner_radius = radius;
        // A pressed kit button keeps its fill, and a hovered one keeps it too and takes the
        // accent outline.
        widgets.active.weak_bg_fill = colors.fill;
        widgets.active.bg_fill = colors.fill;
        widgets.active.bg_stroke = stroke;
        widgets.active.fg_stroke = text;
        widgets.active.corner_radius = radius;
        widgets.hovered.weak_bg_fill = colors.fill;
        widgets.hovered.bg_fill = colors.fill;
        widgets.hovered.fg_stroke = text;
        widgets.hovered.corner_radius = radius;
        let label = RichText::new("Layout")
            .font(FontId::proportional(13.0))
            .color(colors.text);
        ui.menu_button(label, add).response
    })
    .inner
}

/// Draws a panel's card header: the title row and its line, one point in from the tab body's 1 px
/// outline, so the panel starts where a `kit::sheet` body does. `right` draws the header's buttons
/// at the right end; the row lays them out right to left, so the first one added sits rightmost.
fn panel_header(ui: &mut Ui, theme: &Style, title: &str, right: impl FnOnce(&mut Ui)) {
    let spacing = ui.spacing().item_spacing.y;
    ui.spacing_mut().item_spacing.y = 0.0;
    // The tab body paints its outline inside its rect, so the header content starts under it and
    // one point in from either side. The row and its line then end 40 points below the panel's
    // top edge, the same as the stroke, row and line of a sheet card.
    let (rect, _) = ui.allocate_exact_size(
        vec2(ui.available_width(), theme.sizes.sheet_header + 2.0),
        Sense::hover(),
    );
    let mut header = ui.new_child(
        UiBuilder::new()
            .max_rect(Rect::from_min_max(
                rect.min + vec2(1.0, 1.0),
                rect.max - vec2(1.0, 0.0),
            ))
            .layout(Layout::top_down(Align::Min)),
    );
    header.spacing_mut().item_spacing.y = 0.0;
    kit::sheet_header(
        &mut header,
        theme,
        SheetHeader { title, count: None },
        right,
    );
    ui.spacing_mut().item_spacing.y = spacing;
}

/// The text of the re-sort button: how many rows moved since the table kept its order.
fn resort_label(moved: usize) -> String {
    if moved == 1 {
        "Re-sort (1 row moved)".to_owned()
    } else {
        format!("Re-sort ({moved} rows moved)")
    }
}

struct UserViewer<'a, B> {
    panels: &'a [PanelInfo],
    selected_plan: Option<(i64, &'a str)>,
    selected_stockpile: Option<(i64, &'a str)>,
    pin_name: &'a dyn Fn(&Binding) -> Option<String>,
    theme: &'a Style,
    body: B,
}

impl<B: FnMut(&mut Ui, &Slot)> TabViewer for UserViewer<'_, B> {
    type Tab = Slot;

    fn id(&mut self, tab: &mut Slot) -> Id {
        Id::new((
            "user",
            display_name(&tab.panel, self.panels),
            binding_key(&tab.binding),
        ))
    }

    fn title(&mut self, tab: &mut Slot) -> WidgetText {
        display_name(&tab.panel, self.panels).into()
    }

    fn ui(&mut self, ui: &mut Ui, tab: &mut Slot) {
        let id = self.id(tab);
        let name = display_name(&tab.panel, self.panels).to_owned();
        let follows = follows_of(&tab.panel, self.panels);
        let selected_plan = self.selected_plan;
        let selected_stockpile = self.selected_stockpile;
        let pin_name = self.pin_name;
        let theme = self.theme;
        // The flag was set by the body on the previous frame, so it says whether the panel had a
        // table then.
        let table_shown = ui
            .data(|data| data.get_temp::<bool>(has_table(id)))
            .unwrap_or(false);
        // The table wrote these on the previous frame, so they say what it showed then.
        let moved = ui
            .data(|data| data.get_temp::<usize>(rows_moved(id)))
            .unwrap_or(0);
        panel_header(ui, theme, &name, |ui| {
            if table_shown && moved > 0 {
                let label = resort_label(moved);
                if add_small_button(ui, theme, &label, ButtonKind::Plain).clicked() {
                    ui.data_mut(|data| data.insert_temp(resort_request(id), true));
                }
            }
            if table_shown
                && add_small_button(ui, theme, "Copy table", ButtonKind::Plain)
                    .on_hover_text("Copies this table for a spreadsheet (tab-separated).")
                    .clicked()
            {
                ui.data_mut(|data| data.insert_temp(copy_request(id), true));
            }
            layout_menu(
                ui,
                theme,
                &name,
                tab,
                follows,
                selected_plan,
                selected_stockpile,
            );
            // The panel's status sits at the left of the header's right end, after its title.
            if tab.binding == Binding::Follow {
                if let Some(text) = follows.and_then(follows_text) {
                    ui.label(RichText::new(text).color(theme.mute));
                }
            } else {
                let text = match pin_name(&tab.binding) {
                    Some(pin) => format!("pinned: {pin}"),
                    None => "pinned: deleted".to_owned(),
                };
                ui.label(RichText::new(text).color(theme.mute));
            }
        });
        // Cleared before the body draws, so a panel whose table stops showing loses the button on
        // the next frame.
        ui.data_mut(|data| data.remove_temp::<bool>(has_table(id)));
        ui.data_mut(|data| data.remove_temp::<usize>(rows_moved(id)));
        let shows_body = follows.is_some()
            && match &tab.binding {
                Binding::Follow => true,
                Binding::PinPlan(_) | Binding::PinStockpile(_) => pin_name(&tab.binding).is_some(),
            };
        if shows_body {
            (self.body)(ui, tab);
        } else if follows.is_some() {
            let (text, follow) = match &tab.binding {
                Binding::PinStockpile(_) => (
                    "The stockpile this panel was pinned to no longer exists.",
                    "Follow the selected stockpile",
                ),
                Binding::PinPlan(_) | Binding::Follow => (
                    "The plan this panel was pinned to no longer exists.",
                    "Follow the selected plan",
                ),
            };
            ui.label(text);
            if ui.button(follow).clicked() {
                tab.binding = Binding::Follow;
            }
        } else {
            ui.label(format!("\"{name}\" is not in this version of the app."));
        }
        // A copy asked for on a panel with no table is dropped, not kept for a table shown later.
        ui.data_mut(|data| data.remove_temp::<bool>(copy_request(id)));
        // The table clears this itself in the same frame; a body with no table leaves it dropped.
        ui.data_mut(|data| data.remove_temp::<bool>(resort_request(id)));
    }
}

/// The egui_dock style with the lock recipe applied on top of the egui look.
fn style(ui: &Ui, theme: &Style) -> egui_dock::Style {
    let mut style = egui_dock::Style::from_egui(ui.style());
    style.buttons.show_tab_bar_size = 0.0;
    style.buttons.show_tab_bar_hover_expand = 0.0;
    // The pages draw their own headers, so the tab body needs no padding of its own; the Panels
    // strip above the dock takes that room without shrinking the tables.
    style.tab.tab_body.inner_margin = Margin::ZERO;
    style.tab.tab_body.corner_radius = CornerRadius::same(theme.radius);
    // Every panel is a card: the panel fill inside a one point outline, on the page's background.
    style.tab.tab_body.bg_fill = theme.panel;
    style.tab.tab_body.stroke = Stroke::new(1.0, theme.line);
    // The gaps between cards show the page, and light up in the line colour while they are used.
    style.separator.width = theme.sizes.sheet_gap;
    style.separator.color_idle = theme.bg;
    style.separator.color_hovered = theme.line;
    style.separator.color_dragged = theme.line;
    style
}

struct Locked<'a, T, B> {
    theme: &'a Style,
    title: fn(T) -> &'static str,
    body: B,
}

impl<T: Copy, B: FnMut(&mut Ui, T)> TabViewer for Locked<'_, T, B> {
    type Tab = T;

    fn id(&mut self, tab: &mut T) -> Id {
        Id::new((self.title)(*tab))
    }

    fn title(&mut self, tab: &mut T) -> WidgetText {
        (self.title)(*tab).into()
    }

    fn ui(&mut self, ui: &mut Ui, tab: &mut T) {
        let id = self.id(tab);
        let title = (self.title)(*tab);
        let theme = self.theme;
        // The flag was set by the body on the previous frame, so it says whether the panel had a
        // table then.
        let table_shown = ui
            .data(|data| data.get_temp::<bool>(has_table(id)))
            .unwrap_or(false);
        // The table wrote these on the previous frame, so they say what it showed then.
        let moved = ui
            .data(|data| data.get_temp::<usize>(rows_moved(id)))
            .unwrap_or(0);
        panel_header(ui, theme, title, |ui| {
            if table_shown && moved > 0 {
                let label = resort_label(moved);
                if add_small_button(ui, theme, &label, ButtonKind::Plain).clicked() {
                    ui.data_mut(|data| data.insert_temp(resort_request(id), true));
                }
            }
            if table_shown
                && add_small_button(ui, theme, "Copy table", ButtonKind::Plain)
                    .on_hover_text("Copies this table for a spreadsheet (tab-separated).")
                    .clicked()
            {
                ui.data_mut(|data| data.insert_temp(copy_request(id), true));
            }
            layout_button(ui, theme, |ui| add_to_layout(ui, title));
        });
        // Cleared before the body draws, so a panel whose table stops showing loses the button on
        // the next frame.
        ui.data_mut(|data| data.remove_temp::<bool>(has_table(id)));
        ui.data_mut(|data| data.remove_temp::<usize>(rows_moved(id)));
        (self.body)(ui, *tab);
        // A copy asked for on a panel with no table is dropped, not kept for a table shown later.
        ui.data_mut(|data| data.remove_temp::<bool>(copy_request(id)));
        // The table clears this itself in the same frame; a body with no table leaves it dropped.
        ui.data_mut(|data| data.remove_temp::<bool>(resort_request(id)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::panels::panels;
    use egui::{Modifiers, PointerButton, Pos2, Rect, pos2, vec2};
    use egui_dock::Node;
    use egui_kittest::Harness;
    use egui_kittest::kittest::{NodeT, Queryable};
    use std::cell::RefCell;
    use std::rc::Rc;
    use tempfile::TempDir;

    const SETUP_PANELS: [SetupPanel; 9] = [
        SetupPanel::Facilities,
        SetupPanel::FreightServices,
        SetupPanel::PriceHistory,
        SetupPanel::PriceRule,
        SetupPanel::ValuePrices,
        SetupPanel::Structures,
        SetupPanel::Fees,
        SetupPanel::Invention,
        SetupPanel::Display,
    ];

    fn store(dir: &TempDir) -> Store {
        Store::open(&dir.path().join("user.sqlite"), 0)
            .expect("the store opens")
            .store
    }

    fn tabs<T: Copy>(state: &DockState<T>) -> Vec<T> {
        state.iter_all_tabs().map(|(_, tab)| *tab).collect()
    }

    #[test]
    fn a_saved_build_list_closes_the_panels_it_does_not_name() {
        let mut dock = build_layout();
        apply_open(
            &mut dock,
            "Plans,Buy list",
            &BUILD_PANELS,
            BuildPanel::title,
        );
        assert_eq!(
            tabs(&dock),
            [BuildPanel::Plans, BuildPanel::BuyList],
            "only the named panels stay open"
        );
        apply_open(
            &mut dock,
            "Plans,Buy list,Item,Foo",
            &BUILD_PANELS,
            BuildPanel::title,
        );
        assert_eq!(
            tabs(&dock),
            [BuildPanel::Plans, BuildPanel::BuyList],
            "Item is not a dock panel and Foo is unknown, so neither changes the page"
        );
    }

    #[test]
    fn toggling_a_panel_off_removes_it_and_a_page_never_empties() {
        let mut dock = build_layout();
        assert!(toggle(&mut dock, BuildPanel::BuyList), "Buy list closes");
        let open = tabs(&dock);
        assert_eq!(open.len(), 3, "three tabs are left");
        assert!(!open.contains(&BuildPanel::BuyList), "Buy list is gone");

        let mut orders = DockState::new(vec![OrdersPanel::Orders]);
        assert!(
            !toggle(&mut orders, OrdersPanel::Orders),
            "the last tab stays"
        );
        assert_eq!(tabs(&orders), [OrdersPanel::Orders]);
    }

    #[test]
    fn characters_tick_off_and_on_again_right_of_the_layout_at_half() {
        let mut dock = characters_layout();
        assert!(
            toggle(&mut dock, CharactersPanel::JeveData),
            "jEve data closes"
        );
        assert_eq!(
            tabs(&dock),
            [CharactersPanel::Characters],
            "one tab is left"
        );

        assert!(
            toggle(&mut dock, CharactersPanel::JeveData),
            "jEve data opens"
        );
        assert_eq!(
            tabs(&dock),
            [CharactersPanel::Characters, CharactersPanel::JeveData],
            "both tabs are back"
        );
        assert_eq!(
            open_text(&dock, &CHARACTERS_PANELS, CharactersPanel::title),
            "Characters,jEve data"
        );
        match dock.main_surface().root_node() {
            Some(Node::Horizontal(split)) => assert_eq!(split.fraction, 0.5),
            _ => panic!("the opened panel splits the whole layout at half"),
        }
    }

    #[test]
    fn with_saved_reads_the_page_panel_list_and_keeps_the_default_without_one() {
        let dir = TempDir::new().expect("a temp folder");
        let mut store = store(&dir);
        store
            .set_setting("panels.characters", "Characters")
            .expect("the panel list saves");

        let saved = with_saved(
            &store,
            "characters",
            characters_layout(),
            &CHARACTERS_PANELS,
            CharactersPanel::title,
        )
        .expect("the panel list reads");
        assert_eq!(tabs(&saved), [CharactersPanel::Characters]);

        let default = with_saved(
            &store,
            "build",
            build_layout(),
            &BUILD_PANELS,
            BuildPanel::title,
        )
        .expect("no setting is not an error");
        assert_eq!(
            tabs(&default),
            [
                BuildPanel::Plans,
                BuildPanel::Plan,
                BuildPanel::Steps,
                BuildPanel::BuyList,
            ],
            "without a setting the page keeps its default layout"
        );

        store
            .set_setting("panels.build", "Plans,Pasted stock,Plan,Steps,Buy list")
            .expect("an old panel list saves");
        let old = with_saved(
            &store,
            "build",
            build_layout(),
            &BUILD_PANELS,
            BuildPanel::title,
        )
        .expect("the old panel list reads");
        assert_eq!(
            tabs(&old),
            [
                BuildPanel::Plans,
                BuildPanel::Plan,
                BuildPanel::Steps,
                BuildPanel::BuyList,
            ],
            "a saved Pasted stock closes nothing"
        );
    }

    #[test]
    fn the_build_layout_holds_four_tabs_in_order_with_its_fractions() {
        let dock = build_layout();
        assert_eq!(
            tabs(&dock),
            [
                BuildPanel::Plans,
                BuildPanel::Plan,
                BuildPanel::Steps,
                BuildPanel::BuyList,
            ]
        );
        let tree = dock.main_surface();
        let Node::Horizontal(root) = &tree[NodeIndex::root()] else {
            panic!("the Plans list splits the root horizontally");
        };
        assert_eq!(root.fraction, 0.2);
        let plan = NodeIndex::root().right();
        let Node::Vertical(split) = &tree[plan] else {
            panic!("Plan sits above Steps and Buy list");
        };
        assert_eq!(split.fraction, 0.35);
        let Node::Horizontal(lower) = &tree[plan.right()] else {
            panic!("Steps and Buy list split below Plan");
        };
        assert_eq!(lower.fraction, 0.5);
    }

    #[test]
    fn closing_the_last_panel_is_refused_and_an_empty_list_keeps_one() {
        let mut dock = build_layout();
        for panel in [BuildPanel::Plans, BuildPanel::Plan, BuildPanel::BuyList] {
            assert!(toggle(&mut dock, panel), "{} closes", panel.title());
        }
        assert_eq!(
            tabs(&dock),
            [BuildPanel::Steps],
            "the last panel stays open"
        );
        assert!(
            !toggle(&mut dock, BuildPanel::Steps),
            "the last tab cannot close"
        );
        assert_eq!(
            open_text(&dock, &BUILD_PANELS, BuildPanel::title),
            "Steps",
            "the remaining panel is the saved list"
        );

        let dir = TempDir::new().expect("a temp folder");
        let mut store = store(&dir);
        store
            .set_setting("panels.build", "")
            .expect("the empty panel list saves");
        let saved = with_saved(
            &store,
            "build",
            build_layout(),
            &BUILD_PANELS,
            BuildPanel::title,
        )
        .expect("the empty panel list reads");
        assert_eq!(
            tabs(&saved),
            [BuildPanel::BuyList],
            "an empty list closes panels until one is left"
        );
    }

    #[test]
    fn a_saved_text_holds_only_the_pages_own_titles() {
        let mut dock = characters_layout();
        apply_open(
            &mut dock,
            "Characters,Item,Foo,Buy list,, jEve data ",
            &CHARACTERS_PANELS,
            CharactersPanel::title,
        );
        assert_eq!(
            open_text(&dock, &CHARACTERS_PANELS, CharactersPanel::title),
            "Characters,jEve data",
            "names outside the page and empty parts are ignored"
        );
    }

    #[test]
    fn a_name_with_no_match_leaves_the_page_alone() {
        let mut dock = characters_layout();
        apply_open(
            &mut dock,
            " , Foo,",
            &CHARACTERS_PANELS,
            CharactersPanel::title,
        );
        assert_eq!(
            tabs(&dock),
            [CharactersPanel::Characters, CharactersPanel::JeveData],
            "a corrupt list does not empty the page"
        );
    }

    #[test]
    fn the_setup_layout_holds_each_panel_exactly_once_including_display() {
        let dock = setup_layout();
        let panels: Vec<SetupPanel> = dock.iter_all_tabs().map(|(_, panel)| *panel).collect();
        assert_eq!(panels.len(), SETUP_PANELS.len());
        for panel in SETUP_PANELS {
            let count = panels.iter().filter(|shown| **shown == panel).count();
            assert_eq!(count, 1, "{} appears {count} times", panel.title());
        }
    }

    #[test]
    fn invention_sits_in_the_fees_node_right_after_it() {
        let dock = setup_layout();
        let fees = dock
            .iter_leaves()
            .find(|(_, leaf)| leaf.tabs.contains(&SetupPanel::Fees))
            .map(|(_, leaf)| leaf)
            .expect("Fees is in the layout");
        assert_eq!(
            fees.tabs,
            [
                SetupPanel::Structures,
                SetupPanel::Fees,
                SetupPanel::Invention,
            ],
            "Invention is the tab after Fees and skills"
        );
    }

    #[test]
    fn value_prices_sits_in_the_price_rule_node_right_after_it() {
        let dock = setup_layout();
        let price = dock
            .iter_leaves()
            .find(|(_, leaf)| leaf.tabs.contains(&SetupPanel::PriceRule))
            .map(|(_, leaf)| leaf)
            .expect("Price rule is in the layout");
        assert_eq!(
            price.tabs,
            [SetupPanel::PriceRule, SetupPanel::ValuePrices],
            "Value prices is the tab after Price rule"
        );
    }

    #[test]
    fn the_setup_panel_titles_are_unchanged_and_display_has_its_own() {
        assert_eq!(SetupPanel::Facilities.title(), "Facilities");
        assert_eq!(SetupPanel::FreightServices.title(), "Freight services");
        assert_eq!(SetupPanel::PriceHistory.title(), "Price history");
        assert_eq!(SetupPanel::PriceRule.title(), "Price rule");
        assert_eq!(SetupPanel::Structures.title(), "Structures");
        assert_eq!(SetupPanel::Fees.title(), "Fees and skills");
        assert_eq!(SetupPanel::Display.title(), "Display");
    }

    /// The state the user-layout tests keep: the dock, the slots `body` was called for, the
    /// selection a plan panel can pin to, and the name of every pin that still exists.
    struct User {
        dock: DockState<Slot>,
        bodies: Vec<Slot>,
        selected_plan: Option<(i64, &'static str)>,
        pin: Option<String>,
    }

    /// The layout Buy list left, Steps pinned to plan 1 right.
    fn buy_list_steps_dock() -> DockState<Slot> {
        user_dock(
            "EVE Means of Profit layout 1\nname: Test\nsplit right 50%\n  Buy list\n  Steps [pinned plan 1]\n",
        )
    }

    fn user_dock(text: &str) -> DockState<Slot> {
        let parsed =
            crate::layout::text::parse_saved(text, panels()).expect("the test layout reads");
        crate::layout::dock_convert::to_dock(&parsed.layout)
    }

    fn user_harness(
        dock: DockState<Slot>,
        selected_plan: Option<(i64, &'static str)>,
        pin: Option<&str>,
    ) -> Harness<'static, User> {
        let style = Style::eve_dense();
        Harness::builder()
            .with_size(vec2(800.0, 600.0))
            .build_ui_state(
                move |ui, user: &mut User| {
                    style.apply(ui.ctx());
                    let User {
                        dock,
                        bodies,
                        selected_plan,
                        pin,
                    } = user;
                    let pin_name = |binding: &Binding| match binding {
                        Binding::PinPlan(_) | Binding::PinStockpile(_) => pin.clone(),
                        Binding::Follow => None,
                    };
                    let names = UserNames {
                        selected_plan: *selected_plan,
                        selected_stockpile: None,
                        pin_name: &pin_name,
                    };
                    show_user(ui, &style, dock, panels(), &names, |ui, slot| {
                        bodies.push(slot.clone());
                        ui.label("body");
                    });
                },
                User {
                    dock,
                    bodies: Vec::new(),
                    selected_plan,
                    pin: pin.map(str::to_owned),
                },
            )
    }

    fn shown<S>(harness: &Harness<'_, S>, label: &str) -> bool {
        harness.query_by_label(label).is_some()
    }

    fn click<S>(harness: &mut Harness<'_, S>, label: &str) {
        harness.get_by_label(label).click();
        harness.run_steps(3);
    }

    /// Clicks the `Layout` menu of the panel whose title is `panel`, not the one of another panel
    /// that happens to be in the same dock. The header title sits under the tab title, and the
    /// panel's menu is the nearest one to its right; the pin tests then assert on the saved slot
    /// that the menu clicked was this panel's.
    fn click_layout_of(harness: &mut Harness<'_, User>, panel: &str) {
        let title = text_rects(harness, panel)
            .into_iter()
            .max_by(|a, b| a.top().total_cmp(&b.top()))
            .map(|rect| rect.center())
            .unwrap_or_else(|| panic!("\"{panel}\" is not painted"));
        let button = harness
            .get_all_by_label("Layout")
            .filter(|button| button.rect().center().x >= title.x)
            .min_by(|a, b| {
                let da = a.rect().center().distance(title);
                let db = b.rect().center().distance(title);
                da.total_cmp(&db)
            })
            .expect("the panel has a Layout menu");
        button.click();
        harness.run_steps(3);
    }

    fn locked_harness(dock: DockState<BuildPanel>) -> Harness<'static, DockState<BuildPanel>> {
        let style = Style::eve_dense();
        Harness::builder()
            .with_size(vec2(800.0, 600.0))
            .build_ui_state(
                move |ui, dock: &mut DockState<BuildPanel>| {
                    style.apply(ui.ctx());
                    show_locked(ui, &style, dock, BuildPanel::title, |_, _| {});
                },
                dock,
            )
    }

    /// A locked page whose only panel has a table, so its header shows the table buttons. The body
    /// says the panel has a table and draws the table's first row.
    fn table_harness() -> Harness<'static, DockState<BuildPanel>> {
        let style = Style::eve_dense();
        Harness::builder()
            .with_size(vec2(800.0, 600.0))
            .build_ui_state(
                move |ui, dock: &mut DockState<BuildPanel>| {
                    style.apply(ui.ctx());
                    show_locked(ui, &style, dock, BuildPanel::title, |ui, panel| {
                        let id = Id::new(BuildPanel::title(panel));
                        ui.data_mut(|data| data.insert_temp(has_table(id), true));
                        ui.label("first row");
                    });
                },
                DockState::new(vec![BuildPanel::BuyList]),
            )
    }

    #[test]
    fn a_panel_is_a_card_on_the_page_background() {
        let theme = Style::eve_dense();
        let drawn = Rc::new(RefCell::new(None));
        let out = Rc::clone(&drawn);
        let mut harness = Harness::builder()
            .with_size(vec2(400.0, 200.0))
            .build_ui(move |ui| {
                *out.borrow_mut() = Some(style(ui, &theme));
            });
        harness.run_steps(1);
        let drawn = drawn.borrow();
        let drawn = drawn.as_ref().expect("the dock style is drawn");
        assert_eq!(drawn.tab.tab_body.bg_fill, theme.panel);
        assert_eq!(
            drawn.tab.tab_body.stroke,
            Stroke::new(1.0, theme.line),
            "the card has a one point outline"
        );
        assert_eq!(drawn.tab.tab_body.corner_radius, CornerRadius::same(12));
        assert_eq!(drawn.tab.tab_body.inner_margin, Margin::ZERO);
        assert_eq!(drawn.separator.width, 10.0);
        assert_eq!(
            drawn.separator.color_idle, theme.bg,
            "the gaps show the page"
        );
        assert_eq!(drawn.separator.color_hovered, theme.line);
        assert_eq!(drawn.separator.color_dragged, theme.line);
    }

    #[test]
    fn a_locked_panel_header_has_its_title_and_small_buttons_in_a_card_row() {
        let mut harness = table_harness();
        harness.run_steps(3);

        let title = harness.get_by_label("Buy list").rect();
        let layout = harness.get_by_label("Layout").rect();
        let copy = harness.get_by_label("Copy table").rect();
        assert_eq!(layout.height(), 24.0, "Layout is a small button");
        assert_eq!(copy.height(), 24.0, "Copy table is a small button");
        assert!(
            copy.left() > title.right(),
            "Copy table sits at the right end of the title row"
        );
        assert!(
            layout.left() < copy.left(),
            "Layout keeps its place before Copy table"
        );

        let top = harness
            .state()
            .iter_leaves()
            .next()
            .map(|(_, leaf)| leaf.rect.top())
            .expect("the page has one panel");
        assert_eq!(
            harness.get_by_label("first row").rect().top(),
            top + 40.0,
            "the table starts under the card's stroke, title row and line"
        );
    }

    /// The rect of the topmost painted text equal to `text`. Tab titles are painted, not widgets,
    /// so this is how a test finds a tab.
    fn text_rect(harness: &Harness<'_, User>, text: &str) -> Rect {
        text_rects(harness, text)
            .into_iter()
            .min_by(|a, b| a.top().total_cmp(&b.top()))
            .unwrap_or_else(|| panic!("\"{text}\" is not painted"))
    }

    /// Every rect of painted text equal to `text`, in paint order.
    fn text_rects(harness: &Harness<'_, User>, text: &str) -> Vec<Rect> {
        fn collect(shape: &egui::Shape, text: &str, found: &mut Vec<Rect>) {
            match shape {
                egui::Shape::Text(painted) if painted.galley.text() == text => {
                    found.push(Rect::from_min_size(painted.pos, painted.galley.size()));
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        collect(shape, text, found);
                    }
                }
                egui::Shape::Noop
                | egui::Shape::Circle(_)
                | egui::Shape::Ellipse(_)
                | egui::Shape::LineSegment { .. }
                | egui::Shape::Path(_)
                | egui::Shape::Rect(_)
                | egui::Shape::Text(_)
                | egui::Shape::Mesh(_)
                | egui::Shape::QuadraticBezier(_)
                | egui::Shape::CubicBezier(_)
                | egui::Shape::Callback(_) => {}
            }
        }
        let mut found = Vec::new();
        for clipped in &harness.output().shapes {
            collect(&clipped.shape, text, &mut found);
        }
        found
    }

    fn click_at(harness: &mut Harness<'_, User>, pos: Pos2) {
        harness.event(egui::Event::PointerMoved(pos));
        for pressed in [true, false] {
            harness.event(egui::Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            });
        }
        harness.run_steps(3);
    }

    /// Clicks the close button at the right of the tab titled `name`. The close button is 24 points
    /// wide with 8 points of padding before the title, so its centre sits 20 points right of the
    /// painted title.
    fn close_tab(harness: &mut Harness<'_, User>, name: &str) {
        let title = text_rect(harness, name);
        click_at(harness, pos2(title.right() + 20.0, title.center().y));
    }

    fn saved(dock: &DockState<Slot>) -> String {
        crate::layout::text::to_text(
            &crate::layout::dock_convert::from_dock(dock, "Test"),
            panels(),
            crate::layout::text::Mode::Saved,
        )
    }

    /// The text below the header and name lines.
    fn layout_part(text: &str) -> &str {
        text.splitn(3, '\n').nth(2).unwrap_or_default()
    }

    #[test]
    fn headers_say_whether_each_panel_follows_or_is_pinned() {
        let mut harness = user_harness(
            buy_list_steps_dock(),
            Some((2, "Rifter 31")),
            Some("Hobgoblin 10"),
        );
        harness.run_steps(2);

        assert!(shown(&harness, "follows plan"), "Buy list follows the plan");
        assert!(
            shown(&harness, "pinned: Hobgoblin 10"),
            "Steps names the plan it is pinned to"
        );
        let buy_list = Slot {
            panel: PanelRef::Known("build.buy_list"),
            binding: Binding::Follow,
        };
        let steps = Slot {
            panel: PanelRef::Known("build.steps"),
            binding: Binding::PinPlan(1),
        };
        assert!(
            harness.state().bodies.contains(&buy_list),
            "body draws Buy list while it follows"
        );
        assert!(
            harness.state().bodies.contains(&steps),
            "body draws Steps while its pin exists"
        );
    }

    #[test]
    fn the_layout_menu_pins_to_the_selected_plan_and_follows_again() {
        let mut harness = user_harness(
            user_dock(
                "EVE Means of Profit layout 1\nname: Test\nsplit right 50%\n  Plans\n  Buy list\n",
            ),
            Some((2, "Rifter 31")),
            None,
        );
        harness.run_steps(2);
        click_layout_of(&mut harness, "Buy list");
        click(&mut harness, "Pin to \"Rifter 31\"");
        assert!(
            saved(&harness.state().dock).ends_with("  Buy list [pinned plan 2]\n"),
            "the pin is written into the slot: {}",
            saved(&harness.state().dock)
        );

        let mut harness = user_harness(
            user_dock(
                "EVE Means of Profit layout 1\nname: Test\nsplit right 50%\n  Plans\n  Steps [pinned plan 1]\n",
            ),
            Some((2, "Rifter 31")),
            Some("Hobgoblin 10"),
        );
        harness.run_steps(2);
        click_layout_of(&mut harness, "Steps");
        click(&mut harness, "Follow the selected plan");
        assert!(
            saved(&harness.state().dock).ends_with("  Steps\n"),
            "following drops the pin: {}",
            saved(&harness.state().dock)
        );
    }

    #[test]
    fn the_layout_helper_clicks_the_named_panels_menu() {
        let mut harness = user_harness(
            user_dock(
                "EVE Means of Profit layout 1\nname: Test\nsplit right 50%\n  Plans\n  Buy list\n",
            ),
            Some((2, "Rifter 31")),
            None,
        );
        harness.run_steps(2);
        click_layout_of(&mut harness, "Buy list");
        assert!(
            shown(&harness, "Pin to \"Rifter 31\""),
            "the Buy list menu opened, not the Plans one"
        );
    }

    #[test]
    fn the_pin_button_is_disabled_without_a_selected_plan() {
        let mut harness = user_harness(
            user_dock("EVE Means of Profit layout 1\nname: Test\nBuy list\n"),
            None,
            None,
        );
        harness.run_steps(2);
        click(&mut harness, "Layout");
        let pin = harness.get_by_label("Pin to the selected plan");
        assert!(
            pin.accesskit_node().is_disabled(),
            "no plan is selected to pin to"
        );
    }

    #[test]
    fn a_panel_that_follows_nothing_still_offers_add_to_layout() {
        let mut harness = user_harness(
            user_dock("EVE Means of Profit layout 1\nname: Test\nPlans\n"),
            Some((2, "Rifter 31")),
            None,
        );
        harness.run_steps(2);
        assert!(shown(&harness, "Layout"), "every panel can be added");
        click(&mut harness, "Layout");
        assert!(shown(&harness, "Add to layout…"));
    }

    #[test]
    fn a_locked_page_layout_menu_asks_to_add_the_panel() {
        let mut harness = locked_harness(DockState::new(vec![BuildPanel::BuyList]));
        harness.run_steps(2);
        click(&mut harness, "Layout");
        click(&mut harness, "Add to layout…");
        assert_eq!(take_add_request(&harness.ctx), Some("Buy list".to_owned()));
        assert_eq!(take_add_request(&harness.ctx), None);
    }

    #[test]
    fn a_user_dock_layout_menu_asks_to_add_the_panel() {
        let mut harness = user_harness(
            user_dock("EVE Means of Profit layout 1\nname: Test\nBuy list\n"),
            Some((2, "Rifter 31")),
            None,
        );
        harness.run_steps(2);
        click(&mut harness, "Layout");
        click(&mut harness, "Add to layout…");
        assert_eq!(take_add_request(&harness.ctx), Some("Buy list".to_owned()));
        assert_eq!(take_add_request(&harness.ctx), None);
    }

    #[test]
    fn closing_every_tab_empties_the_layout_and_stops_the_body() {
        let mut harness = user_harness(
            buy_list_steps_dock(),
            Some((2, "Rifter 31")),
            Some("Hobgoblin 10"),
        );
        harness.run_steps(2);
        close_tab(&mut harness, "Buy list");
        assert_eq!(
            layout_part(&saved(&harness.state().dock)),
            "Steps [pinned plan 1]\n",
            "only Steps is left, now at the root"
        );

        close_tab(&mut harness, "Steps");
        assert_eq!(
            layout_part(&saved(&harness.state().dock)),
            "empty\n",
            "the layout is empty"
        );
        assert!(
            shown(&harness, EMPTY_LAYOUT),
            "the empty layout shows how to add panels"
        );
        harness.state_mut().bodies.clear();
        harness.run_steps(2);
        assert!(
            harness.state().bodies.is_empty(),
            "an empty layout calls no body"
        );
    }

    #[test]
    fn dragging_the_steps_tab_onto_the_lower_edge_of_buy_list_splits_down() {
        let mut harness = user_harness(
            buy_list_steps_dock(),
            Some((2, "Rifter 31")),
            Some("Hobgoblin 10"),
        );
        harness.run_steps(2);
        let start = text_rect(&harness, "Steps").center();
        let target = pos2(200.0, 580.0);
        harness.hover_at(start);
        harness.run_steps(2);
        harness.drag_at(start);
        harness.run_steps(2);
        // egui_dock starts the drag where the pointer first moved, so move in two steps: the
        // first sets the start, the second passes its threshold and records the dragged tab.
        harness.hover_at(start + vec2(40.0, 0.0));
        harness.run_steps(2);
        harness.hover_at(target);
        harness.run_steps(3);
        harness.drop_at(target);
        harness.run_steps(3);

        let layout = crate::layout::dock_convert::from_dock(&harness.state().dock, "Test");
        let Some(crate::layout::model::Node::Split { dir, first, .. }) = &layout.root else {
            panic!(
                "the drag splits the layout: {}",
                saved(&harness.state().dock)
            );
        };
        assert_eq!(*dir, crate::layout::model::Dir::Down, "Steps lands below");
        assert!(
            matches!(
                first.as_ref(),
                crate::layout::model::Node::Panel(Slot {
                    binding: Binding::Follow,
                    ..
                })
            ),
            "Buy list stays above"
        );
    }

    #[test]
    fn a_pin_that_is_gone_says_so_and_follows_again() {
        let mut harness = user_harness(
            user_dock(
                "EVE Means of Profit layout 1\nname: Test\nsplit right 50%\n  Plans\n  Steps [pinned plan 1]\n",
            ),
            Some((2, "Rifter 31")),
            None,
        );
        harness.run_steps(2);
        assert!(
            shown(
                &harness,
                "The plan this panel was pinned to no longer exists."
            ),
            "a gone pin is named as gone"
        );
        assert!(
            !harness
                .state()
                .bodies
                .iter()
                .any(|slot| slot.panel == PanelRef::Known("build.steps")),
            "a panel pinned to something gone draws no body"
        );
        assert!(shown(&harness, "pinned: deleted"), "the header says so too");

        click(&mut harness, "Follow the selected plan");
        assert!(
            saved(&harness.state().dock).ends_with("  Steps\n"),
            "following drops the gone pin: {}",
            saved(&harness.state().dock)
        );
    }

    #[test]
    fn an_unknown_panel_says_it_is_not_in_this_version_and_draws_no_body() {
        let mut harness = user_harness(
            user_dock("EVE Means of Profit layout 1\nname: Test\nMarket depth\n"),
            Some((2, "Rifter 31")),
            None,
        );
        harness.run_steps(2);
        assert!(
            shown(
                &harness,
                "\"Market depth\" is not in this version of the app."
            ),
            "the unknown panel is named"
        );
        assert!(
            harness.state().bodies.is_empty(),
            "an unknown panel draws no body"
        );
        assert!(
            text_rect(&harness, "Market depth").top() < 100.0,
            "the unknown panel still has its tab at the top"
        );
    }
}

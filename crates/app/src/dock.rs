//! Pages as locked egui_dock layouts. The lock recipe is in the design note (section 5.1): tab
//! bars hidden, tabs not draggable, no close buttons, no tab menus, and the show-tab-bar button
//! kept at size 0. Each panel draws its own header with its name and, when its body has a table, a
//! button that copies the table.

use egui::{CornerRadius, Id, RichText, Ui, WidgetText};
use egui_dock::{DockArea, DockState, NodeIndex, TabViewer};

use crate::Shared;
use crate::assets::AssetsPanel;
use crate::build::BuildPanel;
use crate::characters::CharactersPanel;
use crate::jobs::JobsPanel;
use crate::net_worth::NetWorthPanel;
use crate::orders::OrdersPanel;
use crate::setup::SetupPanel;
use crate::table::{copy_request, has_table};
use crate::theme::Style;

/// The Build page: Plans over Pasted stock on the left; Plan on top, then Steps and Buy list, with
/// Item on the right of them.
pub fn build_layout() -> DockState<BuildPanel> {
    let mut dock = DockState::new(vec![BuildPanel::Plans]);
    let surface = dock.main_surface_mut();
    let [plans, plan] = surface.split_right(NodeIndex::root(), 0.4, vec![BuildPanel::Plan]);
    surface.split_below(plans, 0.5, vec![BuildPanel::PastedStock]);
    let [_plan, lower] = surface.split_below(plan, 0.45, vec![BuildPanel::Steps]);
    let [steps, _item] = surface.split_right(lower, 0.75, vec![BuildPanel::Item]);
    surface.split_right(steps, 0.5, vec![BuildPanel::BuyList]);
    dock
}

/// The Assets page: Places, Assets and Item side by side, at 25%, 50% and 25%.
pub fn assets_layout() -> DockState<AssetsPanel> {
    let mut dock = DockState::new(vec![AssetsPanel::Places]);
    let surface = dock.main_surface_mut();
    let [_places, rest] = surface.split_right(NodeIndex::root(), 0.25, vec![AssetsPanel::Assets]);
    surface.split_right(rest, 2.0 / 3.0, vec![AssetsPanel::Item]);
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

/// The Setup page: Facilities over Price history on the left; Freight services over Price rule over Structures on the right.
pub fn setup_layout() -> DockState<SetupPanel> {
    let mut dock = DockState::new(vec![SetupPanel::Facilities]);
    let surface = dock.main_surface_mut();
    let [facilities, freight] =
        surface.split_right(NodeIndex::root(), 0.5, vec![SetupPanel::FreightServices]);
    surface.split_below(facilities, 0.5, vec![SetupPanel::PriceHistory]);
    let [_freight, price] = surface.split_below(freight, 0.5, vec![SetupPanel::PriceRule]);
    surface.split_below(price, 0.5, vec![SetupPanel::Structures, SetupPanel::Fees]);
    dock
}

/// The body of a panel whose page is not built yet. The header already names the panel, so the body
/// shows only the loading text until the saved character data has arrived.
pub fn placeholder(ui: &mut Ui, cx: &Shared) {
    if !cx.session.loaded() {
        ui.label(RichText::new("Loading character data…").color(cx.style.mute));
    }
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
    let mut viewer = Locked { title, body };
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

/// The egui_dock style with the lock recipe applied on top of the egui look.
fn style(ui: &Ui, theme: &Style) -> egui_dock::Style {
    let mut style = egui_dock::Style::from_egui(ui.style());
    style.buttons.show_tab_bar_size = 0.0;
    style.buttons.show_tab_bar_hover_expand = 0.0;
    style.tab.tab_body.corner_radius = CornerRadius::same(theme.radius);
    style
}

struct Locked<T, B> {
    title: fn(T) -> &'static str,
    body: B,
}

impl<T: Copy, B: FnMut(&mut Ui, T)> TabViewer for Locked<T, B> {
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
        // The flag was set by the body on the previous frame, so it says whether the panel had a
        // table then.
        let table_shown = ui
            .data(|data| data.get_temp::<bool>(has_table(id)))
            .unwrap_or(false);
        ui.horizontal(|ui| {
            ui.strong(title);
            if table_shown
                && ui
                    .button("Copy table")
                    .on_hover_text("Copies this table for a spreadsheet (tab-separated).")
                    .clicked()
            {
                ui.data_mut(|data| data.insert_temp(copy_request(id), true));
            }
        });
        // Cleared before the body draws, so a panel whose table stops showing loses the button on
        // the next frame.
        ui.data_mut(|data| data.remove_temp::<bool>(has_table(id)));
        (self.body)(ui, *tab);
        // A copy asked for on a panel with no table is dropped, not kept for a table shown later.
        ui.data_mut(|data| data.remove_temp::<bool>(copy_request(id)));
    }
}

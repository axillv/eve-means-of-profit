//! Pages as locked egui_dock layouts. The lock recipe is in the design note (section 5.1): tab
//! bars hidden, tabs not draggable, no close buttons, no tab menus, and the show-tab-bar button
//! kept at size 0. Each panel draws its own header with its name and a menu button.

use egui::{CornerRadius, Id, Ui, WidgetText};
use egui_dock::{DockArea, DockState, NodeIndex, TabViewer};

use crate::build::BuildPanel;
use crate::setup::SetupPanel;
use crate::table::copy_request;
use crate::theme::Style;

/// The Build page: Plans over Pasted stock on the left; Plan on top, then Steps beside Buy list.
pub fn build_layout() -> DockState<BuildPanel> {
    let mut dock = DockState::new(vec![BuildPanel::Plans]);
    let surface = dock.main_surface_mut();
    let [plans, plan] = surface.split_right(NodeIndex::root(), 0.4, vec![BuildPanel::Plan]);
    surface.split_below(plans, 0.5, vec![BuildPanel::PastedStock]);
    let [_plan, lower] = surface.split_below(plan, 0.45, vec![BuildPanel::Steps]);
    surface.split_right(lower, 0.5, vec![BuildPanel::BuyList]);
    dock
}

/// The Setup page: Facilities on the left; Freight services over Price rule on the right.
pub fn setup_layout() -> DockState<SetupPanel> {
    let mut dock = DockState::new(vec![SetupPanel::Facilities]);
    let surface = dock.main_surface_mut();
    let [_facilities, freight] =
        surface.split_right(NodeIndex::root(), 0.5, vec![SetupPanel::FreightServices]);
    surface.split_below(freight, 0.5, vec![SetupPanel::PriceRule]);
    dock
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
        if let Some(leaf) = node.get_leaf_mut() {
            leaf.tab_bar_hidden = true;
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
        ui.horizontal(|ui| {
            ui.strong(title);
            ui.menu_button("⋯", |ui| {
                if ui.button("Copy as TSV").clicked() {
                    ui.data_mut(|data| data.insert_temp(copy_request(id), true));
                    ui.close();
                }
            });
        });
        (self.body)(ui, *tab);
        // A copy asked for on a panel with no table is dropped, not kept for a table shown later.
        ui.data_mut(|data| data.remove_temp::<bool>(copy_request(id)));
    }
}

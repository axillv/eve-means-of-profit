//! Pages as locked egui_dock layouts. The lock recipe is in the design note (section 5.1): tab
//! bars hidden, tabs not draggable, no close buttons, no tab menus, and the show-tab-bar button
//! kept at size 0. Each panel draws its own header with its name and, when its body has a table, a
//! button that copies the table.

use egui::{CornerRadius, Id, RichText, Ui, WidgetText};
use egui_dock::{DockArea, DockState, NodeIndex, TabViewer};
use mop_store::Store;
use mop_store::StoreError;

use crate::Shared;
use crate::assets::AssetsPanel;
use crate::build::BuildPanel;
use crate::characters::CharactersPanel;
use crate::jobs::JobsPanel;
use crate::net_worth::NetWorthPanel;
use crate::orders::OrdersPanel;
use crate::setup::SetupPanel;
use crate::stockpiles::StockpilesPanel;
use crate::table::{copy_request, has_table};
use crate::theme::Style;

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
                ui.spacing_mut().button_padding = egui::vec2(6.0, 0.0);
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
    // The pages draw their own headers, so the tab body needs no padding of its own; the Panels
    // strip above the dock takes that room without shrinking the tables.
    style.tab.tab_body.inner_margin = egui::Margin::ZERO;
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

#[cfg(test)]
mod tests {
    use super::*;
    use egui_dock::Node;
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
}

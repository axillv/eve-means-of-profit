//! The rail on the left: one entry per shipped page, grouped by what the player does, then the
//! player's layouts. The entry on show has the accent; a right-click acts on its entry.

use egui::{
    Button, Color32, RichText, Sense, Stroke, TextStyle, TextWrapMode, Ui, WidgetInfo, WidgetType,
    vec2,
};
use egui_phosphor::regular;

use crate::layout::model::MAX_NAME;
use crate::layout::saved::unique_name;
use crate::theme::Style;

/// Empty space above and below a line between groups.
const GROUP_SPACE: f32 = 8.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Today,
    Build,
    Stockpiles,
    Assets,
    NetWorth,
    Orders,
    Jobs,
    Characters,
    Setup,
}

impl Page {
    pub const ALL: [Page; 9] = [
        Page::Today,
        Page::Build,
        Page::Stockpiles,
        Page::Jobs,
        Page::Orders,
        Page::Assets,
        Page::NetWorth,
        Page::Characters,
        Page::Setup,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Today => "Today",
            Self::Build => "Build",
            Self::Stockpiles => "Stockpiles",
            Self::Assets => "Assets",
            Self::NetWorth => "Net worth",
            Self::Orders => "Orders",
            Self::Jobs => "Industry jobs",
            Self::Characters => "Characters",
            Self::Setup => "Settings",
        }
    }

    /// The icon that sits before the label.
    fn icon(self) -> &'static str {
        match self {
            Self::Today => regular::SUN,
            Self::Build => regular::HAMMER,
            Self::Stockpiles => regular::STACK,
            Self::Assets => regular::PACKAGE,
            Self::NetWorth => regular::CHART_LINE_UP,
            Self::Orders => regular::STOREFRONT,
            Self::Jobs => regular::FACTORY,
            Self::Characters => regular::USERS,
            Self::Setup => regular::GEAR,
        }
    }

    /// Whether an entry starts a group, and so has a line above it.
    fn starts_group(self) -> bool {
        matches!(self, Self::Build | Self::Assets | Self::Characters)
    }
}

/// The width of `text` at the Button text size.
fn text_width(ui: &Ui, text: &str) -> f32 {
    let font = TextStyle::Button.resolve(ui.style());
    ui.painter()
        .layout_no_wrap(text.to_owned(), font, Color32::PLACEHOLDER)
        .size()
        .x
}

/// The space a single word of every label needs, at the Button text size.
fn widest_word(ui: &Ui) -> f32 {
    Page::ALL
        .iter()
        .flat_map(|page| page.label().split(' '))
        .map(|word| text_width(ui, word))
        .fold(0.0, f32::max)
}

/// The width of the rail: the widest icon, the gap between icon and label, the widest single word
/// of every label, and the entry and panel padding.
fn rail_width(ui: &Ui) -> f32 {
    let icon = Page::ALL
        .iter()
        .map(|page| text_width(ui, page.icon()))
        .fold(0.0, f32::max);
    let panel_padding = egui::Frame::side_top_panel(ui.style())
        .total_margin()
        .sum()
        .x;
    icon + ui.spacing().icon_spacing
        + widest_word(ui)
        + 2.0 * ui.spacing().button_padding.x
        + panel_padding
}

/// Breaks `label` into at most two rows, only at its spaces, filling each row greedily at the
/// Button text size. A single word wider than `width` stays whole.
pub(crate) fn label_rows(ui: &Ui, label: &str, width: f32) -> Vec<String> {
    let mut rows: Vec<String> = Vec::new();
    for word in label.split(' ') {
        if rows.len() < 2
            && let Some(last) = rows.last_mut()
        {
            let joined = format!("{last} {word}");
            if text_width(ui, &joined) <= width {
                *last = joined;
                continue;
            }
        }
        if rows.len() == 2 {
            if let Some(last) = rows.last_mut() {
                last.push(' ');
                last.push_str(word);
            }
        } else {
            rows.push(word.to_owned());
        }
    }
    rows
}

/// Which entry of the rail is on show: a shipped page, or the layout at that index in the names
/// the rail was given.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shown {
    Page(Page),
    Layout(usize),
}

/// What a right-click on the rail asks for. `index` is a position in the names the rail was given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RailAction {
    SaveCopy(Page),
    Copy(usize),
    SaveToFile(usize),
    Rename { index: usize, name: String },
    Delete(usize),
}

/// The sizes every entry of one rail shares.
#[derive(Clone, Copy)]
struct Metrics {
    content_width: f32,
    label_width: f32,
    icons_only: bool,
}

/// The state of the rename window, in egui memory between frames.
#[derive(Clone)]
struct RenameState {
    index: usize,
    name: String,
    error: Option<String>,
}

/// The state of the delete window, in egui memory between frames.
#[derive(Clone, Copy)]
struct DeleteState {
    index: usize,
}

fn rename_id() -> egui::Id {
    egui::Id::new("rail rename")
}

fn delete_id() -> egui::Id {
    egui::Id::new("rail delete")
}

/// Draws the rail. A click on an entry makes it the page on show.
pub fn show(ui: &mut Ui, style: &Style, page: &mut Page) {
    page_rail(ui, style, page, false);
}

/// Draws the rail, with either each entry's icon and label or, when `icons_only`, the icon alone
/// and the label as hover text.
pub fn show_with(ui: &mut Ui, style: &Style, page: &mut Page, icons_only: bool) {
    page_rail(ui, style, page, icons_only);
}

/// Draws the rail for `page` alone; a click on a page entry changes `page`.
fn page_rail(ui: &mut Ui, style: &Style, page: &mut Page, icons_only: bool) {
    let mut shown = Shown::Page(*page);
    let _action = layouts_rail(ui, style, &mut shown, &[], icons_only);
    if let Shown::Page(next) = shown {
        *page = next;
    }
}

/// Draws the rail with the player's layouts below the shipped pages, and returns the action a
/// right-click asked for. `names` are the layout names, in the order the rail lists them.
pub fn show_with_layouts(
    ui: &mut Ui,
    style: &Style,
    shown: &mut Shown,
    names: &[String],
) -> Option<RailAction> {
    layouts_rail(ui, style, shown, names, false)
}

/// Draws the rail with the layouts, either labelled or, when `icons_only`, as icons with hover
/// text.
fn layouts_rail(
    ui: &mut Ui,
    style: &Style,
    shown: &mut Shown,
    names: &[String],
    icons_only: bool,
) -> Option<RailAction> {
    // A page asked for by an empty state takes effect next frame, since the rail draws first.
    if let Some(next) = crate::empty::take(ui.ctx()) {
        *shown = Shown::Page(next);
    }
    let width = if icons_only {
        style.rail_narrow
    } else {
        rail_width(ui)
    };
    let content_width = width
        - egui::Frame::side_top_panel(ui.style())
            .total_margin()
            .sum()
            .x;
    let metrics = Metrics {
        content_width,
        label_width: widest_word(ui),
        icons_only,
    };
    let mut action = None;

    egui::Panel::left("rail")
        .exact_size(width)
        .resizable(false)
        .show(ui, |ui| {
            if icons_only {
                ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
                    entries(ui, style, shown, names, metrics, &mut action);
                });
            } else {
                entries(ui, style, shown, names, metrics, &mut action);
            }
        });

    action
        .or_else(|| rename_window(ui, style, names))
        .or_else(|| delete_window(ui, names))
}

/// Draws the line between groups, with space above and below.
fn divider(ui: &mut Ui, style: &Style) {
    ui.add_space(GROUP_SPACE);
    let (line, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().hline(
        line.x_range(),
        line.center().y,
        Stroke::new(1.0, style.line),
    );
    ui.add_space(GROUP_SPACE);
}

/// Draws one rail entry, accented when it is the one on show, and returns its response.
fn entry_button(
    ui: &mut Ui,
    style: &Style,
    metrics: Metrics,
    icon: &str,
    label: &str,
    selected: bool,
) -> egui::Response {
    let (fill, text) = if selected {
        (style.accent, style.on_accent)
    } else {
        (style.row, style.text)
    };
    let drawn = if metrics.icons_only {
        Button::new(RichText::new(icon).color(text))
    } else {
        let rows = label_rows(ui, label, metrics.label_width).join("\n");
        Button::new((
            RichText::new(icon).color(text),
            RichText::new(rows).color(text),
        ))
    };
    let button = drawn
        .fill(fill)
        .corner_radius(style.control_radius)
        .wrap_mode(TextWrapMode::Extend)
        .min_size(vec2(metrics.content_width, 40.0));
    let response = ui.add(button);
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), label));
    if metrics.icons_only {
        response.on_hover_text(label)
    } else {
        response
    }
}

/// Draws every entry, in [`Page::ALL`] order, then the layouts when there are any, with the group
/// lines above the entries that start a group. A right-click stores what it asks for in `action`.
fn entries(
    ui: &mut Ui,
    style: &Style,
    shown: &mut Shown,
    names: &[String],
    metrics: Metrics,
    action: &mut Option<RailAction>,
) {
    for entry in Page::ALL {
        if entry.starts_group() {
            divider(ui, style);
        }
        let response = entry_button(
            ui,
            style,
            metrics,
            entry.icon(),
            entry.label(),
            *shown == Shown::Page(entry),
        );
        if response.clicked() {
            *shown = Shown::Page(entry);
        }
        response.context_menu(|menu| {
            if menu.button("Save a copy").clicked() {
                *action = Some(RailAction::SaveCopy(entry));
                menu.close();
            }
        });
    }

    if names.is_empty() {
        return;
    }
    divider(ui, style);
    for (index, name) in names.iter().enumerate() {
        let response = entry_button(
            ui,
            style,
            metrics,
            regular::FOLDER,
            name,
            *shown == Shown::Layout(index),
        );
        if response.clicked() {
            *shown = Shown::Layout(index);
        }
        response.context_menu(|menu| {
            if menu.button("Rename…").clicked() {
                menu.data_mut(|data| {
                    data.insert_temp(
                        rename_id(),
                        RenameState {
                            index,
                            name: name.clone(),
                            error: None,
                        },
                    );
                });
                menu.close();
            }
            if menu.button("Copy layout").clicked() {
                *action = Some(RailAction::Copy(index));
                menu.close();
            }
            if menu.button("Save to file…").clicked() {
                *action = Some(RailAction::SaveToFile(index));
                menu.close();
            }
            if menu.button("Delete…").clicked() {
                menu.data_mut(|data| {
                    data.insert_temp(delete_id(), DeleteState { index });
                });
                menu.close();
            }
        });
    }
}

/// Every name a layout at `index` may not take: every page label and every other layout's name.
fn taken(names: &[String], index: usize) -> Vec<&str> {
    let mut taken: Vec<&str> = Page::ALL.iter().map(|page| page.label()).collect();
    taken.extend(
        names
            .iter()
            .enumerate()
            .filter(|(other, _)| *other != index)
            .map(|(_, name)| name.as_str()),
    );
    taken
}

/// Draws the `Rename layout` window while it is open, and returns the rename it asks for.
fn rename_window(ui: &mut Ui, style: &Style, names: &[String]) -> Option<RailAction> {
    let mut state = ui.data(|data| data.get_temp::<RenameState>(rename_id()))?;
    if state.index >= names.len() {
        ui.data_mut(|data| data.remove::<RenameState>(rename_id()));
        return None;
    }
    let mut action = None;
    let mut close = false;
    egui::Window::new("Rename layout")
        .id(egui::Id::new("rail rename window"))
        .collapsible(false)
        .resizable(false)
        .show(ui.ctx(), |ui| {
            ui.label("Name");
            ui.text_edit_singleline(&mut state.name);
            if let Some(error) = &state.error {
                ui.label(RichText::new(error).color(style.neg));
            }
            ui.horizontal(|ui| {
                if ui.button("Rename").clicked() {
                    let wanted = state.name.trim();
                    if wanted.is_empty() {
                        state.error = Some("The name cannot be empty.".to_owned());
                    } else if wanted.chars().count() > MAX_NAME {
                        state.error =
                            Some(format!("The name is longer than {MAX_NAME} characters."));
                    } else {
                        let taken = taken(names, state.index);
                        action = Some(RailAction::Rename {
                            index: state.index,
                            name: unique_name(wanted, &taken),
                        });
                        close = true;
                    }
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });
    if close {
        ui.data_mut(|data| data.remove::<RenameState>(rename_id()));
    } else {
        ui.data_mut(|data| data.insert_temp(rename_id(), state));
    }
    action
}

/// Draws the `Delete layout` window while it is open, and returns the delete it confirms.
fn delete_window(ui: &mut Ui, names: &[String]) -> Option<RailAction> {
    let state = ui.data(|data| data.get_temp::<DeleteState>(delete_id()))?;
    let Some(name) = names.get(state.index) else {
        ui.data_mut(|data| data.remove::<DeleteState>(delete_id()));
        return None;
    };
    let mut action = None;
    let mut close = false;
    egui::Window::new("Delete layout")
        .id(egui::Id::new("rail delete window"))
        .collapsible(false)
        .resizable(false)
        .show(ui.ctx(), |ui| {
            ui.label(format!("Delete layout \"{name}\"? This cannot be undone."));
            ui.horizontal(|ui| {
                if ui.button("Delete").clicked() {
                    action = Some(RailAction::Delete(state.index));
                    close = true;
                }
                if ui.button("Keep").clicked() {
                    close = true;
                }
            });
        });
    if close {
        ui.data_mut(|data| data.remove::<DeleteState>(delete_id()));
    }
    action
}

#[cfg(test)]
mod tests {
    use super::*;

    use egui::accesskit::Role;
    use egui::{Event, Key, Modifiers};
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    const NAMES: [&str; 2] = ["Daily industry", "Ideas"];

    /// The state the test UI keeps: which entry is on show, the names, and the last action.
    struct Rail {
        shown: Shown,
        names: Vec<String>,
        action: Option<RailAction>,
    }

    fn rail(shown: Shown, names: &[&str]) -> Harness<'static, Rail> {
        let style = Style::eve_dense();
        let state = Rail {
            shown,
            names: names.iter().map(|name| (*name).to_owned()).collect(),
            action: None,
        };
        Harness::builder()
            .with_size(vec2(400.0, 900.0))
            .build_ui_state(
                move |ui, rail: &mut Rail| {
                    style.apply(ui.ctx());
                    let Rail {
                        shown,
                        names,
                        action,
                    } = rail;
                    if let Some(next) = show_with_layouts(ui, &style, shown, names) {
                        *action = Some(next);
                    }
                },
                state,
            )
    }

    fn click(harness: &mut Harness<'_, Rail>, label: &str) {
        harness.get_by_label(label).click();
        harness.run_steps(3);
    }

    fn right_click(harness: &mut Harness<'_, Rail>, label: &str) {
        harness.get_by_label(label).click_secondary();
        harness.run_steps(3);
    }

    fn shown(harness: &Harness<'_, Rail>, label: &str) -> bool {
        harness.query_by_label(label).is_some()
    }

    /// How many horizontal lines, the rail's dividers among them, were painted.
    fn horizontal_lines(harness: &Harness<'_, Rail>) -> usize {
        fn count(shape: &egui::Shape) -> usize {
            if let egui::Shape::Vec(shapes) = shape {
                return shapes.iter().map(count).sum();
            }
            if let egui::Shape::LineSegment { points, .. } = shape {
                return usize::from((points[0].y - points[1].y).abs() < 0.01);
            }
            0
        }
        harness
            .output()
            .shapes
            .iter()
            .map(|clipped| count(&clipped.shape))
            .sum()
    }

    fn open_rename(harness: &mut Harness<'_, Rail>) {
        right_click(harness, "Daily industry");
        click(harness, "Rename…");
        harness.run_steps(2);
    }

    fn type_name(harness: &mut Harness<'_, Rail>, text: &str) {
        harness
            .get_by(|node| node.role() == Role::TextInput)
            .click();
        harness.run_steps(1);
        harness.key_press_modifiers(Modifiers::COMMAND, Key::A);
        harness.run_steps(1);
        if text.is_empty() {
            harness.key_press(Key::Backspace);
        } else {
            harness.event(Event::Text(text.to_owned()));
        }
        harness.run_steps(2);
    }

    /// Opens the rename window, types `text`, presses `Rename`, and gives the action, if any.
    fn rename(harness: &mut Harness<'_, Rail>, text: &str) -> Option<RailAction> {
        harness.state_mut().action = None;
        open_rename(harness);
        type_name(harness, text);
        click(harness, "Rename");
        harness.state().action.clone()
    }

    #[test]
    fn a_label_wraps_only_between_words_and_at_most_twice() {
        let ctx = egui::Context::default();
        let output = ctx.run_ui(Default::default(), |ui| {
            let industry = text_width(ui, "Industry");
            assert_eq!(
                label_rows(ui, "Industry jobs", industry + 2.0),
                ["Industry", "jobs"]
            );
            assert_eq!(label_rows(ui, "Industry jobs", 500.0), ["Industry jobs"]);
            assert_eq!(label_rows(ui, "Characters", 10.0), ["Characters"]);
        });
        output.drop_without_applying_deltas();
    }

    #[test]
    fn the_layouts_come_after_the_pages_and_a_click_shows_one() {
        let mut harness = rail(Shown::Page(Page::Build), &NAMES);
        harness.run_steps(2);

        let pages_bottom = harness.get_by_label(Page::Setup.label()).rect().bottom();
        let daily = harness.get_by_label("Daily industry").rect();
        let ideas = harness.get_by_label("Ideas").rect();
        assert!(daily.top() > pages_bottom, "the divider is below the pages");
        assert!(
            ideas.top() >= daily.bottom(),
            "the layouts keep their order"
        );
        assert_eq!(
            daily.width(),
            harness.get_by_label(Page::Build.label()).rect().width(),
            "a layout entry has the look of a page entry"
        );

        harness.get_by_label("Ideas").click();
        harness.run_steps(3);
        assert_eq!(harness.state().shown, Shown::Layout(1));
        assert_eq!(harness.state().action, None);
    }

    #[test]
    fn the_divider_appears_only_with_layouts() {
        let without = rail(Shown::Page(Page::Build), &[]);
        let with = rail(Shown::Page(Page::Build), &NAMES);
        assert!(!shown(&without, "Daily industry"));
        assert_eq!(
            horizontal_lines(&with),
            horizontal_lines(&without) + 1,
            "one divider, above the layouts"
        );
    }

    #[test]
    fn a_right_click_on_a_page_asks_for_a_copy() {
        let mut harness = rail(Shown::Page(Page::Build), &NAMES);
        right_click(&mut harness, Page::Build.label());
        click(&mut harness, "Save a copy");
        assert_eq!(
            harness.state().action,
            Some(RailAction::SaveCopy(Page::Build))
        );
    }

    #[test]
    fn a_right_click_on_a_layout_asks_to_copy_or_save_it() {
        let mut harness = rail(Shown::Page(Page::Build), &NAMES);

        right_click(&mut harness, "Daily industry");
        click(&mut harness, "Copy layout");
        assert_eq!(harness.state().action, Some(RailAction::Copy(0)));

        harness.state_mut().action = None;
        right_click(&mut harness, "Daily industry");
        click(&mut harness, "Save to file…");
        assert_eq!(harness.state().action, Some(RailAction::SaveToFile(0)));
    }

    #[test]
    fn a_rename_takes_the_first_free_name() {
        let mut harness = rail(Shown::Page(Page::Build), &NAMES);
        assert_eq!(
            rename(&mut harness, "Build"),
            Some(RailAction::Rename {
                index: 0,
                name: "Build (2)".to_owned(),
            })
        );
        assert_eq!(
            rename(&mut harness, "ideas"),
            Some(RailAction::Rename {
                index: 0,
                name: "ideas (2)".to_owned(),
            })
        );
        assert_eq!(
            rename(&mut harness, "Daily industry"),
            Some(RailAction::Rename {
                index: 0,
                name: "Daily industry".to_owned(),
            })
        );
        assert_eq!(
            rename(&mut harness, "  Mine  "),
            Some(RailAction::Rename {
                index: 0,
                name: "Mine".to_owned(),
            })
        );
    }

    #[test]
    fn a_rename_refuses_an_empty_or_too_long_name() {
        let mut harness = rail(Shown::Page(Page::Build), &NAMES);
        assert_eq!(rename(&mut harness, ""), None);
        assert!(shown(&harness, "The name cannot be empty."));

        let long = "a".repeat(MAX_NAME + 1);
        assert_eq!(rename(&mut harness, &long), None);
        assert!(shown(
            &harness,
            &format!("The name is longer than {MAX_NAME} characters.")
        ));
    }

    #[test]
    fn a_rename_can_be_cancelled() {
        let mut harness = rail(Shown::Page(Page::Build), &NAMES);
        open_rename(&mut harness);
        click(&mut harness, "Cancel");
        assert!(!shown(&harness, "Rename layout"));
        assert_eq!(harness.state().action, None);
    }

    #[test]
    fn a_delete_asks_first() {
        let mut harness = rail(Shown::Page(Page::Build), &NAMES);

        right_click(&mut harness, "Ideas");
        click(&mut harness, "Delete…");
        assert!(shown(
            &harness,
            "Delete layout \"Ideas\"? This cannot be undone."
        ));

        click(&mut harness, "Keep");
        assert!(!shown(&harness, "Delete layout"));
        assert_eq!(harness.state().action, None);

        right_click(&mut harness, "Ideas");
        click(&mut harness, "Delete…");
        click(&mut harness, "Delete");
        assert_eq!(harness.state().action, Some(RailAction::Delete(1)));
    }
}

//! The rail on the left: one entry per shipped page, grouped by what the player does. The entry of
//! the page on show has the accent.

use egui::{
    Button, Color32, RichText, Sense, Stroke, TextStyle, TextWrapMode, Ui, WidgetInfo, WidgetType,
    vec2,
};
use egui_phosphor::regular;

use crate::theme::Style;

/// Empty space above and below a line between groups.
const GROUP_SPACE: f32 = 8.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Today,
    Build,
    Assets,
    NetWorth,
    Orders,
    Jobs,
    Characters,
    Setup,
}

impl Page {
    pub const ALL: [Page; 8] = [
        Page::Today,
        Page::Build,
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

/// Draws the rail. A click on an entry makes it the page on show.
pub fn show(ui: &mut Ui, style: &Style, page: &mut Page) {
    show_with(ui, style, page, false);
}

/// Draws the rail, with either each entry's icon and label or, when `icons_only`, the icon alone
/// and the label as hover text.
pub fn show_with(ui: &mut Ui, style: &Style, page: &mut Page, icons_only: bool) {
    // A page asked for by an empty state takes effect next frame, since the rail draws first.
    if let Some(next) = crate::empty::take(ui.ctx()) {
        *page = next;
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
    let label_width = widest_word(ui);

    egui::Panel::left("rail")
        .exact_size(width)
        .resizable(false)
        .show(ui, |ui| {
            if icons_only {
                ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
                    entries(ui, style, page, content_width, label_width, true);
                });
            } else {
                entries(ui, style, page, content_width, label_width, false);
            }
        });
}

/// Draws every entry, in [`Page::ALL`] order, with the group lines above the entries that start a
/// group.
fn entries(
    ui: &mut Ui,
    style: &Style,
    page: &mut Page,
    content_width: f32,
    label_width: f32,
    icons_only: bool,
) {
    for entry in Page::ALL {
        if entry.starts_group() {
            ui.add_space(GROUP_SPACE);
            let (line, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
            ui.painter().hline(
                line.x_range(),
                line.center().y,
                Stroke::new(1.0, style.line),
            );
            ui.add_space(GROUP_SPACE);
        }

        let (fill, text) = if *page == entry {
            (style.accent, style.on_accent)
        } else {
            (style.row, style.text)
        };
        let button = if icons_only {
            Button::new(RichText::new(entry.icon()).color(text))
        } else {
            let label = label_rows(ui, entry.label(), label_width).join("\n");
            Button::new((
                RichText::new(entry.icon()).color(text),
                RichText::new(label).color(text),
            ))
        };
        let button = button
            .fill(fill)
            .corner_radius(style.control_radius)
            .wrap_mode(TextWrapMode::Extend)
            .min_size(vec2(content_width, 40.0));
        let response = ui.add(button);
        response.widget_info(|| {
            WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), entry.label())
        });
        let response = if icons_only {
            response.on_hover_text(entry.label())
        } else {
            response
        };
        if response.clicked() {
            *page = entry;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}

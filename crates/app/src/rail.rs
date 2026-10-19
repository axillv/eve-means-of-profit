//! The rail on the left: one entry per shipped page. The entry of the page on show has the accent.

use egui::{Button, RichText, Ui, vec2};

use crate::theme::Style;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Build,
    Setup,
}

impl Page {
    pub const ALL: [Page; 2] = [Page::Build, Page::Setup];

    pub fn label(self) -> &'static str {
        match self {
            Self::Build => "Build",
            Self::Setup => "Setup",
        }
    }
}

/// Draws the rail. A click on an entry makes it the page on show.
pub fn show(ui: &mut Ui, style: &Style, page: &mut Page) {
    egui::Panel::left("rail")
        .exact_size(style.rail_width)
        .resizable(false)
        .show(ui, |ui| {
            for entry in Page::ALL {
                let (fill, text) = if *page == entry {
                    (style.accent, style.on_accent)
                } else {
                    (style.row, style.text)
                };
                let button = Button::new(RichText::new(entry.label()).color(text))
                    .fill(fill)
                    .corner_radius(style.control_radius)
                    .min_size(vec2(style.rail_width - 16.0, 40.0));
                if ui.add(button).clicked() {
                    *page = entry;
                }
            }
        });
}

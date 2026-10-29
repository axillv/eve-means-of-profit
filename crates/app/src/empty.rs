//! The one empty-panel look: why the panel is empty, what to do, and the one button that fills it.

use egui::{Id, RichText, Ui};

use crate::rail::Page;
use crate::theme::Style;

/// The one button of an empty panel: its label and, when it leads to another page, that page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fill {
    pub label: &'static str,
    pub page: Option<Page>,
}

/// The egui data key a click on a [`Fill`] with a page leaves for the rail.
fn pending() -> Id {
    Id::new("empty goto")
}

/// Draws `why` and `what` and, when `fill` is set, its one button. Returns whether the button was
/// clicked. A click on a fill with a page stores that page for [`take`]; a fill without a page
/// leaves the click to the caller.
pub fn show(ui: &mut Ui, style: &Style, why: &str, what: &str, fill: Option<Fill>) -> bool {
    let mut clicked = false;
    ui.vertical(|ui| {
        ui.label(RichText::new(why).color(style.text));
        ui.label(RichText::new(what).color(style.mute));
        if let Some(fill) = fill
            && ui.button(fill.label).clicked()
        {
            if let Some(page) = fill.page {
                ui.ctx()
                    .data_mut(|data| data.insert_temp(pending(), Some(page)));
                ui.ctx().request_repaint();
            }
            clicked = true;
        }
    });
    clicked
}

/// Removes and returns the page a click on a fill asked for, if any.
pub fn take(ctx: &egui::Context) -> Option<Page> {
    ctx.data_mut(|data| data.remove_temp(pending())).flatten()
}

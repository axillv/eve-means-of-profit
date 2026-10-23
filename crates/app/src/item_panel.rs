//! The Item panel: the selected item, on the Build and the Assets pages. One panel kind, shown on
//! two pages, so it keeps no page state of its own. [`show`] draws the panel's body.

use egui::Ui;

use crate::Shared;
use crate::dock;

/// Draws the body of the Item panel.
pub fn show(ui: &mut Ui, cx: &mut Shared) {
    dock::placeholder(ui, cx);
}

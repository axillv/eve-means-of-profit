//! The Orders page: every market order of the login characters. The page task fills the panel;
//! [`show`] draws its body.

use egui::Ui;

use crate::Shared;
use crate::dock;

#[derive(Debug, Default)]
pub struct OrdersPage;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrdersPanel {
    Orders,
}

impl OrdersPanel {
    pub fn title(self) -> &'static str {
        match self {
            Self::Orders => "Orders",
        }
    }
}

/// Draws the body of the panel of this page.
pub fn show(_page: &mut OrdersPage, _panel: OrdersPanel, ui: &mut Ui, cx: &mut Shared) {
    dock::placeholder(ui, cx);
}

//! The Setup page. Its panels are stubs until M1-14 and M1-16 fill them: the dock draws each
//! panel's header, and [`show`] draws the body, which is empty for now.

use egui::Ui;

use crate::Shared;

#[derive(Debug, Default)]
pub struct SetupPage;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupPanel {
    Facilities,
    FreightServices,
    PriceRule,
}

impl SetupPanel {
    pub fn title(self) -> &'static str {
        match self {
            Self::Facilities => "Facilities",
            Self::FreightServices => "Freight services",
            Self::PriceRule => "Price rule",
        }
    }
}

/// Draws the body of one Setup panel.
pub fn show(_page: &mut SetupPage, _panel: SetupPanel, _ui: &mut Ui, _cx: &mut Shared) {}

//! The Build page. Its panels are stubs until M1-14 and M1-16 fill them: the dock draws each
//! panel's header, and [`show`] draws the body, which is empty for now.

use egui::Ui;

use crate::Shared;

#[derive(Debug, Default)]
pub struct BuildPage;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildPanel {
    Plans,
    PastedStock,
    Plan,
    Steps,
    BuyList,
}

impl BuildPanel {
    pub fn title(self) -> &'static str {
        match self {
            Self::Plans => "Plans",
            Self::PastedStock => "Pasted stock",
            Self::Plan => "Plan",
            Self::Steps => "Steps",
            Self::BuyList => "Buy list",
        }
    }
}

/// Draws the body of one Build panel.
pub fn show(_page: &mut BuildPage, _panel: BuildPanel, _ui: &mut Ui, _cx: &mut Shared) {}

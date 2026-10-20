//! The Build page: the saved plans, the plan being edited, what is pasted from stock, and the chain
//! of steps with what is taken from stock and what is left to buy. [`show`] draws one panel's body;
//! the dock draws each panel's header.

pub mod buy_list;
pub mod inputs;
pub mod plans;
pub mod prices;
pub mod state;
pub mod steps;
pub mod stock;

use egui::Ui;

use crate::Shared;

pub use state::BuildPage;

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
pub fn show(page: &mut BuildPage, panel: BuildPanel, ui: &mut Ui, cx: &mut Shared) {
    match panel {
        BuildPanel::Plans => plans::show(page, ui, cx),
        BuildPanel::PastedStock => stock::show(page, ui, cx),
        BuildPanel::Plan => inputs::show(page, ui, cx),
        BuildPanel::Steps => steps::show(page, ui, cx),
        BuildPanel::BuyList => buy_list::show(page, ui, cx),
    }
}

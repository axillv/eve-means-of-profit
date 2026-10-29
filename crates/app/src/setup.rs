//! The Setup page: the facilities, freight services and default price rule the plans pick from. The
//! dock draws each panel's header, and [`show`] draws the body.

mod facilities;
mod fees;
mod fields;
mod freight;
pub mod history;
mod invention;
mod price_rule;
mod structures;
pub mod value_prices;

use egui::Ui;

use mop_store::StoreError;

use crate::Shared;
use crate::settings_window;

#[derive(Debug, Default)]
pub struct SetupPage;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupPanel {
    Facilities,
    FreightServices,
    PriceHistory,
    PriceRule,
    Structures,
    Fees,
    Invention,
    Display,
    ValuePrices,
}

impl SetupPanel {
    pub fn title(self) -> &'static str {
        match self {
            Self::Facilities => "Facilities",
            Self::FreightServices => "Freight services",
            Self::PriceHistory => "Price history",
            Self::PriceRule => "Price rule",
            Self::Structures => "Structures",
            Self::Fees => "Fees and skills",
            Self::Invention => "Invention skills",
            Self::Display => "Display",
            Self::ValuePrices => "Value prices",
        }
    }
}

/// The text a store error shows under a Setup panel, for the errors the panels share.
pub(super) fn store_text(error: &StoreError) -> String {
    if matches!(error, StoreError::RouteExists) {
        "This route already exists.".to_owned()
    } else if let StoreError::InUse { plans } = error {
        format!("In use by: {}", plans.join(", "))
    } else {
        error.to_string()
    }
}

/// Draws the body of one Setup panel.
pub fn show(_page: &mut SetupPage, panel: SetupPanel, ui: &mut Ui, cx: &mut Shared) {
    match panel {
        SetupPanel::Facilities => facilities::show(ui, cx),
        SetupPanel::FreightServices => freight::show(ui, cx),
        SetupPanel::PriceHistory => history::show(ui, cx),
        SetupPanel::PriceRule => price_rule::show(ui, cx),
        SetupPanel::Structures => structures::show(ui, cx),
        SetupPanel::Fees => fees::show(ui, cx),
        SetupPanel::Invention => invention::show(ui, cx),
        SetupPanel::ValuePrices => value_prices::show(ui, cx),
        SetupPanel::Display => {
            let data_dir = cx.data_dir.clone();
            egui::ScrollArea::vertical().show(ui, |ui| {
                settings_window::show(ui, cx, &data_dir);
            });
        }
    }
}

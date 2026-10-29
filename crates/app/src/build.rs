//! The Build page: the saved plans, the plan being edited, what is pasted from stock, and the chain
//! of steps with what is taken from stock and what is left to buy. [`show`] draws one panel's body;
//! the dock draws each panel's header.

pub mod buy_list;
pub mod costs;
pub mod inputs;
pub mod invention;
pub mod plans;
pub mod prices;
pub mod state;
pub mod steps;
pub mod stock;

use egui::Ui;

use crate::Shared;
use crate::empty::{self, Fill};

pub use state::BuildPage;
use state::Next;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildPanel {
    Plans,
    Plan,
    Steps,
    BuyList,
}

impl BuildPanel {
    pub fn title(self) -> &'static str {
        match self {
            Self::Plans => "Plans",
            Self::Plan => "Plan",
            Self::Steps => "Steps",
            Self::BuyList => "Buy list",
        }
    }
}

/// Draws the body of one Build panel.
pub fn show(page: &mut BuildPage, panel: BuildPanel, ui: &mut Ui, cx: &mut Shared) {
    if empty_state(page, panel, ui, cx) {
        return;
    }
    match panel {
        BuildPanel::Plans => plans::show(page, ui, cx),
        BuildPanel::Plan => inputs::show(page, ui, cx),
        BuildPanel::Steps => steps::show(page, ui, cx),
        BuildPanel::BuyList => buy_list::show(page, ui, cx),
    }
}

/// Draws the panel's empty state when it has one, and returns whether it did. A store error is not
/// empty: the panel keeps its own error.
fn empty_state(page: &mut BuildPage, panel: BuildPanel, ui: &mut Ui, cx: &mut Shared) -> bool {
    let (why, what) = match panel {
        BuildPanel::Plans => match cx.store.plans() {
            Ok(plans) if plans.is_empty() => (
                "No plans yet.",
                "A plan starts with a product and a quantity, then gives you a buy list to paste into EVE.",
            ),
            _ => return false,
        },
        BuildPanel::Plan if page.record.is_none() => (
            "No plan on screen.",
            "Pick one in the Plans list, or start a new plan with a product and a quantity.",
        ),
        BuildPanel::Steps if page.record.is_none() => (
            "No plan on screen.",
            "The steps, and what to build or buy, show when you pick a plan.",
        ),
        BuildPanel::BuyList if page.record.is_none() => (
            "No plan on screen.",
            "What to buy, and the list to paste into EVE, shows when you pick a plan.",
        ),
        BuildPanel::Plan | BuildPanel::Steps | BuildPanel::BuyList => return false,
    };
    let style = cx.style;
    let fill = Fill {
        label: "New plan",
        page: None,
    };
    if empty::show(ui, &style, why, what, Some(fill)) {
        page.request(Next::New, cx);
    }
    true
}

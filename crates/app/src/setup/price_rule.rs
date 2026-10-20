//! The Price rule panel: how a plan prices the material it buys. Saved at once; plans made later
//! start with it, and existing plans keep their own.

use egui::Ui;
use profit_market::book::PriceRule;

use crate::Shared;

const CHOICES: [(PriceRule, &str, &str); 3] = [
    (
        PriceRule::WalkSell,
        "Jita sell, walk the book",
        "What buying the quantity from the cheapest sell orders costs.",
    ),
    (
        PriceRule::LowestSell,
        "Jita sell, lowest × quantity",
        "Lowest sell price × quantity, as other tools show it.",
    ),
    (
        PriceRule::HighestBuy,
        "Jita buy, highest",
        "Highest buy order × quantity, for people who place buy orders.",
    ),
];

pub(super) fn show(ui: &mut Ui, cx: &mut Shared) {
    let current = match cx.store.settings() {
        Ok(settings) => settings.default_price_rule,
        Err(error) => {
            ui.colored_label(cx.style.neg, error.to_string());
            return;
        }
    };
    let mut picked = current;
    for (rule, label, help) in CHOICES {
        ui.radio_value(&mut picked, rule, label);
        ui.colored_label(cx.style.mute, help);
    }
    if picked != current {
        cx.change_settings(|settings| settings.default_price_rule = picked);
    }
}

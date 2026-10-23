//! The Price rule panel: how a plan prices the material it buys. Saved at once; plans made later
//! start with it, and existing plans keep their own.

use egui::Ui;
use mop_market::book::PriceRule;

use crate::Shared;
use crate::build::inputs::{PRICE_RULES, price_text};

pub(super) fn show(ui: &mut Ui, cx: &mut Shared) {
    let current = match cx.store.settings() {
        Ok(settings) => settings.default_price_rule,
        Err(error) => {
            ui.colored_label(cx.style.neg, error.to_string());
            return;
        }
    };
    // A setting saved before the rule list changed may still hold `WalkSell`; it reads as the
    // default's label.
    let shown = if current == PriceRule::WalkSell {
        PriceRule::SellFivePercent
    } else {
        current
    };
    let mut picked = shown;
    for rule in PRICE_RULES {
        ui.radio_value(&mut picked, rule, price_text(rule));
        ui.colored_label(cx.style.mute, help_text(rule));
    }
    if picked != shown {
        cx.change_settings(|settings| settings.default_price_rule = picked);
    }
}

/// What the price under each rule means, in one line.
fn help_text(rule: PriceRule) -> &'static str {
    match rule {
        PriceRule::WalkSell | PriceRule::SellFivePercent => {
            "The price at the cheapest 5% of the sell volume in The Forge."
        }
        PriceRule::LowestSell => "The lowest sell price in The Forge.",
        PriceRule::SellWeightedAverage => "The average sell price, weighted by volume.",
        PriceRule::SellMedian => "The middle sell price.",
        PriceRule::HighestBuy => "The highest buy order, for people who place buy orders.",
        PriceRule::BuyFivePercent => "The price at the highest 5% of the buy volume in The Forge.",
        PriceRule::BuyWeightedAverage => "The average buy price, weighted by volume.",
        PriceRule::BuyMedian => "The middle buy price.",
    }
}

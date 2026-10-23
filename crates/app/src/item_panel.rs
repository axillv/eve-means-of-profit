//! The Item panel: the selected item, on the Build and the Assets pages. One panel kind, shown on
//! two pages, so it keeps no page state of its own. [`show`] draws the panel's body.

mod owned;
mod plans;

use egui::{RichText, Ui};
use mop_core::format::group_thousands;
use mop_core::industry::TypeId;
use mop_core::money::Money;
use mop_market::fuzzwork::{Side, Stat};

use crate::Shared;
use crate::build::prices::{age_text, failure_text};
use crate::build::state::type_name;

/// Prices older than this many seconds show in the stale colour.
const STALE_AFTER: i64 = 600;

/// Draws the body of the Item panel.
pub fn show(ui: &mut Ui, cx: &mut Shared) {
    let Some(type_id) = cx.selection.item else {
        ui.label(RichText::new("Click an item to see it here.").color(cx.style.mute));
        return;
    };
    header(ui, cx, type_id);
    owned::show(ui, cx, type_id);
    prices(ui, cx, type_id);
    plans::show(ui, cx, type_id);
}

/// The item's name in the text colour, then its packaged volume per unit in mute.
fn header(ui: &mut Ui, cx: &mut Shared, type_id: TypeId) {
    let name = type_name(type_id, cx.sde.as_ref(), &mut cx.notices);
    let volume = cx
        .sde
        .as_ref()
        .and_then(|sde| sde.item(type_id).ok().flatten())
        .and_then(|item| item.packaged_volume);
    ui.horizontal(|ui| {
        ui.label(RichText::new(name).color(cx.style.text));
        if let Some(volume) = volume {
            ui.label(RichText::new(volume.format_m3()).color(cx.style.mute));
        }
    });
}

/// The cheapest sell and the highest buy from the held market file, and the age of the file.
fn prices(ui: &mut Ui, cx: &mut Shared, type_id: TypeId) {
    let now = (cx.clock)();
    let failure = cx
        .market
        .last_failure()
        .map(|failure| failure_text(&failure));
    let held = cx.market.held();
    let Some(published) = held.published_at else {
        let text = match failure {
            Some(text) => RichText::new(text).color(cx.style.neg),
            None => RichText::new("No market data yet").color(cx.style.mute),
        };
        ui.label(text);
        return;
    };
    let sell = held
        .rows
        .get(&(type_id, Side::Sell))
        .map(|row| row.price(Side::Sell, Stat::Best));
    let buy = held
        .rows
        .get(&(type_id, Side::Buy))
        .map(|row| row.price(Side::Buy, Stat::Best));
    let age = now.checked_sub(published).unwrap_or(0).max(0);
    let age_colour = if is_stale(age) {
        cx.style.stale
    } else {
        cx.style.text
    };
    ui.horizontal(|ui| {
        ui.label(format!(
            "Forge sell {} · buy {}",
            price_text(sell),
            price_text(buy)
        ));
        ui.label(RichText::new(age_text(age)).color(age_colour));
    });
    if let Some(text) = failure {
        ui.label(RichText::new(text).color(cx.style.neg));
    }
}

/// Whether a price file of `age` seconds is old enough to show in the stale colour.
fn is_stale(age: i64) -> bool {
    age > STALE_AFTER
}

/// The text of a price, or `—` when the side has no orders.
fn price_text(price: Option<Money>) -> String {
    price.map_or_else(|| "—".to_owned(), Money::format_isk)
}

/// A number with thousands grouped, such as `1,050`.
fn grouped(value: u64) -> String {
    i64::try_from(value).map_or_else(|_| value.to_string(), group_thousands)
}

#[cfg(test)]
mod tests {
    use super::{STALE_AFTER, is_stale};

    #[test]
    fn a_file_turns_stale_only_after_the_limit() {
        assert!(!is_stale(STALE_AFTER));
        assert!(is_stale(STALE_AFTER + 1));
        assert!(!is_stale(0));
    }
}

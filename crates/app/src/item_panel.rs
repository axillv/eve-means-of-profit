//! The Item panel: the selected item, on the Build and the Assets pages. One panel kind, shown on
//! two pages, so it keeps no page state of its own. [`show`] draws the panel's body.

mod owned;
mod plans;

use egui::{Id, Key, Modifiers, RichText, Ui, Window, pos2, vec2};
use mop_core::format::group_thousands;
use mop_core::industry::TypeId;
use mop_core::money::Money;
use mop_market::fuzzwork::{Side, Stat};

use crate::Shared;
use crate::build::prices::{age_text, failure_text};
use crate::build::state::type_name;
use crate::table::{copy_request, has_table};

/// Prices older than this many seconds show in the stale colour.
const STALE_AFTER: i64 = 600;

/// Draws the Item window while an item is picked. Escape and the window's close button clear the
/// item unless the Pin holds it.
pub fn inspector(ui: &mut Ui, cx: &mut Shared) {
    if cx.selection.item.is_none() {
        return;
    }
    // The page draws first, so a rename dialog has already taken its Escape. A menu inside the
    // window reads Escape without consuming it, so one Escape closes such a menu and, unpinned,
    // the window.
    if !cx.selection.item_pinned
        && ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape))
    {
        cx.selection.close_item();
        return;
    }
    let id = Id::new("Item");
    // Set by the body's table on the previous frame, so the button says whether the panel had a
    // table then.
    let table_shown = ui
        .data(|data| data.get_temp::<bool>(has_table(id)))
        .unwrap_or(false);
    let mut open = true;
    Window::new("Item")
        .collapsible(false)
        .default_pos(pos2(840.0, 80.0))
        .default_size(vec2(420.0, 640.0))
        .open(&mut open)
        .show(ui.ctx(), |ui| {
            ui.horizontal(|ui| {
                ui.checkbox(&mut cx.selection.item_pinned, "Pin")
                    .on_hover_text("Keeps the panel open: Escape and the close button leave it.");
                if table_shown
                    && ui
                        .button("Copy table")
                        .on_hover_text("Copies this table for a spreadsheet (tab-separated).")
                        .clicked()
                {
                    ui.data_mut(|data| data.insert_temp(copy_request(id), true));
                }
            });
            // Cleared before the body draws, so a panel whose table stops showing loses the
            // button on the next frame.
            ui.data_mut(|data| data.remove_temp::<bool>(has_table(id)));
            show(ui, cx);
            // A copy asked for on a panel with no table is dropped, not kept for a table shown
            // later.
            ui.data_mut(|data| data.remove_temp::<bool>(copy_request(id)));
        });
    if !open {
        cx.selection.close_item();
    }
}

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

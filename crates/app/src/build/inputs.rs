//! The Plan panel: the product and quantity of the plan on screen, where it is built, how it is
//! priced and which route it is shipped by.

use std::collections::BTreeSet;

use egui::{Button, ComboBox, Grid, Key, RichText, TextEdit, Ui};
use mop_core::industry::{Activity, TypeId};
use mop_core::stock::{CharacterId, LocationId, Places, StockScope, resolve};
use mop_market::book::PriceRule;
use mop_sde::Sde;
use mop_store::reservations::PlanStock;
use mop_store::{PlanRecord, StoreError};

use super::state::{BuildPage, push_error, report, type_name};
use crate::Shared;
use crate::session::Source;
use crate::status::Notice;

pub(crate) const PRICE_RULES: [PriceRule; 8] = [
    PriceRule::SellFivePercent,
    PriceRule::LowestSell,
    PriceRule::SellWeightedAverage,
    PriceRule::SellMedian,
    PriceRule::HighestBuy,
    PriceRule::BuyFivePercent,
    PriceRule::BuyWeightedAverage,
    PriceRule::BuyMedian,
];

/// How many product matches the search lists.
const MATCHES: usize = 10;

pub fn show(page: &mut BuildPage, ui: &mut Ui, cx: &mut Shared) {
    if page.record.is_none() {
        ui.label(RichText::new("No plan selected.").color(cx.style.mute));
        return;
    }
    if header(page, ui, cx) {
        page.save(cx);
    }
    Grid::new("build_plan_inputs")
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            product_row(page, ui, cx);
            quantity_row(page, ui, cx);
            facility_row(page, ui, cx, Activity::Manufacturing);
            facility_row(page, ui, cx, Activity::Reaction);
            price_row(page, ui, cx);
            freight_row(page, ui, cx);
            stock_row(page, ui, cx);
        });
}

/// The heading and the Save button. Returns whether Save was clicked.
fn header(page: &BuildPage, ui: &mut Ui, cx: &mut Shared) -> bool {
    let Some(record) = page.record.as_ref() else {
        return false;
    };
    let product = if record.product == 0 {
        "no product yet".to_owned()
    } else {
        type_name(record.product, cx.sde.as_ref(), &mut cx.notices)
    };
    let mut text = format!("PLAN  {product} × {}", record.quantity);
    if page.dirty {
        text.push_str(" (unsaved)");
    }
    let mut save = false;
    ui.horizontal(|ui| {
        ui.strong(text);
        save = ui.add_enabled(page.dirty, Button::new("Save")).clicked();
    });
    save
}

fn product_row(page: &mut BuildPage, ui: &mut Ui, cx: &mut Shared) {
    ui.label("Product");
    let mut picked = None;
    ui.vertical(|ui| {
        let search = ui.add(
            TextEdit::singleline(&mut page.product_query)
                .hint_text("Search for a product")
                .desired_width(260.0),
        );
        let matches = product_matches(&page.product_query, page, cx);
        let enter = search.has_focus() && ui.input(|input| input.key_pressed(Key::Enter));
        if enter {
            picked = matches.first().map(|(id, _)| *id);
        }
        for (id, name) in &matches {
            if ui.button(name.as_str()).clicked() {
                picked = Some(*id);
            }
        }
    });
    if let Some(id) = picked {
        edit(page, cx, |record| record.product = id);
        page.product_query.clear();
    }
    ui.end_row();
}

/// Types with a recipe that match the search, at most [`MATCHES`] of them, in search order.
fn product_matches(query: &str, page: &BuildPage, cx: &mut Shared) -> Vec<(TypeId, String)> {
    let query = query.trim();
    let Some(sde) = cx.sde.as_ref() else {
        return Vec::new();
    };
    if query.is_empty() {
        return Vec::new();
    }
    let ids = match sde.search(query, 20) {
        Ok(ids) => ids,
        Err(error) => {
            push_error(
                &mut cx.notices,
                format!("Could not search the products: {error}"),
            );
            return Vec::new();
        }
    };
    ids.into_iter()
        .filter(|id| page.recipes.get(*id).is_some())
        .take(MATCHES)
        .map(|id| (id, type_name(id, Some(sde), &mut cx.notices)))
        .collect()
}

fn quantity_row(page: &mut BuildPage, ui: &mut Ui, cx: &mut Shared) {
    ui.label("Quantity");
    ui.vertical(|ui| {
        let typed = ui.add(TextEdit::singleline(&mut page.quantity_text).desired_width(120.0));
        if typed.changed() {
            match parse_quantity(&page.quantity_text) {
                Some(quantity) => {
                    page.quantity_ok = true;
                    edit(page, cx, |record| record.quantity = quantity);
                }
                None => page.quantity_ok = false,
            }
        }
        if !page.quantity_ok {
            ui.label(
                RichText::new("Quantity must be a whole number of 1 or more").color(cx.style.neg),
            );
        }
    });
    ui.end_row();
}

/// A whole number of 1 or more, or `None`.
fn parse_quantity(text: &str) -> Option<u64> {
    text.trim()
        .parse::<u64>()
        .ok()
        .filter(|quantity| *quantity >= 1)
}

fn facility_row(page: &mut BuildPage, ui: &mut Ui, cx: &mut Shared, activity: Activity) {
    let (label, none, salt) = match activity {
        Activity::Manufacturing => (
            "Manufacturing",
            "NPC station · high sec",
            "build_manufacturing",
        ),
        Activity::Reaction => ("Reactions", "None", "build_reactions"),
    };
    ui.label(label);
    let current = page.record.as_ref().and_then(|record| match activity {
        Activity::Manufacturing => record.manufacturing,
        Activity::Reaction => record.reactions,
    });
    let options = facility_options(cx, activity, none);
    if let Some(choice) = choose(ui, salt, current, &options) {
        edit(page, cx, |record| match activity {
            Activity::Manufacturing => record.manufacturing = choice,
            Activity::Reaction => record.reactions = choice,
        });
    }
    ui.end_row();
}

/// The first option is `none`; then each saved facility that allows `activity`.
fn facility_options(cx: &mut Shared, activity: Activity, none: &str) -> Vec<(Option<i64>, String)> {
    let mut options = vec![(None, none.to_owned())];
    let rows = match cx.store.facilities() {
        Ok(rows) => rows,
        Err(error) => {
            report(cx, format!("Could not read the facilities: {error}"));
            Vec::new()
        }
    };
    let Some(sde) = cx.sde.as_ref() else {
        return options;
    };
    for row in rows {
        if sde.check_facility(&row.facility, activity).is_err() {
            continue;
        }
        let structure = row.facility.structure.map_or_else(
            || "NPC station".to_owned(),
            |id| type_name(id, Some(sde), &mut cx.notices),
        );
        let text = format!(
            "{} · {} · {} · {} rigs",
            row.name,
            structure,
            security_text(row.facility.security),
            row.facility.rigs.len()
        );
        options.push((Some(row.id), text));
    }
    options
}

fn security_text(security: mop_sde::Security) -> &'static str {
    match security {
        mop_sde::Security::High => "high sec",
        mop_sde::Security::Low => "low sec",
        mop_sde::Security::Null => "null sec",
    }
}

fn price_row(page: &mut BuildPage, ui: &mut Ui, cx: &mut Shared) {
    ui.label("Price");
    let current = page.record.as_ref().map(|record| record.price_rule);
    if let Some(current) = current {
        // A plan saved before the rule list changed may still hold `WalkSell`; it reads as the
        // default's label, and picking anything leaves that old value behind.
        let shown = if current == PriceRule::WalkSell {
            PriceRule::SellFivePercent
        } else {
            current
        };
        let options: Vec<(PriceRule, String)> = PRICE_RULES
            .into_iter()
            .map(|rule| (rule, price_text(rule).to_owned()))
            .collect();
        if let Some(rule) = choose(ui, "build_price", shown, &options) {
            edit(page, cx, |record| record.price_rule = rule);
        }
    }
    ui.end_row();
}

pub(crate) fn price_text(rule: PriceRule) -> &'static str {
    match rule {
        PriceRule::WalkSell | PriceRule::SellFivePercent => "Forge sell, 5% price",
        PriceRule::LowestSell => "Forge sell, cheapest",
        PriceRule::SellWeightedAverage => "Forge sell, weighted average",
        PriceRule::SellMedian => "Forge sell, middle",
        PriceRule::HighestBuy => "Forge buy, highest",
        PriceRule::BuyFivePercent => "Forge buy, 5% price",
        PriceRule::BuyWeightedAverage => "Forge buy, weighted average",
        PriceRule::BuyMedian => "Forge buy, middle",
    }
}

fn freight_row(page: &mut BuildPage, ui: &mut Ui, cx: &mut Shared) {
    ui.label("Freight");
    let current = page.record.as_ref().and_then(|record| record.route);
    let options = route_options(cx);
    if let Some(choice) = choose(ui, "build_freight", current, &options) {
        edit(page, cx, |record| record.route = choice);
    }
    ui.end_row();
}

/// `No freight`, then each saved route as `<from> → <to>`.
fn route_options(cx: &mut Shared) -> Vec<(Option<i64>, String)> {
    let mut options = vec![(None, "No freight".to_owned())];
    let routes = match cx.store.routes() {
        Ok(routes) => routes,
        Err(error) => {
            report(cx, format!("Could not read the routes: {error}"));
            Vec::new()
        }
    };
    for route in routes {
        let from = system_name(route.from_system, cx.sde.as_ref(), &mut cx.notices);
        let to = system_name(route.to_system, cx.sde.as_ref(), &mut cx.notices);
        options.push((Some(route.id), format!("{from} → {to}")));
    }
    options
}

/// The name of a system in the game data, or `system <id>` when the data has no such system. A
/// lookup that fails is reported to `notices` and also reads as `system <id>`.
fn system_name(id: u32, sde: Option<&Sde>, notices: &mut Vec<Notice>) -> String {
    let Some(sde) = sde else {
        return format!("system {id}");
    };
    match sde.system(id) {
        Ok(Some(system)) => system.name,
        Ok(None) => format!("system {id}"),
        Err(error) => {
            push_error(notices, format!("Could not read system {id}: {error}"));
            format!("system {id}")
        }
    }
}

/// The Stock row: where the plan's stock comes from, and for characters which ones and places.
fn stock_row(page: &mut BuildPage, ui: &mut Ui, cx: &mut Shared) {
    ui.label("Stock");
    ui.vertical(|ui| {
        stock_choice(page, ui, cx);
        if let PlanStock::Characters(scope) = page.stock.clone() {
            character_ticks(page, ui, cx, &scope);
            place_ticks(page, ui, cx, &scope);
        }
        if let Some(text) = page.stock_error.clone() {
            ui.label(RichText::new(text).color(cx.style.neg));
        }
    });
    ui.end_row();
}

/// The dropdown: Pasted, or From characters (disabled when no character data is held).
fn stock_choice(page: &mut BuildPage, ui: &mut Ui, cx: &mut Shared) {
    let has_characters = !cx.session.stock_characters().is_empty();
    let from_characters = matches!(page.stock, PlanStock::Characters(_));
    let shown = if from_characters {
        "From characters"
    } else {
        "Pasted"
    };
    let mut picked = None;
    ComboBox::from_id_salt("build_stock")
        .selected_text(shown)
        .show_ui(ui, |ui| {
            if ui.selectable_label(!from_characters, "Pasted").clicked() {
                picked = Some(false);
            }
            let option = ui.add_enabled(
                has_characters,
                Button::selectable(from_characters, "From characters"),
            );
            if option.on_disabled_hover_text(NO_CHARACTERS).clicked() {
                picked = Some(true);
            }
        });
    match picked {
        Some(true) if !from_characters => choose_characters(page, cx),
        Some(false) if from_characters => save_stock(page, cx, PlanStock::Pasted),
        _ => {}
    }
    if !from_characters {
        ui.label(RichText::new("materials and freight only").color(cx.style.mute));
    }
}

const NO_CHARACTERS: &str =
    "No character data yet. Log in or read jEve's file on the Characters page.";

/// Picks From characters: every character of the stock, and all places.
fn choose_characters(page: &mut BuildPage, cx: &mut Shared) {
    let characters = cx
        .session
        .stock_characters()
        .iter()
        .map(|owned| owned.character_id)
        .collect();
    let scope = StockScope {
        characters,
        places: Places::All,
    };
    save_stock(page, cx, PlanStock::Characters(scope));
}

/// The character ticks: each character of the stock, then each scope character with no data.
fn character_ticks(page: &mut BuildPage, ui: &mut Ui, cx: &mut Shared, scope: &StockScope) {
    let mut rows: Vec<(CharacterId, String)> = cx
        .session
        .stock_characters()
        .iter()
        .map(|owned| (owned.character_id, character_name(cx, owned.character_id)))
        .collect();
    for id in &scope.characters {
        if !rows.iter().any(|(row, _)| row == id) {
            rows.push((*id, format!("{id} (no data)")));
        }
    }
    for (id, text) in rows {
        let mut ticked = scope.characters.contains(&id);
        if ui.checkbox(&mut ticked, text).changed() {
            let mut next = scope.clone();
            if ticked {
                next.characters.insert(id);
            } else {
                next.characters.remove(&id);
            }
            save_stock(page, cx, PlanStock::Characters(next));
        }
    }
}

/// The place choice and, under `Only these:`, one tick per place the characters hold stock in.
fn place_ticks(page: &mut BuildPage, ui: &mut Ui, cx: &mut Shared, scope: &StockScope) {
    let all = matches!(scope.places, Places::All);
    let top = counted_places(cx, scope);
    // The choice and the places share few rows: the Plan panel is short and a row cut off
    // by its edge cannot be clicked.
    let (all_clicked, only_clicked) = ui
        .horizontal(|ui| {
            let all_clicked = ui.radio(all, "All places").clicked();
            let only_clicked = ui.radio(!all, "Only these:").clicked();
            (all_clicked, only_clicked)
        })
        .inner;
    if all_clicked && !all {
        let next = StockScope {
            characters: scope.characters.clone(),
            places: Places::All,
        };
        save_stock(page, cx, PlanStock::Characters(next));
    } else if only_clicked && all {
        let next = StockScope {
            characters: scope.characters.clone(),
            places: Places::Only(top.iter().map(|(id, _)| *id).collect()),
        };
        save_stock(page, cx, PlanStock::Characters(next));
    }
    if all {
        return;
    }
    let chosen = match &scope.places {
        Places::Only(set) => set.clone(),
        Places::All => BTreeSet::new(),
    };
    let mut listed = top;
    for id in &chosen {
        if !listed.iter().any(|(row, _)| row == id) {
            listed.push((*id, cx.session.place_label(*id, cx.sde.as_ref())));
        }
    }
    ui.horizontal_wrapped(|ui| {
        for (id, label) in listed {
            let mut ticked = chosen.contains(&id);
            if ui.checkbox(&mut ticked, label).changed() {
                let mut places = chosen.clone();
                if ticked {
                    places.insert(id);
                } else {
                    places.remove(&id);
                }
                let next = StockScope {
                    characters: scope.characters.clone(),
                    places: Places::Only(places),
                };
                save_stock(page, cx, PlanStock::Characters(next));
            }
        }
    });
}

/// The places where the chosen characters hold stock that counts, by label ignoring case, then id.
fn counted_places(cx: &Shared, scope: &StockScope) -> Vec<(LocationId, String)> {
    let mut places: BTreeSet<LocationId> = BTreeSet::new();
    for row in resolve(&cx.session.stock_characters()).assets {
        if row.counts_as_stock && scope.characters.contains(&row.character_id) {
            places.insert(row.place);
        }
    }
    let mut listed: Vec<(LocationId, String)> = places
        .into_iter()
        .map(|id| (id, cx.session.place_label(id, cx.sde.as_ref())))
        .collect();
    listed.sort_by(|a, b| {
        a.1.to_lowercase()
            .cmp(&b.1.to_lowercase())
            .then(a.0.cmp(&b.0))
    });
    listed
}

/// A character's name: the login's name, or jEve's name with ` (jEve)`, or its id.
fn character_name(cx: &Shared, id: CharacterId) -> String {
    if let Ok(login_id) = u64::try_from(id)
        && matches!(cx.session.source(login_id), Some(Source::Login))
        && let Some(saved) = cx
            .session
            .characters()
            .iter()
            .find(|saved| saved.id == login_id)
    {
        return saved.name.clone();
    }
    let jeve = cx
        .session
        .jeve()
        .and_then(|jeve| jeve.characters.iter().find(|held| held.character_id == id));
    match jeve {
        Some(held) => format!("{} (jEve)", held.name),
        None => id.to_string(),
    }
}

/// Saves the plan's stock choice at once and works the plan out again. A choice the store refuses
/// is not kept: its reason shows under the row instead.
fn save_stock(page: &mut BuildPage, cx: &mut Shared, stock: PlanStock) {
    let Some(id) = page.selected else {
        return;
    };
    match cx.store.set_plan_stock(id, &stock) {
        Ok(()) => {
            page.stock = stock;
            page.stock_error = None;
            page.recalc(cx);
        }
        Err(StoreError::InvalidPlanStock(reason)) => {
            page.stock_error = Some(sentence(&reason));
        }
        Err(error) => report(cx, format!("Could not save the plan's stock: {error}")),
    }
}

/// A store reason as shown: capitalised, with a full stop.
fn sentence(reason: &str) -> String {
    let mut chars = reason.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    let mut text = format!("{}{}", first.to_uppercase(), chars.as_str());
    if !text.ends_with(['.', '!', '?']) {
        text.push('.');
    }
    text
}

/// Shows a choice as a dropdown. Returns the option picked this frame, if any.
fn choose<T: Copy + PartialEq>(
    ui: &mut Ui,
    salt: &str,
    current: T,
    options: &[(T, String)],
) -> Option<T> {
    let shown = options
        .iter()
        .find(|(value, _)| *value == current)
        .map_or_else(
            || "not allowed for this step".to_owned(),
            |(_, text)| text.clone(),
        );
    let mut picked = None;
    ComboBox::from_id_salt(salt)
        .selected_text(shown)
        .show_ui(ui, |ui| {
            for (value, text) in options {
                if ui
                    .selectable_label(*value == current, text.as_str())
                    .clicked()
                {
                    picked = Some(*value);
                }
            }
        });
    picked
}

/// Changes the plan on screen and works it out again.
fn edit(page: &mut BuildPage, cx: &mut Shared, change: impl FnOnce(&mut PlanRecord)) {
    if let Some(record) = page.record.as_mut() {
        change(record);
        page.edited(cx);
    }
}

#[cfg(test)]
mod tests {
    use super::sentence;

    #[test]
    fn a_reason_gets_a_capital_and_one_full_stop() {
        assert_eq!(
            sentence("pick at least one character"),
            "Pick at least one character."
        );
        assert_eq!(
            sentence("pick at least one character."),
            "Pick at least one character."
        );
        assert_eq!(sentence(""), "");
    }
}

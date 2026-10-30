//! The Buy list panel: what to buy for the plan on screen, what each line costs and how old its
//! price is, the freight each service charges, and the delivered total. The list copies to EVE's
//! multibuy window. Prices change only on the app's start request or the player's `Refresh now`
//! button; nothing on this panel asks for them on a timer.

use std::collections::BTreeMap;

use egui::{Button, Id, RichText, TextEdit, Ui};
use mop_core::buylist::{BuyInput, BuyList, BuyRow, Delivered, LinePrice, buy_list};
use mop_core::format::group_thousands;
use mop_core::freight::{CargoLine, Contract, FreightQuote, Packing, RateCard};
use mop_core::industry::TypeId;
use mop_core::money::Money;
use mop_core::plan::{BuyLine, Plan};
use mop_core::volume::Volume;
use mop_market::book::PriceRule;
use mop_market::fuzzwork::{Refresh, Side, Stat};
use mop_store::PlanRecord;

use super::costs::{InventionShown, invention_lines};
use super::invention::InventionLine;
use super::prices::{
    self, Held, Manual, OLD_HOVER, OLD_SECS, OutOfRange, Outcome, STALE_SECS, age_text, aged_text,
    failure_text,
};
use super::state::{BuildPage, push_error, report, type_name};
use crate::Shared;
use crate::status::Notice;
use crate::table::model::{Cell, Column, ColumnKind, Row, SortKey};
use crate::table::{Table, hidden_from_text};

mod freight;

pub(crate) use freight::chosen_card;
use freight::{cheapest, freight_legs, freight_reason, freight_row};

/// The column ids, in the order the columns are drawn.
const IDS: [&str; 10] = [
    "item", "need", "own", "buy", "price", "volume", "value", "live", "diff", "age",
];

/// The name the buy list's hidden columns are saved under.
const TABLE: &str = "build_buy_list";

/// What the panel asks the plan to change, applied after the panel is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Edit {
    Service(Option<i64>),
    TopUp(bool),
}

/// One freight service on the plan's route, with its quote or the reason it has none.
struct Offer {
    id: i64,
    name: String,
    outcome: Result<FreightQuote, String>,
    card: RateCard,
}

/// What the table shows beyond the row's own numbers.
#[derive(Debug, Clone, PartialEq, Eq)]
struct View {
    type_id: TypeId,
    /// Whether the market file has been downloaded. Before that a missing price reads `…`.
    fetched: bool,
    /// The live price per unit, when the file has one for the item on the rule's side.
    live: Option<Money>,
    /// The age of the live price in seconds.
    live_age: Option<i64>,
    /// The price the player typed, which the line uses in place of the live one.
    manual: Option<Manual>,
    /// Whether the line is priced from the live price: it has units to buy and no typed price.
    live_used: bool,
    /// The hover on a `—` price: which side of the market has no orders, or why no price exists.
    missing_hover: Option<&'static str>,
}

/// A price cell being typed over: the text, the price it started with, and whether the text was
/// rejected. Kept in the frame memory while the cell has focus or holds a rejected text.
#[derive(Debug, Clone, Default)]
struct PriceEdit {
    text: String,
    original: String,
    bad: bool,
}

/// The columns the player hid on the Buy list, kept in the frame memory after the saved setting is
/// read on the first frame of a session.
fn hidden_key() -> Id {
    Id::new("build buy list hidden columns")
}

/// The hidden columns saved for the buy list.
fn saved_hidden(cx: &mut Shared) -> Vec<String> {
    match cx.store.setting(&format!("columns.{TABLE}")) {
        Ok(Some(text)) => hidden_from_text(&text),
        Ok(None) => Vec::new(),
        Err(error) => {
            report(cx, format!("Could not read the settings: {error}"));
            Vec::new()
        }
    }
}

/// The hidden columns of this session: the saved setting on the first frame, then the frame memory.
fn hidden_columns(ui: &Ui, cx: &mut Shared) -> Vec<String> {
    if let Some(hidden) = ui.data(|data| data.get_temp(hidden_key())) {
        return hidden;
    }
    let hidden = saved_hidden(cx);
    ui.data_mut(|data| data.insert_temp(hidden_key(), hidden.clone()));
    hidden
}

/// Whether the column with `id` is hidden.
fn is_hidden(hidden: &[String], id: &str) -> bool {
    hidden.iter().any(|hidden| hidden == id)
}

pub fn show(page: &mut BuildPage, ui: &mut Ui, cx: &mut Shared) {
    if let Some(edit) = draw(page, ui, cx) {
        apply(page, cx, edit);
    }
}

/// The freight quote the delivered total uses: none needed without a route, else the chosen offer's.
fn quote_of<'a>(
    record: &PlanRecord,
    offers: &'a [Offer],
    chosen: Option<usize>,
    empty: &'a FreightQuote,
) -> Option<&'a FreightQuote> {
    if record.route.is_none() {
        return Some(empty);
    }
    chosen
        .and_then(|index| offers.get(index))
        .and_then(|offer| offer.outcome.as_ref().ok())
}

/// Draws the panel and returns the change asked for in it, if any.
fn draw(page: &BuildPage, ui: &mut Ui, cx: &mut Shared) -> Option<Edit> {
    // With no plan on screen the panel stays empty, as it did before it had rows.
    let record = page.record.as_ref()?;
    if let Err(error) = &page.result {
        let text = format!("Could not work out the plan: {error}");
        ui.label(RichText::new(text).color(cx.style.neg));
        return None;
    }
    let plan = page.result.as_ref().ok()?;
    let now = (cx.clock)();
    for outcome in cx.market.take_outcomes() {
        show_outcome(cx, outcome, now);
    }
    let held = cx.market.held();

    load_typed(ui, cx, page.selected);
    let invention = match invention_lines(ui, page, record, plan, cx, &held) {
        Ok(inventions) => inventions
            .parts
            .into_values()
            .filter_map(|shown| match shown {
                InventionShown::Counted { line, .. } => Some(line),
                InventionShown::Not | InventionShown::Unavailable(_) => None,
            })
            .collect::<Vec<_>>(),
        Err(error) => {
            report(cx, format!("Could not work out the invention: {error}"));
            Vec::new()
        }
    };
    let Some((inputs, views)) = buy_inputs(plan, record, cx, &held, now, &invention) else {
        report(cx, "Could not add up the buy list.");
        return None;
    };
    let Ok(buy) = buy_list(inputs) else {
        report(cx, "Could not add up the buy list.");
        return None;
    };
    let cargo = buy.cargo();
    let offers = offers(ui, record, cx, &cargo);
    let chosen = chosen(record.service, &offers);
    let empty = FreightQuote {
        contracts: Vec::new(),
        total: Money::ZERO,
    };
    let quote = quote_of(record, &offers, chosen, &empty);
    let Ok(delivered) = buy.delivered(quote) else {
        report(cx, "Could not add up the delivered total.");
        return None;
    };

    // The hidden columns are read once from the settings, then kept in the frame memory.
    let mut hidden = hidden_columns(ui, cx);
    let mut edit = None;
    ui.horizontal_wrapped(|ui| {
        edit = freight_row(ui, record, &offers, chosen, cx);
    });
    ui.horizontal_wrapped(|ui| {
        if ui
            .add_enabled(
                !cx.market.running() && !cx.market.stopped(),
                Button::new("Refresh now"),
            )
            .on_hover_text("Asks Fuzzwork for the prices again. Its file changes about every 30 minutes, so a request within 15 minutes of the last one is not made.")
            .clicked()
        {
            // The player started this refresh, so the tables sort at once with the new values.
            crate::table::resort_all(ui.ctx());
            cx.market.refresh(&cx.worker, now, true);
        }
        copy_button(ui, &buy, cx);
        if column_menu(ui, &mut hidden) {
            // An id for a column the table no longer has is dropped when the list is saved.
            hidden.retain(|id| IDS.contains(&id.as_str()));
            ui.data_mut(|data| data.insert_temp(hidden_key(), hidden.clone()));
            cx.save_hidden_columns(TABLE, &hidden);
        }
    });
    ui.label(format!("Price rule: {}", rule_text(record.price_rule)));
    let (typed, clicked) = table(ui, cx, record.id, now, &buy, &views, &hidden);
    save_typed(cx, page.selected, &typed);
    if let Some(key) = clicked
        && let Ok(type_id) = TypeId::try_from(key)
    {
        cx.selection.item = Some(type_id);
    }
    let totals = Totals {
        buy: &buy,
        delivered: &delivered,
        offers: &offers,
        record,
        views: &views,
    };
    footer_line(ui, cx, &totals);
    edit
}

/// Loads the prices typed for a saved plan into the session the first time the plan is shown. Later
/// frames keep what the session holds, so a price the store refused stays on screen.
fn load_typed(ui: &Ui, cx: &mut Shared, saved: Option<i64>) {
    let Some(plan) = saved else {
        return;
    };
    let key = Id::new(("build buy list typed prices loaded", plan));
    if ui.data(|data| data.get_temp::<bool>(key)).is_some() {
        return;
    }
    ui.data_mut(|data| data.insert_temp(key, true));
    match cx.store.manual_prices(plan) {
        Ok(prices) => {
            for price in prices {
                cx.market.set_manual(
                    plan,
                    price.type_id,
                    Some(Manual {
                        per_unit: price.per_unit,
                        set_at: price.set_at,
                    }),
                );
            }
        }
        Err(error) => report(cx, format!("Could not read the prices: {error}")),
    }
}

/// Saves the prices typed on the lines of a saved plan, and clears the ones that were emptied. A
/// price the store refuses stays on screen for the session and is reported.
fn save_typed(cx: &mut Shared, saved: Option<i64>, typed: &[(TypeId, Option<Manual>)]) {
    let Some(plan) = saved else {
        return;
    };
    for (type_id, price) in typed {
        let result = match price {
            Some(manual) => {
                cx.store
                    .set_manual_price(plan, *type_id, manual.per_unit, manual.set_at)
            }
            None => cx.store.clear_manual_price(plan, *type_id),
        };
        if let Err(error) = result {
            report(cx, format!("Could not save the price: {error}"));
        }
    }
}

/// Shows what a refresh answered: a failure always, and a too-soon answer through [`too_soon_notice`].
fn show_outcome(cx: &mut Shared, outcome: Outcome, now: i64) {
    match outcome.result {
        Err(error) => push_error(&mut cx.notices, failure_text(&error)),
        Ok(refresh) => {
            let published = cx.market.published_at();
            if let Some(text) = too_soon_notice(outcome.explicit, &refresh, published, now) {
                cx.notices.push(Notice::info(text));
            }
        }
    }
}

/// The notice for a refresh the market answered as too soon. It is shown only when the player asked
/// for the refresh: a timer refresh that is too soon stays quiet. `published` is when the file held
/// was made.
fn too_soon_notice(
    explicit: bool,
    refresh: &Refresh,
    published: Option<i64>,
    now: i64,
) -> Option<String> {
    let Refresh::TooSoon { next_at } = refresh else {
        return None;
    };
    if !explicit {
        return None;
    }
    // A floor at 0 is meant here: a publish time after `now` (clock skew) shows as an age of zero.
    let age = published.map_or(0, |at| now.saturating_sub(at));
    Some(format!(
        "Prices are {} old; next refresh in {}",
        age_text(age),
        // A floor at 0 is meant here: a refresh time already past shows as zero seconds left.
        age_text(next_at.saturating_sub(now)),
    ))
}

/// Adds the datacores and decryptors of the whole attempts of every invented part to the plan's
/// buy lines. Equal types from several parts share one line. Stock the plan did not use covers the
/// added amount. `None` when the amounts do not fit the supported range.
fn with_invention(
    lines: &[BuyLine],
    invention: &[InventionLine],
    unused_stock: &[(TypeId, u64)],
) -> Option<Vec<BuyLine>> {
    let mut added: BTreeMap<TypeId, u64> = BTreeMap::new();
    for line in invention {
        for (type_id, quantity) in &line.materials {
            let amount = quantity.checked_mul(line.need.attempts)?;
            let total = added.entry(*type_id).or_insert(0);
            *total = total.checked_add(amount)?;
        }
        if let Some(decryptor) = line.decryptor {
            let total = added.entry(decryptor).or_insert(0);
            *total = total.checked_add(line.need.attempts)?;
        }
    }
    let mut result = lines.to_vec();
    for (type_id, amount) in added {
        let stock = unused_stock
            .iter()
            .find(|(id, _)| *id == type_id)
            .map_or(0, |(_, stock)| *stock);
        let from_stock = amount.min(stock);
        if let Some(line) = result.iter_mut().find(|line| line.type_id == type_id) {
            line.needed = line.needed.checked_add(amount)?;
            line.from_stock = line.from_stock.checked_add(from_stock)?;
            line.to_buy = line.needed.checked_sub(line.from_stock)?;
        } else {
            result.push(BuyLine {
                type_id,
                depth: 0,
                needed: amount,
                from_stock,
                to_buy: amount.checked_sub(from_stock)?,
            });
        }
    }
    Some(result)
}

/// The buy inputs of the plan's buy lines and the invention's datacores and decryptors, and what
/// the table shows for each.
fn buy_inputs(
    plan: &Plan,
    record: &PlanRecord,
    cx: &mut Shared,
    held: &Held,
    now: i64,
    invention: &[InventionLine],
) -> Option<(Vec<BuyInput>, BTreeMap<u64, View>)> {
    let lines = with_invention(&plan.buy, invention, &plan.unused_stock)?;
    let (side, _) = prices::rule_source(record.price_rule);
    let mut inputs = Vec::with_capacity(lines.len());
    let mut views = BTreeMap::new();
    for line in &lines {
        let (input, view) = buy_input(line, side, record, cx, held, now);
        views.insert(u64::from(line.type_id), view);
        inputs.push(input);
    }
    Some((inputs, views))
}

/// What one buy line asks for, what it costs and what the table shows of it.
fn buy_input(
    line: &BuyLine,
    side: Side,
    record: &PlanRecord,
    cx: &mut Shared,
    held: &Held,
    now: i64,
) -> (BuyInput, View) {
    let name = type_name(line.type_id, cx.sde.as_ref(), &mut cx.notices);
    let unit_volume = cx
        .sde
        .as_ref()
        .and_then(|sde| sde.item(line.type_id).ok().flatten())
        .and_then(|item| item.haul_volume());
    let live = prices::live(held, line.type_id, record.price_rule);
    let manual = cx.market.manual(record.id, line.type_id);
    let (price, overflow) = if line.to_buy == 0 {
        (LinePrice::Missing, false)
    } else {
        match prices::line_price(live.as_ref(), manual, line.to_buy) {
            Ok(price) => (price, false),
            Err(OutOfRange) => (LinePrice::Missing, true),
        }
    };
    let missing_hover = if overflow {
        Some("price out of range")
    } else if line.to_buy > 0 && matches!(price, LinePrice::Missing) && held.published_at.is_some()
    {
        Some(match side {
            Side::Sell => "no Forge sell orders",
            Side::Buy => "no Forge buy orders",
        })
    } else {
        None
    };
    let view = View {
        type_id: line.type_id,
        fetched: held.published_at.is_some(),
        live: live.map(|live| live.per_unit),
        // A floor at 0 is meant here: a live publish time after `now` (clock skew) shows as age zero.
        live_age: live.map(|live| now.saturating_sub(live.published_at)),
        manual,
        live_used: manual.is_none() && line.to_buy > 0 && live.is_some(),
        missing_hover,
    };
    let input = BuyInput {
        type_id: line.type_id,
        name,
        needed: line.needed,
        from_stock: line.from_stock,
        to_buy: line.to_buy,
        unit_volume,
        price,
    };
    (input, view)
}

/// One freight offer per service on the plan's route, in the order they were saved.
fn offers(ui: &Ui, record: &PlanRecord, cx: &mut Shared, cargo: &[CargoLine]) -> Vec<Offer> {
    let Some(route) = record.route else {
        return Vec::new();
    };
    freight_legs(ui.ctx(), route, cx)
        .into_iter()
        .map(|leg| {
            let outcome = match leg.jumps {
                Ok(Some(jumps)) => freight::quote(&leg.card, jumps, cargo, record.packing)
                    .map_err(|error| freight_reason(&error, cx)),
                Ok(None) => Err("no route".to_owned()),
                Err(error) => Err(error),
            };
            Offer {
                id: leg.id,
                name: leg.name,
                outcome,
                card: leg.card,
            }
        })
        .collect()
}

/// The index of the service that is charged: the saved one when it is still on the route, else the
/// cheapest that can carry the cargo.
fn chosen(service: Option<i64>, offers: &[Offer]) -> Option<usize> {
    if let Some(id) = service
        && let Some(index) = offers.iter().position(|offer| offer.id == id)
    {
        return Some(index);
    }
    cheapest(offers)
}

/// One contract in the hover of the contracts: its number, volume, collateral and charge, and
/// whether the charge is the service's minimum.
fn contract_line(number: usize, contract: &Contract) -> String {
    let minimum = if contract.pays_minimum {
        " · pays the minimum"
    } else {
        ""
    };
    format!(
        "{number}: {} m³ · {} collateral · {}{minimum}",
        contract.volume.format_m3(),
        contract.value.format_isk(),
        contract.charge.format_isk(),
    )
}

/// Copies the multibuy text. Disabled when nothing is to be bought.
fn copy_button(ui: &mut Ui, buy: &BuyList, cx: &mut Shared) {
    let lines = buy.rows.iter().filter(|row| row.to_buy > 0).count();
    let text = buy.multibuy_text();
    let response = ui
        .add_enabled(lines > 0, Button::new("Copy multibuy"))
        .on_hover_text("One line per item: name, tab, quantity. Paste into EVE's multibuy window.");
    if response.clicked() {
        ui.ctx().copy_text(text);
        cx.notices
            .push(Notice::info(format!("Copied {lines} lines for multibuy.")));
    }
}

/// The price rule as the Plan panel names it, read as the side and statistic it uses: a plan on the
/// walk-the-book rule reads the 5% price, so it is named that way.
fn rule_text(rule: PriceRule) -> String {
    let (side, stat) = prices::rule_source(rule);
    let side_text = match side {
        Side::Sell => "sell",
        Side::Buy => "buy",
    };
    let stat_text = match (stat, side) {
        (Stat::Best, Side::Buy) => "highest",
        (Stat::Best, Side::Sell) => "cheapest",
        (Stat::FivePercent, _) => "5% price",
        (Stat::WeightedAverage, _) => "weighted average",
        (Stat::Median, _) => "middle",
    };
    format!("Forge {side_text}, {stat_text}")
}

/// The menu that shows or hides each column. Returns whether the choice changed.
fn column_menu(ui: &mut Ui, hidden: &mut Vec<String>) -> bool {
    let mut changed = false;
    ui.menu_button("Columns", |ui| {
        for column in all_columns() {
            let mut shown = !is_hidden(hidden, column.id);
            if ui.checkbox(&mut shown, column.label.as_str()).changed() {
                changed = true;
                if shown {
                    hidden.retain(|id| id != column.id);
                } else {
                    hidden.push(column.id.to_owned());
                }
            }
        }
    });
    changed
}

/// The table cells of one buy row, in the order of [`IDS`].
fn table_cells(row: &BuyRow, view: &View) -> Vec<Cell> {
    let fetched = view.fetched;
    let price = match &row.price {
        LinePrice::Priced { per_unit, .. } => Cell::money(*per_unit),
        // A thin line is priced at the live price: only its units on the market are short.
        LinePrice::Thin { .. } => view.live.map_or_else(Cell::missing, Cell::money),
        LinePrice::Missing if fetched => Cell::missing(),
        LinePrice::Missing => Cell::text("…"),
    };
    let age = if view.manual.is_some() {
        Cell::text("manual")
    } else {
        view.live_age
            .map_or_else(Cell::missing, |age| Cell::text(aged_text(age)))
    };
    let value = match (&row.value, &row.price) {
        (Some(value), _) => Cell::money(*value),
        (None, LinePrice::Thin { available, .. }) => {
            Cell::text(format!("only {available} on market"))
        }
        (None, _) if fetched => Cell::missing(),
        (None, _) => Cell::text("…"),
    };
    vec![
        Cell::text(row.name.as_str()),
        count(row.needed),
        count(row.from_stock),
        count(row.to_buy),
        price,
        row.volume.map_or_else(Cell::missing, volume_cell),
        value,
        view.live.map_or_else(Cell::missing, Cell::money),
        Cell::text(diff_text(view.manual, view.live)),
        age,
    ]
}

/// The typed price against the live one, in whole percent with a sign, rounded down. Empty
/// without both, or when the live price is zero.
fn diff_text(manual: Option<Manual>, live: Option<Money>) -> String {
    let (Some(manual), Some(live)) = (manual, live) else {
        return String::new();
    };
    if live.ten_thousandths() <= 0 {
        return String::new();
    }
    let change =
        (i128::from(manual.per_unit.ten_thousandths()) - i128::from(live.ten_thousandths())) * 100;
    let percent = change.div_euclid(i128::from(live.ten_thousandths()));
    let sign = if percent < 0 { '-' } else { '+' };
    let size = match i64::try_from(percent.abs()) {
        Ok(size) => group_thousands(size),
        Err(_) => percent.abs().to_string(),
    };
    format!("{sign}{size}%")
}

/// A volume in m³, drawn with two decimals and copied as its plain amount.
fn volume_cell(volume: Volume) -> Cell {
    Cell {
        text: volume.format_m3(),
        plain: volume.to_plain(),
        key: SortKey::Int(i128::from(volume.ten_thousandths())),
    }
}

/// A count, grouped in thousands.
fn count(amount: u64) -> Cell {
    Cell::int(
        i128::from(amount),
        group_thousands(i64::try_from(amount).unwrap_or(i64::MAX)),
    )
}

fn all_columns() -> Vec<Column> {
    vec![
        column("item", "Item", ColumnKind::Text, 170.0),
        column("need", "Need", ColumnKind::Number, 58.0),
        column("own", "Own", ColumnKind::Number, 58.0),
        column("buy", "Buy", ColumnKind::Number, 58.0),
        column("price", "Price", ColumnKind::Money, 100.0),
        column("volume", "m³", ColumnKind::Number, 80.0),
        column("value", "Value", ColumnKind::Money, 120.0),
        column("live", "Live", ColumnKind::Money, 110.0),
        column("diff", "Diff", ColumnKind::Text, 80.0),
        column("age", "Age", ColumnKind::Text, 70.0),
    ]
}

fn column(id: &'static str, label: &str, kind: ColumnKind, width: f32) -> Column {
    Column {
        id,
        label: label.to_owned(),
        kind,
        width,
    }
}

/// The items of a list whose column is shown, in column order.
fn shown<T>(items: Vec<T>, hidden: &[String]) -> Vec<T> {
    items
        .into_iter()
        .zip(IDS)
        .filter(|(_, id)| !is_hidden(hidden, id))
        .map(|(item, _)| item)
        .collect()
}

/// The table's footer row, aligned with the columns: the counts, the volume and the value.
fn footer_cells(buy: &BuyList) -> Vec<Cell> {
    let needed = buy
        .rows
        .iter()
        .fold(0u64, |sum, row| sum.saturating_add(row.needed));
    let from_stock = buy
        .rows
        .iter()
        .fold(0u64, |sum, row| sum.saturating_add(row.from_stock));
    let to_buy = buy
        .rows
        .iter()
        .fold(0u64, |sum, row| sum.saturating_add(row.to_buy));
    vec![
        Cell::text("Total"),
        count(needed),
        count(from_stock),
        count(to_buy),
        Cell::text(""),
        volume_cell(buy.volume),
        Cell::money(buy.value),
        Cell::text(""),
        Cell::text(""),
        Cell::text(""),
    ]
}

/// The table of the buy rows, with the counts, the volume and the value in a footer row.
/// Draws the table and returns the prices typed in it, to save with the plan.
fn table(
    ui: &mut Ui,
    cx: &Shared,
    plan: i64,
    now: i64,
    buy: &BuyList,
    views: &BTreeMap<u64, View>,
    hidden: &[String],
) -> (Vec<(TypeId, Option<Manual>)>, Option<u64>) {
    let mut typed = Vec::new();
    let columns = shown(all_columns(), hidden);
    let rows: Vec<Row> = buy
        .rows
        .iter()
        .filter_map(|row| {
            let view = views.get(&u64::from(row.type_id))?;
            Some(Row {
                key: u64::from(row.type_id),
                depth: 0,
                cells: shown(table_cells(row, view), hidden),
            })
        })
        .collect();
    let footer = shown(footer_cells(buy), hidden);
    let mut draw = |ui: &mut Ui, key: u64, column: &'static str| -> bool {
        let Some(view) = views.get(&key) else {
            return false;
        };
        match column {
            "price" => {
                price_edit(ui, cx, plan, now, view, &mut typed);
                true
            }
            "age" => match (view.manual, view.live_age) {
                (Some(manual), _) => {
                    // A floor at 0 is meant here: a typed time after `now` (clock set back) shows as age zero.
                    let typed = age_text(now.saturating_sub(manual.set_at));
                    ui.label("manual").on_hover_text(format!(
                        "Typed {typed} ago. It does not change with the market."
                    ));
                    true
                }
                (None, Some(age)) if age > OLD_SECS => {
                    ui.colored_label(cx.style.stale, aged_text(age))
                        .on_hover_text(OLD_HOVER);
                    true
                }
                (None, Some(age)) if age > STALE_SECS => {
                    ui.colored_label(cx.style.stale, age_text(age));
                    true
                }
                _ => false,
            },
            _ => false,
        }
    };
    let clicked = Table::new(Id::new(super::BuildPanel::BuyList.title()), &columns)
        .fit_rows(rows.len())
        .icons("item", &|key| u32::try_from(key).ok())
        .show_with(ui, &cx.style, cx.density, &rows, Some(&footer), &mut draw)
        .clicked;
    (typed, clicked)
}

/// The Price cell: the price the line uses, typed over. The text is committed when the field loses
/// focus. Text that is not a price stays with `Not a price` under it, and the price held does not
/// change. An empty field returns the line to the live price.
fn price_edit(
    ui: &mut Ui,
    cx: &Shared,
    plan: i64,
    now: i64,
    view: &View,
    typed: &mut Vec<(TypeId, Option<Manual>)>,
) {
    let key = Id::new(("build buy list price", plan, view.type_id));
    let shown = view.manual.map(|manual| manual.per_unit).or(view.live);
    // A line with no price shows `—`, or `…` until the first download is in; typing over it sets a price.
    let original = match shown {
        Some(price) => price.format_isk(),
        None if view.fetched => "—".to_owned(),
        None => "…".to_owned(),
    };
    let mut edit: PriceEdit = ui
        .data(|data| data.get_temp(key))
        .unwrap_or_else(|| PriceEdit {
            text: original.clone(),
            original: original.clone(),
            bad: false,
        });
    let hint = if view.fetched { "—" } else { "…" };
    let color = if view.manual.is_some() {
        cx.style.accent
    } else {
        cx.style.text
    };
    ui.vertical(|ui| {
        let response = ui
            .add(
                TextEdit::singleline(&mut edit.text)
                    .hint_text(hint)
                    .text_color(color)
                    .desired_width(110.0),
            )
            .on_hover_text(if let Some(hover) = view.missing_hover {
                hover
            } else if view.manual.is_some() {
                "A typed price: the market does not change it. Clear the cell to use the market price."
            } else {
                "The market price. Type a price to freeze it for this plan."
            });
        if response.changed() {
            edit.bad = false;
        }
        if response.lost_focus() && edit.text != edit.original {
            match typed_price(&edit.text) {
                Ok(price) => {
                    let price = price.map(|per_unit| Manual {
                        per_unit,
                        set_at: now,
                    });
                    cx.market.set_manual(plan, view.type_id, price);
                    typed.push((view.type_id, price));
                }
                Err(()) => edit.bad = true,
            }
        }
        if response.has_focus() || edit.bad {
            ui.data_mut(|data| data.insert_temp(key, edit.clone()));
        } else {
            ui.data_mut(|data| data.remove::<PriceEdit>(key));
        }
        if edit.bad {
            ui.label(RichText::new("Not a price").color(cx.style.neg));
        }
    });
}

/// The price typed in a cell: `Ok(None)` for an empty cell, which clears the typed price. A
/// negative or unreadable text is an error.
fn typed_price(text: &str) -> Result<Option<Money>, ()> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    match Money::parse_isk(text) {
        Ok(price) if price.ten_thousandths() >= 0 => Ok(Some(price)),
        _ => Err(()),
    }
}

/// What the footer adds up.
struct Totals<'a> {
    buy: &'a BuyList,
    delivered: &'a Delivered,
    offers: &'a [Offer],
    record: &'a PlanRecord,
    views: &'a BTreeMap<u64, View>,
}

/// The footer line, then the age of the oldest live price among the lines to buy, and the state of
/// a refresh that runs or has stopped. The freight amount lists every service on hover, and an
/// incomplete total says why on hover.
fn footer_line(ui: &mut Ui, cx: &Shared, totals: &Totals) {
    let &Totals {
        buy,
        delivered,
        offers,
        record,
        views,
    } = totals;
    ui.horizontal_wrapped(|ui| {
        ui.label("freight ");
        ui.label(delivered.freight.format_isk_short())
            .on_hover_ui(|ui| {
                if record.route.is_none() {
                    ui.label("No freight");
                    return;
                }
                for offer in offers {
                    match &offer.outcome {
                        Ok(quote) => {
                            ui.label(format!("{} {}", offer.name, quote.total.format_isk()))
                        }
                        Err(reason) => ui.label(format!("{}: {reason}", offer.name)),
                    };
                }
                if offers.is_empty() {
                    ui.label("no freight service on this route");
                }
            });
        ui.label(format!(
            " · collateral {} · delivered {}",
            delivered.collateral.format_isk_short(),
            delivered.total.format_isk_short()
        ))
        .on_hover_text(format!(
            "{} · {} ISK",
            delivered.collateral.format_isk(),
            delivered.total.format_isk()
        ));
        if delivered.incomplete {
            let mut reasons: Vec<String> = buy
                .incomplete
                .iter()
                .filter_map(|type_id| {
                    buy.rows
                        .iter()
                        .find(|row| row.type_id == *type_id)
                        .map(|row| row.name.clone())
                })
                .collect();
            if record.route.is_some() && offers.iter().all(|offer| offer.outcome.is_err()) {
                reasons.push("no freight service can carry this".to_owned());
            }
            ui.colored_label(cx.style.stale, " incomplete")
                .on_hover_text(reasons.join("\n"));
        }
    });
    ui.horizontal_wrapped(|ui| {
        let oldest = views
            .values()
            .filter(|view| view.live_used)
            .filter_map(|view| view.live_age)
            .max();
        if let Some(age) = oldest {
            let text = if age > OLD_SECS {
                format!(
                    "Oldest: {} · Prices over a day old: refresh before you buy.",
                    age_text(age)
                )
            } else {
                format!("Oldest: {}", age_text(age))
            };
            if age > STALE_SECS {
                ui.colored_label(cx.style.stale, text);
            } else {
                ui.label(text);
            }
            if cx.market.running() || cx.market.stopped() {
                ui.label(" · ");
            }
        }
        if cx.market.running() {
            ui.label("Refreshing…");
        } else if cx.market.stopped() {
            ui.label("Prices cannot refresh until the app restarts.");
        }
    });
}

/// Changes the plan on screen and works it out again.
fn apply(page: &mut BuildPage, cx: &mut Shared, edit: Edit) {
    if let Some(record) = page.record.as_mut() {
        match edit {
            Edit::Service(choice) => record.service = choice,
            Edit::TopUp(on) => {
                record.packing = if on {
                    Packing::TopUp
                } else {
                    Packing::FillOnly
                };
            }
        }
        page.edited(cx);
    }
}

#[cfg(test)]
mod tests {
    use mop_core::money::Money;
    use mop_market::fuzzwork::Refresh;

    use mop_core::freight::Contract;
    use mop_core::invention_cost::InventionNeed;
    use mop_core::plan::BuyLine;
    use mop_core::volume::Volume;

    use super::{InventionLine, Manual, contract_line, diff_text, too_soon_notice, with_invention};

    fn isk(text: &str) -> Money {
        Money::parse_isk(text).expect("a price")
    }

    fn need(attempts: u64) -> InventionNeed {
        InventionNeed {
            copies: 1,
            attempts,
            cost_per_copy: Money::ZERO,
            carried: Money::ZERO,
            spare_runs: 0,
        }
    }

    fn invention(
        materials: Vec<(u32, u64)>,
        decryptor: Option<u32>,
        attempts: u64,
    ) -> InventionLine {
        InventionLine {
            type_id: 2456,
            need: need(attempts),
            attempt_cost: Money::ZERO,
            materials,
            decryptor,
        }
    }

    fn plan_line(type_id: u32, depth: u32, needed: u64, from_stock: u64) -> BuyLine {
        BuyLine {
            type_id,
            depth,
            needed,
            from_stock,
            to_buy: needed - from_stock,
        }
    }

    #[test]
    fn every_whole_attempt_buys_its_materials_and_decryptor() {
        let parts = [invention(vec![(20418, 1), (20419, 1)], Some(34202), 3)];
        let lines = with_invention(&[], &parts, &[]).expect("the lines add up");
        assert_eq!(
            lines,
            [
                plan_line(20418, 0, 3, 0),
                plan_line(20419, 0, 3, 0),
                plan_line(34202, 0, 3, 0),
            ]
        );
    }

    #[test]
    fn equal_types_from_several_parts_share_a_line_the_plan_already_has() {
        let parts = [
            invention(vec![(20418, 2)], None, 2),
            invention(vec![(20418, 3)], None, 2),
        ];
        let plan = [plan_line(20418, 1, 5, 1)];
        // The parts need 4 + 6 = 10, and the unused stock of 4 covers part of it.
        let lines = with_invention(&plan, &parts, &[(20418, 4)]).expect("the lines add up");
        assert_eq!(lines, [plan_line(20418, 1, 15, 5)]);
    }

    #[test]
    fn unused_stock_covers_no_more_than_the_added_amount() {
        let parts = [invention(vec![(20418, 1)], None, 3)];
        let lines = with_invention(&[], &parts, &[(20418, 5)]).expect("the lines add up");
        assert_eq!(lines, [plan_line(20418, 0, 3, 3)]);
    }

    #[test]
    fn amounts_that_do_not_fit_the_supported_range_give_no_lines() {
        let parts = [invention(vec![(20418, 2)], None, u64::MAX)];
        assert_eq!(with_invention(&[], &parts, &[]), None);

        let parts = [
            invention(vec![(20418, u64::MAX)], None, 1),
            invention(vec![(20418, 1)], None, 1),
        ];
        assert_eq!(with_invention(&[], &parts, &[]), None);
    }

    fn typed(text: &str) -> Manual {
        Manual {
            per_unit: isk(text),
            set_at: 0,
        }
    }

    #[test]
    fn the_diff_is_a_whole_percent_with_a_sign() {
        let live = Some(isk("100.00"));
        assert_eq!(diff_text(Some(typed("200.00")), live), "+100%");
        assert_eq!(diff_text(Some(typed("50.00")), live), "-50%");
        assert_eq!(diff_text(Some(typed("199.99")), live), "+99%");
        assert_eq!(diff_text(Some(typed("100.00")), live), "+0%");
    }

    #[test]
    fn the_diff_needs_a_typed_price_and_a_live_one() {
        assert_eq!(diff_text(None, Some(isk("100.00"))), "");
        assert_eq!(diff_text(Some(typed("200.00")), None), "");
        assert_eq!(diff_text(Some(typed("200.00")), Some(Money::ZERO)), "");
    }

    #[test]
    fn a_timer_refresh_that_is_too_soon_stays_quiet() {
        let refresh = Refresh::TooSoon { next_at: 2_000_600 };
        assert_eq!(
            too_soon_notice(false, &refresh, Some(1_999_460), 2_000_000),
            None
        );
    }

    #[test]
    fn a_refresh_the_player_asked_for_says_how_old_the_prices_are() {
        let refresh = Refresh::TooSoon { next_at: 2_000_600 };
        assert_eq!(
            too_soon_notice(true, &refresh, Some(1_999_460), 2_000_000).as_deref(),
            Some("Prices are 9m old; next refresh in 10m")
        );
    }

    #[test]
    fn a_contract_at_its_minimum_says_so_in_its_hover() {
        let contract = Contract {
            pieces: Vec::new(),
            volume: Volume::from_ten_thousandths(1_205_000),
            value: isk("1114686.00"),
            charge: isk("200000000.00"),
            pays_minimum: true,
        };
        assert_eq!(
            contract_line(1, &contract),
            "1: 120.50 m³ · 1,114,686.00 collateral · 200,000,000.00 · pays the minimum"
        );
    }

    #[test]
    fn a_contract_above_its_minimum_does_not_say_so() {
        let contract = Contract {
            pieces: Vec::new(),
            volume: Volume::from_ten_thousandths(1_205_000),
            value: isk("1114686.00"),
            charge: isk("107998.43"),
            pays_minimum: false,
        };
        assert_eq!(
            contract_line(2, &contract),
            "2: 120.50 m³ · 1,114,686.00 collateral · 107,998.43"
        );
    }

    #[test]
    fn an_answer_that_is_not_too_soon_gives_no_notice() {
        assert_eq!(
            too_soon_notice(true, &Refresh::Downloaded, Some(1_999_460), 2_000_000),
            None
        );
        assert_eq!(
            too_soon_notice(true, &Refresh::NotModified, Some(1_999_460), 2_000_000),
            None
        );
    }
}

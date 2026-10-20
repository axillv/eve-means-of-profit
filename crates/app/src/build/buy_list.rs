//! The Buy list panel: what to buy at Jita for the plan on screen, what each line costs and how old
//! its price is, the freight each service charges, and the delivered total. The list copies to
//! EVE's multibuy window.

use egui::{Button, Checkbox, ComboBox, Id, RichText, Ui};
use profit_core::buylist::{BuyInput, BuyList, BuyRow, LinePrice, buy_list};
use profit_core::format::group_thousands;
use profit_core::freight::{self, CargoLine, FreightError, FreightQuote, Packing};
use profit_core::industry::TypeId;
use profit_core::money::Money;
use profit_core::plan::Plan;
use profit_market::book::PriceRule;
use profit_sde::Sde;
use profit_store::PlanRecord;
use std::collections::BTreeMap;

use super::prices::{STALE_SECS, age_text, failure_text};
use super::state::{BuildPage, push_error, report, type_name};
use crate::Shared;
use crate::status::Notice;
use crate::table::Table;
use crate::table::model::{Cell, Column, ColumnKind, Row};

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
}

/// What the table shows beyond the row's own numbers.
#[derive(Debug, Clone, PartialEq, Eq)]
struct View {
    /// Whether a book is held for the line. Lines to buy without one show `…`.
    fetched: bool,
    /// The hover on a missing price: which side of the book had no orders.
    missing_hover: Option<&'static str>,
    /// Age of the price in seconds, `None` when there is no price.
    age: Option<i64>,
}

pub fn show(page: &mut BuildPage, ui: &mut Ui, cx: &mut Shared) {
    if let Some(edit) = draw(page, ui, cx) {
        apply(page, cx, edit);
    }
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
    for error in cx.market.take_failures() {
        push_error(&mut cx.notices, failure_text(&error));
    }
    let now = (cx.clock)();
    let wanted: Vec<TypeId> = plan
        .buy
        .iter()
        .filter(|line| line.to_buy > 0)
        .map(|line| line.type_id)
        .collect();
    request_prices(ui, cx, &wanted, now);

    let (inputs, views) = buy_inputs(plan, record, cx, now);
    let Ok(buy) = buy_list(inputs) else {
        report(cx, "Could not add up the buy list.");
        return None;
    };
    let cargo = buy.cargo();
    let offers = offers(record, cx, &cargo);
    let chosen = chosen(record.service, &offers);
    let empty = FreightQuote {
        contracts: Vec::new(),
        total: Money::ZERO,
    };
    let quote = if record.route.is_none() {
        Some(&empty)
    } else {
        chosen
            .and_then(|index| offers.get(index))
            .and_then(|offer| offer.outcome.as_ref().ok())
    };
    let Ok(delivered) = buy.delivered(quote) else {
        report(cx, "Could not add up the delivered total.");
        return None;
    };

    let mut edit = None;
    ui.horizontal_wrapped(|ui| {
        edit = freight_row(ui, record, &offers, chosen, cx);
    });
    ui.horizontal_wrapped(|ui| {
        if ui
            .add_enabled(!wanted.is_empty(), Button::new("Refresh prices"))
            .on_hover_text("Asks Jita prices again for the lines to buy. The cache keeps any price that has not expired.")
            .clicked()
        {
            cx.market.request(&cx.worker, wanted.clone(), now);
        }
        copy_button(ui, &buy, cx);
    });
    table(ui, cx, &buy, &views);
    footer_line(ui, &buy, &delivered, &offers, record, cx, now);
    edit
}

/// Asks for the books of the lines to buy that have none yet. It asks once for each change of the
/// lines; `Refresh prices` asks for them all again.
fn request_prices(ui: &mut Ui, cx: &mut Shared, wanted: &[TypeId], now: i64) {
    let key = Id::new("build buy list prices");
    let previous: Option<Vec<TypeId>> = ui.data(|data| data.get_temp(key));
    if previous.as_deref() == Some(wanted) {
        return;
    }
    let missing: Vec<TypeId> = wanted
        .iter()
        .copied()
        .filter(|type_id| !cx.market.held(*type_id))
        .collect();
    if !missing.is_empty() {
        cx.market.request(&cx.worker, missing, now);
    }
    ui.data_mut(|data| data.insert_temp(key, wanted.to_vec()));
}

/// The buy inputs of the plan's buy lines, in their order, and what the table shows for each.
fn buy_inputs(
    plan: &Plan,
    record: &PlanRecord,
    cx: &mut Shared,
    now: i64,
) -> (Vec<BuyInput>, BTreeMap<u64, View>) {
    let mut inputs = Vec::with_capacity(plan.buy.len());
    let mut views = BTreeMap::new();
    for line in &plan.buy {
        let name = type_name(line.type_id, cx.sde.as_ref(), &mut cx.notices);
        let unit_volume = cx
            .sde
            .as_ref()
            .and_then(|sde| sde.item(line.type_id).ok().flatten())
            .and_then(|item| item.haul_volume());
        let (price, fetched) = if line.to_buy == 0 {
            (LinePrice::Missing, true)
        } else {
            match cx
                .market
                .line_price(line.type_id, record.price_rule, line.to_buy)
            {
                None => (LinePrice::Missing, false),
                Some(Ok(price)) => (price, true),
                Some(Err(error)) => {
                    push_error(&mut cx.notices, failure_text(&error));
                    (LinePrice::Missing, true)
                }
            }
        };
        let age = priced_at(&price).map(|at| now.saturating_sub(at));
        views.insert(
            u64::from(line.type_id),
            View {
                fetched,
                missing_hover: missing_hover(&price, record.price_rule, line.to_buy),
                age,
            },
        );
        inputs.push(BuyInput {
            type_id: line.type_id,
            name,
            needed: line.needed,
            from_stock: line.from_stock,
            to_buy: line.to_buy,
            unit_volume,
            price,
        });
    }
    (inputs, views)
}

fn priced_at(price: &LinePrice) -> Option<i64> {
    match price {
        LinePrice::Priced { priced_at, .. } | LinePrice::Thin { priced_at, .. } => Some(*priced_at),
        LinePrice::Missing => None,
    }
}

/// The hover on a `—` price: the side of the book the rule reads has no orders.
fn missing_hover(price: &LinePrice, rule: PriceRule, to_buy: u64) -> Option<&'static str> {
    if to_buy == 0 || !matches!(price, LinePrice::Missing) {
        return None;
    }
    Some(match rule {
        PriceRule::HighestBuy => "no Jita buy orders",
        PriceRule::WalkSell | PriceRule::LowestSell => "no Jita sell orders",
    })
}

/// One freight service per service on the plan's route, in the order they were saved.
fn offers(record: &PlanRecord, cx: &mut Shared, cargo: &[CargoLine]) -> Vec<Offer> {
    let Some(route_id) = record.route else {
        return Vec::new();
    };
    let routes = match cx.store.routes() {
        Ok(routes) => routes,
        Err(error) => {
            report(cx, format!("Could not read the routes: {error}"));
            return Vec::new();
        }
    };
    let Some(route) = routes.into_iter().find(|route| route.id == route_id) else {
        return Vec::new();
    };
    let star = match cx.sde.as_ref().map(Sde::star_map) {
        Some(Ok(star)) => Some(star),
        Some(Err(error)) => {
            report(cx, format!("Could not read the map: {error}"));
            None
        }
        None => None,
    };
    let services = match cx.store.services(route_id) {
        Ok(services) => services,
        Err(error) => {
            report(cx, format!("Could not read the freight services: {error}"));
            return Vec::new();
        }
    };
    services
        .into_iter()
        .map(|service| {
            let jumps = match (service.jumps_override, &star) {
                (Some(jumps), _) => Ok(Some(jumps)),
                (None, Some(star)) => star
                    .jumps(route.from_system, route.to_system, service.route_kind)
                    .map_err(|error| error.to_string()),
                (None, None) => Ok(None),
            };
            let outcome = match jumps {
                Ok(Some(jumps)) => freight::quote(&service.card, jumps, cargo, record.packing)
                    .map_err(|error| freight_reason(&error, cx)),
                Ok(None) => Err("no route".to_owned()),
                Err(error) => Err(error),
            };
            Offer {
                id: service.id,
                name: service.name,
                outcome,
            }
        })
        .collect()
}

/// Why a service cannot carry the cargo, in words the panel shows.
fn freight_reason(error: &FreightError, cx: &mut Shared) -> String {
    match error {
        FreightError::CannotCarry(type_id) => format!(
            "cannot carry {}",
            type_name(*type_id, cx.sde.as_ref(), &mut cx.notices)
        ),
        FreightError::NegativeRate | FreightError::Overflow => error.to_string(),
    }
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

/// The cheapest offer that has a quote; the earlier one wins a tie.
fn cheapest(offers: &[Offer]) -> Option<usize> {
    let mut best: Option<(usize, Money)> = None;
    for (index, offer) in offers.iter().enumerate() {
        let Ok(quote) = &offer.outcome else { continue };
        if best.is_none_or(|(_, so_far)| quote.total < so_far) {
            best = Some((index, quote.total));
        }
    }
    best.map(|(index, _)| index)
}

/// The freight row: the service dropdown, the top-up tick, and the contracts of the chosen service.
fn freight_row(
    ui: &mut Ui,
    record: &PlanRecord,
    offers: &[Offer],
    chosen: Option<usize>,
    cx: &Shared,
) -> Option<Edit> {
    ui.label("Freight");
    if record.route.is_none() {
        ui.label(RichText::new("No freight").color(cx.style.mute));
        return None;
    }
    let cheapest_name = cheapest(offers)
        .and_then(|index| offers.get(index))
        .map_or("none", |offer| offer.name.as_str());
    let mut options: Vec<(Option<i64>, String)> =
        vec![(None, format!("Cheapest ({cheapest_name})"))];
    options.extend(
        offers
            .iter()
            .map(|offer| (Some(offer.id), offer.name.clone())),
    );
    let mut edit = None;
    if let Some(choice) = choose(ui, "build_buy_service", record.service, &options) {
        edit = Some(Edit::Service(choice));
    }
    let mut top_up = record.packing == Packing::TopUp;
    if ui
        .add(Checkbox::new(&mut top_up, "Top up the last contract"))
        .changed()
    {
        edit = Some(Edit::TopUp(top_up));
    }
    match chosen
        .and_then(|index| offers.get(index))
        .map(|offer| &offer.outcome)
    {
        Some(Ok(quote)) => {
            let count = quote.contracts.len();
            let text = if count == 1 {
                "1 contract".to_owned()
            } else {
                format!("{count} contracts")
            };
            ui.label(text).on_hover_ui(|ui| {
                for (index, contract) in quote.contracts.iter().enumerate() {
                    let minimum = if contract.pays_minimum {
                        " · pays the minimum"
                    } else {
                        ""
                    };
                    ui.label(format!(
                        "{}: {} m³ · {} collateral · {}{minimum}",
                        index + 1,
                        contract.volume.format_m3(),
                        contract.value.format_isk(),
                        contract.charge.format_isk(),
                    ));
                }
            });
        }
        Some(Err(reason)) => {
            ui.label(RichText::new(reason.as_str()).color(cx.style.neg));
        }
        None => {
            ui.label(RichText::new("no freight service can carry this").color(cx.style.neg));
        }
    }
    edit
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

/// The table row of one buy row: its numbers in the order of the columns.
fn table_row(row: &BuyRow, views: &BTreeMap<u64, View>) -> Row {
    let view = views.get(&u64::from(row.type_id));
    let fetched = view.is_some_and(|view| view.fetched);
    let price = match &row.price {
        LinePrice::Priced { per_unit, .. } => Cell::money(*per_unit),
        LinePrice::Thin {
            available,
            total_for_available,
            ..
        } => total_for_available
            .checked_div_qty(*available)
            .map_or_else(Cell::missing, Cell::money),
        LinePrice::Missing if fetched => Cell::missing(),
        LinePrice::Missing => Cell::text("…"),
    };
    let age = view
        .and_then(|view| view.age)
        .map_or_else(Cell::missing, |age| Cell::text(age_text(age)));
    let value = match (&row.value, &row.price) {
        (Some(value), _) => Cell::money(*value),
        (None, LinePrice::Thin { available, .. }) => {
            Cell::text(format!("only {available} on market"))
        }
        (None, _) if fetched => Cell::missing(),
        (None, _) => Cell::text("…"),
    };
    Row {
        key: u64::from(row.type_id),
        depth: 0,
        cells: vec![
            Cell::text(row.name.as_str()),
            count(row.needed),
            count(row.from_stock),
            count(row.to_buy),
            row.volume.map_or_else(Cell::missing, |volume| {
                Cell::int(i128::from(volume.ten_thousandths()), volume.format_m3())
            }),
            price,
            age,
            value,
        ],
    }
}

/// A count, grouped in thousands.
fn count(amount: u64) -> Cell {
    Cell::int(
        i128::from(amount),
        group_thousands(i64::try_from(amount).unwrap_or(i64::MAX)),
    )
}

fn columns() -> Vec<Column> {
    vec![
        column("item", "Item", ColumnKind::Text, 220.0),
        column("need", "Need", ColumnKind::Number, 80.0),
        column("own", "Own", ColumnKind::Number, 80.0),
        column("buy", "Buy", ColumnKind::Number, 80.0),
        column("m3", "m³", ColumnKind::Number, 90.0),
        column("price", "Price", ColumnKind::Money, 130.0),
        column("age", "Age", ColumnKind::Text, 60.0),
        column("value", "Value", ColumnKind::Money, 150.0),
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

/// The table's footer row, aligned with the columns: the volume and the value.
fn footer_cells(buy: &BuyList) -> Vec<Cell> {
    vec![
        Cell::text("Total"),
        Cell::text(""),
        Cell::text(""),
        Cell::text(""),
        Cell::int(
            i128::from(buy.volume.ten_thousandths()),
            buy.volume.format_m3(),
        ),
        Cell::text(""),
        Cell::text(""),
        Cell::money(buy.value),
    ]
}

/// The table of the buy rows, with the volume and value in a footer row.
fn table(ui: &mut Ui, cx: &Shared, buy: &BuyList, views: &BTreeMap<u64, View>) {
    let columns = columns();
    let rows: Vec<Row> = buy.rows.iter().map(|row| table_row(row, views)).collect();
    let footer = footer_cells(buy);
    let mut draw = |ui: &mut Ui, key: u64, column: &'static str| -> bool {
        let Some(view) = views.get(&key) else {
            return false;
        };
        match column {
            "price" => match view.missing_hover {
                Some(hover) if view.fetched => {
                    ui.label("—").on_hover_text(hover);
                    true
                }
                _ => false,
            },
            "age" => match view.age {
                Some(age) if age > STALE_SECS => {
                    ui.colored_label(cx.style.stale, age_text(age));
                    true
                }
                _ => false,
            },
            _ => false,
        }
    };
    Table::new(Id::new(super::BuildPanel::BuyList.title()), &columns)
        .fit_rows(rows.len())
        .show_with(ui, &cx.style, cx.density, &rows, Some(&footer), &mut draw);
}

/// The footer line, then the age of the oldest price among the lines to buy. The freight amount
/// lists every service on hover, and an incomplete total says why on hover.
fn footer_line(
    ui: &mut Ui,
    buy: &BuyList,
    delivered: &profit_core::buylist::Delivered,
    offers: &[Offer],
    record: &PlanRecord,
    cx: &Shared,
    now: i64,
) {
    ui.horizontal_wrapped(|ui| {
        ui.label(format!(
            "m³ {} · value {} · freight ",
            buy.volume.format_m3(),
            buy.value.format_isk()
        ));
        ui.label(delivered.freight.format_isk()).on_hover_ui(|ui| {
            if record.route.is_none() {
                ui.label("No freight");
                return;
            }
            for offer in offers {
                match &offer.outcome {
                    Ok(quote) => ui.label(format!("{} {}", offer.name, quote.total.format_isk())),
                    Err(reason) => ui.label(format!("{}: {reason}", offer.name)),
                };
            }
            if offers.is_empty() {
                ui.label("no freight service on this route");
            }
        });
        ui.label(format!(
            " · collateral {} · delivered {}",
            delivered.collateral.format_isk(),
            delivered.total.format_isk()
        ));
        if delivered.incomplete {
            let mut reasons: Vec<String> = buy
                .incomplete
                .iter()
                .map(|type_id| {
                    buy.rows
                        .iter()
                        .find(|row| row.type_id == *type_id)
                        .map_or_else(String::new, |row| row.name.clone())
                })
                .collect();
            if record.route.is_some() && offers.iter().all(|offer| offer.outcome.is_err()) {
                reasons.push("no freight service can carry this".to_owned());
            }
            ui.colored_label(cx.style.stale, " incomplete")
                .on_hover_text(reasons.join("\n"));
        }
    });
    if let Some(at) = buy.oldest_priced_at {
        let age = now.saturating_sub(at);
        let text = format!("Oldest: {}", age_text(age));
        if age > STALE_SECS {
            ui.colored_label(cx.style.stale, text);
        } else {
            ui.label(text);
        }
    }
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

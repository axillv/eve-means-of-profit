//! The `Buy what is short` window: the rows still short as a buy list, with its prices and freight.

use egui::{Button, ComboBox, Id, Ui};
use mop_core::buylist::{BuyInput, BuyList, LinePrice, buy_list};
use mop_core::freight::{FreightQuote, Packing, quote};
use mop_core::money::Money;
use mop_core::reserve::ShortMode;
use mop_core::stockpile::Status;
use mop_store::{RouteRow, ServiceRow};

use super::{Snapshot, claimed_lines, count_text, live_price, snapshot};
use crate::Shared;
use crate::build::state::{report, type_name};
use crate::status::Notice;
use crate::table::Table;
use crate::table::model::{Cell, Column, ColumnKind, Row};

/// The buy window's state: which stockpile it shows, and the freight picked in it.
#[derive(Debug, Default)]
pub(crate) struct State {
    open: Option<Open>,
    route: Option<i64>,
    service: Option<i64>,
}

#[derive(Debug, Clone)]
struct Open {
    id: i64,
    name: String,
}

impl State {
    pub(crate) fn open(&mut self, id: i64, name: String) {
        self.open = Some(Open { id, name });
        self.route = None;
        self.service = None;
    }

    pub(crate) fn show(&mut self, ui: &mut Ui, cx: &mut Shared) {
        let Some(open) = self.open.clone() else {
            return;
        };
        let snapshot = snapshot(cx);
        if !snapshot.all.contains_key(&open.id) {
            self.open = None;
            return;
        }
        let Some(buy) = buy_for(&snapshot, cx, open.id, snapshot.mode(open.id)) else {
            return;
        };
        let routes = match cx.store.routes() {
            Ok(routes) => routes,
            Err(error) => {
                report(cx, format!("Could not read the freight routes: {error}"));
                Vec::new()
            }
        };
        let services = match self.route {
            Some(route) => match cx.store.services(route) {
                Ok(services) => services,
                Err(error) => {
                    report(cx, format!("Could not read the freight services: {error}"));
                    Vec::new()
                }
            },
            None => Vec::new(),
        };
        let mut visible = true;
        let mut route = self.route;
        let mut service = self.service;
        egui::Window::new(format!("Buy what is short: {}", open.name))
            .open(&mut visible)
            .collapsible(false)
            .resizable(true)
            .show(ui.ctx(), |ui| {
                freight_choices(ui, &routes, &services, &mut route, &mut service);
                table(ui, cx, &buy);
                let delivered = delivered(&buy, &services, service, route);
                footer(ui, cx, &buy, &delivered);
                copy_button(ui, &buy, cx);
            });
        self.route = route;
        self.service = service;
        if !visible {
            self.open = None;
        }
    }

    pub(crate) fn close(&mut self) {
        self.open = None;
    }
}

/// Builds the window's buy list from the short rows of the stockpile.
fn buy_for(snapshot: &Snapshot, cx: &mut Shared, id: i64, mode: ShortMode) -> Option<BuyList> {
    let sde = cx.sde.as_ref()?;
    let lines = match claimed_lines(snapshot, id, mode, sde, &cx.session) {
        Ok(lines) => lines,
        Err(error) => {
            report(cx, format!("Could not read the stockpile: {error}"));
            return None;
        }
    };
    let held = cx.market.held();
    let rule = snapshot.price_rule;
    let mut short: Vec<(String, &mop_core::reserve::ClaimedLine)> = lines
        .iter()
        .filter(|line| line.short > 0)
        .map(|line| {
            (
                type_name(line.line.type_id, cx.sde.as_ref(), &mut cx.notices),
                line,
            )
        })
        .collect();
    short.sort_by_cached_key(|(name, line)| (status_rank(line.status), name.to_lowercase()));
    let mut inputs = Vec::new();
    for (name, line) in short {
        let unit_volume = cx
            .sde
            .as_ref()
            .and_then(|sde| sde.item(line.line.type_id).ok().flatten())
            .and_then(|item| item.haul_volume());
        let price = match live_price(line.line.type_id, rule, &held) {
            Some(quote) => match crate::build::prices::line_price(Some(&quote), None, line.short) {
                Ok(price) => price,
                Err(_) => LinePrice::Missing,
            },
            None => LinePrice::Missing,
        };
        inputs.push(BuyInput {
            type_id: line.line.type_id,
            name,
            needed: line.short,
            from_stock: 0,
            to_buy: line.short,
            unit_volume,
            price,
        });
    }
    match buy_list(inputs) {
        Ok(buy) => Some(buy),
        Err(_) => {
            report(cx, "Could not add up the buy list.");
            None
        }
    }
}

fn freight_choices(
    ui: &mut Ui,
    routes: &[RouteRow],
    services: &[ServiceRow],
    route: &mut Option<i64>,
    service: &mut Option<i64>,
) {
    ui.horizontal(|ui| {
        ui.label("Route");
        let selected = route
            .and_then(|id| routes.iter().find(|route| route.id == id))
            .map_or_else(|| "No freight".to_owned(), route_name);
        ComboBox::from_id_salt("stockpiles buy route")
            .selected_text(selected)
            .show_ui(ui, |ui| {
                if ui.selectable_label(route.is_none(), "No freight").clicked() {
                    *route = None;
                    *service = None;
                }
                for candidate in routes {
                    if ui
                        .selectable_label(*route == Some(candidate.id), route_name(candidate))
                        .clicked()
                    {
                        *route = Some(candidate.id);
                        *service = None;
                    }
                }
            });
        ui.add_enabled_ui(route.is_some(), |ui| {
            ui.label("Service");
            let selected = service
                .and_then(|id| services.iter().find(|service| service.id == id))
                .map_or_else(|| "No freight".to_owned(), |service| service.name.clone());
            ComboBox::from_id_salt("stockpiles buy service")
                .selected_text(selected)
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(service.is_none(), "No freight")
                        .clicked()
                    {
                        *service = None;
                    }
                    for candidate in services {
                        if ui
                            .selectable_label(
                                *service == Some(candidate.id),
                                candidate.name.as_str(),
                            )
                            .clicked()
                        {
                            *service = Some(candidate.id);
                        }
                    }
                });
        });
    });
}

fn route_name(route: &RouteRow) -> String {
    format!("{} → {}", route.from_system, route.to_system)
}

fn table(ui: &mut Ui, cx: &Shared, buy: &BuyList) {
    let columns = [
        column("item", "Item", ColumnKind::Text, 200.0),
        column("buy", "Buy", ColumnKind::Number, 80.0),
        column("volume", "m³", ColumnKind::Number, 90.0),
        column("price", "Price", ColumnKind::Money, 120.0),
        column("value", "Value", ColumnKind::Money, 130.0),
    ];
    let rows: Vec<Row> = buy
        .rows
        .iter()
        .map(|row| Row {
            key: u64::from(row.type_id),
            depth: 0,
            cells: vec![
                Cell::text(row.name.as_str()),
                Cell::int(i128::from(row.to_buy), count_text(row.to_buy)),
                row.volume.map_or_else(Cell::missing, |volume| {
                    Cell::int(i128::from(volume.ten_thousandths()), volume.format_m3())
                }),
                price_cell(&row.price),
                row.value.map_or_else(Cell::missing, Cell::money),
            ],
        })
        .collect();
    Table::new(Id::new("stockpiles buy"), &columns).show(ui, &cx.style, cx.density, &rows, None);
}

fn status_rank(status: Status) -> u8 {
    match status {
        Status::Short => 0,
        Status::Part => 1,
        Status::Ok => 2,
    }
}

fn price_cell(price: &LinePrice) -> Cell {
    match price {
        LinePrice::Priced { per_unit, .. } => Cell::money(*per_unit),
        LinePrice::Thin { .. } | LinePrice::Missing => Cell::missing(),
    }
}

fn column(id: &'static str, label: &str, kind: ColumnKind, width: f32) -> Column {
    Column {
        id,
        label: label.to_owned(),
        kind,
        width,
    }
}

fn delivered(
    buy: &BuyList,
    services: &[ServiceRow],
    service: Option<i64>,
    route: Option<i64>,
) -> Result<Option<mop_core::buylist::Delivered>, String> {
    let quote = match (route, service) {
        (Some(_), Some(service)) => services
            .iter()
            .find(|candidate| candidate.id == service)
            .map(|service| {
                let jumps = service.jumps_override.unwrap_or(0);
                let cargo = buy.cargo();
                quote(&service.card, jumps, &cargo, Packing::TopUp)
                    .map_err(|error| format!("Freight: {error}"))
            })
            .transpose()?,
        _ => Some(FreightQuote {
            contracts: Vec::new(),
            total: Money::ZERO,
        }),
    };
    let Some(quote) = quote else {
        return Ok(None);
    };
    let delivered = buy
        .delivered(Some(&quote))
        .map_err(|error| format!("Could not add up the delivered total: {error:?}"))?;
    Ok(Some(delivered))
}

fn footer(
    ui: &mut Ui,
    cx: &Shared,
    buy: &BuyList,
    delivered: &Result<Option<mop_core::buylist::Delivered>, String>,
) {
    ui.horizontal_wrapped(|ui| {
        ui.label(format!(
            "m³ {} · value {}",
            buy.volume.format_m3(),
            buy.value.format_isk()
        ));
        match delivered {
            Ok(Some(delivered)) => {
                ui.label(format!(
                    " · freight {} · collateral {} · delivered {}",
                    delivered.freight.format_isk(),
                    delivered.collateral.format_isk(),
                    delivered.total.format_isk()
                ));
                if delivered.incomplete {
                    ui.colored_label(cx.style.stale, " incomplete");
                }
            }
            Ok(None) => {
                ui.label(" · no freight service picked");
            }
            Err(error) => {
                ui.colored_label(cx.style.neg, error.as_str());
            }
        }
    });
}

fn copy_button(ui: &mut Ui, buy: &BuyList, cx: &mut Shared) {
    let lines = buy.rows.iter().filter(|row| row.to_buy > 0).count();
    let text = buy.multibuy_text();
    if ui
        .add_enabled(lines > 0, Button::new("Copy multibuy"))
        .on_hover_text("One line per item: name, tab, quantity. Paste into EVE's multibuy window.")
        .clicked()
    {
        ui.ctx().copy_text(text);
        cx.notices
            .push(Notice::info(format!("Copied {lines} lines for multibuy.")));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_price_is_a_dash() {
        assert_eq!(price_cell(&LinePrice::Missing), Cell::missing());
        assert_eq!(
            price_cell(&LinePrice::Priced {
                total: Money::from_ten_thousandths(100),
                per_unit: Money::from_ten_thousandths(10),
                priced_at: 0,
            }),
            Cell::money(Money::from_ten_thousandths(10))
        );
    }

    #[test]
    fn a_route_is_named_by_its_systems() {
        let route = RouteRow {
            id: 1,
            from_system: 30_000_142,
            to_system: 30_000_144,
        };
        assert_eq!(route_name(&route), "30000142 → 30000144");
    }
}

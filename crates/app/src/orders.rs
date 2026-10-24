//! The Orders page: every market order of the login characters, with how each stands against The
//! Forge's best price. The rows are in `rows.rs`; this file draws the table and its chips.

pub(crate) mod rows;

use egui::{Id, RichText, Ui};

use crate::Shared;
use crate::build::state::report;
use crate::table::model::{Cell, Column, ColumnKind, Row};
use crate::table::{Table, hidden_from_text};
use rows::{Entry, STALE_SECS, age_label, expires_text, grouped, utc_text};

/// The table's name for its saved hidden columns: `columns.orders`.
const TABLE: &str = "orders";
/// The setting that keeps the chips: the sides shown, as `sell,buy`.
const CHIPS: &str = "orders.chips";
const EMPTY: &str = "No market orders. Refresh on the Characters page to fetch them.";
/// The most characters of a location name the Location column shows; the full name is on hover.
const LOCATION_CHARS: usize = 24;

#[derive(Debug, Default)]
pub struct OrdersPage;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrdersPanel {
    Orders,
}

impl OrdersPanel {
    pub fn title(self) -> &'static str {
        match self {
            Self::Orders => "Orders",
        }
    }
}

/// Which sides the table shows. Both are on until the player turns one off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Chips {
    sell: bool,
    buy: bool,
}

impl Chips {
    fn parse(text: &str) -> Self {
        Self {
            sell: text.split(',').any(|part| part == "sell"),
            buy: text.split(',').any(|part| part == "buy"),
        }
    }

    fn text(self) -> String {
        match (self.sell, self.buy) {
            (true, true) => "sell,buy".to_owned(),
            (true, false) => "sell".to_owned(),
            (false, true) => "buy".to_owned(),
            (false, false) => String::new(),
        }
    }
}

/// Draws the body of the panel of this page.
pub fn show(_page: &mut OrdersPage, _panel: OrdersPanel, ui: &mut Ui, cx: &mut Shared) {
    let now = (cx.clock)();
    let all = rows::entries(cx, now);
    if all.is_empty() {
        ui.label(RichText::new(EMPTY).color(cx.style.mute));
        return;
    }
    let chips = chip_row(ui, cx);
    let shown: Vec<&Entry> = all
        .iter()
        .filter(|entry| if entry.sell { chips.sell } else { chips.buy })
        .collect();
    ui.label(RichText::new(rows::footer(&shown)).color(cx.style.mute));

    let mut hidden = hidden_columns(ui, cx);
    let all_columns = columns();
    if column_menu(ui, &all_columns, &mut hidden) {
        ui.data_mut(|data| data.insert_temp(hidden_key(), hidden.clone()));
        cx.save_hidden_columns(TABLE, &hidden);
    }
    let columns: Vec<Column> = all_columns
        .into_iter()
        .filter(|column| !hidden.iter().any(|id| id == column.id))
        .collect();
    let table_rows: Vec<Row> = shown
        .iter()
        .enumerate()
        .map(|(index, entry)| Row {
            key: u64::try_from(index).unwrap_or(u64::MAX),
            depth: 0,
            cells: cells(entry),
        })
        .collect();
    let mut draw = |ui: &mut Ui, key: u64, column: &'static str| -> bool {
        let Some(entry) = usize::try_from(key).ok().and_then(|key| shown.get(key)) else {
            return false;
        };
        match (column, entry.beaten_by, entry.forge_best) {
            ("location", _, _) => {
                ui.label(rows::short(&entry.location, LOCATION_CHARS))
                    .on_hover_text(&entry.location);
                true
            }
            ("forge", Some(gap), Some(best)) => {
                ui.colored_label(cx.style.stale, best.format_isk())
                    .on_hover_text(format!("Beaten by {}", gap.format_isk()));
                true
            }
            ("age", _, _) => match entry.age {
                Some(age) if age > STALE_SECS => {
                    ui.colored_label(cx.style.stale, age_label(Some(age)));
                    true
                }
                _ => false,
            },
            _ => false,
        }
    };
    let response = Table::new(Id::new("orders"), &columns).show_with(
        ui,
        &cx.style,
        cx.density,
        &table_rows,
        None,
        &mut draw,
    );
    if let Some(key) = response.clicked {
        let picked = usize::try_from(key)
            .ok()
            .and_then(|key| shown.get(key))
            .map(|entry| entry.type_id);
        if let Some(type_id) = picked {
            cx.selection.item = Some(type_id);
        }
    }
}

/// The Sell and Buy chips: which sides are shown. A change is saved to the settings.
fn chip_row(ui: &mut Ui, cx: &mut Shared) -> Chips {
    let chips = chips(ui, cx);
    let mut sell = chips.sell;
    let mut buy = chips.buy;
    ui.horizontal(|ui| {
        let mut changed = ui.toggle_value(&mut sell, "Sell").changed();
        changed |= ui.toggle_value(&mut buy, "Buy").changed();
        if changed {
            let next = Chips { sell, buy };
            ui.data_mut(|data| data.insert_temp(chips_key(), next));
            if let Err(error) = cx.store.set_setting(CHIPS, &next.text()) {
                report(cx, format!("Could not save the settings: {error}"));
            }
        }
    });
    Chips { sell, buy }
}

/// The columns the table can have. Every one of them can be hidden from the Columns menu.
fn columns() -> Vec<Column> {
    vec![
        column("side", "Side", ColumnKind::Text, 70.0),
        column("item", "Item", ColumnKind::Text, 190.0),
        column("character", "Character", ColumnKind::Text, 110.0),
        column("location", "Location", ColumnKind::Text, 170.0),
        column("price", "Price", ColumnKind::Money, 110.0),
        column("left", "Left", ColumnKind::Number, 80.0),
        column("issued", "Issued", ColumnKind::Text, 120.0),
        column("expires", "Expires", ColumnKind::Number, 90.0),
        column("forge", "Forge best", ColumnKind::Money, 100.0),
        column("age", "Age", ColumnKind::Number, 60.0),
    ]
}

fn cells(entry: &Entry) -> Vec<Cell> {
    let side = match (entry.sell, entry.corporation) {
        (true, false) => "Sell",
        (true, true) => "Sell (corp)",
        (false, false) => "Buy",
        (false, true) => "Buy (corp)",
    };
    let left = format!("{} / {}", grouped(entry.left), grouped(entry.total));
    vec![
        Cell::text(side),
        Cell::text(&entry.item),
        Cell::text(&entry.character),
        Cell::text(&entry.location),
        Cell::money(entry.price),
        Cell::int(i128::from(entry.left), left),
        issued_cell(entry),
        expires_cell(entry),
        match entry.forge_best {
            Some(best) => Cell::money(best),
            None => Cell::missing(),
        },
        match entry.age {
            Some(age) => Cell::int(i128::from(age), age_label(Some(age))),
            None => Cell::missing(),
        },
    ]
}

fn issued_cell(entry: &Entry) -> Cell {
    Cell::int(i128::from(entry.issued), utc_text(entry.issued))
}

fn expires_cell(entry: &Entry) -> Cell {
    match entry.expires_in {
        Some(seconds) => Cell::int(i128::from(seconds), expires_text(Some(seconds))),
        None => Cell::missing(),
    }
}

/// The chips of this session: the saved setting on the first frame, then the frame memory.
fn chips(ui: &Ui, cx: &mut Shared) -> Chips {
    if let Some(chips) = ui.data(|data| data.get_temp::<Chips>(chips_key())) {
        return chips;
    }
    let chips = match cx.store.setting(CHIPS) {
        Ok(Some(text)) => Chips::parse(&text),
        Ok(None) => Chips {
            sell: true,
            buy: true,
        },
        Err(error) => {
            report(cx, format!("Could not read the settings: {error}"));
            Chips {
                sell: true,
                buy: true,
            }
        }
    };
    ui.data_mut(|data| data.insert_temp(chips_key(), chips));
    chips
}

fn chips_key() -> Id {
    Id::new("orders chips")
}

/// The hidden columns of this session: the saved setting on the first frame, then the frame memory.
fn hidden_columns(ui: &Ui, cx: &mut Shared) -> Vec<String> {
    if let Some(hidden) = ui.data(|data| data.get_temp::<Vec<String>>(hidden_key())) {
        return hidden;
    }
    let hidden = match cx.store.setting(&format!("columns.{TABLE}")) {
        Ok(Some(text)) => hidden_from_text(&text),
        Ok(None) => Vec::new(),
        Err(error) => {
            report(cx, format!("Could not read the settings: {error}"));
            Vec::new()
        }
    };
    ui.data_mut(|data| data.insert_temp(hidden_key(), hidden.clone()));
    hidden
}

fn hidden_key() -> Id {
    Id::new("orders hidden columns")
}

/// The Columns menu: a tick per column. Returns whether a tick changed.
fn column_menu(ui: &mut Ui, all: &[Column], hidden: &mut Vec<String>) -> bool {
    let mut changed = false;
    ui.menu_button("Columns", |ui| {
        for column in all {
            let mut shown = !hidden.iter().any(|id| id == column.id);
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

fn column(id: &'static str, label: &str, kind: ColumnKind, width: f32) -> Column {
    Column {
        id,
        label: label.to_owned(),
        kind,
        width,
    }
}

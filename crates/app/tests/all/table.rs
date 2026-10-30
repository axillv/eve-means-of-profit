#![cfg(test)]

//! Acceptance tests for the table: the sort, the rows in view, the folds, the copied text, and the
//! widget on screen. Row keys in the test table are 1 to 7, in the order of the rows.

use std::rc::Rc;
use std::sync::{Arc, Mutex};

use egui::{
    Context, Event, Id, Modifiers, MouseWheelUnit, OutputCommand, PointerButton, Pos2, TextStyle,
    TouchPhase, Ui, Vec2, pos2, vec2,
};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use eve_means_of_profit::dock::show_locked;
use eve_means_of_profit::images::{self, Fetch, Picture};
use eve_means_of_profit::table::model::{
    Cell, Column, ColumnKind, Direction, Row, row_height, row_range, sort_order, span, to_tsv,
    visible,
};
use eve_means_of_profit::table::{
    Level, Table, TableResponse, level as table_level, sort_from_text, sort_mark_points,
    sort_to_text,
};
use eve_means_of_profit::theme::Style;
use mop_core::money::Money;
use mop_store::Density;
use std::collections::BTreeSet;
use tempfile::tempdir;

mod draw;
mod model;
mod panels;
mod saved_sort;
mod snapshots;
mod tree;

fn columns() -> Vec<Column> {
    vec![
        Column {
            id: "item",
            label: "Item".to_owned(),
            kind: ColumnKind::Text,
            width: 220.0,
        },
        Column {
            id: "need",
            label: "Need".to_owned(),
            kind: ColumnKind::Number,
            width: 80.0,
        },
        Column {
            id: "value",
            label: "Value".to_owned(),
            kind: ColumnKind::Money,
            width: 140.0,
        },
    ]
}

/// A row with a missing value when `value` is `None`; amounts are in ten-thousandths of ISK.
fn row(key: u64, depth: u32, item: &str, need: i128, value: Option<i64>) -> Row {
    Row {
        key,
        depth,
        cells: vec![
            Cell::text(item),
            Cell::int(need, need.to_string()),
            value.map_or_else(Cell::missing, |amount| {
                Cell::money(Money::from_ten_thousandths(amount))
            }),
        ],
    }
}

/// The seven rows in tree order, with the keys 1 to 7.
fn test_rows() -> Vec<Row> {
    vec![
        row(1, 0, "Hobgoblin II", 10, Some(10_000_000)),
        row(2, 1, "Particle Accelerator Unit", 10, Some(3_090_000_000)),
        row(3, 2, "Crystalline Carbonide", 166, None),
        row(4, 2, "Fullerides", 59, Some(1_500_000)),
        row(5, 1, "Hobgoblin I", 10, Some(2_750_000_000)),
        row(6, 1, "Morphite", 10, None),
        row(7, 0, "Rifter", 31, Some(21_250)),
    ]
}

fn test_footer() -> Vec<Cell> {
    vec![
        Cell::text("Total"),
        Cell::missing(),
        Cell::money(Money::from_ten_thousandths(5_851_521_250)),
    ]
}

/// A primary click at `pos`: the pointer moves there, then presses and releases.
fn click_at(harness: &Harness<'_>, pos: Pos2) {
    harness.hover_at(pos);
    for pressed in [true, false] {
        harness.event(Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        });
    }
}

/// What the table showed and reported, frame by frame.
#[derive(Default)]
struct Seen {
    /// The keys whose Item cell was drawn in the last frame, in the order they were drawn.
    order: Vec<u64>,
    /// How many rows were drawn in each frame.
    drawn: Vec<usize>,
    responses: Vec<TableResponse>,
}

/// A harness with the table in a pane of `size`. `checkbox_key` draws a checkbox in the Need
/// column of that row.
fn table_harness(
    rows: Vec<Row>,
    footer: Option<Vec<Cell>>,
    density: Density,
    size: Vec2,
    checkbox_key: Option<u64>,
    seen: &Arc<Mutex<Seen>>,
) -> Harness<'static> {
    let seen = Arc::clone(seen);
    let columns = columns();
    let style = Style::eve_dense();
    Harness::builder()
        .with_size(size)
        .build_ui(move |ui: &mut Ui| {
            style.apply(ui.ctx());
            let mut order = Vec::new();
            let mut checked = false;
            let response = {
                let mut draw = |ui: &mut Ui, key: u64, column: &'static str| -> bool {
                    if column == "item" {
                        order.push(key);
                    }
                    if checkbox_key == Some(key) && column == "need" {
                        ui.checkbox(&mut checked, "Pick");
                        return true;
                    }
                    false
                };
                Table::new(Id::new("table test"), &columns).show_with(
                    ui,
                    &style,
                    density,
                    &rows,
                    footer.as_deref(),
                    &mut draw,
                )
            };
            if let Ok(mut seen) = seen.lock() {
                seen.drawn.push(order.len());
                seen.order = order;
                seen.responses.push(response);
            }
        })
}

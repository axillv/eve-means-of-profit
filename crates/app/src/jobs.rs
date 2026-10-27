//! The Industry jobs page: every job of the login characters, with what it makes and when it ends.
//! The rows are in `rows.rs`; this file draws the table and its chip.

pub(crate) mod rows;

use egui::{Id, RichText, Ui};

use crate::Shared;
use crate::build::state::report;
use crate::orders::rows::grouped;
use crate::table::model::{Cell, Column, ColumnKind, Row};
use crate::table::{Table, hidden_from_text};
use rows::{Entry, footer, status_text};

/// The table's name for its saved hidden columns: `columns.jobs`.
const TABLE: &str = "jobs";
/// The setting that keeps the chip: `finished` when finished jobs are shown, else empty.
const CHIPS: &str = "jobs.chips";
const EMPTY: &str = "No industry jobs. Refresh on the Characters page to fetch them.";

#[derive(Debug, Default)]
pub struct JobsPage;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobsPanel {
    IndustryJobs,
}

impl JobsPanel {
    pub fn title(self) -> &'static str {
        match self {
            Self::IndustryJobs => "Industry jobs",
        }
    }
}

/// Draws the body of the panel of this page.
pub fn show(_page: &mut JobsPage, panel: JobsPanel, ui: &mut Ui, cx: &mut Shared) {
    let now = (cx.clock)();
    let all = rows::entries(cx, now);
    if all.is_empty() {
        ui.label(RichText::new(EMPTY).color(cx.style.mute));
        return;
    }
    let finished_shown = chip(ui, cx);
    let mut shown_flag = finished_shown;
    ui.horizontal(|ui| {
        if ui.toggle_value(&mut shown_flag, "Show finished").changed() {
            ui.data_mut(|data| data.insert_temp(chip_key(), shown_flag));
            let text = if shown_flag { "finished" } else { "" };
            if let Err(error) = cx.store.set_setting(CHIPS, text) {
                report(cx, format!("Could not save the settings: {error}"));
            }
        }
    });
    let shown: Vec<&Entry> = all
        .iter()
        .filter(|entry| shown_flag || !entry.finished)
        .collect();
    ui.label(RichText::new(footer(&shown)).color(cx.style.mute));

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
    let response = Table::new(Id::new(panel.title()), &columns).show(
        ui,
        &cx.style,
        cx.density,
        &table_rows,
        None,
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

/// The columns the table can have. Every one of them can be hidden from the Columns menu.
fn columns() -> Vec<Column> {
    vec![
        column("activity", "Activity", ColumnKind::Text, 150.0),
        column("product", "Product", ColumnKind::Text, 220.0),
        column("runs", "Runs", ColumnKind::Number, 70.0),
        column("character", "Character", ColumnKind::Text, 120.0),
        column("location", "Location", ColumnKind::Text, 200.0),
        column("status", "Status", ColumnKind::Text, 100.0),
        column("ends", "Ends", ColumnKind::Number, 140.0),
    ]
}

fn cells(entry: &Entry) -> Vec<Cell> {
    vec![
        Cell::text(&entry.activity),
        Cell::text(&entry.product),
        Cell::int(i128::from(entry.runs), grouped(u64::from(entry.runs))),
        Cell::text(&entry.character),
        Cell::text(&entry.location),
        Cell::text(status_text(entry.status)),
        Cell::int(i128::from(entry.end), entry.ends.as_str()),
    ]
}

/// Whether the finished jobs are shown: the saved setting on the first frame, then the frame memory.
fn chip(ui: &Ui, cx: &mut Shared) -> bool {
    if let Some(shown) = ui.data(|data| data.get_temp::<bool>(chip_key())) {
        return shown;
    }
    let shown = match cx.store.setting(CHIPS) {
        Ok(text) => text.as_deref() == Some("finished"),
        Err(error) => {
            report(cx, format!("Could not read the settings: {error}"));
            false
        }
    };
    ui.data_mut(|data| data.insert_temp(chip_key(), shown));
    shown
}

fn chip_key() -> Id {
    Id::new("jobs chip")
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
    Id::new("jobs hidden columns")
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

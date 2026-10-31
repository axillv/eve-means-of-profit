//! The Industry jobs page: every job of the login characters, with what it makes and when it ends.
//! The rows are in `rows.rs`; this file draws the table and its chip.

pub(crate) mod rows;

use egui::{Button, ComboBox, Id, RichText, Ui};

use crate::Shared;
use crate::build::state::report;
use crate::characters::short_names;
use crate::empty::{self, Fill};
use crate::orders::rows::grouped;
use crate::rail::Page;
use crate::table::model::{Cell, Column, ColumnKind, Row};
use crate::table::{self, Level, Table, hidden_from_text};
use rows::{Entry, PlanCell, footer, status_text};

/// The table's name for its saved hidden columns: `columns.jobs`.
const TABLE: &str = "jobs";
/// The setting that keeps the chip: `finished` when finished jobs are shown, else empty.
const CHIPS: &str = "jobs.chips";
/// The columns Summary shows, in their order: the five a builder checks.
const SUMMARY: [&str; 5] = ["product", "activity", "runs", "character", "ends"];

/// Why the page has no rows and what fills it: no characters, no fetched jobs, or fetched jobs
/// that hold none.
fn blank(cx: &Shared) -> (&'static str, &'static str, Option<Fill>) {
    let characters = cx.session.characters();
    if characters.is_empty() {
        return (
            "No characters yet.",
            "Jobs are fetched from EVE for each character you add.",
            Some(Fill {
                label: "Add character",
                page: Some(Page::Characters),
            }),
        );
    }
    let fetched = characters.iter().any(|character| {
        cx.session
            .character_data(character.id)
            .and_then(|data| data.jobs.as_ref())
            .is_some()
    });
    if !fetched {
        return (
            "No industry jobs.",
            "Refresh a character on the Characters page to fetch them.",
            Some(Fill {
                label: "Refresh a character",
                page: Some(Page::Characters),
            }),
        );
    }
    (
        "No industry jobs.",
        "You have none running right now. Start one in EVE, then refresh.",
        None,
    )
}

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
        let (why, what, fill) = blank(cx);
        empty::show(ui, &cx.style, why, what, fill);
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
    ui.label(RichText::new(footer(&shown)).size(16.0));

    let id = Id::new(panel.title());
    let mut level = table::level(ui, id, Level::Summary);
    let all_columns = columns();
    let mut hidden = hidden_columns(ui, cx);
    let (hidden_changed, picked) = column_menu(ui, level, &all_columns, &mut hidden);
    if let Some(picked) = picked {
        level = picked;
        table::set_level(ui, id, picked);
    }
    if hidden_changed {
        ui.data_mut(|data| data.insert_temp(hidden_key(), hidden.clone()));
        cx.save_hidden_columns(TABLE, &hidden);
    }
    let columns = columns_for(level, &all_columns, &hidden);
    let table_rows: Vec<Row> = shown
        .iter()
        .enumerate()
        .map(|(index, entry)| Row {
            key: u64::try_from(index).unwrap_or(u64::MAX),
            depth: 0,
            cells: cells(entry, &columns),
        })
        .collect();
    let style = cx.style;
    let density = cx.density;
    let mut draw = |ui: &mut Ui, key: u64, column: &'static str| -> bool {
        if column != "plan" && column != "character" {
            return false;
        }
        let Some(entry) = usize::try_from(key).ok().and_then(|key| shown.get(key)) else {
            return false;
        };
        match column {
            "character" => short_names::label(ui, cx, entry.character.as_str()),
            _ => draw_plan(ui, entry, cx),
        }
    };
    let response = Table::new(id, &columns)
        .level(level)
        .icons("product", &|key| {
            usize::try_from(key)
                .ok()
                .and_then(|index| shown.get(index))
                .map(|entry| entry.type_id)
        })
        .show_with(ui, &style, density, &table_rows, None, &mut draw);
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
        column("product", "Product", ColumnKind::Text, 220.0),
        column("activity", "Activity", ColumnKind::Text, 150.0),
        column("plan", "Plan", ColumnKind::Text, 150.0),
        column("runs", "Runs", ColumnKind::Number, 70.0),
        column("character", "Character", ColumnKind::Text, 120.0),
        column("location", "Location", ColumnKind::Text, 180.0),
        column("ends", "Ends", ColumnKind::Number, 140.0),
    ]
}

/// The columns to draw at `level`, after `hidden` is removed: the five Summary columns in their
/// order, else the full set in `all`'s order.
fn columns_for(level: Level, all: &[Column], hidden: &[String]) -> Vec<Column> {
    let shown = |column: &&Column| !hidden.iter().any(|id| id == column.id);
    match level {
        Level::Summary => SUMMARY
            .iter()
            .filter_map(|id| all.iter().find(|column| column.id == *id))
            .filter(shown)
            .cloned()
            .collect(),
        Level::Standard | Level::Technical => all.iter().filter(shown).cloned().collect(),
    }
}

/// The cells of one job in `columns` order: a lookup by column id.
fn cells(entry: &Entry, columns: &[Column]) -> Vec<Cell> {
    let plan = match &entry.plan {
        PlanCell::Assigned { name, .. } => name.clone(),
        PlanCell::None | PlanCell::Suggest(_) => String::new(),
    };
    columns
        .iter()
        .filter_map(|column| match column.id {
            "product" => Some(Cell::text(&entry.product)),
            "activity" => Some(Cell::text(&entry.activity)),
            "plan" => Some(Cell::text(plan.clone())),
            "runs" => Some(Cell::int(
                i128::from(entry.runs),
                grouped(u64::from(entry.runs)),
            )),
            "character" => Some(Cell::text(&entry.character)),
            "location" => Some(Cell::text(&entry.location)),
            "ends" => Some(Cell::int(
                i128::from(entry.end),
                status_and_ends(entry).as_str(),
            )),
            _ => None,
        })
        .collect()
}

/// The Ends cell of a job: its status and end date once it is over, else when it ends.
fn status_and_ends(entry: &Entry) -> String {
    if entry.finished {
        format!("{} {}", status_text(entry.status), entry.ends)
    } else {
        entry.ends.clone()
    }
}

/// Draws the Plan cell of one row. Returns whether it drew the cell.
fn draw_plan(ui: &mut Ui, entry: &Entry, cx: &mut Shared) -> bool {
    match &entry.plan {
        PlanCell::None => false,
        PlanCell::Assigned { name, .. } => {
            ui.label(name.as_str());
            if ui
                .add(Button::new("×").small())
                .on_hover_text("Take the job off its plan")
                .clicked()
            {
                unassign(entry.job_id, cx);
            }
            true
        }
        PlanCell::Suggest(plans) if plans.is_empty() => {
            ui.label(RichText::new("no plan builds this").color(cx.style.mute));
            true
        }
        PlanCell::Suggest(plans) => {
            let mut picked = None;
            ComboBox::from_id_salt(("jobs plan", entry.job_id))
                .selected_text("Assign…")
                .show_ui(ui, |ui| {
                    for (plan_id, name) in plans {
                        if ui.selectable_label(false, name.as_str()).clicked() {
                            picked = Some(*plan_id);
                        }
                    }
                });
            if let Some(plan_id) = picked {
                assign(entry.job_id, plan_id, cx);
            }
            true
        }
    }
}

/// Assigns a job to a plan, then asks for the claims to be worked out again.
fn assign(job_id: i64, plan_id: i64, cx: &mut Shared) {
    let now = (cx.clock)();
    match cx.store.assign_job(job_id, plan_id, now) {
        Ok(()) => cx.claims.mark_stale(),
        Err(error) => report(cx, format!("Could not assign the job: {error}")),
    }
}

/// Takes a job off its plan, then asks for the claims to be worked out again.
fn unassign(job_id: i64, cx: &mut Shared) {
    match cx.store.unassign_job(job_id) {
        Ok(()) => cx.claims.mark_stale(),
        Err(error) => report(cx, format!("Could not take the job off its plan: {error}")),
    }
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

/// The Columns menu: the levels, then a tick per column. Returns whether a tick changed and the
/// level picked.
fn column_menu(
    ui: &mut Ui,
    level: Level,
    all: &[Column],
    hidden: &mut Vec<String>,
) -> (bool, Option<Level>) {
    let mut changed = false;
    let mut picked = None;
    ui.menu_button("Columns", |ui| {
        picked = table::level_choices(ui, level);
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
    (changed, picked)
}

fn column(id: &'static str, label: &str, kind: ColumnKind, width: f32) -> Column {
    Column {
        id,
        label: label.to_owned(),
        kind,
        width,
    }
}

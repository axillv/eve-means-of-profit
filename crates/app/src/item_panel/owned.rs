//! The Owned table and the Running jobs table of the Item panel: who holds the selected item and
//! where, and what the running jobs will deliver. Neither changes the stock.

use std::collections::BTreeMap;

use egui::{Id, RichText, Ui};
use mop_core::industry::TypeId;
use mop_core::stock::{CharacterId, LocationId, resolve};
use mop_esi::character_data::JobStatus;

use super::grouped;
use crate::Shared;
use crate::build::prices::age_text;
use crate::build::state::report;
use crate::session::Source;
use crate::table::model::{Cell, Column, ColumnKind, Row};
use crate::table::{Table, hidden_from_text};

/// The table's name for its saved hidden columns: `columns.item_owned`.
const TABLE: &str = "item_owned";
/// The column the player can hide.
const COUNTED: &str = "counted";

/// One row of the Owned table: a character's units of the item in one place.
struct Holding {
    owner: String,
    place: String,
    units: u64,
    counted: u64,
}

/// One row of the Running jobs table.
struct Running {
    owner: String,
    place: String,
    units: u64,
    end: i64,
}

pub(super) fn show(ui: &mut Ui, cx: &mut Shared, type_id: TypeId) {
    owned(ui, cx, type_id);
    jobs(ui, cx, type_id);
}

fn owned(ui: &mut Ui, cx: &mut Shared, type_id: TypeId) {
    ui.label(RichText::new("Owned").color(cx.style.mute));
    let Some(mut holdings) = holdings(cx, type_id) else {
        ui.label(RichText::new("Could not add up the stock.").color(cx.style.neg));
        return;
    };
    if holdings.is_empty() {
        ui.label(RichText::new("None owned.").color(cx.style.mute));
        return;
    }
    holdings.sort_by(|left, right| {
        right
            .units
            .cmp(&left.units)
            .then_with(|| left.owner.cmp(&right.owner))
            .then_with(|| left.place.cmp(&right.place))
    });

    let mut hidden = hidden_columns(ui, cx);
    ui.horizontal(|ui| {
        if column_menu(ui, &mut hidden) {
            hidden.retain(|id| id == COUNTED);
            ui.data_mut(|data| data.insert_temp(hidden_key(), hidden.clone()));
            cx.save_hidden_columns(TABLE, &hidden);
        }
    });
    let counted_shown = !hidden.iter().any(|id| id == COUNTED);

    let mut columns = vec![
        column("owner", "Character", ColumnKind::Text, 240.0),
        column("place", "Place", ColumnKind::Text, 260.0),
        column("units", "Units", ColumnKind::Number, 90.0),
    ];
    if counted_shown {
        columns.push(column("counted", "Counted", ColumnKind::Number, 90.0));
    }
    let rows: Vec<Row> = holdings
        .iter()
        .enumerate()
        .map(|(index, holding)| {
            let mut cells = vec![
                Cell::text(&holding.owner),
                Cell::text(&holding.place),
                Cell::int(i128::from(holding.units), grouped(holding.units)),
            ];
            if counted_shown {
                cells.push(Cell::int(
                    i128::from(holding.counted),
                    grouped(holding.counted),
                ));
            }
            Row {
                key: u64::try_from(index).unwrap_or(u64::MAX),
                depth: 0,
                cells,
            }
        })
        .collect();
    let Some((units, counted)) = totals(&holdings) else {
        ui.label(RichText::new("Could not add up the stock.").color(cx.style.neg));
        return;
    };
    let mut footer = vec![
        Cell::text("Total"),
        Cell::text(""),
        Cell::int(i128::from(units), grouped(units)),
    ];
    if counted_shown {
        footer.push(Cell::int(i128::from(counted), grouped(counted)));
    }
    Table::new(Id::new("item panel owned"), &columns).show(
        ui,
        &cx.style,
        cx.density,
        &rows,
        Some(&footer),
    );
}

/// The units and the counted units over every holding, or `None` when the sum does not fit.
fn totals(holdings: &[Holding]) -> Option<(u64, u64)> {
    holdings
        .iter()
        .try_fold((0u64, 0u64), |(units, counted), holding| {
            Some((
                units.checked_add(holding.units)?,
                counted.checked_add(holding.counted)?,
            ))
        })
}

/// The holdings of the item: one per character and place, from the stock the pages count.
fn holdings(cx: &Shared, type_id: TypeId) -> Option<Vec<Holding>> {
    let sde = cx.sde.as_ref();
    let mut sums: BTreeMap<(CharacterId, LocationId), (u64, u64)> = BTreeMap::new();
    for row in resolve(&cx.session.stock_characters()).assets {
        if row.asset.type_id != type_id || row.asset.quantity == 0 {
            continue;
        }
        let sum = sums.entry((row.character_id, row.place)).or_insert((0, 0));
        sum.0 = sum.0.checked_add(row.asset.quantity)?;
        if row.counts_as_stock {
            sum.1 = sum.1.checked_add(row.asset.quantity)?;
        }
    }
    Some(
        sums.into_iter()
            .map(|((character, place), (units, counted))| Holding {
                owner: owner_name(cx, character),
                place: cx.session.place_label(place, sde),
                units,
                counted,
            })
            .collect(),
    )
}

/// The login's name for a login character. Otherwise jEve's name with ` (jEve)` after it.
fn owner_name(cx: &Shared, character: CharacterId) -> String {
    if let Ok(id) = u64::try_from(character)
        && matches!(cx.session.source(id), Some(Source::Login))
        && let Some(login) = cx.session.characters().iter().find(|saved| saved.id == id)
    {
        return login.name.clone();
    }
    let jeve = cx.session.jeve().and_then(|jeve| {
        jeve.characters
            .iter()
            .find(|held| held.character_id == character)
    });
    match jeve {
        Some(held) => format!("{} (jEve)", held.name),
        None => character.to_string(),
    }
}

/// The running jobs of every login character that make the item: status Active, Paused or Ready.
/// Hidden when there are none.
fn jobs(ui: &mut Ui, cx: &mut Shared, type_id: TypeId) {
    let Some(jobs) = running(cx, type_id) else {
        ui.label(RichText::new("Could not add up the running jobs.").color(cx.style.neg));
        return;
    };
    if jobs.is_empty() {
        return;
    }
    let now = (cx.clock)();
    ui.label(RichText::new("Running jobs").color(cx.style.mute));
    let columns = [
        column("owner", "Character", ColumnKind::Text, 240.0),
        column("place", "Place", ColumnKind::Text, 260.0),
        column("units", "Units", ColumnKind::Number, 90.0),
        column("ends", "Ends", ColumnKind::Text, 110.0),
    ];
    let rows: Vec<Row> = jobs
        .iter()
        .enumerate()
        .map(|(index, job)| Row {
            key: u64::try_from(index).unwrap_or(u64::MAX),
            depth: 0,
            cells: vec![
                Cell::text(&job.owner),
                Cell::text(&job.place),
                Cell::int(i128::from(job.units), grouped(job.units)),
                Cell::text(ends_text(job.end, now)),
            ],
        })
        .collect();
    Table::new(Id::new("item panel jobs"), &columns).show(ui, &cx.style, cx.density, &rows, None);
}

/// When a job ends: `in <age>`, or `ready` once the end time has passed.
fn ends_text(end: i64, now: i64) -> String {
    match end.checked_sub(now) {
        Some(left) if left > 0 => format!("in {}", age_text(left)),
        _ => "ready".to_owned(),
    }
}

/// The running jobs for the item, or `None` when the recipe or a units count does not work out.
fn running(cx: &Shared, type_id: TypeId) -> Option<Vec<Running>> {
    let mut found = Vec::new();
    for character in cx.session.characters() {
        let Some(fetched) = cx
            .session
            .character_data(character.id)
            .and_then(|data| data.jobs.as_ref())
        else {
            continue;
        };
        for job in &fetched.data {
            let running = matches!(
                job.status,
                JobStatus::Active | JobStatus::Paused | JobStatus::Ready
            );
            if running && job.product_type_id == Some(type_id) {
                found.push((character.name.as_str(), job));
            }
        }
    }
    if found.is_empty() {
        return Some(Vec::new());
    }
    let sde = cx.sde.as_ref()?;
    let output = sde.recipe_book().ok()?.get(type_id)?.output_per_run;
    found
        .into_iter()
        .map(|(owner, job)| {
            Some(Running {
                owner: owner.to_owned(),
                place: cx.session.place_label(job.facility_id, Some(sde)),
                units: u64::from(job.runs).checked_mul(output)?,
                end: job.end,
            })
        })
        .collect()
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
    Id::new("item panel owned hidden columns")
}

/// The Columns menu: a tick per optional column. Returns whether a tick changed.
fn column_menu(ui: &mut Ui, hidden: &mut Vec<String>) -> bool {
    let mut changed = false;
    ui.menu_button("Columns", |ui| {
        let mut shown = !hidden.iter().any(|id| id == COUNTED);
        if ui.checkbox(&mut shown, "Counted").changed() {
            changed = true;
            if shown {
                hidden.retain(|id| id != COUNTED);
            } else {
                hidden.push(COUNTED.to_owned());
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

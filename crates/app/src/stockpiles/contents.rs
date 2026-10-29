//! The Contents panel: one stockpile's targets, what is held and claimed, and what is short.

use egui::{ComboBox, Id, RichText, Ui};
use mop_core::industry::TypeId;
use mop_core::reserve::{ClaimedLine, ShortMode};
use mop_core::stockpile::Status;

use super::{
    Snapshot, StockpilesPage, StockpilesPanel, build_short, claimed_lines, count_text, short_total,
    short_value, snapshot,
};
use crate::Shared;
use crate::build::prices::{Held, age_text};
use crate::build::state::{report, type_name};
use crate::table::Table;
use crate::table::model::{Cell, Column, ColumnKind, Row};

const NO_FILTERS: &str = "This stockpile has no filters, so it counts nothing. Edit it to add one.";
const NO_STOCKPILE: &str = "Pick a stockpile on the left, or make one with New.";
const AFTER_PLANS_HELP: &str =
    "Short counts what is left after plans with Reserve take their part.";
const RIGHT_NOW_HELP: &str = "Short counts everything you hold; plan claims are only shown.";

/// Draws the contents of the selected stockpile.
pub(crate) fn show(page: &mut StockpilesPage, ui: &mut Ui, cx: &mut Shared) {
    let Some(id) = cx.selection.stockpile else {
        ui.label(RichText::new(NO_STOCKPILE).color(cx.style.mute));
        page.buy.show(ui, cx);
        return;
    };
    let snapshot = snapshot(cx);
    let Some(stockpile) = snapshot.all.get(&id).cloned() else {
        cx.selection.stockpile = None;
        ui.label(RichText::new(NO_STOCKPILE).color(cx.style.mute));
        page.buy.show(ui, cx);
        return;
    };
    let mode = mode_dropdown(ui, cx, &stockpile.name, id, snapshot.mode(id));
    if stockpile.filters.is_empty() {
        ui.label(RichText::new(NO_FILTERS).color(cx.style.mute));
    }
    let Some(sde) = cx.sde.as_ref() else {
        ui.label(RichText::new("Loading game data…").color(cx.style.mute));
        page.buy.show(ui, cx);
        return;
    };
    let mut errors = Vec::new();
    let lines = match claimed_lines(&snapshot, id, mode, sde, &cx.session) {
        Ok(lines) => lines,
        Err(error) => {
            errors.push(format!("Could not read the stockpile: {error}"));
            Vec::new()
        }
    };
    let recipes = match page.recipes(sde) {
        Ok(recipes) => Some(recipes),
        Err(error) => {
            errors.push(format!("Could not read the recipes: {error}"));
            None
        }
    };
    for text in errors {
        report(cx, text);
    }
    let held = cx.market.held();
    let mut names = std::collections::BTreeMap::new();
    for line in &lines {
        let name = type_name(line.line.type_id, cx.sde.as_ref(), &mut cx.notices);
        names.insert(line.line.type_id, name);
    }
    let mut rows = lines;
    rows.sort_by_cached_key(|line| {
        (
            status_rank(line.status),
            names
                .get(&line.line.type_id)
                .map(|name| name.to_lowercase())
                .unwrap_or_default(),
        )
    });
    let (value, incomplete) = short_total(&rows, snapshot.price_rule, &held);
    table(ui, cx, &snapshot, &rows, mode, &names, &held);
    footer(ui, cx, &snapshot, value, incomplete);
    build_short::buttons(
        page,
        ui,
        cx,
        &build_short::Buttons {
            id,
            stockpile: &stockpile,
            rows: &rows,
            names: &names,
            recipes: recipes.as_deref(),
        },
    );
    page.buy.show(ui, cx);
}

/// The Short counts dropdown. A change is saved at once; a save the store refuses is reported and
/// the dropdown keeps the saved choice.
fn mode_dropdown(ui: &mut Ui, cx: &mut Shared, name: &str, id: i64, mode: ShortMode) -> ShortMode {
    ui.horizontal(|ui| {
        ui.strong(format!("CONTENTS  {name}"));
        ui.label("Short counts");
        let mut picked = mode;
        ComboBox::from_id_salt(("stockpile short mode", id))
            .selected_text(mode_label(mode))
            .show_ui(ui, |ui| {
                for choice in [ShortMode::AfterPlans, ShortMode::RightNow] {
                    if ui
                        .selectable_label(choice == mode, mode_label(choice))
                        .clicked()
                    {
                        picked = choice;
                    }
                }
            })
            .response
            .on_hover_text(mode_help(mode));
        if picked != mode {
            match cx.store.set_short_mode(id, picked) {
                Ok(()) => {}
                Err(error) => report(cx, format!("Could not save the short mode: {error}")),
            }
        }
        if picked != mode { picked } else { mode }
    })
    .inner
}

fn mode_label(mode: ShortMode) -> &'static str {
    match mode {
        ShortMode::AfterPlans => "Full after my plans",
        ShortMode::RightNow => "Full right now",
    }
}

fn mode_help(mode: ShortMode) -> &'static str {
    match mode {
        ShortMode::AfterPlans => AFTER_PLANS_HELP,
        ShortMode::RightNow => RIGHT_NOW_HELP,
    }
}

fn status_rank(status: Status) -> u8 {
    match status {
        Status::Short => 0,
        Status::Part => 1,
        Status::Ok => 2,
    }
}

fn columns(mode: ShortMode) -> Vec<Column> {
    let mut columns = vec![
        column("item", "Item", ColumnKind::Text, 180.0),
        column("target", "Target", ColumnKind::Number, 70.0),
        column("have", "Have", ColumnKind::Number, 70.0),
        column("claimed", "Claimed", ColumnKind::Text, 150.0),
    ];
    if mode == ShortMode::AfterPlans {
        columns.push(column("free", "Free", ColumnKind::Number, 70.0));
    }
    columns.extend([
        column("short", "Short", ColumnKind::Number, 70.0),
        column("value", "Value", ColumnKind::Money, 110.0),
        column("status", "Status", ColumnKind::Text, 80.0),
    ]);
    columns
}

fn column(id: &'static str, label: &str, kind: ColumnKind, width: f32) -> Column {
    Column {
        id,
        label: label.to_owned(),
        kind,
        width,
    }
}

fn table(
    ui: &mut Ui,
    cx: &mut Shared,
    snapshot: &Snapshot,
    lines: &[ClaimedLine],
    mode: ShortMode,
    names: &std::collections::BTreeMap<TypeId, String>,
    held: &Held,
) {
    let columns = columns(mode);
    let rows: Vec<Row> = lines
        .iter()
        .map(|line| {
            let name = names
                .get(&line.line.type_id)
                .cloned()
                .unwrap_or_else(|| format!("type {}", line.line.type_id));
            let value = short_value(line.line.type_id, line.short, snapshot.price_rule, held);
            Row {
                key: u64::from(line.line.type_id),
                depth: 0,
                cells: shown_cells(line, mode, &name, value, snapshot),
            }
        })
        .collect();
    let mut draw = |ui: &mut Ui, key: u64, column: &'static str| -> bool {
        let Some(line) = lines
            .iter()
            .find(|line| u64::from(line.line.type_id) == key)
        else {
            return false;
        };
        match column {
            "have" => {
                let have = line.line.have;
                ui.label(count_text(line.line.total)).on_hover_text(format!(
                    "Assets {} · Sell orders {} · Buy orders {} · Jobs {}",
                    count_text(have.assets),
                    count_text(have.sell_orders),
                    count_text(have.buy_orders),
                    count_text(have.jobs)
                ));
                true
            }
            "claimed" => {
                if line.claimed == 0 {
                    return false;
                }
                let label = claimed_text(line, snapshot);
                if let Some(hover) = claimed_hover(line, snapshot) {
                    ui.label(label).on_hover_text(hover);
                } else {
                    ui.label(label);
                }
                true
            }
            "status" => {
                ui.colored_label(status_colour(cx, line.status), status_text(line.status));
                true
            }
            _ => false,
        }
    };
    let response = Table::new(Id::new(StockpilesPanel::Contents.title()), &columns)
        .show_with(ui, &cx.style, cx.density, &rows, None, &mut draw);
    if let Some(key) = response.clicked
        && let Ok(type_id) = TypeId::try_from(key)
    {
        cx.selection.item = Some(type_id);
    }
}

fn shown_cells(
    line: &ClaimedLine,
    mode: ShortMode,
    name: &str,
    value: Option<mop_core::money::Money>,
    snapshot: &Snapshot,
) -> Vec<Cell> {
    let mut cells = vec![
        Cell::text(name),
        Cell::int(i128::from(line.line.target), count_text(line.line.target)),
        Cell::int(i128::from(line.line.total), count_text(line.line.total)),
        Cell::text(claimed_text(line, snapshot)),
    ];
    if mode == ShortMode::AfterPlans {
        cells.push(Cell::int(i128::from(line.free), count_text(line.free)));
    }
    cells.extend([
        Cell::int(i128::from(line.short), count_text(line.short)),
        value.map_or_else(Cell::missing, Cell::money),
        Cell::text(status_text(line.status)),
    ]);
    cells
}

fn claimed_text(line: &ClaimedLine, snapshot: &Snapshot) -> String {
    if line.claimed == 0 {
        return "—".to_owned();
    }
    if let [(plan_id, _)] = line.by.as_slice() {
        return format!(
            "{} · {}",
            count_text(line.claimed),
            plan_name(*plan_id, snapshot)
        );
    }
    format!("{} · {} plans", count_text(line.claimed), line.by.len())
}

fn claimed_hover(line: &ClaimedLine, snapshot: &Snapshot) -> Option<String> {
    if line.by.is_empty() {
        return None;
    }
    Some(
        line.by
            .iter()
            .map(|(plan_id, units)| {
                format!("{}: {}", plan_name(*plan_id, snapshot), count_text(*units))
            })
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

fn plan_name(plan_id: i64, snapshot: &Snapshot) -> String {
    snapshot
        .plan_names
        .get(&plan_id)
        .cloned()
        .unwrap_or_else(|| format!("plan {plan_id}"))
}

fn status_text(status: Status) -> &'static str {
    match status {
        Status::Ok => "ok",
        Status::Part => "part",
        Status::Short => "short",
    }
}

fn status_colour(cx: &Shared, status: Status) -> egui::Color32 {
    match status {
        Status::Ok => cx.style.pos,
        Status::Part => cx.style.stale,
        Status::Short => cx.style.neg,
    }
}

fn footer(
    ui: &mut Ui,
    cx: &Shared,
    snapshot: &Snapshot,
    value: mop_core::money::Money,
    incomplete: usize,
) {
    let mut text = format!("Short value {}", value.format_isk());
    if incomplete > 0 {
        text.push_str(&format!(" (incomplete: {incomplete} without price)"));
    }
    ui.label(RichText::new(text).color(cx.style.mute));
    let age = cx
        .session
        .oldest_fetch()
        // A floor at 0 is meant here: a fetch time after `now` shows as an age of zero.
        .map(|oldest| age_text(snapshot.now.saturating_sub(oldest)));
    ui.label(
        RichText::new(format!("Oldest: {}", age.unwrap_or_else(|| "—".to_owned())))
            .color(cx.style.mute),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use mop_core::stockpile::{Have, Line};
    use mop_market::book::PriceRule;
    use std::collections::BTreeMap;

    fn line(type_id: TypeId, claimed: u64, by: Vec<(i64, u64)>) -> ClaimedLine {
        ClaimedLine {
            line: Line {
                type_id,
                target: 10,
                have: Have {
                    assets: 7,
                    jobs: 5,
                    ..Have::default()
                },
                total: 12,
                short: 3,
                status: Status::Part,
            },
            claimed,
            by,
            free: 5,
            short: 3,
            status: Status::Part,
        }
    }

    #[test]
    fn the_modes_name_and_help_read_as_the_owner_wrote_them() {
        assert_eq!(mode_label(ShortMode::AfterPlans), "Full after my plans");
        assert_eq!(mode_label(ShortMode::RightNow), "Full right now");
        assert_eq!(mode_help(ShortMode::AfterPlans), AFTER_PLANS_HELP);
        assert_eq!(mode_help(ShortMode::RightNow), RIGHT_NOW_HELP);
    }

    #[test]
    fn claimed_names_one_plan_or_counts_several() {
        let snapshot = Snapshot {
            all: Default::default(),
            ordered: Vec::new(),
            plan_names: BTreeMap::from([(7, "Widget A".to_owned())]),
            assets: Vec::new(),
            sell_orders: Vec::new(),
            buy_orders: Vec::new(),
            jobs: Vec::new(),
            claims: Default::default(),
            modes: Default::default(),
            price_rule: PriceRule::SellFivePercent,
            now: 0,
        };
        let one = line(200, 7, vec![(7, 7)]);
        assert_eq!(claimed_text(&one, &snapshot), "7 · Widget A");
        let cells = shown_cells(&one, ShortMode::AfterPlans, "Part", None, &snapshot);
        assert_eq!(
            cells[3].text, "7 · Widget A",
            "the copied cell names the plan"
        );
        assert_eq!(
            claimed_hover(&one, &snapshot).as_deref(),
            Some("Widget A: 7")
        );
        let none = line(200, 0, Vec::new());
        assert_eq!(claimed_text(&none, &snapshot), "—");
        let many = line(200, 9, vec![(7, 7), (8, 2)]);
        assert_eq!(claimed_text(&many, &snapshot), "9 · 2 plans");
        assert_eq!(
            claimed_hover(&many, &snapshot).as_deref(),
            Some("Widget A: 7\nplan 8: 2")
        );
    }

    #[test]
    fn statuses_rank_short_part_ok() {
        assert!(status_rank(Status::Short) < status_rank(Status::Part));
        assert!(status_rank(Status::Part) < status_rank(Status::Ok));
        assert_eq!(status_text(Status::Ok), "ok");
    }
}

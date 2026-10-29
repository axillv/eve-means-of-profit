//! The Stockpiles panel: the stockpiles by group, and the buttons that change them.

use std::collections::BTreeMap;

use egui::{Button, CollapsingHeader, RichText, Ui};
use mop_core::stockpile::Stockpile;
use mop_store::StoreError;

use super::{Snapshot, StockpilesPage, claimed_lines, snapshot};
use crate::Shared;
use crate::build::state::{push_error, report};

/// The list's own state: the delete question.
#[derive(Debug, Default)]
pub(crate) struct State {
    dialog: Option<Delete>,
}

impl State {
    pub(crate) fn reset(&mut self) {
        self.dialog = None;
    }
}

#[derive(Debug, Clone)]
struct Delete {
    id: i64,
    name: String,
}

/// Draws the list panel.
pub(crate) fn show(page: &mut StockpilesPage, ui: &mut Ui, cx: &mut Shared) {
    delete_dialog(page, ui, cx);
    let snapshot = snapshot(cx);
    let selected = cx.selection.stockpile;
    let counts = short_counts(&snapshot, cx);
    buttons(page, ui, cx, &snapshot, selected);
    groups(ui, cx, &snapshot, &counts, selected);
}

/// One group of the list, in the order the panel draws them: the groups sorted ignoring case, and
/// `No group` last. The stockpiles inside a group keep the store's order.
fn grouped(ordered: &[Stockpile]) -> Vec<(Option<String>, Vec<&Stockpile>)> {
    let mut named: BTreeMap<String, Vec<&Stockpile>> = BTreeMap::new();
    let mut no_group = Vec::new();
    for stockpile in ordered {
        match &stockpile.group {
            Some(group) => named.entry(group.clone()).or_default().push(stockpile),
            None => no_group.push(stockpile),
        }
    }
    let mut names: Vec<String> = named.keys().cloned().collect();
    names.sort_by_cached_key(|name| name.to_lowercase());
    let mut groups: Vec<(Option<String>, Vec<&Stockpile>)> = names
        .into_iter()
        .map(|name| {
            let list = named.remove(&name).unwrap_or_default();
            (Some(name), list)
        })
        .collect();
    if !no_group.is_empty() {
        groups.push((None, no_group));
    }
    groups
}

/// Each stockpile's count of lines that are short under its own mode.
fn short_counts(snapshot: &Snapshot, cx: &Shared) -> BTreeMap<i64, usize> {
    let Some(sde) = cx.sde.as_ref() else {
        return BTreeMap::new();
    };
    let mut counts = BTreeMap::new();
    for stockpile in &snapshot.ordered {
        let lines = claimed_lines(
            snapshot,
            stockpile.id,
            snapshot.mode(stockpile.id),
            sde,
            &cx.session,
        );
        if let Ok(lines) = lines {
            counts.insert(
                stockpile.id,
                lines.iter().filter(|line| line.short > 0).count(),
            );
        }
    }
    counts
}

fn groups(
    ui: &mut Ui,
    cx: &mut Shared,
    snapshot: &Snapshot,
    counts: &BTreeMap<i64, usize>,
    selected: Option<i64>,
) {
    for (group, stockpiles) in grouped(&snapshot.ordered) {
        let name = group.as_deref().unwrap_or("No group");
        CollapsingHeader::new(name)
            .id_salt(("stockpile group", group.clone()))
            .default_open(true)
            .show(ui, |ui| {
                for stockpile in stockpiles {
                    row(ui, cx, stockpile, counts, selected);
                }
            });
    }
}

fn row(
    ui: &mut Ui,
    cx: &mut Shared,
    stockpile: &Stockpile,
    counts: &BTreeMap<i64, usize>,
    selected: Option<i64>,
) {
    let is_selected = selected == Some(stockpile.id);
    ui.horizontal(|ui| {
        if ui
            .selectable_label(is_selected, stockpile.name.as_str())
            .clicked()
        {
            cx.selection.stockpile = Some(stockpile.id);
        }
        if stockpile.multiplier != 1 {
            ui.label(RichText::new(format!(" ×{}", stockpile.multiplier)).color(cx.style.mute));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            match counts.get(&stockpile.id).copied().unwrap_or(0) {
                0 => ui.label(RichText::new("ok").color(cx.style.mute)),
                short => ui.label(RichText::new(format!("{short} short")).color(cx.style.stale)),
            };
        });
    });
}

fn buttons(
    page: &mut StockpilesPage,
    ui: &mut Ui,
    cx: &mut Shared,
    snapshot: &Snapshot,
    selected: Option<i64>,
) {
    ui.horizontal_wrapped(|ui| {
        if ui.button("New").clicked() {
            new_stockpile(page, cx);
            return;
        }
        let can_edit = selected.is_some();
        if ui.add_enabled(can_edit, Button::new("Edit")).clicked()
            && let Some(id) = selected
        {
            page.edit(id, cx);
        }
        if ui.add_enabled(can_edit, Button::new("Delete")).clicked()
            && let Some(stockpile) = selected.and_then(|id| snapshot.all.get(&id))
        {
            page.list.dialog = Some(Delete {
                id: stockpile.id,
                name: stockpile.name.clone(),
            });
        }
        let position = selected.and_then(|id| snapshot.ordered.iter().position(|s| s.id == id));
        let up = position.is_some_and(|at| at > 0);
        let down = position.is_some_and(|at| at + 1 < snapshot.ordered.len());
        if ui.add_enabled(up, Button::new("Move up")).clicked()
            && let Some(at) = position
        {
            move_to(cx, snapshot, at, at - 1);
        }
        if ui.add_enabled(down, Button::new("Move down")).clicked()
            && let Some(at) = position
        {
            move_to(cx, snapshot, at, at + 1);
        }
        if ui.button("Import from jEve…").clicked() {
            page.open_import();
        }
    });
}

/// Makes `New stockpile`, or the first free `New stockpile N`, selects it and opens its editor.
fn new_stockpile(page: &mut StockpilesPage, cx: &mut Shared) {
    let taken = match cx.store.stockpiles() {
        Ok(list) => list
            .into_iter()
            .map(|stockpile| stockpile.name)
            .collect::<Vec<_>>(),
        Err(error) => {
            report(cx, format!("Could not read the stockpiles: {error}"));
            return;
        }
    };
    let name = free_name(&taken);
    match cx.store.create_stockpile(&name, None) {
        Ok(id) => {
            cx.selection.stockpile = Some(id);
            page.edit(id, cx);
        }
        Err(error) => report(cx, format!("Could not make the stockpile: {error}")),
    }
}

/// `New stockpile`, then `New stockpile 2`, `New stockpile 3`… the first one no saved stockpile has.
fn free_name(taken: &[String]) -> String {
    let mut number = 1_u32;
    loop {
        let candidate = if number == 1 {
            "New stockpile".to_owned()
        } else {
            format!("New stockpile {number}")
        };
        if !taken.contains(&candidate) {
            return candidate;
        }
        number += 1;
    }
}

/// Swaps two stockpiles of the store order and saves the whole order.
fn move_to(cx: &mut Shared, snapshot: &Snapshot, from: usize, to: usize) {
    let mut ids: Vec<i64> = snapshot
        .ordered
        .iter()
        .map(|stockpile| stockpile.id)
        .collect();
    if from >= ids.len() || to >= ids.len() {
        return;
    }
    ids.swap(from, to);
    if let Err(error) = cx.store.set_stockpile_order(&ids) {
        report(cx, format!("Could not move the stockpile: {error}"));
    }
}

/// Draws the delete question while it is open.
fn delete_dialog(page: &mut StockpilesPage, ui: &mut Ui, cx: &mut Shared) {
    let Some(delete) = page.list.dialog.clone() else {
        return;
    };
    let mut confirm = false;
    let mut cancel = false;
    egui::Window::new(format!("Delete stockpile {}?", delete.name))
        .collapsible(false)
        .resizable(false)
        .show(ui.ctx(), |ui| {
            cancel |= ui.button("Cancel").clicked();
            confirm |= ui.button("Delete").clicked();
        });
    if cancel {
        page.list.dialog = None;
    } else if confirm {
        page.list.dialog = None;
        delete_stockpile(cx, &delete);
    }
}

fn delete_stockpile(cx: &mut Shared, delete: &Delete) {
    match cx.store.delete_stockpile(delete.id) {
        Ok(()) => {
            if cx.selection.stockpile == Some(delete.id) {
                cx.selection.stockpile = None;
            }
        }
        Err(StoreError::InUse { plans }) => push_error(
            &mut cx.notices,
            format!("{} is included in: {}", delete.name, plans.join(", ")),
        ),
        Err(error) => report(cx, format!("Could not delete the stockpile: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stockpiles::count_text;

    fn stockpile(id: i64, name: &str, group: Option<&str>) -> Stockpile {
        Stockpile {
            id,
            name: name.to_owned(),
            group: group.map(str::to_owned),
            multiplier: 1,
            targets: Vec::new(),
            includes: Vec::new(),
            filters: Vec::new(),
        }
    }

    #[test]
    fn groups_sort_ignoring_case_and_no_group_comes_last() {
        let list = [
            stockpile(1, "B", Some("zulu")),
            stockpile(2, "A", None),
            stockpile(3, "C", Some("Alpha")),
            stockpile(4, "D", Some("zulu")),
        ];
        let groups = grouped(&list);
        let names: Vec<Option<&str>> = groups.iter().map(|(name, _)| name.as_deref()).collect();
        assert_eq!(names, [Some("Alpha"), Some("zulu"), None]);
        let zulu: Vec<&str> = groups[1].1.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(zulu, ["B", "D"], "inside a group the store order stays");
    }

    #[test]
    fn new_names_take_the_first_free_number() {
        assert_eq!(free_name(&[]), "New stockpile");
        assert_eq!(free_name(&["New stockpile".to_owned()]), "New stockpile 2");
        assert_eq!(
            free_name(&["New stockpile".to_owned(), "New stockpile 2".to_owned()]),
            "New stockpile 3"
        );
    }

    #[test]
    fn a_short_count_is_shown_as_short_or_ok() {
        let list = [
            stockpile(1, "Kit", None),
            stockpile(2, "Weekly", Some("Drones")),
        ];
        let groups = grouped(&list);
        assert_eq!(groups.len(), 2);
        assert_eq!(count_text(1_250), "1,250");
    }
}

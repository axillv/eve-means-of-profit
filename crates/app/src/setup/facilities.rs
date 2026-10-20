//! The Facilities panel: the places the player builds, each with its structure, security and rigs.

use egui::{Button, ComboBox, Id, ScrollArea, Ui};
use profit_core::industry::{Activity, TypeId};
use profit_sde::{Facility, Sde, Security, Structure, StructureKind};
use profit_store::{FacilityRow, StoreError};

use super::fields::text_field;
use crate::Shared;
use crate::table::Table;
use crate::table::model::{Cell, Column, ColumnKind, Row};
use crate::theme::Style;

const NPC_STATION: &str = "NPC station";
/// The most rig dropdowns a form shows: a structure takes at most this many rigs.
const RIG_SLOTS: usize = 3;

/// What the panel keeps between frames, in the window's temporary memory.
#[derive(Debug, Clone, Default)]
struct Panel {
    selected: Option<i64>,
    form: Option<Form>,
    message: Option<String>,
}

/// The form for a new facility, or for an existing one under `editing`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Form {
    editing: Option<i64>,
    name: String,
    /// `None` is an NPC station.
    structure: Option<TypeId>,
    security: Security,
    rigs: [Option<TypeId>; RIG_SLOTS],
}

/// What a button or the form asked for in this frame.
#[derive(Clone, Copy)]
enum Action {
    New,
    Edit(i64),
    Delete(i64),
    Save,
    Cancel,
}

impl Form {
    fn new() -> Self {
        Self {
            editing: None,
            name: String::new(),
            structure: None,
            security: Security::High,
            rigs: [None; RIG_SLOTS],
        }
    }

    fn edit(row: &FacilityRow) -> Self {
        let mut rigs = [None; RIG_SLOTS];
        for (slot, rig) in rigs.iter_mut().zip(&row.facility.rigs) {
            *slot = Some(*rig);
        }
        Self {
            editing: Some(row.id),
            name: row.name.clone(),
            structure: row.facility.structure,
            security: row.facility.security,
            rigs,
        }
    }

    /// The facility as the form describes it: the rigs that are picked, in slot order.
    fn facility(&self) -> Facility {
        Facility {
            structure: self.structure,
            security: self.security,
            rigs: self.rigs.iter().flatten().copied().collect(),
        }
    }
}

pub(super) fn show(ui: &mut Ui, cx: &mut Shared) {
    let id = Id::new("setup facilities panel");
    let mut panel = ui
        .data(|data| data.get_temp::<Panel>(id))
        .unwrap_or_default();
    // The form sits under the list, so a short panel scrolls rather than hides the form.
    ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| draw(&mut panel, ui, cx));
    ui.data_mut(|data| data.insert_temp(id, panel));
}

fn draw(panel: &mut Panel, ui: &mut Ui, cx: &mut Shared) {
    let Some(sde) = cx.sde.as_ref() else {
        return;
    };
    let structures = match sde.structures() {
        Ok(structures) => structures,
        Err(error) => {
            ui.colored_label(cx.style.neg, error.to_string());
            return;
        }
    };
    let facilities = match cx.store.facilities() {
        Ok(facilities) => facilities,
        Err(error) => {
            ui.colored_label(cx.style.neg, error.to_string());
            return;
        }
    };

    let style = &cx.style;
    let mut action = toolbar(ui, panel.selected);
    let rows: Vec<Row> = facilities
        .iter()
        .map(|row| facility_row(row, &structures, sde))
        .collect();
    let columns = columns();
    let response = Table::new(Id::new("setup facilities"), &columns)
        .selected(panel.selected.and_then(|id| u64::try_from(id).ok()))
        .show(ui, style, cx.density, &rows, None);
    if let Some(key) = response.clicked
        && let Ok(id) = i64::try_from(key)
    {
        panel.selected = Some(id);
    }

    if let Some(form) = panel.form.as_mut() {
        ui.separator();
        if let Some(form_action) = draw_form(ui, style, sde, &structures, form) {
            action = Some(form_action);
        }
    }
    if let Some(message) = &panel.message {
        ui.colored_label(style.neg, message.as_str());
    }

    if let Some(action) = action {
        apply(panel, cx, &facilities, action);
        // The store may have changed: repaint now, so the list shows the change at once.
        ui.ctx().request_repaint();
    }
}

/// The New, Edit and Delete buttons. Edit and Delete need a selected facility.
fn toolbar(ui: &mut Ui, selected: Option<i64>) -> Option<Action> {
    let mut action = None;
    ui.horizontal(|ui| {
        if ui.add(Button::new("New")).clicked() {
            action = Some(Action::New);
        }
        if ui
            .add_enabled(selected.is_some(), Button::new("Edit"))
            .clicked()
        {
            action = selected.map(Action::Edit);
        }
        if ui
            .add_enabled(selected.is_some(), Button::new("Delete"))
            .clicked()
        {
            action = selected.map(Action::Delete);
        }
    });
    action
}

fn apply(panel: &mut Panel, cx: &mut Shared, facilities: &[FacilityRow], action: Action) {
    match action {
        Action::New => {
            panel.form = Some(Form::new());
            panel.message = None;
        }
        Action::Edit(id) => {
            if let Some(row) = facilities.iter().find(|row| row.id == id) {
                panel.form = Some(Form::edit(row));
                panel.message = None;
            }
        }
        Action::Delete(id) => delete(panel, cx, id),
        Action::Save => save(panel, cx),
        Action::Cancel => {
            panel.form = None;
            panel.message = None;
        }
    }
}

fn delete(panel: &mut Panel, cx: &mut Shared, id: i64) {
    match cx.store.delete_facility(id) {
        Ok(()) => {
            panel.selected = None;
            panel.message = None;
        }
        Err(error) => panel.message = Some(store_text(&error)),
    }
}

/// Saves the open form as a new facility, or over the one it was made from.
fn save(panel: &mut Panel, cx: &mut Shared) {
    let Some(form) = panel.form.clone() else {
        return;
    };
    let facility = form.facility();
    let saved = match form.editing {
        None => cx.store.add_facility(&form.name, &facility),
        Some(id) => cx
            .store
            .update_facility(id, &form.name, &facility)
            .map(|()| id),
    };
    match saved {
        Ok(id) => {
            panel.selected = Some(id);
            panel.form = None;
            panel.message = None;
        }
        Err(error) => panel.message = Some(store_text(&error)),
    }
}

/// Draws the form. Returns the button pressed, if any. Save is off while the name is empty or the
/// set-up is not one the game allows; the reason is shown under the form.
fn draw_form(
    ui: &mut Ui,
    style: &Style,
    sde: &Sde,
    structures: &[Structure],
    form: &mut Form,
) -> Option<Action> {
    text_field(ui, style, "Name", &mut form.name, "", None);
    structure_field(ui, structures, form);
    let structure = structure_of(structures, form.structure);
    security_field(ui, form, structure);
    if let Some(structure) = structure {
        rig_fields(ui, sde, structure, form);
    }

    let error = sde
        .check_facility(&form.facility(), activity_of(structure))
        .err()
        .map(|error| error.to_string());
    if let Some(error) = &error {
        ui.colored_label(style.neg, error.as_str());
    }
    let can_save = error.is_none() && !form.name.trim().is_empty();
    save_buttons(ui, can_save)
}

fn structure_field(ui: &mut Ui, structures: &[Structure], form: &mut Form) {
    let before = form.structure;
    ui.horizontal(|ui| {
        ui.label("Structure");
        let current =
            structure_of(structures, form.structure).map_or(NPC_STATION, |s| s.name.as_str());
        ComboBox::from_id_salt("facility structure")
            .selected_text(current)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut form.structure, None, NPC_STATION);
                for choice in structures {
                    ui.selectable_value(&mut form.structure, Some(choice.type_id), &choice.name);
                }
            });
    });
    if form.structure != before {
        // Rigs fit one structure only, and a refinery has no high sec: start the set-up again.
        form.rigs = [None; RIG_SLOTS];
        let allowed = securities(structure_of(structures, form.structure));
        if let Some(first) = allowed
            .first()
            .filter(|_| !allowed.contains(&form.security))
        {
            form.security = *first;
        }
    }
}

fn security_field(ui: &mut Ui, form: &mut Form, structure: Option<&Structure>) {
    ui.horizontal(|ui| {
        ui.label("Security");
        ComboBox::from_id_salt("facility security")
            .selected_text(security_word(form.security))
            .show_ui(ui, |ui| {
                for choice in securities(structure) {
                    ui.selectable_value(&mut form.security, *choice, security_word(*choice));
                }
            });
    });
}

/// One dropdown per rig slot, each listing the rigs that fit this structure for its activity.
fn rig_fields(ui: &mut Ui, sde: &Sde, structure: &Structure, form: &mut Form) {
    let rigs = sde
        .rigs_for(structure.type_id, activity_of(Some(structure)))
        .unwrap_or_default();
    for (slot, choice) in form.rigs.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            ui.label(format!("Rig {}", slot + 1));
            let current = rigs
                .iter()
                .find(|rig| Some(rig.type_id) == *choice)
                .map_or("(none)", |rig| rig.name.as_str());
            ComboBox::from_id_salt(("facility rig", slot))
                .selected_text(current)
                .show_ui(ui, |ui| {
                    ui.selectable_value(choice, None, "(none)");
                    for rig in &rigs {
                        ui.selectable_value(choice, Some(rig.type_id), &rig.name);
                    }
                });
        });
    }
}

fn save_buttons(ui: &mut Ui, can_save: bool) -> Option<Action> {
    let mut action = None;
    ui.horizontal(|ui| {
        if ui.add_enabled(can_save, Button::new("Save")).clicked() {
            action = Some(Action::Save);
        }
        if ui.button("Cancel").clicked() {
            action = Some(Action::Cancel);
        }
    });
    action
}

fn columns() -> Vec<Column> {
    vec![
        Column {
            id: "name",
            label: "Name".to_owned(),
            kind: ColumnKind::Text,
            width: 200.0,
        },
        Column {
            id: "structure",
            label: "Structure".to_owned(),
            kind: ColumnKind::Text,
            width: 150.0,
        },
        Column {
            id: "security",
            label: "Security".to_owned(),
            kind: ColumnKind::Text,
            width: 90.0,
        },
        Column {
            id: "rigs",
            label: "Rigs".to_owned(),
            kind: ColumnKind::Text,
            width: 320.0,
        },
    ]
}

fn facility_row(row: &FacilityRow, structures: &[Structure], sde: &Sde) -> Row {
    let structure = match (
        row.facility.structure,
        structure_of(structures, row.facility.structure),
    ) {
        (None, _) => NPC_STATION.to_owned(),
        (Some(id), None) => format!("type {id}"),
        (Some(_), Some(structure)) => structure.name.clone(),
    };
    let rigs = row
        .facility
        .rigs
        .iter()
        .map(|id| match sde.item(*id) {
            Ok(Some(item)) => item.name,
            Ok(None) => format!("type {id}"),
            Err(error) => error.to_string(),
        })
        .collect::<Vec<_>>()
        .join(", ");
    Row {
        key: u64::try_from(row.id).unwrap_or_default(),
        depth: 0,
        cells: vec![
            Cell::text(row.name.clone()),
            Cell::text(structure),
            Cell::text(security_word(row.facility.security)),
            Cell::text(rigs),
        ],
    }
}

fn structure_of(structures: &[Structure], id: Option<TypeId>) -> Option<&Structure> {
    id.and_then(|id| structures.iter().find(|structure| structure.type_id == id))
}

/// Refineries make reactions; every other place builds manufacturing jobs.
fn activity_of(structure: Option<&Structure>) -> Activity {
    match structure.map(|structure| structure.kind) {
        Some(StructureKind::Refinery) => Activity::Reaction,
        _ => Activity::Manufacturing,
    }
}

/// The securities a structure can be set up in: a refinery is never in high sec.
fn securities(structure: Option<&Structure>) -> &'static [Security] {
    match structure.map(|structure| structure.kind) {
        Some(StructureKind::Refinery) => &[Security::Low, Security::Null],
        _ => &[Security::High, Security::Low, Security::Null],
    }
}

fn security_word(security: Security) -> &'static str {
    match security {
        Security::High => "High",
        Security::Low => "Low",
        Security::Null => "Null",
    }
}

/// The text a store error shows under the panel: a name already used, or the rest as shared.
fn store_text(error: &StoreError) -> String {
    if let StoreError::NameTaken(name) = error {
        format!("A facility named {name} already exists.")
    } else {
        super::store_text(error)
    }
}

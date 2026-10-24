//! The Facilities panel: the places the player builds, each with its structure, security and rigs.

use egui::{Button, ComboBox, Id, ScrollArea, TextEdit, Ui};
use mop_core::industry::{Activity, TypeId};
use mop_core::money::Rate;
use mop_sde::{Facility, FacilityError, Sde, Security, Structure, StructureKind};
use mop_store::{FacilityCosts, FacilityRow, StoreError};

use super::fees::percent_text;
use super::fields::{percent, text_field};
use crate::Shared;
use crate::table::Table;
use crate::table::model::{Cell, Column, ColumnKind, Row};
use crate::theme::Style;

const NPC_STATION: &str = "NPC station";
/// The most rig dropdowns a form shows: a structure takes at most this many rigs.
const RIG_SLOTS: usize = 3;
/// The most facility rows the list shows before it scrolls.
const TABLE_ROWS: usize = 12;
/// The most system names the search lists under the System field.
const SEARCH_LIMIT: usize = 10;
/// The tax an NPC station charges: the tax field starts with it and returns to it.
const NPC_TAX: &str = "0.25";
/// The mute hint of the Tax field.
fn tax_hint() -> String {
    format!("The structure owner's tax. NPC stations: {NPC_TAX}%.")
}

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
    /// The system as typed; empty is no system.
    system: String,
    /// The tax as typed, in percent; empty is no tax.
    tax: String,
    /// Once the user has typed in the tax, the form no longer changes it.
    tax_typed: bool,
}

/// What a button or the form asked for in this frame.
#[derive(Clone, Copy)]
enum Action {
    New,
    Edit(i64),
    Delete(i64),
    Save(FacilityCosts),
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
            system: String::new(),
            tax: NPC_TAX.to_owned(),
            tax_typed: false,
        }
    }

    fn edit(row: &FacilityRow, costs: FacilityCosts, system: String) -> Self {
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
            system,
            tax: costs.tax.map(Rate::to_percent_string).unwrap_or_default(),
            tax_typed: true,
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
    let (costs, costs_error) = match cx.store.all_facility_costs() {
        Ok(costs) => (costs, None),
        Err(error) => (Default::default(), Some(error.to_string())),
    };

    let style = &cx.style;
    let mut action = toolbar(ui, panel.selected);
    // The form comes before the list, so it is in view whatever the panel's height.
    if let Some(form) = panel.form.as_mut() {
        ui.separator();
        if let Some(form_action) = draw_form(ui, style, sde, &structures, form) {
            action = Some(form_action);
        }
        ui.separator();
    }
    if let Some(message) = &panel.message {
        ui.colored_label(style.neg, message.as_str());
    }
    let rows: Vec<Row> = facilities
        .iter()
        .map(|row| facility_row(row, &structures, sde, costs.get(&row.id).copied()))
        .collect();
    let columns = columns();
    let response = Table::new(Id::new("setup facilities"), &columns)
        .selected(panel.selected.and_then(|id| u64::try_from(id).ok()))
        .fit_rows(TABLE_ROWS)
        .show(ui, style, cx.density, &rows, None);
    if let Some(key) = response.clicked
        && let Ok(id) = i64::try_from(key)
    {
        panel.selected = Some(id);
    }
    if let Some(error) = costs_error {
        ui.colored_label(style.neg, error);
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
                edit(panel, cx, row);
            }
        }
        Action::Delete(id) => delete(panel, cx, id),
        Action::Save(costs) => save(panel, cx, costs),
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

/// Opens the form over a facility, with its system and tax as the store holds them.
fn edit(panel: &mut Panel, cx: &mut Shared, row: &FacilityRow) {
    match cx.store.facility_costs(row.id) {
        Ok(costs) => {
            let system = match (cx.sde.as_ref(), costs.system) {
                (Some(sde), Some(system)) => system_name(sde, system),
                _ => String::new(),
            };
            panel.form = Some(Form::edit(row, costs, system));
            panel.message = None;
        }
        Err(error) => panel.message = Some(error.to_string()),
    }
}

/// Saves the open form as a new facility, or over the one it was made from, then its costs. If
/// the costs fail the facility is saved all the same, and the form stays open over it.
fn save(panel: &mut Panel, cx: &mut Shared, costs: FacilityCosts) {
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
        Ok(id) => match cx.store.set_facility_costs(id, &costs) {
            Ok(()) => {
                panel.selected = Some(id);
                panel.form = None;
                panel.message = None;
            }
            Err(error) => {
                panel.selected = Some(id);
                panel.form = Some(Form {
                    editing: Some(id),
                    ..form
                });
                panel.message = Some(error.to_string());
            }
        },
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
    let system = system_field(ui, style, sde, &mut form.system);
    let tax = tax_field(ui, style, form);

    let error = sde
        .check_facility(&form.facility(), activity_of(structure))
        .err()
        .map(|error| facility_error_text(sde, &error));
    if let Some(error) = &error {
        ui.colored_label(style.neg, error.as_str());
    }
    let can_save = error.is_none() && tax.is_ok() && !form.name.trim().is_empty();
    let costs = FacilityCosts {
        system,
        tax: tax.ok().flatten(),
    };
    save_buttons(ui, can_save, costs)
}

/// The System field: the system the text names exactly, if any. Until then the systems it could
/// be are listed under it, and a click on one fills it in.
fn system_field(ui: &mut Ui, style: &Style, sde: &Sde, query: &mut String) -> Option<u32> {
    let chosen = match sde.find_system(query) {
        Ok(chosen) => chosen,
        Err(error) => {
            ui.colored_label(style.neg, error.to_string());
            None
        }
    };
    ui.horizontal(|ui| {
        let caption = ui.label("System");
        ui.add(
            TextEdit::singleline(query)
                .hint_text("Needed for job costs.")
                .desired_width(220.0),
        )
        .labelled_by(caption.id);
    });
    if chosen.is_none() && !query.trim().is_empty() {
        let candidates = match sde.search_systems(query, SEARCH_LIMIT) {
            Ok(candidates) => candidates,
            Err(error) => {
                ui.colored_label(style.neg, error.to_string());
                Vec::new()
            }
        };
        if candidates.is_empty() {
            ui.colored_label(style.mute, "No system has that name.");
        }
        for id in candidates {
            let name = system_name(sde, id);
            if ui.button(name.as_str()).clicked() {
                *query = name;
            }
        }
    }
    chosen
}

/// The Tax % field. Typing in it is what keeps the form from changing it again. Returns the tax
/// the text holds: `None` when it is empty.
fn tax_field(ui: &mut Ui, style: &Style, form: &mut Form) -> Result<Option<Rate>, String> {
    let before = form.tax.clone();
    text_field(ui, style, "Tax %", &mut form.tax, &tax_hint(), None);
    if form.tax != before {
        form.tax_typed = true;
    }
    let tax = percent("Tax %", &form.tax);
    if let Err(error) = &tax {
        ui.colored_label(style.neg, error.as_str());
    }
    tax
}

/// The error text with the rig named, so the player can tell which of the dropdowns it is about.
fn facility_error_text(sde: &Sde, error: &FacilityError) -> String {
    let rig_name = |rig: TypeId| match sde.item(rig) {
        Ok(Some(item)) => item.name,
        Ok(None) | Err(_) => format!("rig {rig}"),
    };
    if let FacilityError::RigGroupTwice(rig) = error {
        format!(
            "{} is a second rig of a group that is already fitted",
            rig_name(*rig)
        )
    } else if let FacilityError::RigDoesNotFit { rig, structure } = error {
        let place = if structure.is_some() {
            "this structure"
        } else {
            "an NPC station"
        };
        format!("{} does not fit {place}", rig_name(*rig))
    } else {
        error.to_string()
    }
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
        // The tax follows the structure until the user types one: an NPC station charges 0.25%.
        if !form.tax_typed {
            form.tax = if form.structure.is_none() {
                NPC_TAX.to_owned()
            } else {
                String::new()
            };
        }
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
            // Rig names are long: use the room there is, up to the length of the longest.
            ComboBox::from_id_salt(("facility rig", slot))
                .width((ui.available_width() - 24.0).clamp(120.0, 440.0))
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

fn save_buttons(ui: &mut Ui, can_save: bool, costs: FacilityCosts) -> Option<Action> {
    let mut action = None;
    ui.horizontal(|ui| {
        if ui.add_enabled(can_save, Button::new("Save")).clicked() {
            action = Some(Action::Save(costs));
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
            width: 140.0,
        },
        Column {
            id: "structure",
            label: "Structure".to_owned(),
            kind: ColumnKind::Text,
            width: 130.0,
        },
        Column {
            id: "security",
            label: "Security".to_owned(),
            kind: ColumnKind::Text,
            width: 80.0,
        },
        Column {
            id: "system",
            label: "System".to_owned(),
            kind: ColumnKind::Text,
            width: 120.0,
        },
        Column {
            id: "tax",
            label: "Tax".to_owned(),
            kind: ColumnKind::Text,
            width: 90.0,
        },
        Column {
            id: "rigs",
            label: "Rigs".to_owned(),
            kind: ColumnKind::Text,
            width: 200.0,
        },
    ]
}

/// The system and tax cells: a dash where the store holds none.
fn facility_row(
    row: &FacilityRow,
    structures: &[Structure],
    sde: &Sde,
    costs: Option<FacilityCosts>,
) -> Row {
    let costs = costs.unwrap_or(FacilityCosts {
        system: None,
        tax: None,
    });
    let system = costs
        .system
        .map_or_else(|| "—".to_owned(), |system| system_name(sde, system));
    let tax = costs.tax.map_or_else(|| "—".to_owned(), percent_text);
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
            Cell::text(system),
            Cell::text(tax),
            Cell::text(rigs),
        ],
    }
}

/// The name of a system, as the Setup pages show it.
fn system_name(sde: &Sde, id: u32) -> String {
    match sde.system(id) {
        Ok(Some(system)) => system.name,
        Ok(None) => format!("system {id}"),
        Err(error) => error.to_string(),
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

//! The Structures panel: every structure the app knows, with EVE's name for it and the name the user
//! typed. A typed name shows everywhere a place is shown; clearing it brings EVE's name back.

use std::collections::{BTreeMap, BTreeSet};

use egui::{Id, Key, Modifiers, ScrollArea, Sense, TextEdit, Ui};
use mop_esi::places::{self, StructureNames};
use mop_sde::Sde;

use crate::Shared;
use crate::build::prices::age_text;
use crate::empty::{self, Fill};
use crate::rail::Page;
use crate::table::Table;
use crate::table::model::{Cell, Column, ColumnKind, Row};

const MUTE: &str = "Names EVE gives come from your characters' data. A name you type here shows everywhere instead.";
const WHY: &str = "No structures yet.";
const WHAT_ADD: &str =
    "The app lists the structures your items sit in once you add a character and refresh it.";
const WHAT_REFRESH: &str =
    "Refresh a character on the Characters page to find the structures your items are in.";
const NO_VALUE: &str = "—";
const NO_ACCESS: &str = " · no access";
/// The most structure rows the table shows before it scrolls.
const TABLE_ROWS: usize = 12;

/// What the panel keeps between frames, in the window's temporary memory.
#[derive(Debug, Clone, Default)]
struct Panel {
    selected: Option<i64>,
    editing: Option<Edit>,
    message: Option<String>,
}

/// The name being typed for one structure.
#[derive(Debug, Clone)]
struct Edit {
    id: i64,
    text: String,
    error: Option<String>,
    focus: bool,
}

impl Edit {
    fn new(id: i64, text: String) -> Self {
        Self {
            id,
            text,
            error: None,
            focus: true,
        }
    }
}

/// One row before it is drawn.
struct Place {
    id: i64,
    structure: String,
    typed: Option<String>,
    system: String,
    checked: String,
}

/// Work the table found, carried out after it is drawn.
enum Action {
    Save { id: i64, text: String },
}

pub(super) fn show(ui: &mut Ui, cx: &mut Shared) {
    let id = Id::new("setup structures panel");
    let mut panel = ui
        .data(|data| data.get_temp::<Panel>(id))
        .unwrap_or_default();
    ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| draw(&mut panel, ui, cx));
    ui.data_mut(|data| data.insert_temp(id, panel));
}

fn draw(panel: &mut Panel, ui: &mut Ui, cx: &mut Shared) {
    ui.colored_label(cx.style.mute, MUTE);
    let typed = match cx.store.structure_names() {
        Ok(typed) => typed,
        Err(error) => {
            ui.colored_label(cx.style.neg, error.to_string());
            return;
        }
    };
    let now = (cx.clock)();
    let places = places_of(cx.session.structures(), &typed, cx.sde.as_ref(), now);
    if places.is_empty() {
        let no_character = cx.session.characters().is_empty() && cx.session.jeve().is_none();
        let (what, label) = if no_character {
            (WHAT_ADD, "Add character")
        } else {
            (WHAT_REFRESH, "Refresh a character")
        };
        let fill = Fill {
            label,
            page: Some(Page::Characters),
        };
        empty::show(ui, &cx.style, WHY, what, Some(fill));
        return;
    }

    // Enter opens the selected row's name, and the key is taken so the field does not save at once.
    if panel.editing.is_none()
        && let Some(id) = panel.selected
        && ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Enter))
        && let Some(place) = places.iter().find(|place| place.id == id)
    {
        panel.editing = Some(Edit::new(id, place.typed.clone().unwrap_or_default()));
    }
    if let Some(message) = &panel.message {
        ui.colored_label(cx.style.neg, message.as_str());
    }

    let rows: Vec<Row> = places.iter().map(place_row).collect();
    let columns = columns();
    let mut action = None;
    let selected = panel.selected.and_then(|id| u64::try_from(id).ok());
    let response = {
        let mut draw_cell = |ui: &mut Ui, key: u64, column: &'static str| -> bool {
            if column != "name" {
                return false;
            }
            let Ok(id) = i64::try_from(key) else {
                return false;
            };
            name_cell(ui, panel, &places, id, &mut action)
        };
        Table::new(Id::new(super::SetupPanel::Structures.title()), &columns)
            .selected(selected)
            .fit_rows(TABLE_ROWS)
            .show_with(ui, &cx.style, cx.density, &rows, None, &mut draw_cell)
    };
    if let Some(key) = response.clicked
        && let Ok(id) = i64::try_from(key)
    {
        panel.selected = Some(id);
    }
    ui.label(footer(places.len()));

    if let Some(action) = action {
        apply(panel, cx, action);
        // The names changed: repaint now, so every page shows the new name at once.
        ui.ctx().request_repaint();
    }
}

/// The name cell of one row. Double-click opens the field; while it is open, Enter or leaving it
/// saves, and Esc closes it. Returns whether the cell was drawn here.
fn name_cell(
    ui: &mut Ui,
    panel: &mut Panel,
    places: &[Place],
    id: i64,
    action: &mut Option<Action>,
) -> bool {
    let editing = panel.editing.as_ref().is_some_and(|edit| edit.id == id);
    if !editing {
        let Some(place) = places.iter().find(|place| place.id == id) else {
            return false;
        };
        let response = ui.interact(
            ui.max_rect(),
            Id::new(("structure name", id)),
            Sense::click(),
        );
        ui.label(place.typed.as_deref().unwrap_or_default());
        if response.clicked() {
            panel.selected = Some(id);
        }
        if response.double_clicked() {
            panel.editing = Some(Edit::new(id, place.typed.clone().unwrap_or_default()));
        }
        return true;
    }

    if ui.input(|input| input.key_pressed(Key::Escape)) {
        panel.editing = None;
        return true;
    }
    let Some(edit) = panel.editing.as_mut() else {
        return false;
    };
    let field = ui.add(TextEdit::singleline(&mut edit.text).desired_width(160.0));
    if edit.focus {
        field.request_focus();
        edit.focus = false;
    }
    if let Some(error) = &edit.error {
        ui.label(error.as_str());
    }
    if field.lost_focus() {
        *action = Some(Action::Save {
            id,
            text: edit.text.clone(),
        });
    }
    true
}

/// Saves the typed name, then reloads the session's names so every page shows the change. A save
/// that fails keeps the field open with the error under it.
fn apply(panel: &mut Panel, cx: &mut Shared, action: Action) {
    let Action::Save { id, text } = action;
    let now = (cx.clock)();
    match cx.store.set_structure_name(id, &text, now) {
        Ok(()) => {
            panel.editing = None;
            panel.message = None;
            match cx.store.structure_names() {
                Ok(typed) => cx.session.set_typed_names(typed),
                Err(error) => panel.message = Some(error.to_string()),
            }
        }
        Err(error) => {
            if let Some(edit) = panel.editing.as_mut() {
                edit.error = Some(error.to_string());
            }
        }
    }
}

/// One row per structure the session knows, and one per id that has only a typed name. Sorted by
/// the EVE name ignoring case, then by id.
fn places_of(
    structures: &StructureNames,
    typed: &BTreeMap<i64, String>,
    sde: Option<&Sde>,
    now: i64,
) -> Vec<Place> {
    let ids: BTreeSet<i64> = structures.keys().chain(typed.keys()).copied().collect();
    let mut places: Vec<Place> = ids
        .into_iter()
        .map(|id| {
            let entry = structures.get(&id);
            let system = entry
                .and_then(|entry| entry.system_id)
                .and_then(|system| sde?.system(system).ok().flatten())
                .map_or_else(|| NO_VALUE.to_owned(), |system| system.name);
            let checked = entry.map_or_else(
                || NO_VALUE.to_owned(),
                |entry| {
                    // A floor at 0 is meant here: a check time in the future (clock set back) gives an
                    // age of zero, which is the right answer, not an error.
                    let age = age_text(now.saturating_sub(entry.checked_at));
                    if entry.refused { age + NO_ACCESS } else { age }
                },
            );
            Place {
                id,
                structure: places::place_label(id, None, None, structures),
                typed: typed.get(&id).cloned(),
                system,
                checked,
            }
        })
        .collect();
    places.sort_by_cached_key(|place| (place.structure.to_lowercase(), place.id));
    places
}

fn place_row(place: &Place) -> Row {
    Row {
        key: u64::try_from(place.id).unwrap_or_default(),
        depth: 0,
        cells: vec![
            Cell::text(place.structure.as_str()),
            Cell::text(place.typed.as_deref().unwrap_or_default()),
            Cell::text(place.system.as_str()),
            Cell::text(place.checked.as_str()),
        ],
    }
}

fn columns() -> Vec<Column> {
    vec![
        Column {
            id: "structure",
            label: "Structure".to_owned(),
            kind: ColumnKind::Text,
            width: 230.0,
        },
        Column {
            id: "name",
            label: "Your name".to_owned(),
            kind: ColumnKind::Text,
            width: 170.0,
        },
        Column {
            id: "system",
            label: "System".to_owned(),
            kind: ColumnKind::Text,
            width: 120.0,
        },
        Column {
            id: "checked",
            label: "Checked".to_owned(),
            kind: ColumnKind::Text,
            width: 170.0,
        },
    ]
}

fn footer(count: usize) -> String {
    if count == 1 {
        "1 structure".to_owned()
    } else {
        format!("{count} structures")
    }
}

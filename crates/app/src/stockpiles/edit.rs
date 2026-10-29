//! The `Edit stockpile` window: the name and group, the targets, the stockpiles it includes, and
//! the filters that say what counts. Each field is parsed on every change, its error shows under
//! it, and Save stays off while one has an error. Save runs on the worker; a refusal from the store
//! stays in the window.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::mpsc::{self, Receiver, TryRecvError};

use egui::{Button, ComboBox, Id, Key, TextEdit, Ui};
use mop_core::industry::TypeId;
use mop_core::stock::CharacterId;
use mop_core::stockpile::{Characters, Filter, Include, Place, Source, Stockpile, Target};
use mop_sde::Sde;
use mop_store::{Store, StoreError};

use crate::Shared;
use crate::build::state::{push_error, type_name};
use crate::session::Session;
use crate::theme::Style;

/// The setting that keeps the recently picked places across restarts. Lines of
/// `kind<TAB>id<TAB>label`, the newest first in each kind.
const RECENTS_KEY: &str = "stockpile_recent_places";
const RECENTS_PER_KIND: usize = 5;
const SEARCH_LIMIT: usize = 10;

/// The editor's state: the open form and the worker's save.
#[derive(Default)]
pub(crate) struct State {
    form: Option<Form>,
    pending: Option<Receiver<Result<(), String>>>,
    /// The stockpile the pending save is for, selected once it succeeds.
    saving: Option<i64>,
    error: Option<String>,
}

impl fmt::Debug for State {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("State")
            .field("form", &self.form.is_some())
            .field("pending", &self.pending.is_some())
            .field("error", &self.error)
            .finish()
    }
}

impl State {
    /// Opens the editor on one stockpile. A stockpile the store does not hold closes it.
    pub(crate) fn open(&mut self, id: i64, cx: &mut Shared) {
        match cx.store.stockpile(id) {
            Ok(Some(stockpile)) => {
                let others = match cx.store.stockpiles() {
                    Ok(list) => list,
                    Err(error) => {
                        push_error(
                            &mut cx.notices,
                            format!("Could not read the stockpiles: {error}"),
                        );
                        vec![stockpile.clone()]
                    }
                };
                let mut groups: Vec<String> = others
                    .iter()
                    .filter_map(|stockpile| stockpile.group.clone())
                    .collect();
                groups.sort_by_cached_key(|name| name.to_lowercase());
                groups.dedup();
                let recents = read_recents(&mut cx.store);
                self.form = Some(Form::new(
                    stockpile,
                    others,
                    groups,
                    recents,
                    &cx.session,
                    cx.sde.as_ref(),
                ));
                self.pending = None;
                self.saving = None;
                self.error = None;
            }
            Ok(None) => {
                self.form = None;
                push_error(
                    &mut cx.notices,
                    "That stockpile is no longer there.".to_owned(),
                );
            }
            Err(error) => {
                self.form = None;
                push_error(
                    &mut cx.notices,
                    format!("Could not open the stockpile: {error}"),
                );
            }
        }
    }

    /// Draws the window while it is open, and takes the worker's save when it has arrived.
    pub(crate) fn show(&mut self, ui: &mut Ui, cx: &mut Shared) {
        self.drain(cx);
        let Some(mut form) = self.form.take() else {
            return;
        };
        let error = self.error.take();
        let mut action = Action::None;
        egui::Modal::new(Id::new("edit stockpile"))
            .frame(egui::Frame::popup(ui.style()).inner_margin(16.0))
            .show(ui.ctx(), |ui| {
                ui.set_min_width(680.0);
                ui.heading("Edit stockpile");
                ui.add_space(4.0);
                egui::ScrollArea::vertical()
                    .max_height(680.0)
                    .min_scrolled_height(640.0)
                    .show(ui, |ui| {
                        draw(&mut form, ui, cx, &mut action, error.as_deref());
                    });
            });
        if ui.input(|input| input.key_pressed(Key::Escape)) {
            action = Action::Cancel;
        }
        match action {
            Action::None => {
                self.form = Some(form);
                self.error = error;
            }
            Action::Cancel => {
                self.form = None;
                self.error = None;
            }
            Action::Save(stockpile) => {
                let id = stockpile.id;
                self.form = Some(form);
                self.error = None;
                self.begin_save(stockpile, id, cx);
            }
        }
    }

    fn drain(&mut self, cx: &mut Shared) {
        let received = match self.pending.as_ref() {
            Some(receiver) => receiver.try_recv(),
            None => return,
        };
        match received {
            Ok(Ok(())) => {
                self.pending = None;
                self.form = None;
                self.error = None;
                if let Some(id) = self.saving.take() {
                    cx.selection.stockpile = Some(id);
                }
            }
            Ok(Err(text)) => {
                self.pending = None;
                self.error = Some(text);
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.pending = None;
                self.error = Some("The save stopped unexpectedly.".to_owned());
            }
        }
    }

    /// Saves on the worker, so a slow database does not hold the frame.
    fn begin_save(&mut self, stockpile: Stockpile, id: i64, cx: &mut Shared) {
        let user_db = cx.data_dir.join("user.sqlite");
        let now = (cx.clock)();
        let (tx, receiver) = mpsc::channel();
        let job = move || {
            let result = match Store::open(&user_db, now) {
                Ok(opened) => {
                    let mut store = opened.store;
                    store
                        .save_stockpile(&stockpile)
                        .map_err(|error: StoreError| error.to_string())
                }
                Err(error) => Err(error.to_string()),
            };
            drop(tx.send(result));
        };
        if cx.worker.run(Box::new(job)) {
            self.pending = Some(receiver);
            self.saving = Some(id);
        } else {
            self.error = Some("The save could not start.".to_owned());
        }
    }
}

/// What the window's buttons asked for, applied after it closes.
enum Action {
    None,
    Cancel,
    Save(Stockpile),
}

/// One open editor.
#[derive(Debug)]
struct Form {
    id: i64,
    name: String,
    group: String,
    multiplier: String,
    targets: Vec<TargetRow>,
    includes: Vec<IncludeRow>,
    filters: Vec<FilterRow>,
    item_query: String,
    /// The item whose row a re-picked search result focuses.
    focus: Option<TypeId>,
    /// The stockpiles that may be included, by name.
    others: Vec<(i64, String)>,
    groups: Vec<String>,
    recents: Recents,
}

#[derive(Debug)]
struct TargetRow {
    type_id: TypeId,
    units: String,
    ignore: bool,
}

#[derive(Debug)]
struct IncludeRow {
    stockpile_id: i64,
    count: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum PlaceChoice {
    Everywhere,
    Location,
    System,
    Region,
}

impl PlaceChoice {
    fn label(self) -> &'static str {
        match self {
            Self::Everywhere => "Everywhere",
            Self::Location => "Station or structure",
            Self::System => "System",
            Self::Region => "Region",
        }
    }

    fn of(place: Place) -> Self {
        match place {
            Place::Everywhere => Self::Everywhere,
            Place::Location(_) => Self::Location,
            Place::System(_) => Self::System,
            Place::Region(_) => Self::Region,
        }
    }

    fn kind(self) -> Option<PlaceKind> {
        match self {
            Self::Everywhere => None,
            Self::Location => Some(PlaceKind::Location),
            Self::System => Some(PlaceKind::System),
            Self::Region => Some(PlaceKind::Region),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum PlaceKind {
    Location,
    System,
    Region,
}

impl PlaceKind {
    fn of(place: Place) -> Option<Self> {
        match place {
            Place::Everywhere => None,
            Place::Location(_) => Some(Self::Location),
            Place::System(_) => Some(Self::System),
            Place::Region(_) => Some(Self::Region),
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::Location => "location",
            Self::System => "system",
            Self::Region => "region",
        }
    }

    fn from_key(key: &str) -> Option<Self> {
        match key {
            "location" => Some(Self::Location),
            "system" => Some(Self::System),
            "region" => Some(Self::Region),
            _ => None,
        }
    }
}

#[derive(Debug)]
struct FilterRow {
    characters: CharChoice,
    only: BTreeSet<CharacterId>,
    place: PlaceChoice,
    /// The picked place and the label it shows under.
    chosen: Option<(Place, String)>,
    query: String,
    sources: BTreeSet<Source>,
}

impl FilterRow {
    fn new(filter: &Filter, session: &Session, sde: Option<&Sde>) -> Self {
        let (characters, only) = match &filter.characters {
            Characters::All => (CharChoice::All, BTreeSet::new()),
            Characters::Only(set) => (CharChoice::Only, set.clone()),
        };
        let place = PlaceChoice::of(filter.place);
        let chosen = match filter.place {
            Place::Everywhere => None,
            Place::Location(_) | Place::System(_) | Place::Region(_) => {
                Some((filter.place, place_label(session, sde, filter.place)))
            }
        };
        Self {
            characters,
            only,
            place,
            query: chosen
                .as_ref()
                .map(|(_, label)| label.clone())
                .unwrap_or_default(),
            chosen,
            sources: filter.sources.clone(),
        }
    }

    fn filter(&self) -> Filter {
        let characters = match self.characters {
            CharChoice::All => Characters::All,
            CharChoice::Only => Characters::Only(self.only.clone()),
        };
        let place = self
            .chosen
            .as_ref()
            .map_or(Place::Everywhere, |(place, _)| *place);
        Filter {
            characters,
            place,
            sources: self.sources.clone(),
        }
    }

    /// Whether the place dropdown asks for a place the user has not picked.
    fn needs_place(&self) -> bool {
        self.place != PlaceChoice::Everywhere && self.chosen.is_none()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CharChoice {
    All,
    Only,
}

/// One recent place and the label it showed when picked.
#[derive(Debug, Clone)]
struct Recent {
    place: Place,
    label: String,
}

/// The recently picked places, up to five of each kind, the newest first.
#[derive(Debug, Default)]
struct Recents {
    by_kind: BTreeMap<PlaceKind, Vec<Recent>>,
}

impl Recents {
    fn record(&mut self, place: Place, label: String) {
        let Some(kind) = PlaceKind::of(place) else {
            return;
        };
        let list = self.by_kind.entry(kind).or_default();
        list.retain(|recent| recent.place != place);
        list.insert(0, Recent { place, label });
        list.truncate(RECENTS_PER_KIND);
    }

    fn of(&self, kind: PlaceKind) -> &[Recent] {
        self.by_kind
            .get(&kind)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }
}

/// The places read from the setting. A missing or unreadable setting is simply empty.
fn read_recents(store: &mut Store) -> Recents {
    let mut recents = Recents::default();
    let Ok(Some(text)) = store.setting(RECENTS_KEY) else {
        return recents;
    };
    for line in text.lines() {
        let mut parts = line.splitn(3, '\t');
        let (Some(kind), Some(id), Some(label)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        let (Some(kind), Ok(id)) = (PlaceKind::from_key(kind), id.parse::<i64>()) else {
            continue;
        };
        let place = match kind {
            PlaceKind::Location => Some(Place::Location(id)),
            PlaceKind::System => u32::try_from(id).ok().map(Place::System),
            PlaceKind::Region => u32::try_from(id).ok().map(Place::Region),
        };
        if let Some(place) = place
            && !label.trim().is_empty()
        {
            recents.by_kind.entry(kind).or_default().push(Recent {
                place,
                label: label.to_owned(),
            });
        }
    }
    for list in recents.by_kind.values_mut() {
        list.truncate(RECENTS_PER_KIND);
    }
    recents
}

/// Writes the recents back to the setting. A store that refuses says why.
fn write_recents(store: &mut Store, recents: &Recents) -> Result<(), String> {
    let mut lines = Vec::new();
    for (kind, list) in &recents.by_kind {
        for recent in list {
            lines.push(format!(
                "{}\t{}\t{}",
                kind.key(),
                place_id(recent.place),
                recent.label
            ));
        }
    }
    store
        .set_setting(RECENTS_KEY, &lines.join("\n"))
        .map_err(|error| error.to_string())
}

fn place_id(place: Place) -> i64 {
    match place {
        Place::Everywhere => 0,
        Place::Location(id) => id,
        Place::System(id) | Place::Region(id) => i64::from(id),
    }
}

impl Form {
    fn new(
        stockpile: Stockpile,
        others: Vec<Stockpile>,
        groups: Vec<String>,
        recents: Recents,
        session: &Session,
        sde: Option<&Sde>,
    ) -> Self {
        let own = stockpile.id;
        let mut others: Vec<(i64, String)> = others
            .into_iter()
            .filter(|other| other.id != own)
            .map(|other| (other.id, other.name))
            .collect();
        others.sort_by_cached_key(|(_, name)| name.to_lowercase());
        Self {
            id: stockpile.id,
            name: stockpile.name,
            group: stockpile.group.unwrap_or_default(),
            multiplier: stockpile.multiplier.to_string(),
            targets: stockpile
                .targets
                .into_iter()
                .map(|target| TargetRow {
                    type_id: target.type_id,
                    units: target.units.to_string(),
                    ignore: target.ignore_multiplier,
                })
                .collect(),
            includes: stockpile
                .includes
                .into_iter()
                .map(|include| IncludeRow {
                    stockpile_id: include.stockpile_id,
                    count: include.count.to_string(),
                })
                .collect(),
            filters: stockpile
                .filters
                .iter()
                .map(|filter| FilterRow::new(filter, session, sde))
                .collect(),
            item_query: String::new(),
            focus: None,
            others,
            groups,
            recents,
        }
    }

    /// The stockpile the form describes. Every value has been checked before Save is enabled.
    fn stockpile(&self) -> Stockpile {
        Stockpile {
            id: self.id,
            name: self.name.clone(),
            group: {
                let group = self.group.trim();
                (!group.is_empty()).then(|| group.to_owned())
            },
            multiplier: parse_multiplier(&self.multiplier).unwrap_or(1),
            targets: self
                .targets
                .iter()
                .filter_map(|target| {
                    Some(Target {
                        type_id: target.type_id,
                        units: parse_units(&target.units)?,
                        ignore_multiplier: target.ignore,
                    })
                })
                .collect(),
            includes: self
                .includes
                .iter()
                .filter_map(|include| {
                    Some(Include {
                        stockpile_id: include.stockpile_id,
                        count: parse_count(&include.count)?,
                    })
                })
                .collect(),
            filters: self.filters.iter().map(FilterRow::filter).collect(),
        }
    }

    /// Whether every field reads and every filter has the place it asks for.
    fn can_save(&self) -> bool {
        parse_multiplier(&self.multiplier).is_ok()
            && self
                .targets
                .iter()
                .all(|target| parse_units(&target.units).is_some())
            && self
                .includes
                .iter()
                .all(|include| parse_count(&include.count).is_some())
            && self.filters.iter().all(|filter| !filter.needs_place())
    }
}

fn parse_multiplier(text: &str) -> Result<u32, String> {
    let text = text.trim();
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("Multiplier: a whole number from 1".to_owned());
    }
    text.parse::<u32>()
        .ok()
        .filter(|value| *value >= 1)
        .ok_or_else(|| "Multiplier: a whole number from 1".to_owned())
}

fn parse_units(text: &str) -> Option<u64> {
    let text = text.trim();
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse::<u64>().ok().filter(|value| *value >= 1)
}

/// An include's count: a whole number from 1 that fits the stored count.
fn parse_count(text: &str) -> Option<u32> {
    u32::try_from(parse_units(text)?).ok()
}

/// The label a place shows in the editor.
fn place_label(session: &Session, sde: Option<&Sde>, place: Place) -> String {
    match place {
        Place::Everywhere => "Everywhere".to_owned(),
        Place::Location(id) => session.place_label(id, sde),
        Place::System(id) => match sde.map(|sde| sde.system(id)) {
            Some(Ok(Some(system))) => system.name,
            _ => format!("System {id}"),
        },
        Place::Region(id) => format!("Region {id}"),
    }
}

/// Draws the form. `action` is set by the Save and Cancel buttons.
fn draw(form: &mut Form, ui: &mut Ui, cx: &mut Shared, action: &mut Action, error: Option<&str>) {
    let style = cx.style;
    let multiplier = parse_multiplier(&form.multiplier);
    text_field(ui, &style, "Name", &mut form.name, "", None);
    group_field(form, ui);
    text_field(
        ui,
        &style,
        "Multiplier",
        &mut form.multiplier,
        "",
        multiplier.as_ref().err().map(String::as_str),
    );

    ui.add_space(8.0);
    ui.strong("Targets");
    targets(form, ui, cx);
    ui.add_space(8.0);

    ui.strong("Included stockpiles");
    includes(form, ui, cx);
    ui.add_space(8.0);

    ui.strong("Filters");
    filters(form, ui, cx);
    if ui.button("Add filter").clicked() {
        form.filters.push(FilterRow {
            characters: CharChoice::All,
            only: BTreeSet::new(),
            place: PlaceChoice::Everywhere,
            chosen: None,
            query: String::new(),
            sources: BTreeSet::from([Source::Assets]),
        });
    }
    ui.add_space(8.0);

    if let Some(error) = error {
        ui.colored_label(style.neg, error);
    }
    let can_save = form.can_save();
    ui.horizontal(|ui| {
        if ui.add_enabled(can_save, Button::new("Save")).clicked() {
            *action = Action::Save(form.stockpile());
        }
        if ui.button("Cancel").clicked() {
            *action = Action::Cancel;
        }
    });
}

fn group_field(form: &mut Form, ui: &mut Ui) {
    ui.horizontal(|ui| {
        let caption = ui.label("Group");
        ui.add(
            TextEdit::singleline(&mut form.group)
                .hint_text("No group")
                .desired_width(180.0),
        )
        .labelled_by(caption.id);
        ComboBox::from_id_salt(("stockpile group", form.id))
            .selected_text(if form.group.trim().is_empty() {
                "Pick a group"
            } else {
                form.group.as_str()
            })
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(form.group.is_empty(), "No group")
                    .clicked()
                {
                    form.group.clear();
                }
                for group in &form.groups {
                    if ui
                        .selectable_label(form.group == *group, group.as_str())
                        .clicked()
                    {
                        form.group = group.clone();
                    }
                }
            });
    });
}

fn text_field(
    ui: &mut Ui,
    style: &Style,
    label: &str,
    text: &mut String,
    hint: &str,
    error: Option<&str>,
) {
    ui.horizontal(|ui| {
        let caption = ui.label(label);
        ui.add(
            TextEdit::singleline(text)
                .hint_text(hint)
                .desired_width(220.0),
        )
        .labelled_by(caption.id);
    });
    if let Some(error) = error {
        ui.colored_label(style.neg, error);
    }
}

/// The targets table and the item search under it.
fn targets(form: &mut Form, ui: &mut Ui, cx: &mut Shared) {
    let form_id = form.id;
    let focus = form.focus;
    let mut remove: Option<usize> = None;
    egui::Grid::new(("stockpile targets", form_id))
        .num_columns(4)
        .spacing([8.0, 4.0])
        .show(ui, |ui| {
            ui.strong("Item");
            let units_header = ui.strong("Units");
            ui.strong("Ignore multiplier");
            ui.strong("Remove");
            ui.end_row();
            for (row, target) in form.targets.iter_mut().enumerate() {
                let name = type_name(target.type_id, cx.sde.as_ref(), &mut cx.notices);
                ui.label(name);
                let units = ui
                    .add(
                        TextEdit::singleline(&mut target.units)
                            .id(Id::new(("stockpile target units", form_id, target.type_id)))
                            .desired_width(100.0),
                    )
                    .labelled_by(units_header.id);
                if focus == Some(target.type_id) {
                    units.request_focus();
                }
                ui.checkbox(&mut target.ignore, "");
                if ui.button("Remove").clicked() {
                    remove = Some(row);
                }
                ui.end_row();
                if parse_units(&target.units).is_none() {
                    ui.label("");
                    ui.colored_label(cx.style.neg, "Units: a whole number from 1");
                    ui.end_row();
                }
            }
        });
    form.focus = None;
    if let Some(row) = remove {
        form.targets.remove(row);
    }

    ui.horizontal(|ui| {
        let caption = ui.label("Add item");
        ui.add(
            TextEdit::singleline(&mut form.item_query)
                .hint_text("Search every item")
                .desired_width(220.0),
        )
        .labelled_by(caption.id);
    });
    if form.item_query.trim().is_empty() {
        return;
    }
    let mut picked = None;
    if let Some(sde) = cx.sde.as_ref() {
        match sde.search(&form.item_query, SEARCH_LIMIT) {
            Ok(ids) => {
                for id in ids {
                    let name = type_name(id, Some(sde), &mut cx.notices);
                    if ui.button(name.as_str()).clicked() {
                        picked = Some(id);
                    }
                }
            }
            Err(error) => push_error(
                &mut cx.notices,
                format!("Could not search the items: {error}"),
            ),
        }
    }
    if let Some(id) = picked {
        if form.targets.iter().any(|target| target.type_id == id) {
            form.focus = Some(id);
        } else {
            form.targets.push(TargetRow {
                type_id: id,
                units: "1".to_owned(),
                ignore: false,
            });
        }
        form.item_query.clear();
    }
}

/// The included stockpiles table and the Include dropdown.
fn includes(form: &mut Form, ui: &mut Ui, cx: &mut Shared) {
    let names: BTreeMap<i64, String> = form.others.iter().cloned().collect();
    let mut remove: Option<usize> = None;
    egui::Grid::new(("stockpile includes", form.id))
        .num_columns(3)
        .spacing([8.0, 4.0])
        .show(ui, |ui| {
            ui.strong("Stockpile");
            let count_header = ui.strong("Count");
            ui.strong("Remove");
            ui.end_row();
            for (row, include) in form.includes.iter_mut().enumerate() {
                let name = names
                    .get(&include.stockpile_id)
                    .cloned()
                    .unwrap_or_else(|| format!("stockpile {}", include.stockpile_id));
                ui.label(name);
                let count = ui.add(TextEdit::singleline(&mut include.count).desired_width(100.0));
                count.labelled_by(count_header.id);
                if ui.button("Remove").clicked() {
                    remove = Some(row);
                }
                ui.end_row();
                if parse_count(&include.count).is_none() {
                    ui.label("");
                    ui.colored_label(cx.style.neg, "Count: a whole number from 1");
                    ui.end_row();
                }
            }
        });
    if let Some(row) = remove {
        form.includes.remove(row);
    }

    let taken: BTreeSet<i64> = form
        .includes
        .iter()
        .map(|include| include.stockpile_id)
        .chain(std::iter::once(form.id))
        .collect();
    let available: Vec<(i64, String)> = form
        .others
        .iter()
        .filter(|(id, _)| !taken.contains(id))
        .cloned()
        .collect();
    let mut include = None;
    ui.horizontal(|ui| {
        ComboBox::from_id_salt(("stockpile include", form.id))
            .selected_text("Include…")
            .show_ui(ui, |ui| {
                for (id, name) in &available {
                    if ui.selectable_label(false, name.as_str()).clicked() {
                        include = Some(*id);
                    }
                }
            });
        if available.is_empty() {
            ui.label("No other stockpile to include.");
        }
    });
    if let Some(id) = include {
        form.includes.push(IncludeRow {
            stockpile_id: id,
            count: "1".to_owned(),
        });
    }
}

/// The filter boxes and Add filter.
fn filters(form: &mut Form, ui: &mut Ui, cx: &mut Shared) {
    let characters = character_names(&cx.session);
    let id = form.id;
    let Form {
        filters, recents, ..
    } = form;
    let mut remove: Option<usize> = None;
    for (index, filter) in filters.iter_mut().enumerate() {
        ui.group(|ui| {
            ui.horizontal(|ui| {
                ui.strong(format!("Filter {}", index + 1));
                if ui.button("Remove").clicked() {
                    remove = Some(index);
                }
            });
            characters_field(filter, ui, &characters);
            place_field(id, index, filter, recents, ui, cx);
            ui.horizontal(|ui| {
                ui.label("Counts");
                for (source, label) in [
                    (Source::Assets, "Assets"),
                    (Source::SellOrders, "Sell orders"),
                    (Source::BuyOrders, "Buy orders"),
                    (Source::Jobs, "Running jobs"),
                ] {
                    let mut on = filter.sources.contains(&source);
                    if ui.checkbox(&mut on, label).changed() {
                        if on {
                            filter.sources.insert(source);
                        } else {
                            filter.sources.remove(&source);
                        }
                    }
                }
            });
        });
    }
    if let Some(index) = remove {
        filters.remove(index);
    }
}

fn characters_field(filter: &mut FilterRow, ui: &mut Ui, characters: &[(CharacterId, String)]) {
    ui.horizontal(|ui| {
        ui.label("Characters");
        ui.radio_value(&mut filter.characters, CharChoice::All, "All characters");
        ui.radio_value(&mut filter.characters, CharChoice::Only, "Only these");
    });
    if filter.characters == CharChoice::Only {
        for (id, name) in characters {
            let mut ticked = filter.only.contains(id);
            if ui.checkbox(&mut ticked, name.as_str()).changed() {
                if ticked {
                    filter.only.insert(*id);
                } else {
                    filter.only.remove(id);
                }
            }
        }
    }
    // A filter saved for a character the session no longer lists keeps it.
    for id in &filter.only {
        if !characters.iter().any(|(known, _)| known == id) {
            ui.label(format!("Character {id}"));
        }
    }
}

/// The known characters: the login's, then jEve's not logged in, by name.
fn character_names(session: &Session) -> Vec<(CharacterId, String)> {
    let mut names = Vec::new();
    let mut seen = BTreeSet::new();
    for character in session.characters() {
        if let Ok(id) = CharacterId::try_from(character.id)
            && seen.insert(id)
        {
            names.push((id, character.name.clone()));
        }
    }
    if let Some(jeve) = session.jeve() {
        let mut rest: Vec<(CharacterId, String)> = jeve
            .characters
            .iter()
            .filter(|character| seen.insert(character.character_id))
            .map(|character| (character.character_id, character.name.clone()))
            .collect();
        rest.sort_by_cached_key(|(_, name)| name.to_lowercase());
        names.extend(rest);
    }
    names
}

/// One filter's place dropdown, its search box and the results or the recent picks.
fn place_field(
    form_id: i64,
    index: usize,
    filter: &mut FilterRow,
    recents: &mut Recents,
    ui: &mut Ui,
    cx: &mut Shared,
) {
    ui.horizontal(|ui| {
        ui.label("Place");
        let before = filter.place;
        ComboBox::from_id_salt(("stockpile place", form_id, index))
            .selected_text(filter.place.label())
            .show_ui(ui, |ui| {
                for choice in [
                    PlaceChoice::Everywhere,
                    PlaceChoice::Location,
                    PlaceChoice::System,
                    PlaceChoice::Region,
                ] {
                    if ui
                        .selectable_label(choice == filter.place, choice.label())
                        .clicked()
                    {
                        filter.place = choice;
                    }
                }
            });
        if filter.place != before {
            filter.chosen = None;
            filter.query.clear();
        }
    });
    let Some(kind) = filter.place.kind() else {
        return;
    };
    let chosen_label = filter.chosen.as_ref().map(|(_, label)| label.clone());
    ui.horizontal(|ui| {
        let caption = ui.label(filter.place.label());
        let response = ui
            .add(
                TextEdit::singleline(&mut filter.query)
                    .hint_text(filter.place.label())
                    .desired_width(240.0),
            )
            .labelled_by(caption.id);
        if response.changed()
            && chosen_label
                .as_deref()
                .is_none_or(|label| label != filter.query)
        {
            filter.chosen = None;
        }
    });

    let mut picked: Option<(Place, String)> = None;
    if filter.chosen.is_none() && !filter.query.trim().is_empty() {
        for (place, label) in place_matches(kind, &filter.query, cx) {
            if ui.button(label.as_str()).clicked() {
                picked = Some((place, label));
            }
        }
    } else if filter.chosen.is_none() {
        for recent in recents.of(kind) {
            if ui.button(recent.label.as_str()).clicked() {
                picked = Some((recent.place, recent.label.clone()));
            }
        }
    }

    if filter.needs_place() {
        ui.colored_label(cx.style.neg, "Place: pick a place");
    }

    if let Some((place, label)) = picked {
        filter.chosen = Some((place, label.clone()));
        filter.query = label.clone();
        recents.record(place, label);
        if let Err(error) = write_recents(&mut cx.store, recents) {
            push_error(
                &mut cx.notices,
                format!("Could not keep the recent places: {error}"),
            );
        }
    }
}

/// The places of one kind that match a query: stations and structures, systems, or regions.
fn place_matches(kind: PlaceKind, query: &str, cx: &mut Shared) -> Vec<(Place, String)> {
    let mut found = Vec::new();
    let Some(sde) = cx.sde.as_ref() else {
        return found;
    };
    match kind {
        PlaceKind::Location => {
            match sde.search_stations(query, SEARCH_LIMIT) {
                Ok(stations) => found.extend(
                    stations
                        .into_iter()
                        .map(|(id, name)| (Place::Location(id), name)),
                ),
                Err(error) => push_error(
                    &mut cx.notices,
                    format!("Could not search the stations: {error}"),
                ),
            }
            let needle = query.trim().to_lowercase();
            let mut structures: Vec<(Place, String)> = cx
                .session
                .structures()
                .keys()
                .map(|id| (Place::Location(*id), cx.session.place_label(*id, Some(sde))))
                .filter(|(_, label)| label.to_lowercase().contains(&needle))
                .collect();
            structures.sort_by_cached_key(|(_, label)| label.to_lowercase());
            found.extend(structures);
        }
        PlaceKind::System => match sde.search_systems(query, SEARCH_LIMIT) {
            Ok(systems) => {
                for id in systems {
                    let name = sde
                        .system(id)
                        .ok()
                        .flatten()
                        .map_or_else(|| format!("System {id}"), |system| system.name);
                    found.push((Place::System(id), name));
                }
            }
            Err(error) => push_error(
                &mut cx.notices,
                format!("Could not search the systems: {error}"),
            ),
        },
        PlaceKind::Region => match sde.search_regions(query, SEARCH_LIMIT) {
            Ok(regions) => found.extend(
                regions
                    .into_iter()
                    .map(|(id, name)| (Place::Region(id), name)),
            ),
            Err(error) => push_error(
                &mut cx.notices,
                format!("Could not search the regions: {error}"),
            ),
        },
    }
    found.truncate(SEARCH_LIMIT);
    found
}

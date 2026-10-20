//! The Freight services panel: the routes the player ships along, and the rate cards (services) that
//! carry along each route.

use std::sync::Arc;

use egui::{Button, ComboBox, Id, ScrollArea, TextEdit, Ui};
use profit_core::freight::RateCard;
use profit_core::money::{Money, Rate};
use profit_core::volume::Volume;
use profit_sde::{RouteKind, Sde, StarMap};
use profit_store::{Density, RouteRow, ServiceRow, Store};

use super::fields::{self, text_field};
use super::store_text;
use crate::Shared;
use crate::table::Table;
use crate::table::model::{Cell, Column, ColumnKind, Row};
use crate::theme::Style;

/// How many systems the search under a route field lists.
const SEARCH_LIMIT: usize = 10;

/// What the panel keeps between frames, in the window's temporary memory.
#[derive(Clone, Default)]
struct Panel {
    /// The gates of the map, read once: the jump counts come from it every frame.
    map: Option<Arc<StarMap>>,
    selected_route: Option<i64>,
    route_form: Option<RouteForm>,
    selected_service: Option<i64>,
    service_form: Option<ServiceForm>,
    message: Option<String>,
}

/// The two system fields of the new-route form: what is typed in each.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct RouteForm {
    from: String,
    to: String,
}

/// The service form: one text per field, so an error stays put while the rest is typed.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ServiceForm {
    editing: Option<i64>,
    name: String,
    per_m3: String,
    per_jump: String,
    collateral: String,
    minimum: String,
    max_m3: String,
    max_collateral: String,
    route_kind: RouteKind,
    jumps: String,
}

/// The values of a service form that parsed, ready to store.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ServiceValues {
    card: RateCard,
    route_kind: RouteKind,
    jumps: Option<u32>,
}

/// What a button or a form asked for in this frame.
enum Action {
    NewRoute,
    AddRoute(u32, u32),
    CancelRoute,
    SelectRoute(i64),
    DeleteRoute,
    NewService,
    EditService(i64),
    DeleteService(i64),
    SaveService(ServiceValues),
    CancelService,
}

/// The parsed number fields of the service form. Each one is the value or the error its field shows.
struct Numbers {
    per_m3: Result<Option<Money>, String>,
    per_jump: Result<Option<Money>, String>,
    collateral: Result<Option<Rate>, String>,
    minimum: Result<Option<Money>, String>,
    max_m3: Result<Option<Volume>, String>,
    max_collateral: Result<Option<Money>, String>,
    jumps: Result<Option<u32>, String>,
}

impl ServiceForm {
    fn new() -> Self {
        Self {
            editing: None,
            name: String::new(),
            per_m3: String::new(),
            per_jump: String::new(),
            collateral: String::new(),
            minimum: String::new(),
            max_m3: String::new(),
            max_collateral: String::new(),
            route_kind: RouteKind::Shortest,
            jumps: String::new(),
        }
    }

    fn edit(service: &ServiceRow) -> Self {
        let card = &service.card;
        Self {
            editing: Some(service.id),
            name: service.name.clone(),
            per_m3: card.per_m3.map_or_else(String::new, Money::to_plain),
            per_jump: card.per_jump.map_or_else(String::new, Money::to_plain),
            collateral: card
                .collateral
                .map_or_else(String::new, Rate::to_percent_string),
            minimum: card.minimum.map_or_else(String::new, Money::to_plain),
            max_m3: card.max_m3.map_or_else(String::new, Volume::to_plain),
            max_collateral: card
                .max_collateral
                .map_or_else(String::new, Money::to_plain),
            route_kind: service.route_kind,
            jumps: service
                .jumps_override
                .map_or_else(String::new, |jumps| jumps.to_string()),
        }
    }
}

pub(super) fn show(ui: &mut Ui, cx: &mut Shared) {
    let id = Id::new("setup freight panel");
    let mut panel = ui
        .data(|data| data.get_temp::<Panel>(id))
        .unwrap_or_default();
    // Routes, services and their forms stack up, so a short panel scrolls rather than hides them.
    ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| draw(&mut panel, ui, cx));
    ui.data_mut(|data| data.insert_temp(id, panel));
}

fn draw(panel: &mut Panel, ui: &mut Ui, cx: &mut Shared) {
    let Some(sde) = cx.sde.as_ref() else {
        return;
    };
    let routes = match cx.store.routes() {
        Ok(routes) => routes,
        Err(error) => {
            ui.colored_label(cx.style.neg, error.to_string());
            return;
        }
    };
    if panel.map.is_none() {
        match sde.star_map() {
            Ok(map) => panel.map = Some(Arc::new(map)),
            Err(error) => {
                ui.colored_label(cx.style.neg, error.to_string());
                return;
            }
        }
    }
    let Some(map_arc) = panel.map.clone() else {
        return;
    };
    let map: &StarMap = &map_arc;

    let mut action = route_list(ui, &cx.style, sde, map, &routes, panel);
    let selected = routes
        .iter()
        .find(|route| Some(route.id) == panel.selected_route);
    if let Some(route) = selected {
        ui.separator();
        let services = service_list(ui, &cx.style, &mut cx.store, cx.density, map, route, panel);
        action = action.or(services);
    }
    if let Some(message) = &panel.message {
        ui.colored_label(cx.style.neg, message.as_str());
    }

    if let Some(action) = action {
        apply(panel, cx, action);
        // The store may have changed: repaint now, so the lists show the change at once.
        ui.ctx().request_repaint();
    }
}

/// The routes, with their New and Delete buttons and the new-route form under them.
fn route_list(
    ui: &mut Ui,
    style: &Style,
    sde: &Sde,
    map: &StarMap,
    routes: &[RouteRow],
    panel: &mut Panel,
) -> Option<Action> {
    let mut action = None;
    ui.horizontal(|ui| {
        if ui.button("New route").clicked() {
            action = Some(Action::NewRoute);
        }
        if ui
            .add_enabled(panel.selected_route.is_some(), Button::new("Delete route"))
            .clicked()
        {
            action = Some(Action::DeleteRoute);
        }
    });
    for route in routes {
        let selected = panel.selected_route == Some(route.id);
        if ui
            .selectable_label(selected, route_line(sde, map, route))
            .clicked()
        {
            action = Some(Action::SelectRoute(route.id));
        }
    }
    if let Some(form) = panel.route_form.as_mut() {
        ui.separator();
        if let Some(route_action) = draw_route_form(ui, style, sde, form) {
            action = Some(route_action);
        }
    }
    action
}

/// The services of the selected route, with their New, Edit and Delete buttons and the form.
fn service_list(
    ui: &mut Ui,
    style: &Style,
    store: &mut Store,
    density: Density,
    map: &StarMap,
    route: &RouteRow,
    panel: &mut Panel,
) -> Option<Action> {
    let services = match store.services(route.id) {
        Ok(services) => services,
        Err(error) => {
            ui.colored_label(style.neg, error.to_string());
            return None;
        }
    };
    let mut action = None;
    ui.horizontal(|ui| {
        if ui.button("New").clicked() {
            action = Some(Action::NewService);
        }
        let selected = panel.selected_service;
        if ui
            .add_enabled(selected.is_some(), Button::new("Edit"))
            .clicked()
        {
            action = selected.map(Action::EditService);
        }
        if ui
            .add_enabled(selected.is_some(), Button::new("Delete"))
            .clicked()
        {
            action = selected.map(Action::DeleteService);
        }
    });
    let rows: Vec<Row> = services
        .iter()
        .map(|service| service_row(service, map, route))
        .collect();
    let columns = columns();
    let response = Table::new(Id::new("setup services"), &columns)
        .selected(panel.selected_service.and_then(|id| u64::try_from(id).ok()))
        .show(ui, style, density, &rows, None);
    if let Some(key) = response.clicked
        && let Ok(id) = i64::try_from(key)
    {
        panel.selected_service = Some(id);
    }
    if let Some(form) = panel.service_form.as_mut() {
        ui.separator();
        let hint = jumps_text(
            map_jumps(map, route.from_system, route.to_system, form.route_kind),
            "no route",
            " (map)",
        );
        if let Some(service_action) = draw_service_form(ui, style, &hint, form) {
            action = Some(service_action);
        }
    }
    action
}

fn apply(panel: &mut Panel, cx: &mut Shared, action: Action) {
    match action {
        Action::NewRoute => {
            panel.route_form = Some(RouteForm::default());
            panel.message = None;
        }
        Action::AddRoute(from, to) => add_route(panel, cx, from, to),
        Action::CancelRoute => {
            panel.route_form = None;
            panel.message = None;
        }
        Action::SelectRoute(id) => {
            select_route(panel, id);
        }
        Action::DeleteRoute => delete_route(panel, cx),
        Action::NewService => {
            panel.service_form = Some(ServiceForm::new());
            panel.message = None;
        }
        Action::EditService(id) => edit_service(panel, cx, id),
        Action::DeleteService(id) => delete_service(panel, cx, id),
        Action::SaveService(values) => save_service(panel, cx, &values),
        Action::CancelService => {
            panel.service_form = None;
            panel.message = None;
        }
    }
}

fn add_route(panel: &mut Panel, cx: &mut Shared, from: u32, to: u32) {
    match cx.store.add_route(from, to) {
        Ok(id) => {
            panel.route_form = None;
            select_route(panel, id);
        }
        Err(error) => panel.message = Some(store_text(&error)),
    }
}

/// Selects a route and closes anything that belonged to the one before it.
fn select_route(panel: &mut Panel, id: i64) {
    panel.selected_route = Some(id);
    panel.selected_service = None;
    panel.service_form = None;
    panel.message = None;
}

fn delete_route(panel: &mut Panel, cx: &mut Shared) {
    let Some(id) = panel.selected_route else {
        return;
    };
    match cx.store.delete_route(id) {
        Ok(()) => {
            panel.selected_route = None;
            panel.selected_service = None;
            panel.service_form = None;
            panel.message = None;
        }
        Err(error) => panel.message = Some(store_text(&error)),
    }
}

fn edit_service(panel: &mut Panel, cx: &mut Shared, id: i64) {
    let Some(route) = panel.selected_route else {
        return;
    };
    match cx.store.services(route) {
        Ok(services) => {
            if let Some(service) = services.iter().find(|service| service.id == id) {
                panel.service_form = Some(ServiceForm::edit(service));
                panel.message = None;
            }
        }
        Err(error) => panel.message = Some(store_text(&error)),
    }
}

fn delete_service(panel: &mut Panel, cx: &mut Shared, id: i64) {
    match cx.store.delete_service(id) {
        Ok(()) => {
            panel.selected_service = None;
            panel.message = None;
        }
        Err(error) => panel.message = Some(store_text(&error)),
    }
}

/// Saves the open service form as a new service on the selected route, or over the one it was
/// made from.
fn save_service(panel: &mut Panel, cx: &mut Shared, values: &ServiceValues) {
    let (Some(route_id), Some(form)) = (panel.selected_route, panel.service_form.clone()) else {
        return;
    };
    let saved = match form.editing {
        None => cx.store.add_service(
            route_id,
            &form.name,
            &values.card,
            values.route_kind,
            values.jumps,
        ),
        Some(id) => cx
            .store
            .update_service(
                id,
                &form.name,
                &values.card,
                values.route_kind,
                values.jumps,
            )
            .map(|()| id),
    };
    match saved {
        Ok(id) => {
            panel.selected_service = Some(id);
            panel.service_form = None;
            panel.message = None;
        }
        Err(error) => panel.message = Some(store_text(&error)),
    }
}

/// One route as the list shows it: `Jita → Kiainti · 11 jumps · 12 high sec`.
fn route_line(sde: &Sde, map: &StarMap, route: &RouteRow) -> String {
    let shortest = jumps_text(
        map_jumps(map, route.from_system, route.to_system, RouteKind::Shortest),
        "no route",
        " jumps",
    );
    let high_sec = jumps_text(
        map_jumps(
            map,
            route.from_system,
            route.to_system,
            RouteKind::HighSecOnly,
        ),
        "no high-sec route",
        " high sec",
    );
    format!(
        "{} → {} · {shortest} · {high_sec}",
        system_name(sde, route.from_system),
        system_name(sde, route.to_system),
    )
}

/// The fewest jumps on the map for this kind of route; `None` when there is no such route.
fn map_jumps(map: &StarMap, from: u32, to: u32, kind: RouteKind) -> Result<Option<u32>, String> {
    map.jumps(from, to, kind).map_err(|error| error.to_string())
}

/// A jump count as the lists show it: the number and its unit, the `none` text when there is no
/// route, or the error text when the map could not answer.
fn jumps_text(result: Result<Option<u32>, String>, none: &str, unit: &str) -> String {
    match result {
        Ok(Some(jumps)) => format!("{jumps}{unit}"),
        Ok(None) => none.to_owned(),
        Err(error) => error,
    }
}

fn system_name(sde: &Sde, id: u32) -> String {
    match sde.system(id) {
        Ok(Some(system)) => system.name,
        Ok(None) => format!("system {id}"),
        Err(error) => error.to_string(),
    }
}

/// Draws the new-route form. Each system is the one whose name the typed text matches exactly; until
/// then the systems it could be are listed under the field, and a click on one fills it in.
fn draw_route_form(ui: &mut Ui, style: &Style, sde: &Sde, form: &mut RouteForm) -> Option<Action> {
    let from = system_field(ui, style, sde, "From", &mut form.from);
    let to = system_field(ui, style, sde, "To", &mut form.to);
    let mut action = None;
    ui.horizontal(|ui| {
        if ui
            .add_enabled(from.is_some() && to.is_some(), Button::new("Add route"))
            .clicked()
            && let (Some(from), Some(to)) = (from, to)
        {
            action = Some(Action::AddRoute(from, to));
        }
        if ui.button("Cancel").clicked() {
            action = Some(Action::CancelRoute);
        }
    });
    action
}

/// One system field of the route form. Returns the system the text names exactly, if any.
fn system_field(
    ui: &mut Ui,
    style: &Style,
    sde: &Sde,
    label: &str,
    query: &mut String,
) -> Option<u32> {
    let chosen = match sde.find_system(query) {
        Ok(chosen) => chosen,
        Err(error) => {
            ui.colored_label(style.neg, error.to_string());
            None
        }
    };
    ui.horizontal(|ui| {
        let caption = ui.label(label);
        ui.add(
            TextEdit::singleline(query)
                .hint_text("System name")
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

fn columns() -> Vec<Column> {
    vec![
        column("name", "Name", ColumnKind::Text, 130.0),
        column("per_m3", "ISK/m³", ColumnKind::Money, 100.0),
        column("per_jump", "ISK/jump", ColumnKind::Money, 100.0),
        column("collateral", "Collateral %", ColumnKind::Text, 110.0),
        column("minimum", "Minimum", ColumnKind::Money, 130.0),
        column("max_m3", "Max m³", ColumnKind::Text, 110.0),
        column("max_collateral", "Max collateral", ColumnKind::Money, 150.0),
        column("route", "Route", ColumnKind::Text, 110.0),
        column("jumps", "Jumps", ColumnKind::Text, 110.0),
    ]
}

fn column(id: &'static str, label: &str, kind: ColumnKind, width: f32) -> Column {
    Column {
        id,
        label: label.to_owned(),
        kind,
        width,
    }
}

fn service_row(service: &ServiceRow, map: &StarMap, route: &RouteRow) -> Row {
    let card = &service.card;
    let jumps = match service.jumps_override {
        Some(jumps) => jumps.to_string(),
        None => jumps_text(
            map_jumps(map, route.from_system, route.to_system, service.route_kind),
            "no route",
            " (map)",
        ),
    };
    Row {
        key: u64::try_from(service.id).unwrap_or_default(),
        depth: 0,
        cells: vec![
            Cell::text(service.name.clone()),
            card.per_m3.map_or_else(Cell::missing, Cell::money),
            card.per_jump.map_or_else(Cell::missing, Cell::money),
            card.collateral
                .map_or_else(Cell::missing, |rate| Cell::text(rate.to_percent_string())),
            card.minimum.map_or_else(Cell::missing, Cell::money),
            card.max_m3
                .map_or_else(Cell::missing, |volume| Cell::text(volume.format_m3())),
            card.max_collateral.map_or_else(Cell::missing, Cell::money),
            Cell::text(route_kind_word(service.route_kind)),
            Cell::text(jumps),
        ],
    }
}

/// Draws the service form. Returns the button pressed, if any. Save is off while the name is empty
/// or a field does not parse; each field shows its own error.
fn draw_service_form(
    ui: &mut Ui,
    style: &Style,
    jumps_hint: &str,
    form: &mut ServiceForm,
) -> Option<Action> {
    text_field(ui, style, "Name", &mut form.name, "", None);
    let numbers = number_fields(ui, style, form, jumps_hint);
    ui.horizontal(|ui| {
        ui.label("Route");
        ComboBox::from_id_salt("service route kind")
            .selected_text(route_kind_word(form.route_kind))
            .show_ui(ui, |ui| {
                for kind in [RouteKind::Shortest, RouteKind::HighSecOnly] {
                    ui.selectable_value(&mut form.route_kind, kind, route_kind_word(kind));
                }
            });
    });
    let save = values(&numbers, form.route_kind).filter(|_| !form.name.trim().is_empty());
    save_buttons(ui, save)
}

/// Draws the eight fields that take numbers, each with its own error under it.
fn number_fields(ui: &mut Ui, style: &Style, form: &mut ServiceForm, jumps_hint: &str) -> Numbers {
    let numbers = Numbers {
        per_m3: fields::isk("ISK/m³", &form.per_m3),
        per_jump: fields::isk("ISK/jump", &form.per_jump),
        collateral: fields::percent("Collateral %", &form.collateral),
        minimum: fields::isk("Minimum", &form.minimum),
        max_m3: fields::m3("Max m³", &form.max_m3),
        max_collateral: fields::isk("Max collateral", &form.max_collateral),
        jumps: fields::jumps(&form.jumps),
    };
    text_field(
        ui,
        style,
        "ISK/m³",
        &mut form.per_m3,
        "",
        error_of(&numbers.per_m3),
    );
    text_field(
        ui,
        style,
        "ISK/jump",
        &mut form.per_jump,
        "",
        error_of(&numbers.per_jump),
    );
    text_field(
        ui,
        style,
        "Collateral %",
        &mut form.collateral,
        "",
        error_of(&numbers.collateral),
    );
    text_field(
        ui,
        style,
        "Minimum",
        &mut form.minimum,
        "",
        error_of(&numbers.minimum),
    );
    text_field(
        ui,
        style,
        "Max m³",
        &mut form.max_m3,
        "",
        error_of(&numbers.max_m3),
    );
    text_field(
        ui,
        style,
        "Max collateral",
        &mut form.max_collateral,
        "",
        error_of(&numbers.max_collateral),
    );
    text_field(
        ui,
        style,
        "Jumps",
        &mut form.jumps,
        jumps_hint,
        error_of(&numbers.jumps),
    );
    numbers
}

/// The stored values, when every number field parsed.
fn values(numbers: &Numbers, route_kind: RouteKind) -> Option<ServiceValues> {
    let (
        Ok(per_m3),
        Ok(per_jump),
        Ok(collateral),
        Ok(minimum),
        Ok(max_m3),
        Ok(max_collateral),
        Ok(jumps),
    ) = (
        &numbers.per_m3,
        &numbers.per_jump,
        &numbers.collateral,
        &numbers.minimum,
        &numbers.max_m3,
        &numbers.max_collateral,
        &numbers.jumps,
    )
    else {
        return None;
    };
    Some(ServiceValues {
        card: RateCard {
            per_m3: *per_m3,
            per_jump: *per_jump,
            collateral: *collateral,
            minimum: *minimum,
            max_m3: *max_m3,
            max_collateral: *max_collateral,
        },
        route_kind,
        jumps: *jumps,
    })
}

fn save_buttons(ui: &mut Ui, save: Option<ServiceValues>) -> Option<Action> {
    let mut action = None;
    ui.horizontal(|ui| {
        if ui
            .add_enabled(save.is_some(), Button::new("Save"))
            .clicked()
        {
            action = save.map(Action::SaveService);
        }
        if ui.button("Cancel").clicked() {
            action = Some(Action::CancelService);
        }
    });
    action
}

fn error_of<T>(result: &Result<T, String>) -> Option<&str> {
    result.as_ref().err().map(String::as_str)
}

fn route_kind_word(kind: RouteKind) -> &'static str {
    match kind {
        RouteKind::Shortest => "Shortest",
        RouteKind::HighSecOnly => "High sec only",
    }
}

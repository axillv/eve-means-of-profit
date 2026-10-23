//! The Steps panel: every step and every bought type of the plan on screen, as a tree. A built
//! type's children are its materials in recipe order. A type with several consumers is shown once,
//! under the consumer with the lowest (depth, type id) one level up.

use std::collections::BTreeMap;

use egui::{Checkbox, DragValue, Id, RichText, Ui};
use mop_core::format::group_thousands;
use mop_core::industry::{Activity, JobSplit, Recipe, RecipeBook, TypeId};
use mop_core::plan::{BuyLine, Plan, Step};
use mop_sde::Sde;
use mop_store::PlanRecord;

use super::state::{BuildPage, NO_REACTIONS, me_of, type_name};
use crate::Shared;
use crate::status::Notice;
use crate::table::Table;
use crate::table::model::{Cell, Column, ColumnKind, Row};

/// A change the table asked for, applied to the plan after the table is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Edit {
    Build(TypeId, bool),
    Me(TypeId, u8),
}

/// One type of the chain, with its step or its bought line.
struct Line<'a> {
    type_id: TypeId,
    depth: u32,
    step: Option<&'a Step>,
    buy: Option<&'a BuyLine>,
    recipe: Option<&'a Recipe>,
}

/// What the drawing of one row needs beyond its cells.
struct View {
    type_id: TypeId,
    has_recipe: bool,
    ticked: bool,
    tick_enabled: bool,
    tick_hover: Option<&'static str>,
    me: Option<u8>,
    jobs_text: String,
    jobs_hover: String,
}

pub fn show(page: &mut BuildPage, ui: &mut Ui, cx: &mut Shared) {
    let edits = draw(page, ui, cx);
    if !edits.is_empty() {
        apply(page, cx, &edits);
    }
}

/// Draws the table and returns the changes asked for in it.
fn draw(page: &BuildPage, ui: &mut Ui, cx: &mut Shared) -> Vec<Edit> {
    let Some(record) = page.record.as_ref() else {
        ui.label(RichText::new("No plan selected.").color(cx.style.mute));
        return Vec::new();
    };
    let plan = match &page.result {
        Ok(plan) => plan,
        Err(error) => {
            let text = format!("Could not work out the plan: {error}");
            ui.label(RichText::new(text).color(cx.style.neg));
            return Vec::new();
        }
    };
    let lines = tree(plan, &page.recipes, record.product);
    let views: BTreeMap<u64, View> = lines
        .iter()
        .map(|line| (u64::from(line.type_id), view(line, record)))
        .collect();
    let rows: Vec<Row> = lines
        .iter()
        .map(|line| {
            row(
                line,
                views.get(&u64::from(line.type_id)),
                cx.sde.as_ref(),
                &mut cx.notices,
            )
        })
        .collect();
    let columns = columns();
    let mut edits = Vec::new();
    let mut draw = |ui: &mut Ui, key: u64, column: &'static str| -> bool {
        let Some(view) = views.get(&key) else {
            return false;
        };
        match column {
            "build" => build_tick(ui, view, &mut edits),
            "me" => me_box(ui, view, &mut edits),
            "jobs" => jobs_label(ui, view),
            _ => false,
        }
    };
    let response = Table::new(Id::new("build_steps"), &columns)
        .show_with(ui, &cx.style, cx.density, &rows, None, &mut draw);
    if let Some(key) = response.clicked
        && let Ok(type_id) = TypeId::try_from(key)
    {
        cx.selection.item = Some(type_id);
    }
    edits
}

/// The Build tick: drawn for types with a recipe. Returns whether it drew the cell.
fn build_tick(ui: &mut Ui, view: &View, edits: &mut Vec<Edit>) -> bool {
    if !view.has_recipe {
        return false;
    }
    let mut ticked = view.ticked;
    let response = ui.add_enabled(view.tick_enabled, Checkbox::without_text(&mut ticked));
    let changed = response.changed();
    if let Some(hover) = view.tick_hover {
        response.on_disabled_hover_text(hover);
    }
    if changed {
        edits.push(Edit::Build(view.type_id, ticked));
    }
    true
}

/// The ME box: drawn for built manufacturing rows, `—` otherwise.
fn me_box(ui: &mut Ui, view: &View, edits: &mut Vec<Edit>) -> bool {
    let Some(mut me) = view.me else {
        return false;
    };
    if ui.add(DragValue::new(&mut me).range(0..=10)).changed() {
        edits.push(Edit::Me(view.type_id, me));
    }
    true
}

/// The Jobs cell, with the job split on hover.
fn jobs_label(ui: &mut Ui, view: &View) -> bool {
    if view.jobs_text.is_empty() {
        return false;
    }
    let response = ui.label(view.jobs_text.as_str());
    if !view.jobs_hover.is_empty() {
        response.on_hover_text(view.jobs_hover.as_str());
    }
    true
}

fn apply(page: &mut BuildPage, cx: &mut Shared, edits: &[Edit]) {
    if let Some(record) = page.record.as_mut() {
        for edit in edits {
            match *edit {
                Edit::Build(id, true) => {
                    record.build.insert(id);
                }
                Edit::Build(id, false) => {
                    record.build.remove(&id);
                }
                Edit::Me(id, me) => {
                    record.me.insert(id, me);
                }
            }
        }
    }
    page.edited(cx);
}

/// The chain as a tree, in the order the rows show: each type, then its children in recipe order.
fn tree<'a>(plan: &'a Plan, recipes: &'a RecipeBook, product: TypeId) -> Vec<Line<'a>> {
    let steps: BTreeMap<TypeId, &Step> =
        plan.steps.iter().map(|step| (step.type_id, step)).collect();
    let buys: BTreeMap<TypeId, &BuyLine> =
        plan.buy.iter().map(|line| (line.type_id, line)).collect();
    let owners = owners(&plan.steps, recipes);
    let children = children(&plan.steps, recipes, &owners);
    let mut lines = Vec::new();
    let context = Tree {
        steps,
        buys,
        children,
        recipes,
    };
    walk(product, 0, &context, &mut lines);
    lines
}

struct Tree<'a> {
    steps: BTreeMap<TypeId, &'a Step>,
    buys: BTreeMap<TypeId, &'a BuyLine>,
    children: BTreeMap<TypeId, Vec<TypeId>>,
    recipes: &'a RecipeBook,
}

/// The consumer each material is shown under: the built type with the lowest (depth, type id).
fn owners(steps: &[Step], recipes: &RecipeBook) -> BTreeMap<TypeId, TypeId> {
    let mut best: BTreeMap<TypeId, (u32, TypeId)> = BTreeMap::new();
    for step in steps {
        let Some(recipe) = recipes.get(step.type_id) else {
            continue;
        };
        let key = (step.depth, step.type_id);
        for (material, _) in &recipe.materials {
            best.entry(*material)
                .and_modify(|kept| {
                    if key < *kept {
                        *kept = key;
                    }
                })
                .or_insert(key);
        }
    }
    best.into_iter()
        .map(|(material, (_, consumer))| (material, consumer))
        .collect()
}

/// The materials each built type shows, in recipe order.
fn children(
    steps: &[Step],
    recipes: &RecipeBook,
    owners: &BTreeMap<TypeId, TypeId>,
) -> BTreeMap<TypeId, Vec<TypeId>> {
    let mut children: BTreeMap<TypeId, Vec<TypeId>> = BTreeMap::new();
    for step in steps {
        let Some(recipe) = recipes.get(step.type_id) else {
            continue;
        };
        let shown = children.entry(step.type_id).or_default();
        for (material, _) in &recipe.materials {
            if owners.get(material) == Some(&step.type_id) && !shown.contains(material) {
                shown.push(*material);
            }
        }
    }
    children
}

fn walk<'a>(type_id: TypeId, depth: u32, tree: &Tree<'a>, out: &mut Vec<Line<'a>>) {
    let recipes = tree.recipes;
    out.push(Line {
        type_id,
        depth,
        step: tree.steps.get(&type_id).copied(),
        buy: tree.buys.get(&type_id).copied(),
        recipe: recipes.get(type_id),
    });
    for child in tree.children.get(&type_id).into_iter().flatten() {
        walk(*child, depth + 1, tree, out);
    }
}

fn view(line: &Line, record: &PlanRecord) -> View {
    let is_product = line.type_id == record.product;
    let reaction = line
        .recipe
        .is_some_and(|recipe| recipe.activity == Activity::Reaction);
    let reactions_picked = record.reactions.is_some();
    let me = match (line.recipe, line.step) {
        (Some(recipe), Some(_)) if recipe.activity == Activity::Manufacturing => {
            Some(me_of(record, line.type_id, recipe.made_by_invention))
        }
        _ => None,
    };
    View {
        type_id: line.type_id,
        has_recipe: line.recipe.is_some(),
        ticked: is_product || record.build.contains(&line.type_id),
        tick_enabled: !is_product && (!reaction || reactions_picked),
        tick_hover: (reaction && !reactions_picked && !is_product).then_some(NO_REACTIONS),
        me,
        jobs_text: line
            .step
            .map(|step| grouped(step.jobs.job_count()))
            .unwrap_or_default(),
        jobs_hover: line
            .step
            .map(|step| jobs_hover(&step.jobs))
            .unwrap_or_default(),
    }
}

/// `<full_jobs> × <runs_per_full_job> runs + 1 × <last_job_runs> runs`, each part only when not 0.
fn jobs_hover(jobs: &JobSplit) -> String {
    let mut parts = Vec::new();
    if jobs.full_jobs != 0 {
        parts.push(format!(
            "{} × {} runs",
            jobs.full_jobs, jobs.runs_per_full_job
        ));
    }
    if jobs.last_job_runs != 0 {
        parts.push(format!("1 × {} runs", jobs.last_job_runs));
    }
    parts.join(" + ")
}

fn row(line: &Line, view: Option<&View>, sde: Option<&Sde>, notices: &mut Vec<Notice>) -> Row {
    let me = match view.and_then(|view| view.me) {
        Some(me) => Cell::int(i128::from(me), me.to_string()),
        None => Cell::text("—"),
    };
    let mut cells = vec![
        Cell::text(type_name(line.type_id, sde, notices)),
        Cell::text(""),
        me,
    ];
    match (line.step, line.buy) {
        (Some(step), _) => {
            cells.extend(amounts(step.needed, step.from_stock));
            cells.push(number(step.to_build));
            cells.push(Cell::text("—"));
            cells.push(number(step.runs));
            cells.push(number(step.jobs.job_count()));
            cells.push(spare(step.spare));
        }
        (None, Some(buy)) => {
            cells.extend(amounts(buy.needed, buy.from_stock));
            cells.push(Cell::text("—"));
            cells.push(number(buy.to_buy));
            cells.extend([Cell::text(""), Cell::text(""), Cell::text("")]);
        }
        (None, None) => {}
    }
    Row {
        key: u64::from(line.type_id),
        depth: line.depth,
        cells,
    }
}

/// Need and Own.
fn amounts(needed: u64, own: u64) -> [Cell; 2] {
    [number(needed), number(own)]
}

fn number(value: u64) -> Cell {
    Cell::int(i128::from(value), grouped(value))
}

/// A Spare of 0 shows nothing.
fn spare(value: u64) -> Cell {
    if value == 0 {
        Cell::int(0, "")
    } else {
        number(value)
    }
}

fn grouped(value: u64) -> String {
    i64::try_from(value).map_or_else(|_| value.to_string(), group_thousands)
}

fn columns() -> Vec<Column> {
    let column = |id: &'static str, label: &str, kind: ColumnKind, width: f32| Column {
        id,
        label: label.to_owned(),
        kind,
        width,
    };
    vec![
        column("item", "Item", ColumnKind::Text, 230.0),
        column("build", "Build", ColumnKind::Text, 56.0),
        column("me", "ME", ColumnKind::Number, 50.0),
        column("need", "Need", ColumnKind::Number, 90.0),
        column("own", "Own", ColumnKind::Number, 90.0),
        column("make", "Make", ColumnKind::Number, 90.0),
        column("buy", "Buy", ColumnKind::Number, 90.0),
        column("runs", "Runs", ColumnKind::Number, 80.0),
        column("jobs", "Jobs", ColumnKind::Number, 80.0),
        column("spare", "Spare", ColumnKind::Number, 90.0),
    ]
}

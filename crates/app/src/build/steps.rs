//! The Steps panel: every step and every bought type of the plan on screen, as a tree. A built
//! type's children are its materials in recipe order. A type with several consumers is shown once,
//! under the consumer with the lowest (depth, type id) one level up.

use std::collections::{BTreeMap, BTreeSet};

use egui::text::{LayoutJob, TextFormat};
use egui::{Checkbox, DragValue, Id, Label, RichText, Sense, TextStyle, Ui};
use mop_core::format::group_thousands;
use mop_core::industry::{Activity, JobSplit, Recipe, RecipeBook, TypeId};
use mop_core::make_or_buy::{Choice, Decision};
use mop_core::plan::{BuyLine, Plan, PlanProduct, Step};
use mop_core::reserve;
use mop_sde::Sde;
use mop_store::PlanRecord;

use super::costs::{self, Costs, PENDING};
use super::prices::age_text;
use super::state::{BuildPage, NO_REACTIONS, me_of, push_error, type_name};
use crate::status::Notice;
use crate::table::Table;
use crate::table::model::{Cell, Column, ColumnKind, Row};
use crate::theme::Style;
use crate::{INDUSTRY_AGE_SECS, Shared};

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
    /// The units the row takes from stock, split from the running jobs in its hover.
    own: u64,
    own_hover: Option<String>,
    jobs_text: String,
    jobs_hover: String,
    cost: Figure,
    decide: Figure,
}

/// The text of a Job cost or Make or buy cell, its tone, its hover, and the change a click asks for.
#[derive(Debug, Default)]
struct Figure {
    text: String,
    tone: Tone,
    hover: String,
    click: Option<Edit>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Tone {
    #[default]
    Plain,
    Mute,
    Amber,
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
    let now = (cx.clock)();
    let assigned = match reserve::assigned_to(record.id, &cx.claims.running, &cx.claims.assignments)
    {
        Ok(assigned) => assigned,
        Err(error) => {
            push_error(
                &mut cx.notices,
                format!("Could not work out plan reservations: {error}"),
            );
            BTreeMap::new()
        }
    };
    let running = running_by_type(plan, &assigned);
    let costs = costs::compute(ui, page, record, plan, cx, now);
    let mut names = BTreeMap::new();
    let steps = costs
        .jobs
        .values()
        .filter_map(|outcome| outcome.as_ref().ok());
    for step in steps {
        for &type_id in &step.missing_prices {
            names
                .entry(type_id)
                .or_insert_with(|| type_name(type_id, cx.sde.as_ref(), &mut cx.notices));
        }
    }
    let lines = tree(plan, &page.recipes, &page.products);
    let views: BTreeMap<u64, View> = lines
        .iter()
        .map(|line| {
            let units = running.get(&line.type_id).copied().unwrap_or(0);
            (
                u64::from(line.type_id),
                view(line, record, &page.products, &costs, &names, units),
            )
        })
        .collect();
    let rows: Vec<Row> = lines
        .iter()
        .map(|line| {
            let units = running.get(&line.type_id).copied().unwrap_or(0);
            row(
                line,
                views.get(&u64::from(line.type_id)),
                units,
                cx.sde.as_ref(),
                &mut cx.notices,
            )
        })
        .collect();
    if let Some(error) = &costs.error {
        ui.label(
            RichText::new(format!("Could not work out the costs: {error}")).color(cx.style.neg),
        );
    }
    let columns = columns();
    let mut edits = Vec::new();
    let style = cx.style;
    let mut draw = |ui: &mut Ui, key: u64, column: &'static str| -> bool {
        let Some(view) = views.get(&key) else {
            return false;
        };
        match column {
            "build" => build_tick(ui, view, &mut edits),
            "me" => me_box(ui, view, &mut edits),
            "own" => own_cell(ui, view),
            "jobs" => jobs_label(ui, view),
            "cost" => figure_cell(ui, &view.cost, &style, &mut edits),
            "decide" => figure_cell(ui, &view.decide, &style, &mut edits),
            _ => false,
        }
    };
    let response = Table::new(Id::new(super::BuildPanel::Steps.title()), &columns)
        .show_with(ui, &cx.style, cx.density, &rows, None, &mut draw);
    if let Some(key) = response.clicked
        && let Ok(type_id) = TypeId::try_from(key)
    {
        cx.selection.item = Some(type_id);
    }
    total_line(ui, &costs, &cx.style);
    edits
}

/// A Job cost or Make or buy cell: its text in its tone, its hover, and its click. Returns
/// whether it drew the cell.
fn figure_cell(ui: &mut Ui, figure: &Figure, style: &Style, edits: &mut Vec<Edit>) -> bool {
    if figure.text.is_empty() {
        return false;
    }
    let color = match figure.tone {
        Tone::Plain => style.text,
        Tone::Mute => style.mute,
        Tone::Amber => style.stale,
    };
    let mut response =
        ui.add(Label::new(RichText::new(figure.text.as_str()).color(color)).sense(Sense::click()));
    if !figure.hover.is_empty() {
        response = response.on_hover_text(figure.hover.as_str());
    }
    if let Some(edit) = figure.click
        && response.clicked()
    {
        edits.push(edit);
    }
    true
}

/// The total under the table: the built steps' job costs, and the age of the data behind them.
fn total_line(ui: &mut Ui, costs: &Costs, style: &Style) {
    if costs.error.is_some() {
        return;
    }
    let (sum, incomplete) = costs.total();
    let sum_text = if costs.pending {
        PENDING.to_owned()
    } else {
        sum.map_or_else(|| "—".to_owned(), |money| money.format_isk())
    };
    let plain = TextFormat {
        color: ui.visuals().text_color(),
        font_id: TextStyle::Body.resolve(ui.style()),
        ..TextFormat::default()
    };
    let amber = TextFormat {
        color: style.stale,
        ..plain.clone()
    };
    let age_format = |seconds: Option<i64>| {
        if seconds.is_some_and(|seconds| seconds > INDUSTRY_AGE_SECS) {
            amber.clone()
        } else {
            plain.clone()
        }
    };
    let mut job = LayoutJob::default();
    job.append(
        &format!("Job cost {sum_text} · cost indices "),
        0.0,
        plain.clone(),
    );
    job.append(
        &age_label(costs.systems_age),
        0.0,
        age_format(costs.systems_age),
    );
    job.append(" · adjusted prices ", 0.0, plain.clone());
    job.append(
        &age_label(costs.prices_age),
        0.0,
        age_format(costs.prices_age),
    );
    if incomplete && !costs.pending {
        job.append(" incomplete", 0.0, amber);
    }
    ui.label(job);
}

/// How old a fetch is, or `not fetched`.
fn age_label(seconds: Option<i64>) -> String {
    seconds.map_or_else(|| "not fetched".to_owned(), age_text)
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

/// The Own cell, with the hover that splits the hangar units from the running jobs' units. Rows
/// with no running units are drawn by the table as usual.
fn own_cell(ui: &mut Ui, view: &View) -> bool {
    let Some(hover) = view.own_hover.as_deref() else {
        return false;
    };
    ui.add(Label::new(grouped(view.own)).selectable(false))
        .on_hover_text(hover);
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

/// The chain as a tree, in the order the rows show: each product at depth 0 in the page's order,
/// then its children in recipe order. A type is drawn once: a product is never drawn as a child,
/// and any other type already drawn is skipped.
fn tree<'a>(plan: &'a Plan, recipes: &'a RecipeBook, products: &[PlanProduct]) -> Vec<Line<'a>> {
    let steps: BTreeMap<TypeId, &Step> =
        plan.steps.iter().map(|step| (step.type_id, step)).collect();
    let buys: BTreeMap<TypeId, &BuyLine> =
        plan.buy.iter().map(|line| (line.type_id, line)).collect();
    let owners = owners(&plan.steps, recipes);
    let children = children(&plan.steps, recipes, &owners);
    let product_ids: BTreeSet<TypeId> = products.iter().map(|product| product.type_id).collect();
    let mut lines = Vec::new();
    let mut drawn = BTreeSet::new();
    let context = Tree {
        steps,
        buys,
        children,
        recipes,
    };
    for product in products {
        walk(
            product.type_id,
            0,
            &context,
            &product_ids,
            &mut drawn,
            &mut lines,
        );
    }
    lines
}

struct Tree<'a> {
    steps: BTreeMap<TypeId, &'a Step>,
    buys: BTreeMap<TypeId, &'a BuyLine>,
    children: BTreeMap<TypeId, Vec<TypeId>>,
    recipes: &'a RecipeBook,
}

/// The consumer each material is shown under: the built type with the lowest (depth, type id).
/// A built type that makes nothing consumes none of its materials, so its rows are not its own:
/// a material another step makes is shown under that step.
fn owners(steps: &[Step], recipes: &RecipeBook) -> BTreeMap<TypeId, TypeId> {
    let mut best: BTreeMap<TypeId, (u32, TypeId)> = BTreeMap::new();
    for step in steps {
        if step.to_build == 0 {
            continue;
        }
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

/// The materials each built type shows, in recipe order. A built type that makes nothing shows
/// none of its materials.
fn children(
    steps: &[Step],
    recipes: &RecipeBook,
    owners: &BTreeMap<TypeId, TypeId>,
) -> BTreeMap<TypeId, Vec<TypeId>> {
    let mut children: BTreeMap<TypeId, Vec<TypeId>> = BTreeMap::new();
    for step in steps {
        if step.to_build == 0 {
            continue;
        }
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

fn walk<'a>(
    type_id: TypeId,
    depth: u32,
    tree: &Tree<'a>,
    products: &BTreeSet<TypeId>,
    drawn: &mut BTreeSet<TypeId>,
    out: &mut Vec<Line<'a>>,
) {
    if !drawn.insert(type_id) {
        return;
    }
    let recipes = tree.recipes;
    out.push(Line {
        type_id,
        depth,
        step: tree.steps.get(&type_id).copied(),
        buy: tree.buys.get(&type_id).copied(),
        recipe: recipes.get(type_id),
    });
    for child in tree.children.get(&type_id).into_iter().flatten() {
        if products.contains(child) {
            continue;
        }
        walk(*child, depth + 1, tree, products, drawn, out);
    }
}

fn view(
    line: &Line,
    record: &PlanRecord,
    products: &[PlanProduct],
    costs: &Costs,
    names: &BTreeMap<TypeId, String>,
    running: u64,
) -> View {
    let is_product = is_product(products, line.type_id);
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
    let own = line
        .step
        .map(|step| step.from_stock)
        .or_else(|| line.buy.map(|buy| buy.from_stock))
        .unwrap_or(0);
    let own_hover = (running > 0).then(|| {
        format!(
            "{} in hangars, {running} from running jobs",
            own.saturating_sub(running)
        )
    });
    View {
        type_id: line.type_id,
        has_recipe: line.recipe.is_some(),
        ticked: !is_product && record.build.contains(&line.type_id),
        tick_enabled: !is_product && (!reaction || reactions_picked),
        tick_hover: (reaction && !reactions_picked && !is_product).then_some(NO_REACTIONS),
        me,
        own,
        own_hover,
        jobs_text: line
            .step
            .map(|step| grouped(step.jobs.job_count()))
            .unwrap_or_default(),
        jobs_hover: line
            .step
            .map(|step| jobs_hover(&step.jobs))
            .unwrap_or_default(),
        cost: cost_figure(line, costs, names),
        decide: decide_figure(line, products, costs),
    }
}

/// Whether a type is one of the plan's products.
fn is_product(products: &[PlanProduct], type_id: TypeId) -> bool {
    products.iter().any(|product| product.type_id == type_id)
}

/// The Job cost of a built step: the total, amber with ` incomplete` when an adjusted price is
/// missing, or the set-up message in mute.
fn cost_figure(line: &Line, costs: &Costs, names: &BTreeMap<TypeId, String>) -> Figure {
    let Some(outcome) = costs.jobs.get(&line.type_id) else {
        return Figure::default();
    };
    match outcome {
        Err(message) if message == PENDING => Figure {
            text: PENDING.to_owned(),
            ..Figure::default()
        },
        Err(message) => Figure {
            text: message.clone(),
            tone: Tone::Mute,
            ..Figure::default()
        },
        Ok(step) => {
            let cost = &step.cost;
            let mut figure = Figure {
                text: cost.total.format_isk(),
                hover: format!(
                    "item value {} · system cost {} · structure bonus -{} · facility tax {} · SCC {}",
                    cost.item_value.format_isk(),
                    cost.system_cost.format_isk(),
                    cost.structure_bonus.format_isk(),
                    cost.facility_tax.format_isk(),
                    cost.scc.format_isk(),
                ),
                ..Figure::default()
            };
            if !step.missing_prices.is_empty() {
                let listed: Vec<String> = step
                    .missing_prices
                    .iter()
                    .map(|type_id| {
                        names
                            .get(type_id)
                            .cloned()
                            .unwrap_or_else(|| type_id.to_string())
                    })
                    .collect();
                figure.text.push_str(" incomplete");
                figure.tone = Tone::Amber;
                figure.hover = format!(
                    "{} · No adjusted price: {}",
                    figure.hover,
                    listed.join(", ")
                );
            }
            figure
        }
    }
}

/// The Make or buy verdict. A product and a part without a recipe show nothing. A part whose
/// activity cannot be costed shows its set-up message in mute. A verdict the row does not match is
/// amber, and a click applies it.
fn decide_figure(line: &Line, products: &[PlanProduct], costs: &Costs) -> Figure {
    if is_product(products, line.type_id) || line.recipe.is_none() {
        return Figure::default();
    }
    if let Some(message) = costs.messages.get(&line.type_id) {
        return Figure {
            text: message.clone(),
            tone: Tone::Mute,
            ..Figure::default()
        };
    }
    match costs.decisions.get(&line.type_id) {
        Some(decision) => decision_figure(line, decision, costs.reasons.get(&line.type_id)),
        None => Figure::default(),
    }
}

fn decision_figure(line: &Line, decision: &Decision, reason: Option<&String>) -> Figure {
    let mut text = reason.cloned().unwrap_or_else(|| decision.reason_text());
    if !decision.job_cost_incomplete.is_empty() {
        text.push_str(" (job cost incomplete)");
    }
    match decision.choice {
        Choice::Unknown => Figure {
            text,
            tone: Tone::Mute,
            ..Figure::default()
        },
        Choice::Build | Choice::Buy => {
            let build = decision.choice == Choice::Build;
            if build == line.step.is_some() {
                return Figure {
                    text,
                    ..Figure::default()
                };
            }
            Figure {
                text,
                tone: Tone::Amber,
                hover: if build {
                    "Click to build this part".to_owned()
                } else {
                    "Click to buy this part".to_owned()
                },
                click: Some(Edit::Build(line.type_id, build)),
            }
        }
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

fn row(
    line: &Line,
    view: Option<&View>,
    running: u64,
    sde: Option<&Sde>,
    notices: &mut Vec<Notice>,
) -> Row {
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
            cells.push(blank_when_zero(running));
            cells.push(number(step.to_build));
            cells.push(Cell::text("—"));
            cells.push(number(step.runs));
            cells.push(number(step.jobs.job_count()));
            cells.push(blank_when_zero(step.spare));
        }
        (None, Some(buy)) => {
            cells.extend(amounts(buy.needed, buy.from_stock));
            cells.push(blank_when_zero(running));
            cells.push(Cell::text("—"));
            cells.push(number(buy.to_buy));
            cells.extend([Cell::text(""), Cell::text(""), Cell::text("")]);
        }
        (None, None) => {}
    }
    cells.push(Cell::text(""));
    cells.push(Cell::text(""));
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

/// A value of 0 shows nothing.
fn blank_when_zero(value: u64) -> Cell {
    if value == 0 {
        Cell::int(0, "")
    } else {
        number(value)
    }
}

/// The units per type the assigned jobs account for, capped at what each row takes from stock:
/// the running jobs are used before the hangar units. Types with no units are left out.
fn running_by_type(plan: &Plan, assigned: &BTreeMap<TypeId, u64>) -> BTreeMap<TypeId, u64> {
    let mut running: BTreeMap<TypeId, u64> = BTreeMap::new();
    for step in &plan.steps {
        if let Some(&units) = assigned.get(&step.type_id) {
            let units = units.min(step.from_stock);
            if units != 0 {
                running.insert(step.type_id, units);
            }
        }
    }
    for line in &plan.buy {
        if let Some(&units) = assigned.get(&line.type_id) {
            let units = units.min(line.from_stock);
            if units != 0 {
                running.insert(line.type_id, units);
            }
        }
    }
    running
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
        column("running", "Running", ColumnKind::Number, 70.0),
        column("make", "Make", ColumnKind::Number, 90.0),
        column("buy", "Buy", ColumnKind::Number, 90.0),
        column("runs", "Runs", ColumnKind::Number, 80.0),
        column("jobs", "Jobs", ColumnKind::Number, 80.0),
        column("spare", "Spare", ColumnKind::Number, 90.0),
        column("cost", "Job cost", ColumnKind::Number, 150.0),
        column("decide", "Make or buy", ColumnKind::Text, 330.0),
    ]
}

#[cfg(test)]
mod tests {
    use mop_core::industry::{Activity, JobSplit};

    use super::*;

    const HOBGOBLIN_I: TypeId = 2454;
    const TRITANIUM: TypeId = 34;

    /// Hobgoblin I with 2 of its units from stock, and a Tritanium line with no units from stock.
    fn plan() -> Plan {
        Plan {
            steps: vec![Step {
                type_id: HOBGOBLIN_I,
                activity: Activity::Manufacturing,
                depth: 0,
                needed: 2,
                from_stock: 2,
                to_build: 0,
                runs: 0,
                jobs: JobSplit {
                    full_jobs: 0,
                    runs_per_full_job: 1,
                    last_job_runs: 0,
                },
                spare: 0,
            }],
            buy: vec![BuyLine {
                type_id: TRITANIUM,
                depth: 1,
                needed: 3,
                from_stock: 0,
                to_buy: 3,
            }],
            unused_stock: Vec::new(),
        }
    }

    #[test]
    fn running_is_the_assigned_units_up_to_what_the_row_takes_from_stock() {
        let plan = plan();
        let assigned = BTreeMap::from([(HOBGOBLIN_I, 5), (TRITANIUM, 3)]);
        assert_eq!(
            running_by_type(&plan, &assigned),
            BTreeMap::from([(HOBGOBLIN_I, 2)])
        );
        let assigned = BTreeMap::from([(HOBGOBLIN_I, 1)]);
        assert_eq!(
            running_by_type(&plan, &assigned),
            BTreeMap::from([(HOBGOBLIN_I, 1)])
        );
        assert_eq!(running_by_type(&plan, &BTreeMap::new()), BTreeMap::new());
    }

    const HOBGOBLIN_II: TypeId = 2456;
    const RIFTER: TypeId = 587;
    const ROBOTICS: TypeId = 11_481;

    fn recipe(activity: Activity, output: u64, max: u64, materials: &[(TypeId, u64)]) -> Recipe {
        Recipe {
            activity,
            output_per_run: output,
            max_runs_per_job: max,
            materials: materials.to_vec(),
            made_by_invention: false,
        }
    }

    /// The recipes of the acceptance's products and their materials, from SDE build 3607705.
    fn game_book() -> RecipeBook {
        let mut book = RecipeBook::new();
        book.insert(
            HOBGOBLIN_II,
            recipe(Activity::Manufacturing, 1, 10, &[(ROBOTICS, 1)]),
        );
        book.insert(
            ROBOTICS,
            recipe(
                Activity::Manufacturing,
                100,
                200,
                &[(34, 556), (35, 444), (36, 222), (37, 82), (38, 36)],
            ),
        );
        book.insert(
            RIFTER,
            recipe(
                Activity::Manufacturing,
                1,
                30,
                &[(34, 32_000), (35, 6_000), (36, 2_500), (37, 500)],
            ),
        );
        book
    }

    fn step(
        type_id: TypeId,
        depth: u32,
        needed: u64,
        from_stock: u64,
        to_build: u64,
        runs: u64,
        spare: u64,
    ) -> Step {
        Step {
            type_id,
            activity: Activity::Manufacturing,
            depth,
            needed,
            from_stock,
            to_build,
            runs,
            jobs: JobSplit {
                full_jobs: 0,
                runs_per_full_job: runs,
                last_job_runs: runs,
            },
            spare,
        }
    }

    fn buy(type_id: TypeId, depth: u32, needed: u64, from_stock: u64, to_buy: u64) -> BuyLine {
        BuyLine {
            type_id,
            depth,
            needed,
            from_stock,
            to_buy,
        }
    }

    /// The acceptance's plan, holding R.A.M.- Robotics and a Rifter with 1,000 Tritanium in stock.
    fn products_plan() -> Plan {
        Plan {
            steps: vec![
                step(RIFTER, 0, 2, 0, 2, 2, 0),
                step(ROBOTICS, 0, 150, 0, 150, 2, 50),
            ],
            buy: vec![
                buy(34, 1, 58_601, 1_000, 57_601),
                buy(35, 1, 11_600, 0, 11_600),
                buy(36, 1, 4_900, 0, 4_900),
                buy(37, 1, 1_048, 0, 1_048),
                buy(38, 1, 65, 0, 65),
            ],
            unused_stock: Vec::new(),
        }
    }

    fn products() -> [PlanProduct; 2] {
        [
            PlanProduct {
                type_id: ROBOTICS,
                quantity: 150,
            },
            PlanProduct {
                type_id: RIFTER,
                quantity: 2,
            },
        ]
    }

    fn record() -> PlanRecord {
        PlanRecord {
            id: 1,
            name: "Test plan".to_owned(),
            product: HOBGOBLIN_II,
            quantity: 1,
            build: BTreeSet::new(),
            me: BTreeMap::new(),
            manufacturing: None,
            reactions: None,
            price_rule: Default::default(),
            route: None,
            service: None,
            packing: Default::default(),
            pasted_stock: String::new(),
            created_at: 0,
            updated_at: 0,
        }
    }

    #[test]
    fn products_are_roots_and_shared_materials_show_once() {
        let plan = products_plan();
        let book = game_book();
        let lines = tree(&plan, &book, &products());
        let types: Vec<TypeId> = lines.iter().map(|line| line.type_id).collect();
        // The shared materials sit under the lowest (depth, type id) consumer, the Rifter.
        assert_eq!(types, [ROBOTICS, 38, RIFTER, 34, 35, 36, 37]);

        assert_eq!(lines[0].depth, 0);
        assert!(lines[0].step.is_some());
        assert_eq!(lines[1].depth, 1);
        assert_eq!(lines[2].depth, 0);
        assert!(lines[2].step.is_some());
        assert!(lines[3..].iter().all(|line| line.depth == 1));

        assert_eq!(types.iter().filter(|&&type_id| type_id == 34).count(), 1);
        let tritanium = lines
            .iter()
            .find(|line| line.type_id == 34)
            .expect("the Tritanium line");
        let buy = tritanium.buy.expect("a bought line");
        assert_eq!(
            (buy.needed, buy.from_stock, buy.to_buy),
            (58_601, 1_000, 57_601)
        );
        let products = products();
        let costs = Costs::default();
        let names = BTreeMap::new();
        let record = record();
        let tritanium_view = view(tritanium, &record, &products, &costs, &names, 0);
        let mut notices = Vec::new();
        let tritanium_row = row(tritanium, Some(&tritanium_view), 0, None, &mut notices);
        assert_eq!(
            tritanium_row.cells[3..=4]
                .iter()
                .map(|cell| cell.text.as_str())
                .collect::<Vec<_>>(),
            ["58,601", "1,000"]
        );
        assert_eq!(tritanium_row.cells[7].text, "57,601");
        for type_id in [35, 36, 37, 38] {
            assert_eq!(
                lines.iter().filter(|line| line.type_id == type_id).count(),
                1
            );
        }
    }

    #[test]
    fn a_product_is_never_drawn_as_a_child() {
        let plan = Plan {
            steps: vec![
                step(HOBGOBLIN_II, 0, 10, 0, 10, 10, 0),
                step(ROBOTICS, 0, 150, 0, 150, 2, 50),
            ],
            buy: Vec::new(),
            unused_stock: Vec::new(),
        };
        let products = [
            PlanProduct {
                type_id: HOBGOBLIN_II,
                quantity: 10,
            },
            PlanProduct {
                type_id: ROBOTICS,
                quantity: 150,
            },
        ];
        let book = game_book();
        let lines = tree(&plan, &book, &products);
        let types: Vec<TypeId> = lines.iter().map(|line| line.type_id).collect();
        assert_eq!(types, [HOBGOBLIN_II, ROBOTICS, 34, 35, 36, 37, 38]);
        assert_eq!(lines[1].depth, 0);
        assert_eq!(
            types.iter().filter(|&&type_id| type_id == ROBOTICS).count(),
            1
        );
    }

    #[test]
    fn a_one_product_plan_keeps_its_rows() {
        let mut plan = products_plan();
        plan.steps.remove(0);
        let book = game_book();
        let lines = tree(
            &plan,
            &book,
            &[PlanProduct {
                type_id: ROBOTICS,
                quantity: 150,
            }],
        );
        let types: Vec<TypeId> = lines.iter().map(|line| line.type_id).collect();
        assert_eq!(types, [ROBOTICS, 34, 35, 36, 37, 38]);
        assert_eq!(lines[0].depth, 0);
        assert!(lines[1..].iter().all(|line| line.depth == 1));
    }

    #[test]
    fn every_product_has_no_clickable_tick_and_no_make_or_buy() {
        let plan = Plan {
            steps: vec![
                step(HOBGOBLIN_II, 0, 10, 0, 10, 10, 0),
                step(RIFTER, 0, 1, 0, 1, 1, 0),
            ],
            buy: Vec::new(),
            unused_stock: Vec::new(),
        };
        let products = [
            PlanProduct {
                type_id: HOBGOBLIN_II,
                quantity: 10,
            },
            PlanProduct {
                type_id: RIFTER,
                quantity: 1,
            },
        ];
        let mut costs = Costs::default();
        costs
            .messages
            .insert(HOBGOBLIN_II, "no facility".to_owned());
        costs.messages.insert(RIFTER, "no facility".to_owned());
        let names = BTreeMap::new();
        let record = record();
        let book = game_book();
        let lines = tree(&plan, &book, &products);

        for product in &products {
            let line = lines
                .iter()
                .find(|line| line.type_id == product.type_id)
                .expect("the product's row");
            let view = view(line, &record, &products, &costs, &names, 0);
            assert!(view.has_recipe);
            assert!(!view.ticked);
            assert!(!view.tick_enabled);
            assert!(view.decide.text.is_empty());
        }

        let material = lines
            .iter()
            .find(|line| line.type_id == ROBOTICS)
            .expect("a material with a recipe");
        let view = view(material, &record, &products, &costs, &names, 0);
        assert!(view.tick_enabled);
    }
}

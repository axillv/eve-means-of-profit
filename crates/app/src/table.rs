//! The table widget: a header that sorts, rows that scroll without laying out every row, tree rows
//! that fold, a footer, and custom cells. What it shows is worked out in [`model`].

pub mod model;

use std::collections::{BTreeMap, BTreeSet};
use std::iter;

use egui::epaint::PathStroke;
use egui::{
    Align, Color32, CornerRadius, Id, Label, Layout, Pos2, Rect, RichText, ScrollArea, Sense,
    Shape, TextStyle, TextWrapMode, Ui, UiBuilder, WidgetInfo, WidgetText, WidgetType, pos2, vec2,
};
use mop_store::Density;

use crate::Shared;
use crate::status::Notice;
use crate::theme::Style;
use model::{
    Cell, Column, ColumnKind, Direction, Row, cell_values, changed_cells, frozen_order, moved_rows,
    row_height, sort_order, span, to_tsv, visible,
};

/// The memory key that asks the table with this id to copy itself as TSV. The panel's `Copy table`
/// button sets it; the table clears it when it copies. A panel's table takes the panel's id, so the
/// button finds it.
pub fn copy_request(table: Id) -> Id {
    table.with("copy as TSV")
}

/// The memory key that says the panel with this id has a table. The panel header reads it to show
/// its copy button; a table sets it every frame it draws. A panel's table takes the panel's id, so
/// the header finds it.
pub fn has_table(panel: Id) -> Id {
    panel.with("has a table")
}

/// The memory key of how many rows the table of this panel moved since it kept its order. The table
/// writes it every frame; the panel header reads it to show its re-sort button.
pub fn rows_moved(panel: Id) -> Id {
    panel.with("rows moved")
}

/// The memory key that asks the table of this panel to sort at once with the new values. The
/// panel's `Re-sort` button sets it; the table clears it.
pub fn resort_request(panel: Id) -> Id {
    panel.with("re-sort")
}

/// The key of the counter that asks every table to sort at once.
fn resort_all_key() -> Id {
    Id::new("table resort")
}

/// Asks every table to sort at once with the values it shows. A refresh the player starts calls
/// this, so the tables replace their kept order instead of freezing it.
pub fn resort_all(ctx: &egui::Context) {
    let count = ctx.data(|data| data.get_temp::<u64>(resort_all_key()).unwrap_or(0));
    ctx.data_mut(|data| {
        data.insert_temp(resort_all_key(), count.saturating_add(1));
    });
}

/// The hidden column ids in `text`: the parts between commas, without empty parts.
pub fn hidden_from_text(text: &str) -> Vec<String> {
    text.split(',')
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The text to save `hidden` as: the ids joined with commas.
pub fn hidden_to_text(hidden: &[String]) -> String {
    hidden.join(",")
}

/// The level the table with `id` was left at, or `start` before the table has drawn with one.
pub fn level(ui: &Ui, id: Id, start: Level) -> Level {
    ui.data(|data| data.get_temp::<State>(id).and_then(|state| state.level))
        .unwrap_or(start)
}

/// Sets the level of the table with `id`; the table takes it up the next time it draws.
pub fn set_level(ui: &mut Ui, id: Id, level: Level) {
    ui.data_mut(|data| {
        data.get_temp_mut_or_default::<State>(id).level = Some(level);
    });
}

/// The level choices of a Columns menu, with `current` selected. Returns the one clicked.
pub fn level_choices(ui: &mut Ui, current: Level) -> Option<Level> {
    let mut picked = None;
    for level in Level::ALL {
        if ui
            .selectable_label(current == level, level.label())
            .clicked()
        {
            picked = Some(level);
        }
    }
    picked
}

/// How many leading columns show before a `+N columns` cell: all of them when they fit the panel,
/// else the first and every further one that still leaves room for the `+N` cell.
fn fit_count(columns: &[Column], panel_width: f32) -> usize {
    let total: f32 = columns.iter().map(|column| column.width).sum();
    if total <= panel_width {
        return columns.len();
    }
    let mut used = 0.0;
    let mut count = 0;
    for column in columns {
        if count == 0 || used + column.width <= panel_width - MORE_WIDTH {
            used += column.width;
            count += 1;
        } else {
            break;
        }
    }
    count
}

/// The columns to draw, the width to lay them out in, and how many are hidden behind a `+N` cell.
fn visible_columns(
    columns: &[Column],
    level: Level,
    panel_width: f32,
) -> (&[Column], f32, Option<usize>) {
    match level {
        Level::Technical => {
            let total: f32 = columns.iter().map(|column| column.width).sum();
            (columns, panel_width.max(total), None)
        }
        Level::Summary | Level::Standard => {
            let count = fit_count(columns, panel_width);
            let more = count < columns.len();
            (
                columns.get(..count).unwrap_or(columns),
                panel_width,
                more.then_some(columns.len() - count),
            )
        }
    }
}

/// The level this frame draws with: the kept level, else `start`, else Technical. A change clears
/// the sort, whose index belongs to the columns of the old level.
fn keep_level(state: &mut State, start: Option<Level>) -> Level {
    let level = state.level.unwrap_or(start.unwrap_or(Level::Technical));
    if state.level.is_some_and(|previous| previous != level) {
        state.sort = None;
    }
    state.level = Some(level);
    level
}

/// Drops a sort on a column Summary or Standard hides: it would order the rows by a column that is
/// not drawn and has no header mark. A page hiding a column only rebuilds its own list, so
/// Technical keeps the sort it was given.
fn drop_hidden_sort(state: &mut State, level: Level, drawn: usize) {
    if matches!(level, Level::Summary | Level::Standard)
        && state.sort.is_some_and(|(position, _)| position >= drawn)
    {
        state.sort = None;
    }
}

/// Applies a header click and reports whether the sort may be saved.
fn apply_header_click(
    state: &mut State,
    click: Option<HeaderClick>,
    level: Level,
    ui: &Ui,
) -> bool {
    match click {
        Some(HeaderClick::Sort) => {
            // The player picked a sort, so it takes effect now, with no kept order and no mark.
            state.frozen = None;
            state.changed.clear();
            state.resort_pending = false;
            true
        }
        Some(HeaderClick::More) => {
            // The player asked for the hidden columns: raise the level and redraw at once.
            state.sort = None;
            state.level = Some(level.next());
            ui.ctx().request_repaint();
            false
        }
        None => true,
    }
}

/// The text to save `sort` as: the column id and `asc` or `desc`, such as `need:desc`. Empty when
/// there is no sort, or its column is past the end of the table.
pub fn sort_to_text(columns: &[Column], sort: Option<(usize, Direction)>) -> String {
    let Some((position, direction)) = sort else {
        return String::new();
    };
    let Some(column) = columns.get(position) else {
        return String::new();
    };
    let direction = match direction {
        Direction::Ascending => "asc",
        Direction::Descending => "desc",
    };
    format!("{}:{direction}", column.id)
}

/// The sort named by `text`, such as `need:desc`, read by column id. A malformed text, or one
/// naming a column the table does not have, is no sort.
pub fn sort_from_text(columns: &[Column], text: &str) -> Option<(usize, Direction)> {
    let (id, direction) = text.split_once(':')?;
    let direction = match direction {
        "asc" => Direction::Ascending,
        "desc" => Direction::Descending,
        _ => return None,
    };
    let position = columns.iter().position(|column| column.id == id)?;
    Some((position, direction))
}

/// The setting key of the saved sort of the table with id `table`.
fn sort_key(table: u64) -> String {
    format!("table_sort.{table:016x}")
}

/// The sorts travelling between the tables and [`sync_sorts`], which owns the store: the ids still
/// waiting for their saved answer, the answers, and the sorts that changed and must be saved.
#[derive(Debug, Default, Clone)]
struct SortMailbox {
    wanted: BTreeSet<u64>,
    saved: BTreeMap<u64, String>,
    changed: BTreeMap<u64, String>,
}

fn mailbox_key() -> Id {
    Id::new("table sorts")
}

fn mailbox(ui: &Ui) -> SortMailbox {
    ui.data(|data| {
        data.get_temp::<SortMailbox>(mailbox_key())
            .unwrap_or_default()
    })
}

fn put_mailbox(ui: &Ui, mailbox: SortMailbox) {
    ui.data_mut(|data| data.insert_temp(mailbox_key(), mailbox));
}

/// Answers the tables' questions about their saved sorts and saves the ones that changed. Called
/// once at the end of each frame, after the pages have drawn. Without an app (as in the table's own
/// tests) nothing calls this, so the tables keep their sort for the run only.
pub(crate) fn sync_sorts(ctx: &egui::Context, cx: &mut Shared) {
    let Some(mut mailbox) = ctx.data(|data| data.get_temp::<SortMailbox>(mailbox_key())) else {
        return;
    };
    for table in std::mem::take(&mut mailbox.wanted) {
        let answer = match cx.store.setting(&sort_key(table)) {
            Ok(Some(text)) => text,
            Ok(None) => String::new(),
            Err(error) => {
                cx.notices.push(Notice::error(format!(
                    "Could not read the settings: {error}"
                )));
                String::new()
            }
        };
        mailbox.saved.insert(table, answer);
        ctx.request_repaint();
    }
    for (table, text) in std::mem::take(&mut mailbox.changed) {
        if let Err(error) = cx.store.set_setting(&sort_key(table), &text) {
            cx.notices.push(Notice::error(format!(
                "Could not save the settings: {error}"
            )));
        }
    }
    ctx.data_mut(|data| data.insert_temp(mailbox_key(), mailbox));
}

/// The width of the fold arrow and the indent per depth, in points.
const ARROW_WIDTH: f32 = 14.0;
/// The gap between a tree's fold arrow and the cell text after it.
const ARROW_GAP: f32 = 4.0;
const INDENT: f32 = 14.0;
/// How far a sort triangle sits in from the edge of its header cell.
const MARK_INSET: f32 = 8.0;
/// The room left at each side of a cell's text, so a number does not touch the next column's text.
const CELL_PAD: f32 = 8.0;

/// The header row's height, the same at every density.
const HEADER_HEIGHT: f32 = 28.0;
/// The width of the `+N columns` cell that says how many columns are hidden.
const MORE_WIDTH: f32 = 90.0;

/// How many of a table's columns are shown: the few of Summary, the ones of Standard that fit, or
/// every one of Technical.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Level {
    Summary,
    Standard,
    Technical,
}

impl Level {
    /// Every level, from the fewest columns to the most.
    pub const ALL: [Level; 3] = [Level::Summary, Level::Standard, Level::Technical];

    /// The name shown in the Columns menu.
    pub fn label(self) -> &'static str {
        match self {
            Level::Summary => "Summary",
            Level::Standard => "Standard",
            Level::Technical => "Technical",
        }
    }

    /// The next level; Technical is the last.
    pub fn next(self) -> Level {
        match self {
            Level::Summary => Level::Standard,
            Level::Standard | Level::Technical => Level::Technical,
        }
    }
}

/// What the table reports from one frame.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TableResponse {
    /// A row was clicked somewhere other than its arrow.
    pub clicked: Option<u64>,
    /// A row's arrow was clicked, so its children were folded or unfolded.
    pub toggled: Option<u64>,
}

/// What the table keeps between frames, under its id.
#[derive(Debug, Default, Clone)]
struct State {
    sort: Option<(usize, Direction)>,
    /// Whether the sort is the player's or the saved one: until then a table waits for its answer.
    sort_loaded: bool,
    /// The level the player last chose, once the table has drawn with one.
    level: Option<Level>,
    /// Whether the start fields have been applied.
    started: bool,
    collapsed: BTreeSet<u64>,
    /// The row keys in the order kept from before a value changed, until a re-sort.
    frozen: Option<Vec<u64>>,
    /// The row keys of the order the last frame showed.
    last: Vec<u64>,
    /// Each cell's plain value in the last frame, by row key and column.
    seen: BTreeMap<(u64, usize), String>,
    /// The cells whose plain value changed while the order was kept.
    changed: BTreeSet<(u64, usize)>,
    /// The column ids of the last frame, to notice a hidden or restored column.
    column_ids: Vec<&'static str>,
    /// The value of the resort counter the table has already obeyed.
    resort_seen: u64,
    /// Whether the next changed value must sort at once instead of keeping the order.
    resort_pending: bool,
}

/// Takes the saved sort from the mailbox, or asks for it, until the table has an answer.
fn load_sort(ui: &Ui, columns: &[Column], id: Id, state: &mut State) {
    if state.sort_loaded {
        return;
    }
    // Without an app nothing answers, so the table waits for the rest of the run and keeps the sort
    // it was given.
    let table = id.value();
    let mut mailbox = mailbox(ui);
    match mailbox.saved.remove(&table) {
        Some(text) => {
            // An empty saved sort, or one naming a column the table does not have, leaves the
            // table's own sort (the start sort) in place.
            if let Some(sort) = sort_from_text(columns, &text) {
                state.sort = Some(sort);
            }
            state.sort_loaded = true;
        }
        None => {
            mailbox.wanted.insert(table);
        }
    }
    put_mailbox(ui, mailbox);
}

/// Clears the kept order and the marks a hidden column or a refresh the player started makes
/// meaningless.
fn notice_changes(ui: &Ui, columns: &[Column], panel: Id, state: &mut State) {
    // A hidden or restored column shifts every cell position, so the old values mean nothing.
    let column_ids: Vec<&'static str> = columns.iter().map(|column| column.id).collect();
    if state.column_ids != column_ids {
        state.column_ids = column_ids;
        state.seen.clear();
        state.changed.clear();
    }
    // A refresh the player started asks every table to sort at once; the panel's re-sort button
    // asks one table to.
    let resort = ui.data(|data| data.get_temp::<u64>(resort_all_key()).unwrap_or(0));
    if resort > state.resort_seen {
        state.resort_seen = resort;
        state.frozen = None;
        state.changed.clear();
        state.resort_pending = true;
    }
    if ui
        .data_mut(|data| data.remove_temp::<bool>(resort_request(panel)))
        .unwrap_or(false)
    {
        state.frozen = None;
        state.changed.clear();
        state.resort_pending = false;
    }
}

/// The order to show this frame and how many rows it moved from the kept order, after marking the
/// cells whose value changed. `fresh` is the order the sort gives now.
fn arrange(rows: &[Row], fresh: Vec<usize>, state: &mut State) -> (Vec<usize>, usize) {
    let values = cell_values(rows);
    let now_changed = changed_cells(rows, &state.seen);
    if !now_changed.is_empty() {
        if state.resort_pending {
            // The player started this refresh, so it sorts at once instead of keeping the order.
            state.resort_pending = false;
        } else if state.frozen.is_none() && !state.last.is_empty() {
            // The first change since the last sort keeps the order the player was reading.
            state.frozen = Some(state.last.clone());
        }
        if state.frozen.is_some() {
            state.changed.extend(now_changed);
        }
    }
    state.seen = values;

    let frozen = state.frozen.take();
    let (order, moved) = match &frozen {
        Some(frozen) => match frozen_order(rows, &fresh, frozen) {
            Some(order) => {
                let moved = moved_rows(rows, &fresh, frozen);
                state.frozen = Some(keys_of(rows, &order));
                (order, moved)
            }
            None => {
                // A new child row has no place in the kept order, so sort at once.
                state.changed.clear();
                (fresh, 0)
            }
        },
        None => {
            state.changed.clear();
            (fresh, 0)
        }
    };
    state.last = keys_of(rows, &order);
    (order, moved)
}

/// What a row click did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hit {
    Clicked(u64),
    Toggled(u64),
}

/// The column whose cells get an icon, and the type id of a row's picture.
type Icon<'a> = (&'static str, &'a dyn Fn(u64) -> Option<u32>);

/// What every row needs to be drawn.
#[derive(Clone, Copy)]
struct Look<'a> {
    id: Id,
    columns: &'a [Column],
    style: &'a Style,
    height: f32,
    selected: Option<u64>,
    collapsed: &'a BTreeSet<u64>,
    changed: &'a BTreeSet<(u64, usize)>,
    icons: Option<Icon<'a>>,
}

/// A table of `rows` under `columns`. Pass the same `id` every frame: the sort and the folds are kept under it.
pub struct Table<'a> {
    id: Id,
    panel: Id,
    columns: &'a [Column],
    selected: Option<u64>,
    fit_rows: Option<usize>,
    icons: Option<Icon<'a>>,
    start_folded: BTreeSet<u64>,
    start_sort: Option<(usize, Direction)>,
    expanded: bool,
    level: Option<Level>,
}

impl<'a> Table<'a> {
    pub fn new(id: Id, columns: &'a [Column]) -> Self {
        Self {
            id,
            panel: id,
            columns,
            selected: None,
            fit_rows: None,
            icons: None,
            start_folded: BTreeSet::new(),
            start_sort: None,
            expanded: false,
            level: None,
        }
    }

    /// The level the table starts at, until the player picks another. Without this every column is
    /// shown, scrolls sideways and never gets a `+N columns` cell.
    pub fn level(mut self, level: Level) -> Self {
        self.level = Some(level);
        self
    }

    /// The folds and the sort the table starts with, until the player changes them. A saved sort
    /// replaces the start sort when it names a column; otherwise the start sort stays.
    pub fn start(mut self, folded: BTreeSet<u64>, sort: Option<(usize, Direction)>) -> Self {
        self.start_folded = folded;
        self.start_sort = sort;
        self
    }

    /// Shows every row, as when a search shows the ancestors of its matches: the folds are neither
    /// used nor changed, so clearing the search brings the player's folds back.
    pub fn expanded(mut self, expanded: bool) -> Self {
        self.expanded = expanded;
        self
    }

    /// Seeds the table's folds and sort once: after that the player's folds and sort own it.
    fn seed_start(&self, state: &mut State) {
        if state.started {
            return;
        }
        state.collapsed = self.start_folded.clone();
        state.sort = self.start_sort;
        state.started = true;
    }

    /// Draws the item icon of each row before the text of `column`. `icon` reads the row key and
    /// gives the type id of its picture, or none when the row has no icon.
    pub fn icons(mut self, column: &'static str, icon: &'a dyn Fn(u64) -> Option<u32>) -> Self {
        self.icons = Some((column, icon));
        self
    }

    /// Tells the table which panel it belongs to, when the panel draws more than one table: the
    /// panel's header reads `has_table(panel)` and asks for a copy under `copy_request(panel)`.
    /// Defaults to the table's own id.
    pub fn panel(mut self, panel: Id) -> Self {
        self.panel = panel;
        self
    }

    /// Makes the table only as tall as its rows, up to `rows` of them, instead of taking all the
    /// height the panel has left. A panel with a form under or over the table needs this.
    pub fn fit_rows(mut self, rows: usize) -> Self {
        self.fit_rows = Some(rows);
        self
    }

    /// The row to draw as selected.
    pub fn selected(mut self, key: Option<u64>) -> Self {
        self.selected = key;
        self
    }

    pub fn show(
        self,
        ui: &mut Ui,
        style: &Style,
        density: Density,
        rows: &[Row],
        footer: Option<&[Cell]>,
    ) -> TableResponse {
        self.show_with(ui, style, density, rows, footer, &mut |_, _, _| false)
    }

    /// Like [`Table::show`], but `draw` is called for each visible cell first, with the row key and
    /// the column id. When it returns true it has drawn the cell, and the table draws nothing there.
    pub fn show_with(
        self,
        ui: &mut Ui,
        style: &Style,
        density: Density,
        rows: &[Row],
        footer: Option<&[Cell]>,
        draw: &mut dyn FnMut(&mut Ui, u64, &'static str) -> bool,
    ) -> TableResponse {
        let panel_width = ui.available_width();
        let panel_height = ui.available_height();
        // The kept level, else the one the table was built with: Summary and Standard fit the
        // panel, while Technical and a table with no level scroll sideways instead.
        let kept = ui.data(|data| {
            data.get_temp::<State>(self.id)
                .and_then(|state| state.level)
        });
        let scroll = match kept.or(self.level) {
            Some(Level::Summary | Level::Standard) => false,
            Some(Level::Technical) | None => true,
        };
        let id = self.id;
        let show = |ui: &mut Ui| {
            self.show_inner(
                ui,
                (panel_width, panel_height),
                style,
                density,
                rows,
                footer,
                draw,
            )
        };
        if scroll {
            ScrollArea::horizontal()
                .id_salt(id.with("sideways"))
                .auto_shrink([false, true])
                .show(ui, show)
                .inner
        } else {
            show(ui)
        }
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "the arguments of `show_with`, plus the panel size taken before the scroll area"
    )]
    fn show_inner(
        self,
        ui: &mut Ui,
        (panel_width, panel_height): (f32, f32),
        style: &Style,
        density: Density,
        rows: &[Row],
        footer: Option<&[Cell]>,
        draw: &mut dyn FnMut(&mut Ui, u64, &'static str) -> bool,
    ) -> TableResponse {
        // The panel header reads this to show its copy button, so it is set every frame the table draws.
        ui.data_mut(|data| data.insert_temp(has_table(self.panel), true));
        let mut state: State =
            ui.data_mut(|data| data.get_temp_mut_or_default::<State>(self.id).clone());
        self.seed_start(&mut state);
        let table = self.id.value();
        load_sort(ui, self.columns, self.id, &mut state);
        let level = keep_level(&mut state, self.level);
        let height = row_height(density);
        let mut response = TableResponse::default();

        let (columns, width, more) = visible_columns(self.columns, level, panel_width);
        // A saved or start sort may name a column the level hides.
        drop_hidden_sort(&mut state, level, columns.len());
        let sort_before = state.sort;
        notice_changes(ui, self.columns, self.panel, &mut state);

        let (header, _) = ui.allocate_exact_size(vec2(width, HEADER_HEIGHT), Sense::hover());
        let click = draw_header(ui, self.id, columns, more, header, style, &mut state.sort);
        let save_sort = apply_header_click(&mut state, click, level, ui);
        // The hairline under the header takes its own point, so no data row loses one.
        horizontal(ui, header.min.x, header.max.x, header.max.y, style.line);
        // The header allocation already left the item spacing after it: take it back, then leave
        // exactly the one point of the hairline.
        ui.add_space(1.0 - ui.spacing().item_spacing.y);

        let (column, direction) = state
            .sort
            .map_or((None, Direction::Ascending), |(column, direction)| {
                (Some(column), direction)
            });
        let fresh = sort_order(rows, column, direction);
        let (order, moved) = arrange(rows, fresh, &mut state);
        // An expanded table, such as a search's ancestors, shows every row whatever the folds.
        let no_folds = BTreeSet::new();
        let collapsed = if self.expanded {
            &no_folds
        } else {
            &state.collapsed
        };
        let shown = visible(rows, &order, collapsed);
        copy_if_asked(ui, self.panel, self.columns, rows, &shown, footer);

        let look = Look {
            id: self.id,
            columns,
            style,
            height,
            selected: self.selected,
            collapsed,
            changed: &state.changed,
            icons: self.icons,
        };
        // The footer is drawn below the scroll area, so the rows leave room for it.
        let footer_height = if footer.is_some() { height } else { 0.0 };
        let mut body = (panel_height - footer_height - HEADER_HEIGHT - 1.0).max(0.0);
        if let Some(fit) = self.fit_rows {
            // A table with no rows keeps one row of room, so it does not vanish.
            let wanted = shown.len().clamp(1, fit.max(1));
            // Past 65,535 rows the table is as tall as the panel allows anyway.
            let content = f32::from(u16::try_from(wanted).unwrap_or(u16::MAX)) * height;
            body = body.min(content);
        }
        ScrollArea::vertical()
            .id_salt(self.id)
            .auto_shrink([false, false])
            .max_height(body)
            .show_viewport(ui, |ui, viewport| {
                let view = span(viewport.min.y, viewport.height(), height, shown.len());
                ui.set_height(view.total);
                let origin = ui.max_rect().min;
                let row_width = ui.max_rect().width();
                let tops = iter::successors(Some(origin.y + view.top), |top| Some(top + height));
                for (position, top) in view.rows.zip(tops) {
                    let Some(&index) = shown.get(position) else {
                        continue;
                    };
                    let rect = Rect::from_min_size(pos2(origin.x, top), vec2(row_width, height));
                    let hit = draw_row(look, ui, rows, index, rect, draw);
                    match hit {
                        Some(Hit::Clicked(key)) => response.clicked = Some(key),
                        Some(Hit::Toggled(key)) => response.toggled = Some(key),
                        None => {}
                    }
                }
            });

        if let Some(cells) = footer {
            let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
            draw_footer(ui, columns, cells, rect, style);
        }

        // Unfold the row if it was folded, else fold it. An expanded table keeps its folds for
        // when it shrinks again.
        if let Some(key) = response.toggled
            && !self.expanded
            && !state.collapsed.remove(&key)
        {
            state.collapsed.insert(key);
        }
        if save_sort && state.sort != sort_before {
            // The saved sort, or the player's click, is the sort from now on; the mailbox saves it.
            state.sort_loaded = true;
            let mut mailbox = mailbox(ui);
            mailbox
                .changed
                .insert(table, sort_to_text(self.columns, state.sort));
            put_mailbox(ui, mailbox);
        }
        if response.toggled.is_some() || state.sort != sort_before {
            ui.ctx().request_repaint();
        }
        // The panel header reads this next frame to show its re-sort button.
        ui.data_mut(|data| data.insert_temp(rows_moved(self.panel), moved));
        ui.data_mut(|data| data.insert_temp(self.id, state));
        response
    }
}

/// Copies the shown rows when the panel's copy button asked for it; the text is built only then.
fn copy_if_asked(
    ui: &mut Ui,
    panel: Id,
    columns: &[Column],
    rows: &[Row],
    shown: &[usize],
    footer: Option<&[Cell]>,
) {
    if ui
        .data_mut(|data| data.remove_temp::<bool>(copy_request(panel)))
        .unwrap_or(false)
    {
        ui.ctx().copy_text(to_tsv(columns, rows, shown, footer));
    }
}

/// The row keys of `order`, skipping an index past the rows.
fn keys_of(rows: &[Row], order: &[usize]) -> Vec<u64> {
    order
        .iter()
        .filter_map(|&index| rows.get(index).map(|row| row.key))
        .collect()
}

/// The rectangle of each column in a row at `rect`, left to right.
fn cell_rects(columns: &[Column], rect: Rect) -> Vec<Rect> {
    let mut x = rect.min.x;
    columns
        .iter()
        .map(|column| {
            let cell = Rect::from_min_size(pos2(x, rect.min.y), vec2(column.width, rect.height()));
            x += column.width;
            cell
        })
        .collect()
}

/// A label that takes no clicks, so the click area under it still gets the click.
fn plain_label(ui: &mut Ui, text: &str) {
    ui.add(Label::new(text).selectable(false));
}

/// A one-point line across the table from `left` to `right` at `top`.
fn horizontal(ui: &Ui, left: f32, right: f32, top: f32, color: Color32) {
    ui.painter().rect_filled(
        Rect::from_min_max(pos2(left, top), pos2(right, top + 1.0)),
        CornerRadius::ZERO,
        color,
    );
}

/// A header title: small, strong and grey, and as unclickable as a cell label. It takes the whole
/// cell, so its label is the column rather than just the width of its letters.
fn header_label(ui: &mut Ui, text: &str, style: &Style) {
    let rect = ui.available_rect_before_wrap();
    let response = ui.allocate_rect(rect, Sense::hover());
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Label, ui.is_enabled(), text));
    let title = RichText::new(text).small().strong().color(style.mute);
    let galley = WidgetText::from(title).into_galley(
        ui,
        Some(TextWrapMode::Extend),
        f32::INFINITY,
        TextStyle::Small,
    );
    let x = match ui.layout().horizontal_placement() {
        Align::Min => rect.min.x,
        Align::Center => rect.center().x - galley.size().x / 2.0,
        Align::Max => rect.max.x - galley.size().x,
    };
    let y = rect.center().y - galley.size().y / 2.0;
    ui.painter().galley(pos2(x, y), galley, style.mute);
}

fn layout_for(kind: ColumnKind) -> Layout {
    match kind {
        ColumnKind::Text => Layout::left_to_right(Align::Center),
        ColumnKind::Number | ColumnKind::Money => Layout::right_to_left(Align::Center),
    }
}

/// What a header click asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HeaderClick {
    Sort,
    More,
}

/// Draws the header: the drawn columns, and a `+N columns` cell when `more` is set. Reports what
/// was clicked.
fn draw_header(
    ui: &mut Ui,
    id: Id,
    columns: &[Column],
    more: Option<usize>,
    rect: Rect,
    style: &Style,
    sort: &mut Option<(usize, Direction)>,
) -> Option<HeaderClick> {
    ui.painter()
        .rect_filled(rect, CornerRadius::ZERO, style.panel);
    let mut clicked = None;
    for (position, (column, cell)) in columns.iter().zip(cell_rects(columns, rect)).enumerate() {
        let click = ui.interact(cell, id.with(("head", position)), Sense::click());
        if click.clicked() {
            *sort = Some(next_sort(*sort, position, column.kind));
            clicked = Some(HeaderClick::Sort);
        }
        ui.scope_builder(
            UiBuilder::new()
                .max_rect(cell.shrink2(vec2(CELL_PAD, 0.0)))
                .layout(layout_for(column.kind)),
            |ui| {
                header_label(ui, &column.label, style);
            },
        );
        match *sort {
            Some((sorted, direction)) if sorted == position => {
                sort_mark(ui, cell, column.kind, direction, style.text);
            }
            Some(_) | None => {}
        }
    }
    if let Some(more) = more {
        let cell = Rect::from_min_size(
            pos2(rect.max.x - MORE_WIDTH, rect.min.y),
            vec2(MORE_WIDTH, rect.height()),
        );
        if ui
            .interact(cell, id.with("more columns"), Sense::click())
            .clicked()
        {
            clicked = Some(HeaderClick::More);
        }
        ui.scope_builder(
            UiBuilder::new()
                .max_rect(cell.shrink2(vec2(CELL_PAD, 0.0)))
                .layout(Layout::left_to_right(Align::Center)),
            |ui| {
                header_label(ui, &format!("+{more} columns"), style);
            },
        );
    }
    clicked
}

/// The corners of the sort triangle for the sort direction, on the side of the cell away from its text.
pub fn sort_mark_points(cell: Rect, kind: ColumnKind, direction: Direction) -> Vec<Pos2> {
    let x = match kind {
        ColumnKind::Text => cell.max.x - MARK_INSET,
        ColumnKind::Number | ColumnKind::Money => cell.min.x + MARK_INSET,
    };
    let centre = pos2(x, cell.center().y);
    match direction {
        Direction::Ascending => triangle(centre, 3.0, -3.0),
        Direction::Descending => triangle(centre, -3.0, 3.0),
    }
}

fn sort_mark(ui: &Ui, cell: Rect, kind: ColumnKind, direction: Direction, color: Color32) {
    let points = sort_mark_points(cell, kind, direction);
    ui.painter()
        .add(Shape::convex_polygon(points, color, PathStroke::NONE));
}

/// A fold arrow: pointing right when folded, down when unfolded.
fn fold_mark(ui: &Ui, rect: Rect, collapsed: bool, color: Color32) {
    let centre = rect.center();
    let points = if collapsed {
        vec![
            pos2(centre.x - 3.0, centre.y - 4.0),
            pos2(centre.x + 4.0, centre.y),
            pos2(centre.x - 3.0, centre.y + 4.0),
        ]
    } else {
        triangle(centre, -4.0, 3.0)
    };
    ui.painter()
        .add(Shape::convex_polygon(points, color, PathStroke::NONE));
}

/// A triangle whose base is at `centre.y + base` and whose tip is at `centre.y + apex`, so the tip
/// points up when `apex` is above `base`.
fn triangle(centre: Pos2, base: f32, apex: f32) -> Vec<Pos2> {
    vec![
        pos2(centre.x - 4.0, centre.y + base),
        pos2(centre.x + 4.0, centre.y + base),
        pos2(centre.x, centre.y + apex),
    ]
}

/// The sort after a click on `column`: the next direction if it is already sorted, else its default.
fn next_sort(
    current: Option<(usize, Direction)>,
    column: usize,
    kind: ColumnKind,
) -> (usize, Direction) {
    let direction = match current {
        Some((sorted, direction)) if sorted == column => flip(direction),
        Some(_) | None => match kind {
            ColumnKind::Text => Direction::Ascending,
            ColumnKind::Number | ColumnKind::Money => Direction::Descending,
        },
    };
    (column, direction)
}

fn flip(direction: Direction) -> Direction {
    match direction {
        Direction::Ascending => Direction::Descending,
        Direction::Descending => Direction::Ascending,
    }
}

fn draw_footer(ui: &mut Ui, columns: &[Column], cells: &[Cell], rect: Rect, style: &Style) {
    ui.painter()
        .rect_filled(rect, CornerRadius::ZERO, style.panel);
    for (position, (column, cell)) in columns.iter().zip(cell_rects(columns, rect)).enumerate() {
        let text = cells.get(position).map_or("", |cell| cell.text.as_str());
        ui.scope_builder(
            UiBuilder::new()
                .max_rect(cell.shrink2(vec2(CELL_PAD, 0.0)))
                .layout(layout_for(column.kind)),
            |ui| {
                plain_label(ui, text);
            },
        );
    }
}

/// Draws one row at `rect` and reports a click on it, or on its arrow.
///
/// The row's own click area is created first and the cells and arrow after it, so a cell that
/// takes clicks (such as a custom checkbox) and the arrow win over the row.
fn draw_row(
    look: Look,
    ui: &mut Ui,
    rows: &[Row],
    index: usize,
    rect: Rect,
    draw: &mut dyn FnMut(&mut Ui, u64, &'static str) -> bool,
) -> Option<Hit> {
    let row = rows.get(index)?;
    let hit_row = ui.interact(rect, look.id.with(("row", row.key)), Sense::click());
    let selected = look.selected == Some(row.key);
    ui.painter().rect_filled(
        rect,
        CornerRadius::ZERO,
        look.style.row_fill(false, hit_row.hovered(), selected),
    );

    // The cell padding comes first, then the indent of this row's depth and its arrow.
    // Depths past 65,535 levels are indented as 65,535: no tree is that deep, and the list still draws.
    let depth = f32::from(u16::try_from(row.depth).unwrap_or(u16::MAX));
    let mut arrow = None;
    for (position, (column, cell)) in look
        .columns
        .iter()
        .zip(cell_rects(look.columns, rect))
        .enumerate()
    {
        if look.changed.contains(&(row.key, position)) {
            ui.painter()
                .rect_filled(cell, CornerRadius::ZERO, look.style.sel);
        }
        let content = if position == 0 && has_children(rows, index) {
            let arrow_rect = Rect::from_min_size(
                pos2(cell.min.x + CELL_PAD + depth * INDENT, cell.min.y),
                vec2(ARROW_WIDTH, look.height),
            );
            fold_mark(
                ui,
                arrow_rect,
                look.collapsed.contains(&row.key),
                look.style.text,
            );
            arrow = Some(arrow_rect);
            Rect::from_min_max(pos2(arrow_rect.max.x, cell.min.y), cell.max)
        } else if position == 0 {
            // A leaf keeps the text place an arrow would take, so siblings line up.
            let left = cell.min.x + CELL_PAD + depth * INDENT + ARROW_WIDTH;
            Rect::from_min_max(pos2(left, cell.min.y), cell.max)
        } else {
            cell
        };
        // The first column's padding is before its indent and arrow; the others take it here.
        let text_rect = if position == 0 {
            Rect::from_min_max(
                pos2(content.min.x + ARROW_GAP, content.min.y),
                pos2(cell.max.x - CELL_PAD, content.max.y),
            )
        } else {
            content.shrink2(vec2(CELL_PAD, 0.0))
        };
        ui.scope_builder(
            UiBuilder::new()
                .max_rect(text_rect)
                .layout(layout_for(column.kind)),
            |ui| {
                // A cell the callback drew is left as it is.
                if !draw(ui, row.key, column.id) {
                    let icon = look
                        .icons
                        .filter(|(id, _)| *id == column.id)
                        .and_then(|(_, icon)| icon(row.key));
                    if let Some(id) = icon {
                        crate::images::show(
                            ui,
                            crate::images::Picture::Type(id),
                            (look.height - 4.0).max(8.0),
                        );
                        ui.add_space(4.0);
                    }
                    let text = row
                        .cells
                        .get(position)
                        .map_or("", |cell| cell.text.as_str());
                    plain_label(ui, text);
                }
            },
        );
    }

    // The hairline under the row sits inside its bottom edge, the full table width.
    horizontal(ui, rect.min.x, rect.max.x, rect.max.y - 1.0, look.style.row);

    if let Some(arrow_rect) = arrow {
        let hit_arrow = ui.interact(
            arrow_rect,
            look.id.with(("toggle", row.key)),
            Sense::click(),
        );
        if hit_arrow.clicked() {
            return Some(Hit::Toggled(row.key));
        }
    }
    hit_row.clicked().then_some(Hit::Clicked(row.key))
}

/// A row has children when the next row is deeper.
fn has_children(rows: &[Row], index: usize) -> bool {
    match (rows.get(index), rows.get(index + 1)) {
        (Some(row), Some(next)) => next.depth > row.depth,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell as Flag;
    use std::rc::Rc;
    use std::sync::{Arc, Mutex};

    use egui::{Event, Modifiers, PointerButton};
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use super::*;
    use crate::dock::show_locked;

    /// The row keys the table laid out in the last frame, in order.
    #[derive(Default)]
    struct Seen {
        order: Vec<u64>,
    }

    fn columns() -> Vec<Column> {
        vec![
            Column {
                id: "item",
                label: "Item".to_owned(),
                kind: ColumnKind::Text,
                width: 220.0,
            },
            Column {
                id: "need",
                label: "Need".to_owned(),
                kind: ColumnKind::Number,
                width: 80.0,
            },
            Column {
                id: "value",
                label: "Value".to_owned(),
                kind: ColumnKind::Money,
                width: 140.0,
            },
        ]
    }

    #[derive(Clone, Copy)]
    enum ReSortPanel {
        Panel,
    }

    fn resort_title(_: ReSortPanel) -> &'static str {
        "Re-sort panel"
    }

    /// A flat row whose Value cell is `value`, for the tables that keep their order.
    fn value_row(key: u64, item: &str, value: i128) -> Row {
        Row {
            key,
            depth: 0,
            cells: vec![
                Cell::text(item),
                Cell::int(1, "1"),
                Cell::int(value, value.to_string()),
            ],
        }
    }

    /// Three rows whose values, sorted descending, are A, B, C.
    fn value_rows() -> Vec<Row> {
        vec![
            value_row(1, "A", 300),
            value_row(2, "B", 200),
            value_row(3, "C", 100),
        ]
    }

    /// A panel whose table shows the shared `rows`, so the test can change them between frames.
    fn resort_harness(rows: Arc<Mutex<Vec<Row>>>, seen: &Arc<Mutex<Seen>>) -> Harness<'static> {
        resort_harness_with(rows, None, seen)
    }

    /// Like [`resort_harness`], but with the Need column hidden while `hidden` is set.
    fn resort_harness_with(
        rows: Arc<Mutex<Vec<Row>>>,
        hidden: Option<Arc<Mutex<bool>>>,
        seen: &Arc<Mutex<Seen>>,
    ) -> Harness<'static> {
        let seen = Arc::clone(seen);
        let style = Style::eve_dense();
        let mut state = egui_dock::DockState::new(vec![ReSortPanel::Panel]);
        Harness::builder()
            .with_size(vec2(800.0, 400.0))
            .build_ui(move |ui| {
                show_locked(ui, &style, &mut state, resort_title, |ui, _| {
                    let mut columns = columns();
                    if hidden
                        .as_ref()
                        .is_some_and(|hidden| *hidden.lock().unwrap())
                    {
                        columns.retain(|column| column.id != "need");
                    }
                    let rows = rows.lock().unwrap().clone();
                    let mut order = Vec::new();
                    let mut draw = |_ui: &mut Ui, key: u64, column: &'static str| -> bool {
                        if column == "item" {
                            order.push(key);
                        }
                        false
                    };
                    Table::new(Id::new("Re-sort panel"), &columns).show_with(
                        ui,
                        &style,
                        Density::Normal,
                        &rows,
                        None,
                        &mut draw,
                    );
                    if let Ok(mut seen) = seen.lock() {
                        seen.order = order;
                    }
                });
            })
    }

    /// The area of the cell of the row labelled `row` under the header labelled `header`.
    fn cell_area(harness: &Harness<'_>, row: &str, header: &str) -> Rect {
        let row = harness.get_by_label(row).rect();
        let header = harness.get_by_label(header).rect();
        Rect::from_min_max(pos2(header.min.x, row.min.y), pos2(header.max.x, row.max.y))
    }

    /// Whether a pixel inside `area` has the selection fill, which marks a changed cell.
    fn marked(harness: &mut Harness<'_>, area: Rect, style: &Style) -> bool {
        let image = harness.render().expect("the frame draws");
        let per_point = harness.ctx.pixels_per_point();
        image.enumerate_pixels().any(|(x, y, pixel)| {
            // Pixel centres, in points.
            let x = (f32::from(u16::try_from(x).unwrap_or(u16::MAX)) + 0.5) / per_point;
            let y = (f32::from(u16::try_from(y).unwrap_or(u16::MAX)) + 0.5) / per_point;
            let [r, g, b, _] = pixel.0;
            area.contains(pos2(x, y)) && Color32::from_rgb(r, g, b) == style.sel
        })
    }

    /// Whether the Value cell of the row labelled `row` is marked.
    fn marked_cell(harness: &mut Harness<'_>, row: &str, style: &Style) -> bool {
        let area = cell_area(harness, row, "Value");
        marked(harness, area, style)
    }

    #[test]
    fn a_changed_value_keeps_the_order_marks_the_cell_and_offers_a_re_sort() {
        let style = Style::eve_dense();
        let rows = Arc::new(Mutex::new(value_rows()));
        let seen = Arc::new(Mutex::new(Seen::default()));
        let mut harness = resort_harness(Arc::clone(&rows), &seen);
        harness.run_steps(2);
        harness.get_by_label("Value").click();
        harness.run_steps(2);
        assert_eq!(seen.lock().unwrap().order, vec![1, 2, 3]);

        // B's new value would sort it first, but the order the player was reading stays.
        rows.lock().unwrap()[1] = value_row(2, "B", 400);
        harness.run_steps(3);
        assert_eq!(seen.lock().unwrap().order, vec![1, 2, 3]);
        assert_eq!(
            harness.query_all_by_label("Re-sort (2 rows moved)").count(),
            1
        );
        assert!(
            marked_cell(&mut harness, "B", &style),
            "B's Value cell is marked"
        );
        assert!(!marked_cell(&mut harness, "A", &style));
        assert!(!marked_cell(&mut harness, "C", &style));

        harness.get_by_label("Re-sort (2 rows moved)").click();
        harness.run_steps(3);
        assert_eq!(seen.lock().unwrap().order, vec![2, 1, 3]);
        assert_eq!(
            harness.query_all_by_label("Re-sort (2 rows moved)").count(),
            0
        );
        assert!(!marked_cell(&mut harness, "B", &style));
    }

    #[test]
    fn a_header_click_after_a_change_sorts_at_once_and_clears_the_mark() {
        let style = Style::eve_dense();
        let rows = Arc::new(Mutex::new(value_rows()));
        let seen = Arc::new(Mutex::new(Seen::default()));
        let mut harness = resort_harness(Arc::clone(&rows), &seen);
        harness.run_steps(2);
        harness.get_by_label("Value").click();
        harness.run_steps(2);
        rows.lock().unwrap()[1] = value_row(2, "B", 400);
        harness.run_steps(3);
        assert!(marked_cell(&mut harness, "B", &style));

        // Value is descending, so the click flips it to ascending: C, A, B.
        harness.get_by_label("Value").click();
        harness.run_steps(3);
        assert_eq!(seen.lock().unwrap().order, vec![3, 1, 2]);
        assert_eq!(
            harness.query_all_by_label("Re-sort (2 rows moved)").count(),
            0
        );
        assert!(!marked_cell(&mut harness, "A", &style));
    }

    #[test]
    fn a_first_fill_and_a_hidden_column_mark_nothing() {
        let style = Style::eve_dense();

        // The first fill has no old values to differ from, so nothing is marked.
        let rows = Arc::new(Mutex::new(value_rows()));
        let seen = Arc::new(Mutex::new(Seen::default()));
        let mut harness = resort_harness(Arc::clone(&rows), &seen);
        harness.run_steps(3);
        assert!(!marked_cell(&mut harness, "A", &style));
        assert!(!marked_cell(&mut harness, "B", &style));
        assert!(!marked_cell(&mut harness, "C", &style));

        // Hiding a column moves every cell, so the old values are forgotten: no mark, no kept
        // order, and the new value sorts at once.
        let hidden = Arc::new(Mutex::new(false));
        let seen = Arc::new(Mutex::new(Seen::default()));
        let mut harness = resort_harness_with(Arc::clone(&rows), Some(Arc::clone(&hidden)), &seen);
        harness.run_steps(2);
        harness.get_by_label("Value").click();
        harness.run_steps(2);
        *hidden.lock().unwrap() = true;
        rows.lock().unwrap()[1] = value_row(2, "B", 400);
        harness.run_steps(3);
        assert_eq!(seen.lock().unwrap().order, vec![2, 1, 3]);
        assert!(!marked_cell(&mut harness, "A", &style));
        assert!(!marked_cell(&mut harness, "B", &style));
        assert!(!marked_cell(&mut harness, "C", &style));
        assert_eq!(
            harness.query_all_by_label("Re-sort (2 rows moved)").count(),
            0
        );
    }

    #[test]
    fn a_refresh_the_player_starts_sorts_at_once_with_no_mark_or_button() {
        let style = Style::eve_dense();
        let rows = Arc::new(Mutex::new(value_rows()));
        let seen = Arc::new(Mutex::new(Seen::default()));
        let mut harness = resort_harness(Arc::clone(&rows), &seen);
        harness.run_steps(2);
        harness.get_by_label("Value").click();
        harness.run_steps(2);
        assert_eq!(seen.lock().unwrap().order, vec![1, 2, 3]);

        resort_all(&harness.ctx);
        harness.run_steps(1);
        rows.lock().unwrap()[1] = value_row(2, "B", 400);
        harness.run_steps(3);
        assert_eq!(seen.lock().unwrap().order, vec![2, 1, 3]);
        assert_eq!(
            harness.query_all_by_label("Re-sort (2 rows moved)").count(),
            0
        );
        assert!(!marked_cell(&mut harness, "B", &style));
    }

    /// A row of the small tree the start tests fold.
    fn tree_row(key: u64, depth: u32, item: &str) -> Row {
        Row {
            key,
            depth,
            cells: vec![Cell::text(item), Cell::int(1, "1"), Cell::missing()],
        }
    }

    /// Root, its child, that child's child, and a sibling: the child has children.
    fn tree_rows() -> Vec<Row> {
        vec![
            tree_row(1, 0, "Root"),
            tree_row(2, 1, "Child"),
            tree_row(3, 2, "Grandchild"),
            tree_row(4, 1, "Sibling"),
        ]
    }

    fn fold(keys: &[u64]) -> BTreeSet<u64> {
        keys.iter().copied().collect()
    }

    /// A table that starts folded at its child and sorted by value, with `expanded` read each frame.
    fn start_harness(expanded: Rc<Flag<bool>>) -> Harness<'static> {
        let style = Style::eve_dense();
        let rows = tree_rows();
        Harness::builder()
            .with_size(vec2(800.0, 400.0))
            .build_ui(move |ui| {
                style.apply(ui.ctx());
                let columns = columns();
                Table::new(Id::new("start test"), &columns)
                    .start(fold(&[2]), Some((2, Direction::Descending)))
                    .expanded(expanded.get())
                    .show(ui, &style, Density::Normal, &rows, None);
            })
    }

    /// A primary click at `pos`: the pointer moves there, then presses and releases.
    fn click_at(harness: &mut Harness<'_>, pos: Pos2) {
        harness.hover_at(pos);
        for pressed in [true, false] {
            harness.event(Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            });
        }
    }

    fn shows(harness: &Harness<'_>, label: &str) -> bool {
        harness.query_all_by_label(label).count() > 0
    }

    #[test]
    fn start_folds_the_given_rows_and_expanded_shows_every_row() {
        let expanded = Rc::new(Flag::new(false));
        let mut harness = start_harness(Rc::clone(&expanded));
        harness.run();
        assert!(shows(&harness, "Root"));
        assert!(shows(&harness, "Child"));
        assert!(shows(&harness, "Sibling"));
        assert!(!shows(&harness, "Grandchild"), "the folded child hides it");

        expanded.set(true);
        harness.run();
        assert!(shows(&harness, "Grandchild"), "an expanded table shows all");
    }

    #[test]
    fn a_fold_click_while_expanded_leaves_the_folds_alone() {
        let expanded = Rc::new(Flag::new(true));
        let mut harness = start_harness(Rc::clone(&expanded));
        harness.run();
        let child = harness.get_by_label("Child").rect();
        click_at(&mut harness, pos2(child.min.x - 7.0, child.center().y));
        harness.run();
        assert!(shows(&harness, "Grandchild"));

        // The folded child keeps its fold: shrinking the table again hides the grandchild.
        expanded.set(false);
        harness.run();
        assert!(!shows(&harness, "Grandchild"));

        // With the table folded again, a click unfolds as usual.
        let child = harness.get_by_label("Child").rect();
        click_at(&mut harness, pos2(child.min.x - 7.0, child.center().y));
        harness.run();
        assert!(shows(&harness, "Grandchild"));
    }

    /// A table filling the harness, with the point it starts at written to `top` every frame.
    fn paint_harness(
        columns: Vec<Column>,
        rows: Vec<Row>,
        density: Density,
        top: &Arc<Mutex<Pos2>>,
    ) -> Harness<'static> {
        let top = Arc::clone(top);
        let style = Style::eve_dense();
        Harness::builder()
            .with_size(vec2(800.0, 400.0))
            .build_ui(move |ui| {
                style.apply(ui.ctx());
                if let Ok(mut top) = top.lock() {
                    *top = ui.cursor().min;
                }
                Table::new(Id::new("paint test"), &columns).show(ui, &style, density, &rows, None);
            })
    }

    /// The centre of the pixel at `x`, `y`, in points.
    fn pixel_centre(x: u32, y: u32, per_point: f32) -> Pos2 {
        let x = f32::from(u16::try_from(x).unwrap_or(u16::MAX));
        let y = f32::from(u16::try_from(y).unwrap_or(u16::MAX));
        pos2((x + 0.5) / per_point, (y + 0.5) / per_point)
    }

    /// The colour drawn at the pixel under `pos`.
    fn colour_at(harness: &mut Harness<'_>, pos: Pos2) -> Color32 {
        let per_point = harness.ctx.pixels_per_point();
        let image = harness.render().expect("the frame draws");
        image
            .enumerate_pixels()
            .find_map(|(x, y, pixel)| {
                let centre = pixel_centre(x, y, per_point);
                let near = (centre.x - pos.x).abs() <= 0.5 / per_point
                    && (centre.y - pos.y).abs() <= 0.5 / per_point;
                let [r, g, b, _] = pixel.0;
                near.then_some(Color32::from_rgb(r, g, b))
            })
            .expect("a pixel under the point")
    }

    /// The heights, in pixels, of the horizontal runs of `colour` near the table's left edge.
    fn colour_bands(harness: &mut Harness<'_>, top: Pos2, colour: Color32) -> Vec<usize> {
        let per_point = harness.ctx.pixels_per_point();
        let image = harness.render().expect("the frame draws");
        let (width, height) = image.dimensions();
        let start = (0..width)
            .find(|&x| pixel_centre(x, 0, per_point).x >= top.x + 1.0)
            .expect("a pixel in the table");
        let mut bands = Vec::new();
        let mut band = 0;
        for y in 0..height {
            let run = (start..start + 40).all(|x| {
                let [r, g, b, _] = image.get_pixel(x, y).0;
                Color32::from_rgb(r, g, b) == colour
            });
            if run {
                band += 1;
            } else if band > 0 {
                bands.push(band);
                band = 0;
            }
        }
        if band > 0 {
            bands.push(band);
        }
        bands
    }

    #[test]
    fn plain_rows_sit_on_the_background_with_a_hairline_after_each() {
        let style = Style::eve_dense();
        let top = Arc::new(Mutex::new(Pos2::ZERO));
        let mut harness = paint_harness(columns(), value_rows(), Density::Normal, &top);
        harness.run();
        let top = *top.lock().unwrap();
        let per_point = harness.ctx.pixels_per_point();
        for label in ["A", "B", "C"] {
            let centre = harness.get_by_label(label).rect().center().y;
            let behind = colour_at(&mut harness, pos2(top.x + 2.0, centre));
            assert_eq!(behind, style.bg, "{label}'s row sits on the background");
        }

        let bands = colour_bands(&mut harness, top, style.row);
        assert_eq!(bands.len(), 3, "one hairline after each of the three rows");
        assert!(
            bands.iter().all(|&height| {
                let height = f32::from(u16::try_from(height).unwrap_or(u16::MAX));
                (height / per_point - 1.0).abs() < 0.01
            }),
            "each hairline is 1.0 tall: {bands:?}"
        );
    }

    #[test]
    fn the_header_is_28_tall_at_every_density_and_a_point_gap_precedes_the_rows() {
        let top = Arc::new(Mutex::new(Pos2::ZERO));
        for density in [Density::Dense, Density::Normal, Density::Sparse] {
            let top_point = Arc::clone(&top);
            let mut harness = paint_harness(columns(), value_rows(), density, &top_point);
            harness.run();
            let top = *top.lock().unwrap();
            let header = harness.get_by_label("Item").rect();
            assert!(
                (header.center().y - top.y - HEADER_HEIGHT / 2.0).abs() <= 0.5,
                "{density:?}'s header is {HEADER_HEIGHT} tall"
            );
            let first = harness.get_by_label("A").rect();
            let wanted = top.y + HEADER_HEIGHT + 1.0 + row_height(density) / 2.0;
            assert!(
                (first.center().y - wanted).abs() <= 0.5,
                "{density:?}'s first row starts a point below the header: {}",
                first.center().y - top.y
            );
        }
    }

    /// Qty first, so the text column after it is plain rather than a tree column.
    fn padding_columns() -> Vec<Column> {
        vec![
            Column {
                id: "qty",
                label: "Qty".to_owned(),
                kind: ColumnKind::Number,
                width: 80.0,
            },
            Column {
                id: "item",
                label: "Item".to_owned(),
                kind: ColumnKind::Text,
                width: 220.0,
            },
            Column {
                id: "value",
                label: "Value".to_owned(),
                kind: ColumnKind::Money,
                width: 140.0,
            },
        ]
    }

    fn padding_rows() -> Vec<Row> {
        vec![Row {
            key: 1,
            depth: 0,
            cells: vec![
                Cell::int(10, "10"),
                Cell::text("Tritanium"),
                Cell::int(4307, "4,307"),
            ],
        }]
    }

    #[test]
    fn cell_text_keeps_8_points_from_its_cell_edges() {
        let top = Arc::new(Mutex::new(Pos2::ZERO));
        let mut harness = paint_harness(padding_columns(), padding_rows(), Density::Normal, &top);
        harness.run();
        let top = *top.lock().unwrap();
        let item = harness.get_by_label("Tritanium").rect();
        let item_left = top.x + 80.0;
        assert!(
            (item.min.x - item_left - CELL_PAD).abs() <= 0.5,
            "Tritanium starts {CELL_PAD} in from its cell: {}",
            item.min.x - item_left
        );
        let value = harness.get_by_label("4,307").rect();
        let value_right = top.x + 80.0 + 220.0 + 140.0;
        assert!(
            (value_right - value.max.x - CELL_PAD).abs() <= 0.5,
            "4,307 ends {CELL_PAD} before its cell's right edge: {}",
            value_right - value.max.x
        );
    }
}

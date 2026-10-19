//! The table widget: a header that sorts, rows that scroll without laying out every row, tree rows
//! that fold, a footer, and custom cells. What it shows is worked out in [`model`].

pub mod model;

use std::collections::BTreeSet;
use std::iter;

use egui::epaint::PathStroke;
use egui::{
    Align, Color32, CornerRadius, Id, Label, Layout, Pos2, Rect, ScrollArea, Sense, Shape, Ui,
    UiBuilder, pos2, vec2,
};
use profit_store::Density;

use crate::theme::Style;
use model::{
    Cell, Column, ColumnKind, Direction, Row, row_height, sort_order, span, to_tsv, visible,
};

/// The memory key that asks the table with this id to copy itself as TSV. The panel menu sets it;
/// the table clears it when it copies. A panel's table takes the panel's id, so the menu finds it.
pub fn copy_request(table: Id) -> Id {
    table.with("copy as TSV")
}

/// The width of the fold arrow and the indent per depth, in points.
const ARROW_WIDTH: f32 = 14.0;
const INDENT: f32 = 14.0;
/// How far a sort triangle sits in from the edge of its header cell.
const MARK_INSET: f32 = 8.0;

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
    collapsed: BTreeSet<u64>,
}

/// What a row click did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hit {
    Clicked(u64),
    Toggled(u64),
}

/// What every row needs to be drawn.
#[derive(Clone, Copy)]
struct Look<'a> {
    id: Id,
    columns: &'a [Column],
    style: &'a Style,
    height: f32,
    selected: Option<u64>,
    collapsed: &'a BTreeSet<u64>,
}

/// A table of `rows` under `columns`. Pass the same `id` every frame: the sort and the folds are kept under it.
pub struct Table<'a> {
    id: Id,
    columns: &'a [Column],
    selected: Option<u64>,
}

impl<'a> Table<'a> {
    pub fn new(id: Id, columns: &'a [Column]) -> Self {
        Self {
            id,
            columns,
            selected: None,
        }
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
        let mut state: State =
            ui.data_mut(|data| data.get_temp_mut_or_default::<State>(self.id).clone());
        let sort_before = state.sort;
        let height = row_height(density);
        let width = ui.available_width();
        let mut response = TableResponse::default();

        let (header, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
        draw_header(ui, self.id, self.columns, header, style, &mut state.sort);

        let (column, direction) = state
            .sort
            .map_or((None, Direction::Ascending), |(column, direction)| {
                (Some(column), direction)
            });
        let order = sort_order(rows, column, direction);
        let shown = visible(rows, &order, &state.collapsed);
        // The panel menu sets this flag to ask for a copy; the text is built only then.
        if ui
            .data_mut(|data| data.remove_temp::<bool>(copy_request(self.id)))
            .unwrap_or(false)
        {
            ui.ctx()
                .copy_text(to_tsv(self.columns, rows, &shown, footer));
        }

        let look = Look {
            id: self.id,
            columns: self.columns,
            style,
            height,
            selected: self.selected,
            collapsed: &state.collapsed,
        };
        // The footer is drawn below the scroll area, so the rows leave room for it.
        let footer_height = if footer.is_some() { height } else { 0.0 };
        let body = (ui.available_height() - footer_height).max(0.0);
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
                    let hit = draw_row(look, ui, rows, index, rect, position % 2 == 1, draw);
                    match hit {
                        Some(Hit::Clicked(key)) => response.clicked = Some(key),
                        Some(Hit::Toggled(key)) => response.toggled = Some(key),
                        None => {}
                    }
                }
            });

        if let Some(cells) = footer {
            let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
            draw_footer(ui, self.columns, cells, rect, style);
        }

        // Unfold the row if it was folded, else fold it.
        if let Some(key) = response.toggled
            && !state.collapsed.remove(&key)
        {
            state.collapsed.insert(key);
        }
        if response.toggled.is_some() || state.sort != sort_before {
            ui.ctx().request_repaint();
        }
        ui.data_mut(|data| data.insert_temp(self.id, state));
        response
    }
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

fn layout_for(kind: ColumnKind) -> Layout {
    match kind {
        ColumnKind::Text => Layout::left_to_right(Align::Center),
        ColumnKind::Number | ColumnKind::Money => Layout::right_to_left(Align::Center),
    }
}

fn draw_header(
    ui: &mut Ui,
    id: Id,
    columns: &[Column],
    rect: Rect,
    style: &Style,
    sort: &mut Option<(usize, Direction)>,
) {
    ui.painter()
        .rect_filled(rect, CornerRadius::ZERO, style.panel);
    for (position, (column, cell)) in columns.iter().zip(cell_rects(columns, rect)).enumerate() {
        let click = ui.interact(cell, id.with(("head", position)), Sense::click());
        if click.clicked() {
            *sort = Some(next_sort(*sort, position, column.kind));
        }
        ui.scope_builder(
            UiBuilder::new()
                .max_rect(cell)
                .layout(layout_for(column.kind)),
            |ui| {
                plain_label(ui, &column.label);
            },
        );
        match *sort {
            Some((sorted, direction)) if sorted == position => {
                sort_mark(ui, cell, column.kind, direction, style.text);
            }
            Some(_) | None => {}
        }
    }
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
                .max_rect(cell)
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
    zebra: bool,
    draw: &mut dyn FnMut(&mut Ui, u64, &'static str) -> bool,
) -> Option<Hit> {
    let row = rows.get(index)?;
    let hit_row = ui.interact(rect, look.id.with(("row", row.key)), Sense::click());
    let selected = look.selected == Some(row.key);
    ui.painter().rect_filled(
        rect,
        CornerRadius::ZERO,
        look.style.row_fill(zebra, hit_row.hovered(), selected),
    );

    // The arrow sits in the first column, after the indent of this row's depth.
    // Depths past 65,535 levels are indented as 65,535: no tree is that deep, and the list still draws.
    let depth = f32::from(u16::try_from(row.depth).unwrap_or(u16::MAX));
    let mut arrow = None;
    for (position, (column, cell)) in look
        .columns
        .iter()
        .zip(cell_rects(look.columns, rect))
        .enumerate()
    {
        let content = if position == 0 && has_children(rows, index) {
            let arrow_rect = Rect::from_min_size(
                pos2(cell.min.x + depth * INDENT, cell.min.y),
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
        } else {
            cell
        };
        ui.scope_builder(
            UiBuilder::new()
                .max_rect(content)
                .layout(layout_for(column.kind)),
            |ui| {
                // A cell the callback drew is left as it is.
                if !draw(ui, row.key, column.id) {
                    let text = row
                        .cells
                        .get(position)
                        .map_or("", |cell| cell.text.as_str());
                    plain_label(ui, text);
                }
            },
        );
    }

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

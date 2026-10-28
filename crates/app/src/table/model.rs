//! The table's logic, with no drawing: the columns, the rows and their cells, the sort order, the
//! rows in view, the copied text and the row heights. The widget in `table.rs` draws it.
//!
//! Rows come in tree order: a row's children follow it with one more depth. A flat table has every
//! depth 0.

use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::ops::Range;

use mop_core::money::Money;
use mop_store::Density;

/// How a column's values are read and aligned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnKind {
    Text,
    Number,
    Money,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Column {
    pub id: &'static str,
    pub label: String,
    pub kind: ColumnKind,
    pub width: f32,
}

/// What a cell sorts by. A missing value sorts last whichever way the column is sorted.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum SortKey {
    Text(String),
    Int(i128),
    Missing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    /// What is drawn, such as `31,500,000.00`.
    pub text: String,
    /// What is copied, such as `31500000`. Empty for a missing value.
    pub plain: String,
    pub key: SortKey,
}

impl Cell {
    /// A missing value: drawn as a dash, copied as an empty field.
    pub fn missing() -> Cell {
        Cell {
            text: "—".to_owned(),
            plain: String::new(),
            key: SortKey::Missing,
        }
    }

    /// Text, sorted ignoring case.
    pub fn text(text: impl Into<String>) -> Cell {
        let text: String = text.into();
        Cell {
            key: SortKey::Text(text.to_lowercase()),
            plain: text.clone(),
            text,
        }
    }

    /// A whole number, drawn as `shown` and copied and sorted by `value`.
    pub fn int(value: i128, shown: impl Into<String>) -> Cell {
        Cell {
            text: shown.into(),
            plain: value.to_string(),
            key: SortKey::Int(value),
        }
    }

    /// An amount of ISK, drawn grouped to 0.01 and copied exactly.
    pub fn money(amount: Money) -> Cell {
        Cell {
            text: amount.format_isk(),
            plain: amount.to_plain(),
            key: SortKey::Int(i128::from(amount.ten_thousandths())),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub key: u64,
    pub depth: u32,
    pub cells: Vec<Cell>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Ascending,
    Descending,
}

/// The row indices in the order to show them. `None` keeps the given order.
///
/// Siblings are sorted under their parent, so children stay under it. The sort is stable.
pub fn sort_order(rows: &[Row], column: Option<usize>, direction: Direction) -> Vec<usize> {
    let Some(column) = column else {
        return (0..rows.len()).collect();
    };
    let (roots, children) = tree(rows);
    let mut order = Vec::with_capacity(rows.len());
    sorted_walk(&roots, &children, rows, column, direction, &mut order);
    order
}

/// The top-level rows, and for each row its children, both in the given order.
fn tree(rows: &[Row]) -> (Vec<usize>, Vec<Vec<usize>>) {
    let mut roots = Vec::new();
    let mut children = vec![Vec::new(); rows.len()];
    // The rows above the current one that can still be its parents, shallowest first.
    let mut open: Vec<usize> = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        while open
            .last()
            .and_then(|&parent| rows.get(parent))
            .is_some_and(|parent| parent.depth >= row.depth)
        {
            open.pop();
        }
        match open.last().and_then(|&parent| children.get_mut(parent)) {
            Some(list) => list.push(index),
            None => roots.push(index),
        }
        open.push(index);
    }
    (roots, children)
}

fn sorted_walk(
    siblings: &[usize],
    children: &[Vec<usize>],
    rows: &[Row],
    column: usize,
    direction: Direction,
    out: &mut Vec<usize>,
) {
    let mut siblings = siblings.to_vec();
    siblings.sort_by(|&a, &b| order(key(rows, a, column), key(rows, b, column), direction));
    for index in siblings {
        out.push(index);
        if let Some(kids) = children.get(index) {
            sorted_walk(kids, children, rows, column, direction, out);
        }
    }
}

static MISSING: SortKey = SortKey::Missing;

fn key(rows: &[Row], index: usize, column: usize) -> &SortKey {
    rows.get(index)
        .and_then(|row| row.cells.get(column))
        .map_or(&MISSING, |cell| &cell.key)
}

/// Missing values go last in both directions; the rest compare by direction.
fn order(a: &SortKey, b: &SortKey, direction: Direction) -> Ordering {
    match (a, b) {
        (SortKey::Missing, SortKey::Missing) => Ordering::Equal,
        (SortKey::Missing, _) => Ordering::Greater,
        (_, SortKey::Missing) => Ordering::Less,
        _ => match direction {
            Direction::Ascending => a.cmp(b),
            Direction::Descending => b.cmp(a),
        },
    }
}

/// The shown rows of `order`, without the descendants of the collapsed rows.
pub fn visible(rows: &[Row], order: &[usize], collapsed: &BTreeSet<u64>) -> Vec<usize> {
    let mut shown = Vec::with_capacity(order.len());
    // The depth of the collapsed row whose descendants are being skipped.
    let mut hiding: Option<u32> = None;
    for &index in order {
        let Some(row) = rows.get(index) else {
            continue;
        };
        if hiding.is_some_and(|depth| row.depth > depth) {
            continue;
        }
        hiding = collapsed.contains(&row.key).then_some(row.depth);
        shown.push(index);
    }
    shown
}

/// The rows that touch the view, one above and one below, and where they sit.
#[derive(Debug, Clone, PartialEq)]
pub struct Span {
    /// The rows to lay out.
    pub rows: Range<usize>,
    /// The top of the first of them, from the top of the content.
    pub top: f32,
    /// The height of all the rows, so the scroll bar covers them.
    pub total: f32,
}

/// The span of the rows in view. The scroll position comes in points; row numbers are whole.
pub fn span(scroll_y: f32, view_height: f32, row_height: f32, count: usize) -> Span {
    if count == 0 || row_height <= 0.0 {
        return Span {
            rows: 0..0,
            top: 0.0,
            total: 0.0,
        };
    }
    let top_edge = scroll_y.max(0.0);
    let bottom_edge = top_edge + view_height.max(0.0);
    // Each row's top is the sum of the heights above it, accumulated in `f32`. The sum is exact
    // for whole-point heights up to about 16.7 million points, far beyond any list shown here.
    let mut first = None; // (index, top, top of the row above) of the first row that touches the view
    let mut last = None; // index of the first row that starts below the view
    let mut above = 0.0_f32;
    let mut top = 0.0_f32;
    for index in 0..count {
        let bottom = top + row_height;
        if first.is_none() && bottom > top_edge {
            first = Some((index, top, above));
        }
        if last.is_none() && top >= bottom_edge {
            last = Some(index);
        }
        above = top;
        top = bottom;
    }
    // Without a row in view, the span is the last row, so it still sits inside the list.
    let (first_index, _, top_above_first) = first.unwrap_or((count, top, above));
    // One row above the first one that touches the view, and never before the first row.
    let start = first_index.max(1) - 1;
    // One row below the last one that touches the view, and never past the last row.
    let end = last.map_or(count, |index| index + 1);
    let start = start.min(end);
    let top_of_start = if start + 1 == first_index.max(1) {
        top_above_first
    } else {
        0.0
    };
    Span {
        rows: start..end,
        top: top_of_start,
        total: top,
    }
}

/// The rows in view; see [`span`].
pub fn row_range(scroll_y: f32, view_height: f32, row_height: f32, count: usize) -> Range<usize> {
    span(scroll_y, view_height, row_height, count).rows
}

/// The copied text: a header line of labels, one line per shown row, then the footer if any.
/// Fields are joined by tabs and lines by newlines, with no trailing newline. Tree rows are not
/// indented.
pub fn to_tsv(
    columns: &[Column],
    rows: &[Row],
    shown: &[usize],
    footer: Option<&[Cell]>,
) -> String {
    let mut lines = vec![line(columns.iter().map(|column| column.label.as_str()))];
    for &index in shown {
        if let Some(row) = rows.get(index) {
            lines.push(line(row.cells.iter().map(|cell| cell.plain.as_str())));
        }
    }
    if let Some(cells) = footer {
        lines.push(line(cells.iter().map(|cell| cell.plain.as_str())));
    }
    lines.join("\n")
}

fn line<'a>(fields: impl Iterator<Item = &'a str>) -> String {
    fields.map(field).collect::<Vec<_>>().join("\t")
}

/// A field with its tabs and line breaks turned into spaces, so it stays in its cell.
fn field(text: &str) -> String {
    text.chars()
        .map(|c| {
            if matches!(c, '\t' | '\n' | '\r') {
                ' '
            } else {
                c
            }
        })
        .collect()
}

/// The height of one row, in points, at each density.
pub fn row_height(density: Density) -> f32 {
    match density {
        Density::Sparse => 28.0,
        Density::Normal => 24.0,
        Density::Dense => 20.0,
    }
}

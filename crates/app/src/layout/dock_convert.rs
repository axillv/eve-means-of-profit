//! The conversion between a layout and the arrangement egui_dock draws.
//!
//! A split's `percent` is the share of its first (left or top) part, and egui_dock's fraction is
//! the same share. The screen gets its fractions from dragging, so reading back rounds to the
//! nearest whole percent and keeps both parts. That is screen geometry, not typed input, so the
//! result is kept inside 1 to 99 rather than reported as out of range.

use egui_dock::{DockState, Node as DockNode, NodeIndex, Tree};

use crate::layout::model::{Dir, Layout, Node, Slot};

/// The arrangement that shows `layout` on the main surface.
pub fn to_dock(layout: &Layout) -> DockState<Slot> {
    match &layout.root {
        None => DockState::new(Vec::new()),
        Some(root) => {
            let mut dock = DockState::new(first_tabs(root));
            build(dock.main_surface_mut(), NodeIndex::root(), root);
            dock
        }
    }
}

/// The layout the main surface shows, under `name`.
pub fn from_dock(dock: &DockState<Slot>, name: &str) -> Layout {
    let tree = dock.main_surface();
    let root = if tree.is_empty() {
        None
    } else {
        convert(tree, NodeIndex::root())
    };
    Layout {
        name: name.to_string(),
        root,
    }
}

/// The tabs a node shows first: the leaf that `build` starts from.
fn first_tabs(node: &Node) -> Vec<Slot> {
    match node {
        Node::Panel(slot) => vec![slot.clone()],
        Node::Tabs(slots) => slots.clone(),
        Node::Split { first, .. } => first_tabs(first),
    }
}

/// Splits the leaf `at`, which already shows the first tabs of `node`, until it matches `node`.
fn build(tree: &mut Tree<Slot>, at: NodeIndex, node: &Node) {
    if let Node::Split {
        dir,
        percent,
        first,
        second,
    } = node
    {
        let fraction = f32::from(*percent) / 100.0;
        let tabs = first_tabs(second);
        let [left, right] = match dir {
            Dir::Right => tree.split_right(at, fraction, tabs),
            Dir::Down => tree.split_below(at, fraction, tabs),
        };
        build(tree, left, first);
        build(tree, right, second);
    }
}

fn convert(tree: &Tree<Slot>, index: NodeIndex) -> Option<Node> {
    match &tree[index] {
        DockNode::Empty => None,
        DockNode::Leaf(leaf) => match leaf.tabs.as_slice() {
            [] => None,
            [only] => Some(Node::Panel(only.clone())),
            tabs => Some(Node::Tabs(tabs.to_vec())),
        },
        DockNode::Horizontal(split) => combine(
            Dir::Right,
            split.fraction,
            convert(tree, index.left()),
            convert(tree, index.right()),
        ),
        DockNode::Vertical(split) => combine(
            Dir::Down,
            split.fraction,
            convert(tree, index.left()),
            convert(tree, index.right()),
        ),
    }
}

/// A split of two parts, or the one part left when the other is empty.
fn combine(dir: Dir, fraction: f32, first: Option<Node>, second: Option<Node>) -> Option<Node> {
    match (first, second) {
        (Some(first), Some(second)) => Some(Node::Split {
            dir,
            percent: percent_of(fraction),
            first: Box::new(first),
            second: Box::new(second),
        }),
        (Some(one), None) | (None, Some(one)) => Some(one),
        (None, None) => None,
    }
}

/// The whole percent nearest to `fraction` times 100, halves rounding up, kept within 1 to 99.
fn percent_of(fraction: f32) -> u8 {
    let target = f64::from(fraction) * 100.0;
    (1..=99u8).fold(1, |best, percent| {
        if (f64::from(percent) - target).abs() <= (f64::from(best) - target).abs() {
            percent
        } else {
            best
        }
    })
}

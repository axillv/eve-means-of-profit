#![cfg(test)]
//! Shared helpers for the UI tests: find a label, check that two texts share a row, click, hover
//! and read the tooltip, check a colour, wait for the app, and take a snapshot. Every helper is
//! generic over the harness state, so the helpers' own tests can use a small test UI.
use std::thread;
use std::time::Duration;

use egui_kittest::kittest::{NodeT, Queryable};
use egui_kittest::{Harness, Node};

/// The first node that shows `text`: labelled with it, or holding it as a value (dropdowns and
/// table cells keep their text as a value). None when no node does.
pub(crate) fn find<'h, S>(harness: &'h Harness<'_, S>, text: &'h str) -> Option<Node<'h>> {
    harness
        .query_all_by_label(text)
        .next()
        .or_else(|| harness.query_all_by_value(text).next())
}

/// The first node showing `text`, or a panic naming it.
fn first<'h, S>(harness: &'h Harness<'_, S>, label: &'h str) -> Node<'h> {
    find(harness, label).unwrap_or_else(|| panic!("no node labelled {label:?}"))
}

/// Whether some node shows exactly `text`, as its label or as its value (dropdowns and table
/// cells keep their text as a value).
pub(crate) fn shown<S>(harness: &Harness<'_, S>, text: &str) -> bool {
    find(harness, text).is_some()
}

/// Runs frames until `done` holds, sleeping between frames so the app's worker thread can work.
pub(crate) fn until<S>(
    harness: &mut Harness<'_, S>,
    what: &str,
    done: impl Fn(&Harness<'_, S>) -> bool,
) {
    for _ in 0..500 {
        harness.run();
        if done(harness) {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("{what} did not show");
}

/// The height of the first node labelled `label`, to tell the rows apart.
pub(crate) fn top<S>(harness: &Harness<'_, S>, label: &str) -> f32 {
    harness
        .query_all_by_label(label)
        .next()
        .unwrap_or_else(|| panic!("no node labelled {label:?}"))
        .rect()
        .min
        .y
}

/// Whether, for every text in `cells`, some node with that text sits on the same row as some node
/// labelled `anchor`: their centres are within 3 points vertically. No `anchor` node gives false.
pub(crate) fn on_row<S>(harness: &Harness<'_, S>, anchor: &str, cells: &[&str]) -> bool {
    let anchors: Vec<f32> = harness
        .query_all_by_label(anchor)
        .map(|node| node.rect().center().y)
        .collect();
    if anchors.is_empty() {
        return false;
    }
    cells.iter().all(|cell| {
        harness.query_all_by_label(cell).any(|node| {
            let y = node.rect().center().y;
            anchors.iter().any(|row| (y - row).abs() <= 3.0)
        })
    })
}

/// Clicks the first node showing `label` (as its label or its value, so a dropdown's current
/// choice can be pressed): it is scrolled into view and hovered first, so a drop-down closed by
/// the pick stays open for the frame the press needs.
pub(crate) fn click<S>(harness: &mut Harness<'_, S>, label: &str) {
    first(harness, label).scroll_to_me();
    harness.run_steps(2);
    first(harness, label).hover();
    harness.run_steps(2);
    first(harness, label).click();
    harness.run_steps(3);
}

/// Every label in the tree, in tree order, repeats included.
fn labels<S>(harness: &Harness<'_, S>) -> Vec<String> {
    let root = harness.root();
    let found: Vec<String> = root
        .children_recursive()
        .filter_map(|node| {
            let node = node.accesskit_node();
            node.label().or_else(|| node.value())
        })
        .collect();
    found
}

/// The labels whose count went up from `before` to `now`, each once, in tree order.
fn new_labels(before: &[String], now: &[String]) -> Vec<String> {
    let count = |list: &[String], label: &str| list.iter().filter(|l| l.as_str() == label).count();
    let mut found: Vec<String> = Vec::new();
    for label in now {
        if count(now, label) > count(before, label) && !found.contains(label) {
            found.push(label.clone());
        }
    }
    found
}

/// Waits up to 8 frames for a tooltip to appear after the pointer rests, then one more frame, and
/// returns what it says. Nothing new: an empty list.
fn tooltip<S>(harness: &mut Harness<'_, S>, before: &[String]) -> Vec<String> {
    for _ in 0..8 {
        harness.step();
        if new_labels(before, &labels(harness)).is_empty() {
            continue;
        }
        harness.step();
        return new_labels(before, &labels(harness));
    }
    Vec::new()
}

/// Scrolls to the first node labelled `label`, hovers it, and returns the tooltip it shows.
pub(crate) fn hover<S>(harness: &mut Harness<'_, S>, label: &str) -> Vec<String> {
    first(harness, label).scroll_to_me();
    harness.step();
    let before = labels(harness);
    first(harness, label).hover();
    tooltip(harness, &before)
}

/// Moves the pointer to `at` and returns the tooltip it shows.
pub(crate) fn hover_at<S>(harness: &mut Harness<'_, S>, at: egui::Pos2) -> Vec<String> {
    let before = labels(harness);
    harness.hover_at(at);
    tooltip(harness, &before)
}

/// Whether some pixel inside `area` (in points) is coloured rather than grey: its channels differ
/// by more than 60. Amber passes; grey and white text do not.
pub(crate) fn coloured<S>(harness: &mut Harness<'_, S>, area: egui::Rect) -> bool {
    let per_point = harness.ctx.pixels_per_point();
    let image = harness.render().expect("the frame draws");
    image.enumerate_pixels().any(|(x, y, pixel)| {
        // Pixel centres, in points.
        let x = (f32::from(u16::try_from(x).expect("a frame under 65536 pixels wide")) + 0.5)
            / per_point;
        let y = (f32::from(u16::try_from(y).expect("a frame under 65536 pixels tall")) + 0.5)
            / per_point;
        let [r, g, b, _] = pixel.0;
        area.contains(egui::pos2(x, y)) && r.max(g).max(b) - r.min(g).min(b) > 60
    })
}

/// Takes a snapshot named `name`, with no hover highlight: the pointer is removed and frames run.
pub(crate) fn snapshot<S>(harness: &mut Harness<'_, S>, name: &str) {
    harness.remove_cursor();
    harness.run();
    harness.snapshot(name);
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Color32, vec2};

    fn test_ui(ui: &mut egui::Ui, presses: &mut u32) {
        ui.label("Alpha");
        ui.horizontal(|ui| {
            ui.label("Row one");
            ui.label("10.00");
        });
        ui.horizontal(|ui| {
            ui.label("Row two");
            ui.label("20.00");
        });
        ui.label("Plain").on_hover_text("Tip one");
        ui.label("Twice").on_hover_text("Alpha");
        ui.colored_label(Color32::from_rgb(230, 160, 30), "Amber");
        ui.label("Grey");
        if ui.button("Count").clicked() {
            *presses += 1;
        }
        ui.label(format!("Clicked {presses}"));
    }

    fn harness() -> Harness<'static, u32> {
        let mut harness = Harness::new_ui_state(test_ui, 0_u32);
        harness.set_size(vec2(400.0, 300.0));
        harness.run();
        harness
    }

    fn rect_of(harness: &Harness<'_, u32>, label: &str) -> egui::Rect {
        first(harness, label).rect()
    }

    #[test]
    fn shown_finds_exact_labels_only() {
        let harness = harness();
        assert!(shown(&harness, "Alpha"));
        assert!(!shown(&harness, "Beta"));
    }

    #[test]
    fn top_orders_rows_and_panics_on_a_missing_label() {
        let harness = harness();
        assert!(top(&harness, "Row one") < top(&harness, "Row two"));
    }

    #[test]
    #[should_panic(expected = "no node labelled \"Missing\"")]
    fn top_panics_on_a_missing_label() {
        let harness = harness();
        top(&harness, "Missing");
    }

    #[test]
    fn on_row_matches_cells_on_the_anchor_row() {
        let harness = harness();
        assert!(on_row(&harness, "Row one", &["10.00"]));
        assert!(!on_row(&harness, "Row one", &["20.00"]));
        assert!(on_row(&harness, "Row two", &["20.00"]));
        assert!(!on_row(&harness, "Row one", &["10.00", "20.00"]));
        assert!(!on_row(&harness, "Nothing", &["10.00"]));
    }

    #[test]
    fn click_presses_each_time_once() {
        let mut harness = harness();
        click(&mut harness, "Count");
        assert!(shown(&harness, "Clicked 1"));
        click(&mut harness, "Count");
        assert!(shown(&harness, "Clicked 2"));
    }

    #[test]
    fn hover_reads_the_tooltip() {
        let mut harness = harness();
        assert_eq!(hover(&mut harness, "Plain"), vec!["Tip one"]);
        assert!(hover(&mut harness, "Grey").is_empty());
        assert_eq!(hover(&mut harness, "Twice"), vec!["Alpha"]);
    }

    #[test]
    #[should_panic(expected = "no node labelled \"Nope\"")]
    fn hover_panics_on_a_missing_label() {
        let mut harness = harness();
        hover(&mut harness, "Nope");
    }

    #[test]
    fn hover_at_reads_the_tooltip_at_a_point() {
        let mut harness = harness();
        let centre = rect_of(&harness, "Plain").center();
        assert_eq!(hover_at(&mut harness, centre), vec!["Tip one"]);
    }

    #[test]
    fn coloured_tells_amber_from_grey() {
        let mut harness = harness();
        let amber = rect_of(&harness, "Amber");
        let grey = rect_of(&harness, "Grey");
        assert!(coloured(&mut harness, amber));
        assert!(!coloured(&mut harness, grey));
    }

    #[test]
    fn until_returns_when_done() {
        let mut harness = harness();
        until(&mut harness, "the count", |h| shown(h, "Clicked 0"));
    }

    #[test]
    #[should_panic(expected = "the moon did not show")]
    fn until_panics_when_never_done() {
        let mut harness = harness();
        until(&mut harness, "the moon", |_| false);
    }

    #[test]
    fn snapshot_has_no_hover_highlight() {
        let mut harness = harness();
        click(&mut harness, "Count");
        snapshot(&mut harness, "ui-helpers");
        assert_eq!(harness.ctx.pointer_hover_pos(), None);
    }
}

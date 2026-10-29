//! Acceptance tests for the table: the sort, the rows in view, the folds, the copied text, and the
//! widget on screen. Row keys in the test table are 1 to 7, in the order of the rows.
#[cfg(test)]
mod tests {

    use std::rc::Rc;
    use std::sync::{Arc, Mutex};

    use egui::{
        Context, Event, Id, Modifiers, MouseWheelUnit, OutputCommand, PointerButton, Pos2,
        TextStyle, TouchPhase, Ui, Vec2, pos2, vec2,
    };
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;
    use eve_means_of_profit::dock::show_locked;
    use eve_means_of_profit::images::{self, Fetch, Picture};
    use eve_means_of_profit::table::model::{
        Cell, Column, ColumnKind, Direction, Row, row_height, row_range, sort_order, span, to_tsv,
        visible,
    };
    use eve_means_of_profit::table::{
        Table, TableResponse, sort_from_text, sort_mark_points, sort_to_text,
    };
    use eve_means_of_profit::theme::Style;
    use mop_core::money::Money;
    use mop_store::Density;
    use std::collections::BTreeSet;
    use tempfile::tempdir;

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

    /// A row with a missing value when `value` is `None`; amounts are in ten-thousandths of ISK.
    fn row(key: u64, depth: u32, item: &str, need: i128, value: Option<i64>) -> Row {
        Row {
            key,
            depth,
            cells: vec![
                Cell::text(item),
                Cell::int(need, need.to_string()),
                value.map_or_else(Cell::missing, |amount| {
                    Cell::money(Money::from_ten_thousandths(amount))
                }),
            ],
        }
    }

    /// The seven rows in tree order, with the keys 1 to 7.
    fn test_rows() -> Vec<Row> {
        vec![
            row(1, 0, "Hobgoblin II", 10, Some(10_000_000)),
            row(2, 1, "Particle Accelerator Unit", 10, Some(3_090_000_000)),
            row(3, 2, "Crystalline Carbonide", 166, None),
            row(4, 2, "Fullerides", 59, Some(1_500_000)),
            row(5, 1, "Hobgoblin I", 10, Some(2_750_000_000)),
            row(6, 1, "Morphite", 10, None),
            row(7, 0, "Rifter", 31, Some(21_250)),
        ]
    }

    fn test_footer() -> Vec<Cell> {
        vec![
            Cell::text("Total"),
            Cell::missing(),
            Cell::money(Money::from_ten_thousandths(5_851_521_250)),
        ]
    }

    fn fold(keys: &[u64]) -> BTreeSet<u64> {
        keys.iter().copied().collect()
    }

    #[test]
    fn sort_without_a_sort_keeps_the_given_order() {
        let rows = test_rows();
        assert_eq!(
            sort_order(&rows, None, Direction::Ascending),
            vec![0, 1, 2, 3, 4, 5, 6]
        );
    }

    #[test]
    fn sort_by_item_keeps_children_under_their_parent() {
        let rows = test_rows();
        assert_eq!(
            sort_order(&rows, Some(0), Direction::Ascending),
            vec![0, 4, 5, 1, 2, 3, 6]
        );
        assert_eq!(
            sort_order(&rows, Some(0), Direction::Descending),
            vec![6, 0, 1, 3, 2, 5, 4]
        );
    }

    #[test]
    fn sort_by_value_puts_missing_last_in_both_directions() {
        let rows = test_rows();
        assert_eq!(
            sort_order(&rows, Some(2), Direction::Descending),
            vec![0, 1, 3, 2, 4, 5, 6]
        );
        assert_eq!(
            sort_order(&rows, Some(2), Direction::Ascending),
            vec![6, 0, 4, 1, 3, 2, 5]
        );
    }

    #[test]
    fn sort_is_stable_for_equal_keys() {
        let rows = test_rows();
        assert_eq!(
            sort_order(&rows, Some(1), Direction::Ascending),
            vec![0, 1, 3, 2, 4, 5, 6]
        );
        assert_eq!(
            sort_order(&rows, Some(1), Direction::Descending),
            vec![6, 0, 1, 2, 3, 4, 5]
        );
    }

    #[test]
    fn a_sort_saves_as_its_column_id_and_direction() {
        let columns = columns();
        assert_eq!(
            sort_to_text(&columns, Some((1, Direction::Descending))),
            "need:desc"
        );
        assert_eq!(
            sort_to_text(&columns, Some((0, Direction::Ascending))),
            "item:asc"
        );
        assert_eq!(sort_to_text(&columns, None), "");
        assert_eq!(sort_to_text(&columns, Some((9, Direction::Ascending))), "");
    }

    #[test]
    fn a_saved_sort_reads_back_by_column_id() {
        let columns = columns();
        assert_eq!(
            sort_from_text(&columns, "value:asc"),
            Some((2, Direction::Ascending))
        );
        assert_eq!(sort_from_text(&columns, "bogus:asc"), None);
        assert_eq!(sort_from_text(&columns, "need"), None);
        assert_eq!(sort_from_text(&columns, "need:up"), None);
        assert_eq!(sort_from_text(&columns, ""), None);
    }

    #[test]
    fn visible_hides_the_descendants_of_collapsed_rows() {
        let rows = test_rows();
        let unsorted: Vec<usize> = (0..7).collect();
        assert_eq!(visible(&rows, &unsorted, &fold(&[2])), vec![0, 1, 4, 5, 6]);
        assert_eq!(visible(&rows, &unsorted, &fold(&[1])), vec![0, 6]);
        assert_eq!(visible(&rows, &unsorted, &fold(&[7])), unsorted);
        let by_item = sort_order(&rows, Some(0), Direction::Ascending);
        assert_eq!(visible(&rows, &by_item, &fold(&[2])), vec![0, 4, 5, 1, 6]);
    }

    #[test]
    fn sort_keeps_missing_siblings_in_their_given_order_in_both_directions() {
        // Under one parent: a present value, then two missing ones, in that order.
        let rows = vec![
            row(1, 0, "Parent", 1, None),
            row(2, 1, "First missing", 1, None),
            row(3, 1, "Second missing", 1, None),
            row(4, 1, "Present", 1, Some(1)),
        ];
        assert_eq!(
            sort_order(&rows, Some(2), Direction::Ascending),
            vec![0, 3, 1, 2]
        );
        assert_eq!(
            sort_order(&rows, Some(2), Direction::Descending),
            vec![0, 3, 1, 2]
        );
    }

    #[test]
    fn sort_keeps_many_equal_missing_siblings_in_their_given_order() {
        let mut rows = vec![row(1, 0, "Parent", 1, None)];
        for key in 2..=26 {
            rows.push(row(key, 1, "Missing", 1, None));
        }
        let given: Vec<usize> = std::iter::once(0).chain(1..26).collect();
        assert_eq!(sort_order(&rows, Some(2), Direction::Ascending), given);
        assert_eq!(sort_order(&rows, Some(2), Direction::Descending), given);
    }

    #[test]
    fn sort_puts_missing_siblings_last_among_present_ones_in_both_directions() {
        // Under one parent, present and missing values interleaved.
        let rows = vec![
            row(1, 0, "Parent", 1, None),
            row(2, 1, "Missing a", 1, None),
            row(3, 1, "Five", 1, Some(50_000)),
            row(4, 1, "Missing b", 1, None),
            row(5, 1, "Three", 1, Some(30_000)),
            row(6, 1, "Missing c", 1, None),
        ];
        assert_eq!(
            sort_order(&rows, Some(2), Direction::Ascending),
            vec![0, 4, 2, 1, 3, 5]
        );
        assert_eq!(
            sort_order(&rows, Some(2), Direction::Descending),
            vec![0, 2, 4, 1, 3, 5]
        );
    }

    #[test]
    fn span_gives_the_top_of_the_first_row_in_view_and_the_total_height() {
        let view = span(2_200.0, 600.0, 22.0, 50_000);
        assert_eq!(view.rows, 99..129);
        assert_eq!(view.top, 2_178.0);
        assert_eq!(view.total, 1_100_000.0);
        assert_eq!(span(0.0, 600.0, 0.0, 10).rows, 0..0);
    }

    #[test]
    fn row_range_covers_the_view_with_one_row_either_side() {
        assert_eq!(row_range(0.0, 600.0, 22.0, 50_000), 0..29);
        assert_eq!(row_range(2_200.0, 600.0, 22.0, 50_000), 99..129);
        assert_eq!(row_range(0.0, 600.0, 22.0, 10), 0..10);
        assert_eq!(row_range(0.0, 600.0, 18.0, 0), 0..0);
    }

    #[test]
    fn to_tsv_copies_the_rows_and_the_footer_as_plain_values() {
        let columns = columns();
        let rows = test_rows();
        let shown: Vec<usize> = (0..7).collect();
        let footer = test_footer();
        assert_eq!(
            to_tsv(&columns, &rows, &shown, Some(&footer)),
            "Item\tNeed\tValue\nHobgoblin II\t10\t1000\nParticle Accelerator Unit\t10\t309000\nCrystalline Carbonide\t166\t\nFullerides\t59\t150\nHobgoblin I\t10\t275000\nMorphite\t10\t\nRifter\t31\t2.125\nTotal\t\t585152.125"
        );
    }

    #[test]
    fn to_tsv_turns_tabs_and_newlines_in_a_field_into_spaces() {
        let columns = columns();
        let rows = vec![Row {
            key: 1,
            depth: 0,
            cells: vec![Cell::text("a\tb\nc"), Cell::int(1, "1"), Cell::missing()],
        }];
        assert_eq!(
            to_tsv(&columns, &rows, &[0], None),
            "Item\tNeed\tValue\na b c\t1\t"
        );
    }

    #[test]
    fn row_heights_follow_the_density() {
        assert_eq!(row_height(Density::Sparse), 28.0);
        assert_eq!(row_height(Density::Normal), 24.0);
        assert_eq!(row_height(Density::Dense), 20.0);
    }

    /// A primary click at `pos`: the pointer moves there, then presses and releases.
    fn click_at(harness: &Harness<'_>, pos: Pos2) {
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

    /// What the table showed and reported, frame by frame.
    #[derive(Default)]
    struct Seen {
        /// The keys whose Item cell was drawn in the last frame, in the order they were drawn.
        order: Vec<u64>,
        /// How many rows were drawn in each frame.
        drawn: Vec<usize>,
        responses: Vec<TableResponse>,
    }

    /// A harness with the table in a pane of `size`. `checkbox_key` draws a checkbox in the Need
    /// column of that row.
    fn table_harness(
        rows: Vec<Row>,
        footer: Option<Vec<Cell>>,
        density: Density,
        size: Vec2,
        checkbox_key: Option<u64>,
        seen: &Arc<Mutex<Seen>>,
    ) -> Harness<'static> {
        let seen = Arc::clone(seen);
        let columns = columns();
        let style = Style::eve_dense();
        Harness::builder()
            .with_size(size)
            .build_ui(move |ui: &mut Ui| {
                style.apply(ui.ctx());
                let mut order = Vec::new();
                let mut checked = false;
                let response = {
                    let mut draw = |ui: &mut Ui, key: u64, column: &'static str| -> bool {
                        if column == "item" {
                            order.push(key);
                        }
                        if checkbox_key == Some(key) && column == "need" {
                            ui.checkbox(&mut checked, "Pick");
                            return true;
                        }
                        false
                    };
                    Table::new(Id::new("table test"), &columns).show_with(
                        ui,
                        &style,
                        density,
                        &rows,
                        footer.as_deref(),
                        &mut draw,
                    )
                };
                if let Ok(mut seen) = seen.lock() {
                    seen.drawn.push(order.len());
                    seen.order = order;
                    seen.responses.push(response);
                }
            })
    }

    #[test]
    fn a_big_table_lays_out_only_the_rows_in_view_and_scrolls_to_the_end() {
        let rows: Vec<Row> = (0..50_000u64)
            .map(|index| row(index, 0, &format!("Item {index}"), i128::from(index), None))
            .collect();
        let seen = Arc::new(Mutex::new(Seen::default()));
        let mut harness =
            table_harness(rows, None, Density::Normal, vec2(800.0, 600.0), None, &seen);
        harness.run();
        harness.hover_at(pos2(300.0, 300.0));
        harness.input_mut().events.push(Event::MouseWheel {
            unit: MouseWheelUnit::Point,
            delta: vec2(0.0, -1_000_000_000.0),
            phase: TouchPhase::Move,
            modifiers: Modifiers::NONE,
        });
        // The scroll eases on for several frames, so run a fixed number rather than until still.
        harness.run_steps(60);
        // The last row is on screen below the header, not just drawn somewhere.
        let header = harness.get_by_label("Item").rect();
        let last = harness.get_by_label("Item 49999").rect();
        assert!(last.center().y > header.center().y && last.center().y < 600.0);
        let seen = seen.lock().unwrap();
        assert!(!seen.drawn.is_empty());
        assert!(seen.drawn.iter().all(|&count| count <= 32));
        assert!(seen.order.contains(&49_999));
    }

    #[test]
    fn clicking_the_need_header_sorts_descending_then_ascending() {
        let seen = Arc::new(Mutex::new(Seen::default()));
        let mut harness = table_harness(
            test_rows(),
            Some(test_footer()),
            Density::Normal,
            vec2(800.0, 400.0),
            None,
            &seen,
        );
        harness.run();
        harness.get_by_label("Need").click();
        harness.run();
        assert_eq!(seen.lock().unwrap().order, vec![7, 1, 2, 3, 4, 5, 6]);
        harness.get_by_label("Need").click();
        harness.run();
        assert_eq!(seen.lock().unwrap().order, vec![1, 2, 4, 3, 5, 6, 7]);
    }

    #[test]
    fn clicking_another_column_sorts_it_descending_first() {
        let seen = Arc::new(Mutex::new(Seen::default()));
        let mut harness = table_harness(
            test_rows(),
            None,
            Density::Normal,
            vec2(800.0, 400.0),
            None,
            &seen,
        );
        harness.run();
        harness.get_by_label("Value").click();
        harness.run();
        harness.get_by_label("Need").click();
        harness.run();
        // Need descending: the Value sort is replaced, not flipped.
        assert_eq!(seen.lock().unwrap().order, vec![7, 1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn clicking_a_fold_arrow_hides_its_children_and_reports_the_toggle() {
        let seen = Arc::new(Mutex::new(Seen::default()));
        let mut harness = table_harness(
            test_rows(),
            None,
            Density::Normal,
            vec2(800.0, 400.0),
            None,
            &seen,
        );
        harness.run();
        // The arrow sits just left of the text of the second row, Particle Accelerator Unit,
        // the row with two children.
        let text = harness.get_by_label("Particle Accelerator Unit").rect();
        click_at(&harness, pos2(text.min.x - 7.0, text.center().y));
        harness.run();
        let seen = seen.lock().unwrap();
        assert!(
            seen.responses
                .iter()
                .any(|response| { response.toggled == Some(2) && response.clicked.is_none() })
        );
        assert_eq!(seen.order, vec![1, 2, 5, 6, 7]);
    }

    #[test]
    fn rows_are_stacked_one_row_height_apart_and_start_at_the_top() {
        let seen = Arc::new(Mutex::new(Seen::default()));
        let mut harness = table_harness(
            test_rows(),
            None,
            Density::Normal,
            vec2(800.0, 400.0),
            None,
            &seen,
        );
        harness.run();
        let first = harness.get_by_label("Hobgoblin II").rect();
        let second = harness.get_by_label("Particle Accelerator Unit").rect();
        // Normal density is 24 points per row; the label of each row sits at the row's centre.
        assert!((second.center().y - first.center().y - 24.0).abs() < 0.5);
        let header = harness.get_by_label("Item").rect();
        assert!(
            first.center().y > header.center().y,
            "the first row sits under the header"
        );
    }

    #[test]
    fn clicking_an_item_reports_the_row() {
        let seen = Arc::new(Mutex::new(Seen::default()));
        let mut harness = table_harness(
            test_rows(),
            None,
            Density::Normal,
            vec2(800.0, 400.0),
            None,
            &seen,
        );
        harness.run();
        harness.get_by_label("Hobgoblin I").click();
        harness.run();
        assert!(
            seen.lock()
                .unwrap()
                .responses
                .iter()
                .any(|response| response.clicked == Some(5))
        );
    }

    #[test]
    fn a_missing_value_shows_a_dash() {
        let seen = Arc::new(Mutex::new(Seen::default()));
        let mut harness = table_harness(
            test_rows(),
            Some(test_footer()),
            Density::Normal,
            vec2(800.0, 400.0),
            None,
            &seen,
        );
        harness.run();
        // Two missing values in the rows, and one in the footer.
        assert_eq!(harness.query_all_by_label("—").count(), 3);
    }

    #[test]
    fn a_custom_cell_is_drawn_by_the_callback_and_its_click_is_not_a_row_click() {
        let seen = Arc::new(Mutex::new(Seen::default()));
        let mut harness = table_harness(
            test_rows(),
            None,
            Density::Normal,
            vec2(800.0, 400.0),
            Some(5),
            &seen,
        );
        harness.run();
        // The checkbox sits in the Need column, on the row of Hobgoblin I: right of its Item text,
        // left of the Value column, and level with its text.
        let pick = harness.get_by_label("Pick").rect();
        let item = harness.get_by_label("Hobgoblin I").rect();
        let value_header = harness.get_by_label("Value").rect();
        assert!(
            pick.min.x > item.max.x,
            "the checkbox is right of the Item cell"
        );
        assert!(
            pick.max.x < value_header.min.x,
            "the checkbox is left of the Value cell"
        );
        assert!((pick.center().y - item.center().y).abs() < 2.0);
        harness.get_by_label("Pick").click();
        harness.run();
        assert!(
            seen.lock()
                .unwrap()
                .responses
                .iter()
                .all(|response| response.clicked != Some(5))
        );
    }

    #[derive(Clone, Copy)]
    enum OnlyPanel {
        Panel,
    }

    fn only_title(_: OnlyPanel) -> &'static str {
        "Only panel"
    }

    #[test]
    fn the_copy_table_button_copies_its_table_as_tsv() {
        let columns = columns();
        let rows = test_rows();
        let style = Style::eve_dense();
        let mut state = egui_dock::DockState::new(vec![OnlyPanel::Panel]);
        let mut harness = Harness::builder()
            .with_size(vec2(800.0, 400.0))
            .build_ui(move |ui| {
                show_locked(ui, &style, &mut state, only_title, |ui, _| {
                    Table::new(Id::new("Only panel"), &columns).show(
                        ui,
                        &style,
                        Density::Normal,
                        &rows,
                        None,
                    );
                });
            });
        // The body's table sets the flag on the first frame, so the button shows from the second.
        harness.run();
        harness.run();
        assert_eq!(harness.query_all_by_label("Copy table").count(), 1);
        harness.get_by_label("Copy table").click();
        // The copy happens in the frame of the click, so collect the copies from each frame.
        let mut copied = Vec::new();
        for _ in 0..5 {
            harness.step();
            for command in &harness.output().platform_output.commands {
                if let OutputCommand::CopyText(text) = command {
                    copied.push(text.clone());
                }
            }
        }
        assert_eq!(copied.len(), 1);
        assert!(copied[0].starts_with("Item\tNeed\tValue\nHobgoblin II\t10\t1000\n"));
        assert!(copied[0].ends_with("Rifter\t31\t2.125"));
    }

    #[test]
    fn a_panel_without_a_table_has_no_copy_button_and_no_menu() {
        let style = Style::eve_dense();
        let mut state = egui_dock::DockState::new(vec![OnlyPanel::Panel]);
        let mut harness = Harness::builder()
            .with_size(vec2(800.0, 400.0))
            .build_ui(move |ui| {
                show_locked(ui, &style, &mut state, only_title, |ui, _| {
                    ui.label("Nothing to copy");
                });
            });
        harness.run_steps(3);
        assert_eq!(harness.query_all_by_label("Copy table").count(), 0);
        assert_eq!(harness.query_all_by_label("⋯").count(), 0);
    }

    #[test]
    fn a_panel_that_stops_showing_its_table_loses_the_copy_button() {
        let columns = columns();
        let rows = test_rows();
        let style = Style::eve_dense();
        let mut state = egui_dock::DockState::new(vec![OnlyPanel::Panel]);
        let show_table = Rc::new(std::cell::Cell::new(true));
        let shown = Rc::clone(&show_table);
        let mut harness = Harness::builder()
            .with_size(vec2(800.0, 400.0))
            .build_ui(move |ui| {
                show_locked(ui, &style, &mut state, only_title, |ui, _| {
                    if shown.get() {
                        Table::new(Id::new("Only panel"), &columns).show(
                            ui,
                            &style,
                            Density::Normal,
                            &rows,
                            None,
                        );
                    } else {
                        ui.label("No table");
                    }
                });
            });
        harness.run();
        harness.run();
        assert_eq!(harness.query_all_by_label("Copy table").count(), 1);
        // With the table gone, the next frame clears the flag (the button still shows for that
        // frame) and the frame after that has no button.
        show_table.set(false);
        harness.run();
        harness.run();
        assert_eq!(harness.query_all_by_label("Copy table").count(), 0);
    }

    #[test]
    fn the_sort_triangle_points_up_when_ascending_and_down_when_descending() {
        // A 100 by 20 cell, centred at y 10. Text marks sit at the right end, the others at the left.
        let cell = egui::Rect::from_min_size(pos2(0.0, 0.0), vec2(100.0, 20.0));
        assert_eq!(
            sort_mark_points(cell, ColumnKind::Text, Direction::Ascending),
            vec![pos2(88.0, 13.0), pos2(96.0, 13.0), pos2(92.0, 7.0)]
        );
        assert_eq!(
            sort_mark_points(cell, ColumnKind::Text, Direction::Descending),
            vec![pos2(88.0, 7.0), pos2(96.0, 7.0), pos2(92.0, 13.0)]
        );
        assert_eq!(
            sort_mark_points(cell, ColumnKind::Money, Direction::Ascending),
            vec![pos2(4.0, 13.0), pos2(12.0, 13.0), pos2(8.0, 7.0)]
        );
        assert_eq!(
            sort_mark_points(cell, ColumnKind::Money, Direction::Descending),
            vec![pos2(4.0, 7.0), pos2(12.0, 7.0), pos2(8.0, 13.0)]
        );
        assert_eq!(
            sort_mark_points(cell, ColumnKind::Number, Direction::Descending),
            vec![pos2(4.0, 7.0), pos2(12.0, 7.0), pos2(8.0, 13.0)]
        );
    }

    #[test]
    fn a_sort_change_and_a_fold_change_each_ask_for_a_repaint() {
        let seen = Arc::new(Mutex::new(Seen::default()));
        let mut harness = table_harness(
            test_rows(),
            None,
            Density::Normal,
            vec2(800.0, 400.0),
            None,
            &seen,
        );
        harness.run();
        harness.get_by_label("Need").click();
        harness.run_steps(1);
        assert!(harness.ctx.has_requested_repaint());
        harness.run();
        let text = harness.get_by_label("Particle Accelerator Unit").rect();
        click_at(&harness, pos2(text.min.x - 7.0, text.center().y));
        harness.run_steps(1);
        assert!(harness.ctx.has_requested_repaint());
    }

    #[test]
    fn the_tree_table_with_footer_matches_its_screenshot() {
        let seen = Arc::new(Mutex::new(Seen::default()));
        let mut harness = table_harness(
            test_rows(),
            Some(test_footer()),
            Density::Normal,
            vec2(800.0, 400.0),
            None,
            &seen,
        );
        harness.run();
        harness.snapshot("table-tree");
    }

    #[test]
    fn the_dense_table_matches_its_screenshot() {
        let seen = Arc::new(Mutex::new(Seen::default()));
        let mut harness = table_harness(
            test_rows(),
            Some(test_footer()),
            Density::Dense,
            vec2(800.0, 400.0),
            None,
            &seen,
        );
        harness.run();
        harness.snapshot("table-dense");
    }

    #[test]
    fn the_table_sorted_by_value_matches_its_screenshot() {
        let seen = Arc::new(Mutex::new(Seen::default()));
        let mut harness = table_harness(
            test_rows(),
            Some(test_footer()),
            Density::Normal,
            vec2(800.0, 400.0),
            None,
            &seen,
        );
        harness.run();
        harness.get_by_label("Value").click();
        harness.run();
        harness.snapshot("table-sorted");
    }

    fn tree_harness() -> (Harness<'static>, Arc<Mutex<Seen>>) {
        let seen = Arc::new(Mutex::new(Seen::default()));
        let harness = table_harness(
            test_rows(),
            None,
            Density::Normal,
            vec2(800.0, 400.0),
            None,
            &seen,
        );
        (harness, seen)
    }

    #[test]
    fn rows_without_children_sit_at_their_depth_like_their_siblings() {
        let (mut harness, _seen) = tree_harness();
        harness.run();
        let x = |label: &str| harness.get_by_label(label).rect().min.x;
        let parent = x("Hobgoblin II");
        assert!((x("Crystalline Carbonide") - parent - 28.0).abs() <= 0.5);
        assert!((x("Fullerides") - x("Crystalline Carbonide")).abs() <= 0.5);
        assert!((x("Particle Accelerator Unit") - parent - 14.0).abs() <= 0.5);
        assert!((x("Hobgoblin I") - x("Particle Accelerator Unit")).abs() <= 0.5);
        assert!((x("Morphite") - x("Particle Accelerator Unit")).abs() <= 0.5);
        assert!((x("Rifter") - parent).abs() <= 0.5);
    }

    #[test]
    fn a_click_on_a_leaf_text_at_its_new_place_reports_the_row() {
        let (mut harness, seen) = tree_harness();
        harness.run();
        let text = harness.get_by_label("Fullerides").rect();
        click_at(&harness, pos2(text.min.x + 2.0, text.center().y));
        harness.run();
        assert!(
            seen.lock()
                .unwrap()
                .responses
                .iter()
                .any(|response| response.clicked == Some(4))
        );
    }

    #[test]
    fn the_theme_sets_14_px_text_and_24_px_controls() {
        let ctx = Context::default();
        Style::eve_dense().apply(&ctx);
        let style = ctx.global_style();
        let size = |text: TextStyle| style.text_styles[&text].size;
        assert_eq!(size(TextStyle::Body), 14.0);
        assert_eq!(size(TextStyle::Button), 14.0);
        assert_eq!(size(TextStyle::Small), 11.0);
        assert_eq!(size(TextStyle::Monospace), 13.0);
        assert_eq!(size(TextStyle::Heading), 18.0);
        assert_eq!(style.spacing.interact_size.y, 24.0);
        assert_eq!(style.spacing.button_padding, vec2(8.0, 4.0));
    }

    /// A 16 by 16 picture that is red everywhere.
    const RED_PNG: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x10, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f,
        0xf3, 0xff, 0x61, 0x00, 0x00, 0x00, 0x19, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63, 0xf8,
        0xcf, 0xc0, 0xf0, 0x9f, 0x12, 0xcc, 0x30, 0x6a, 0xc0, 0xa8, 0x01, 0xa3, 0x06, 0x0c, 0x17,
        0x03, 0x00, 0x30, 0xc4, 0xfe, 0x10, 0x8a, 0xc6, 0x4c, 0xe5, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];

    struct NoImages;

    impl Fetch for NoImages {
        fn get(&self, _url: &str) -> Result<Vec<u8>, String> {
            Err("offline".to_owned())
        }
    }

    fn icon_harness(with_icons: bool) -> Harness<'static> {
        let rows = vec![
            row(34, 0, "Rifter", 1, Some(1)),
            row(99, 0, "Hobgoblin II", 1, Some(1)),
        ];
        let columns = columns();
        let style = Style::eve_dense();
        Harness::builder()
            .with_size(vec2(800.0, 400.0))
            .build_ui(move |ui: &mut Ui| {
                style.apply(ui.ctx());
                let icon = |key: u64| -> Option<u32> { u32::try_from(key).ok() };
                let table = if with_icons {
                    Table::new(Id::new("icons test"), &columns).icons("item", &icon)
                } else {
                    Table::new(Id::new("icons test"), &columns)
                };
                table.show(ui, &style, Density::Normal, &rows, None);
            })
    }

    #[test]
    fn a_kept_picture_draws_by_its_row_and_the_named_column() {
        let dir = tempdir().expect("a temporary folder");
        let path = images::cache_path(dir.path(), Picture::Type(34));
        std::fs::write(&path, RED_PNG).expect("the kept picture");

        let mut harness = icon_harness(true);
        images::init(&harness.ctx, dir.path(), Arc::new(NoImages));
        harness.run();
        assert!(images::is_loaded(&harness.ctx, Picture::Type(34)));
        assert!(!images::is_loaded(&harness.ctx, Picture::Type(99)));
        harness.get_by_label("Rifter");
        harness.get_by_label("Hobgoblin II");

        let mut plain = icon_harness(false);
        images::init(&plain.ctx, dir.path(), Arc::new(NoImages));
        plain.run();
        assert!(!images::is_loaded(&plain.ctx, Picture::Type(34)));
    }

    /// The saved sorts through a whole app: the Orders page's table, saved by its header and read
    /// back by a new app on the same data folder.
    mod saved_sort {
        use std::collections::BTreeSet;
        use std::fs;
        use std::io::Write;
        use std::ops::ControlFlow;
        use std::path::{Path, PathBuf};

        use super::*;
        use crate::ui::{click, shown, top, until};
        use eve_means_of_profit::session::{EveAccess, PendingLogin};
        use eve_means_of_profit::{App, AppConfig};
        use mop_esi::character_data::{CharacterData, Fetched, Kind, Order, Refreshed};
        use mop_esi::characters::{Character, Login};
        use mop_esi::error::EsiError;
        use mop_esi::places::{NamingReport, StructureNames};
        use mop_esi::saved;
        use mop_sde::{Fetch, FetchError, import};
        use mop_store::Store;
        use tempfile::TempDir;

        const NOW: i64 = 1_000_000;
        const MAIN: u64 = 2_112_000_001;
        const JITA_4_4: i64 = 60_003_760;
        const PAU: u32 = 11_688;
        const MORPHITE: u32 = 11_399;
        const FIXTURE: &str = "../sde/tests/fixtures/sde-3607705";

        /// The setting key of the saved sort of the table named `table`.
        fn key_of(table: &str) -> String {
            format!("table_sort.{:016x}", Id::new(table).value())
        }

        /// The game data is installed by the test, so the app never asks the network for anything.
        struct NoNetwork;

        impl Fetch for NoNetwork {
            fn get_text(&self, _url: &str) -> Result<String, FetchError> {
                Err(FetchError::Http { status: 503 })
            }

            fn download(
                &self,
                _url: &str,
                _dest: &mut dyn Write,
                _progress: &mut dyn FnMut(u64, Option<u64>) -> ControlFlow<()>,
            ) -> Result<u64, FetchError> {
                Err(FetchError::Http { status: 503 })
            }
        }

        /// A login with the characters given; logins and refreshes are not started here.
        struct FakeEve {
            characters: Vec<Character>,
        }

        impl EveAccess for FakeEve {
            fn characters(&self) -> Vec<Character> {
                self.characters.clone()
            }

            fn begin_login(&mut self) -> Result<PendingLogin, String> {
                Err("Logins are not started in these tests.".to_owned())
            }

            fn finish_login(&mut self, _code: &str) -> Result<Character, EsiError> {
                Err(EsiError::Cancelled)
            }

            fn refresh_all(
                &mut self,
                _data: &mut CharacterData,
                _now: i64,
            ) -> Vec<(Kind, Result<Refreshed, EsiError>)> {
                Vec::new()
            }

            fn name_structures(
                &mut self,
                _names: &mut StructureNames,
                _ids: &[i64],
                _now: i64,
            ) -> Result<NamingReport, EsiError> {
                Ok(NamingReport {
                    asked: 0,
                    failed: Vec::new(),
                })
            }

            fn save_character(&mut self, _data: &CharacterData) -> Result<(), EsiError> {
                Ok(())
            }

            fn remove(&mut self, _character_id: u64) -> Result<(), EsiError> {
                Err(EsiError::UnknownCharacter(_character_id))
            }

            fn keychain_warning(&self) -> Option<String> {
                None
            }

            fn jeve_folder(&self) -> Option<PathBuf> {
                None
            }
        }

        /// A data folder with the game data imported and a user's store that checks no game data.
        struct Env {
            _dir: TempDir,
            data: PathBuf,
            cache: PathBuf,
        }

        fn env() -> Env {
            let dir = TempDir::new().expect("a temp folder");
            let data = dir.path().join("data");
            let cache = dir.path().join("cache");
            fs::create_dir_all(data.join("esi")).expect("the data folder");
            let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE);
            import(&fixture, &data.join("sde.sqlite")).expect("the fixture imports");
            let mut store = Store::open(&data.join("user.sqlite"), NOW)
                .expect("the store opens")
                .store;
            let mut settings = store.settings().expect("settings");
            settings.last_sde_check = Some(NOW);
            store.save_settings(&settings).expect("settings saved");
            // The background price refresh stays off.
            let mut history = store.history_settings().expect("history settings");
            history.auto_refresh = false;
            store
                .save_history_settings(&history)
                .expect("history settings saved");
            Env {
                _dir: dir,
                data,
                cache,
            }
        }

        fn character(id: u64, name: &str) -> Character {
            Character {
                id,
                name: name.to_owned(),
                scopes: BTreeSet::new(),
                added_at: NOW,
                login: Login::Ok,
            }
        }

        fn order(
            order_id: i64,
            type_id: u32,
            is_buy: bool,
            price: Money,
            (remain, total): (u64, u64),
            expires: i64,
        ) -> Order {
            Order {
                order_id,
                type_id,
                location_id: JITA_4_4,
                region_id: 10_000_002,
                is_buy,
                price,
                volume_remain: remain,
                volume_total: total,
                issued: 900_000,
                expires,
                range: "station".to_owned(),
                escrow: is_buy.then(|| price.checked_mul_qty(total).expect("an escrow that fits")),
                min_volume: None,
                is_corporation: false,
            }
        }

        /// Main's orders: 100 of 200 PAU for sale, 10 of 10 Morphite bought (expired), and 50 of
        /// 50 PAU for sale by the corporation: PAU, Morphite, PAU in the given order.
        fn main_data(fetched: i64) -> CharacterData {
            let mut corp = order(
                3,
                PAU,
                false,
                Money::from_ten_thousandths(700_000_000),
                (50, 50),
                NOW + 260_000,
            );
            corp.is_corporation = true;
            CharacterData {
                character_id: MAIN,
                assets: None,
                orders: Some(Fetched {
                    data: vec![
                        order(
                            1,
                            PAU,
                            false,
                            Money::from_ten_thousandths(615_000_000),
                            (100, 200),
                            NOW + 260_000,
                        ),
                        order(
                            2,
                            MORPHITE,
                            true,
                            Money::from_ten_thousandths(21_000_000),
                            (10, 10),
                            NOW - 5,
                        ),
                        corp,
                    ],
                    fetched_at: fetched,
                    last_modified: None,
                    expires: None,
                }),
                jobs: None,
                wallet: None,
            }
        }

        fn start(env: &Env, characters: Vec<Character>) -> Harness<'static, App> {
            let config = AppConfig {
                data_dir: env.data.clone(),
                cache_dir: env.cache.clone(),
                fetch: Box::new(NoNetwork),
                clock: Box::new(|| NOW),
                eve: Box::new(FakeEve { characters }),
                market: Box::new(crate::offline::Offline),
            };
            Harness::builder()
                .with_size(vec2(1280.0, 800.0))
                .with_max_steps(crate::SETTLE_STEPS)
                .build_eframe(move |cc| App::new(&cc.egui_ctx, config).expect("the app starts"))
        }

        /// Runs frames until the start-up read has arrived: the page's loading line is gone.
        fn loaded(harness: &mut Harness<'_, App>) {
            until(harness, "the saved data", |h| {
                !shown(h, "Loading character data…")
            });
        }

        fn setting(harness: &mut Harness<'_, App>, key: &str) -> Option<String> {
            harness
                .state_mut()
                .shared_mut()
                .store
                .setting(key)
                .expect("the setting reads")
        }

        fn notices(harness: &mut Harness<'_, App>) -> Vec<String> {
            harness
                .state_mut()
                .shared_mut()
                .notices
                .iter()
                .map(|notice| notice.text.clone())
                .collect()
        }

        #[test]
        fn clicking_a_header_saves_the_sort_and_a_new_app_brings_it_back() {
            let env = env();
            saved::save_character(&env.data.join("esi"), &main_data(NOW - 120))
                .expect("Main's orders saved");
            let key = key_of("Orders");
            let mut harness = start(&env, vec![character(MAIN, "Main")]);
            loaded(&mut harness);
            click(&mut harness, "Orders");
            harness.run();
            // The given order is PAU, Morphite, PAU.
            assert!(top(&harness, "Particle Accelerator Unit") < top(&harness, "Morphite"));

            harness.get_by_label("Item").click();
            harness.run();
            assert_eq!(setting(&mut harness, &key).as_deref(), Some("item:asc"));
            assert!(top(&harness, "Morphite") < top(&harness, "Particle Accelerator Unit"));

            harness.get_by_label("Item").click();
            harness.run();
            assert_eq!(setting(&mut harness, &key).as_deref(), Some("item:desc"));
            assert!(top(&harness, "Particle Accelerator Unit") < top(&harness, "Morphite"));

            drop(harness);

            let mut restarted = start(&env, vec![character(MAIN, "Main")]);
            loaded(&mut restarted);
            click(&mut restarted, "Orders");
            restarted.run();
            assert!(
                top(&restarted, "Particle Accelerator Unit") < top(&restarted, "Morphite"),
                "the saved descending sort puts Morphite last with no click"
            );
        }

        #[test]
        fn a_saved_sort_naming_a_missing_column_keeps_the_given_order_and_says_nothing() {
            let env = env();
            saved::save_character(&env.data.join("esi"), &main_data(NOW - 120))
                .expect("Main's orders saved");
            {
                let mut store = Store::open(&env.data.join("user.sqlite"), NOW)
                    .expect("the store opens")
                    .store;
                store
                    .set_setting(&key_of("Orders"), "gone:asc")
                    .expect("the sort saves");
            }
            let mut harness = start(&env, vec![character(MAIN, "Main")]);
            loaded(&mut harness);
            click(&mut harness, "Orders");
            harness.run();
            assert!(top(&harness, "Particle Accelerator Unit") < top(&harness, "Morphite"));
            let notices = notices(&mut harness);
            assert!(
                notices
                    .iter()
                    .all(|notice| !notice.contains("Could not read the settings")),
                "no settings notice: {notices:?}"
            );
        }
    }
}

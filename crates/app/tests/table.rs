//! Acceptance tests for the table: the sort, the rows in view, the folds, the copied text, and the
//! widget on screen. Row keys in the test table are 1 to 7, in the order of the rows.
#[cfg(test)]
mod tests {

    use std::sync::{Arc, Mutex};

    use egui::{
        Event, Id, Modifiers, MouseWheelUnit, OutputCommand, PointerButton, Pos2, TouchPhase, Ui,
        Vec2, pos2, vec2,
    };
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;
    use eve_means_of_profit::dock::show_locked;
    use eve_means_of_profit::table::model::{
        Cell, Column, ColumnKind, Direction, Row, row_height, row_range, sort_order, span, to_tsv,
        visible,
    };
    use eve_means_of_profit::table::{Table, TableResponse};
    use eve_means_of_profit::theme::Style;
    use profit_core::money::Money;
    use profit_store::Density;
    use std::collections::BTreeSet;

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
        assert_eq!(row_height(Density::Normal), 22.0);
        assert_eq!(row_height(Density::Dense), 18.0);
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
        // Normal density is 22 points per row; the label of each row sits at the row's centre.
        assert!((second.center().y - first.center().y - 22.0).abs() < 0.5);
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
    fn the_panel_menu_copies_its_table_as_tsv() {
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
        harness.run();
        harness.get_by_label("⋯").click();
        harness.run();
        harness.get_by_label("Copy as TSV").click();
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
}

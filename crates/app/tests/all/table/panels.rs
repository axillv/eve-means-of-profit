use super::*;

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

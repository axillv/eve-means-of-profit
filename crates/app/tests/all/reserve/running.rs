use super::*;

/// The y of the Steps row labelled `row`.
fn steps_y(harness: &Harness<'_, App>, row: &str) -> f32 {
    let edge = steps_edge(harness);
    harness
        .query_all_by_label(row)
        .find(|node| node.rect().min.x < edge)
        .unwrap_or_else(|| panic!("no {row:?} row"))
        .rect()
        .center()
        .y
}

/// Scrolls the Steps table sideways by `delta` points (negative moves it left), and runs the
/// frames the animated scroll takes.
fn scroll_steps(harness: &mut Harness<'_, App>, delta: f32) {
    let y = steps_y(harness, "Hobgoblin I");
    let x = harness.get_by_label("Steps").rect().min.x + 100.0;
    harness.hover_at(pos2(x, y));
    harness.event(Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: vec2(delta, 0.0),
        phase: egui::TouchPhase::Move,
        modifiers: Modifiers::NONE,
    });
    for _ in 0..30 {
        harness.step();
    }
}

/// Hovers the cell of the Steps row labelled `row` in the column headed `header`, scrolling the
/// table sideways first when the cell is under the Buy list, and returns the hangar split
/// hovers it shows.
fn hover_cell(harness: &mut Harness<'_, App>, row: &str, header: &str) -> Vec<String> {
    for _ in 0..3 {
        let edge = steps_edge(harness);
        let (rect, _) = cell(harness, row, header)
            .unwrap_or_else(|| panic!("no {row:?} cell under {header:?}"));
        if rect.center().x < edge - 40.0 {
            harness.hover_at(rect.center());
            for _ in 0..60 {
                harness.run();
            }
            return harness
                .query_all(by().predicate(|node| node.bounding_box().is_some()))
                .filter_map(|node| {
                    let node = node.accesskit_node();
                    node.label().or_else(|| node.value())
                })
                .filter(|text| text.contains(" in hangars, "))
                .collect();
        }
        scroll_steps(harness, -(rect.center().x - edge + 120.0));
    }
    panic!("the cell {row:?} {header:?} cannot be brought into view");
}

/// Asserts the Steps headers `left`, `middle` and `right` appear in that order, left to right.
fn assert_header_order(harness: &Harness<'_, App>, left: &str, middle: &str, right: &str) {
    let y = harness.get_by_label("Make or buy").rect().center().y;
    let centre = |name: &str| {
        harness
            .query_all_by_label(name)
            .find(|node| (node.rect().center().y - y).abs() < 4.0)
            .unwrap_or_else(|| panic!("no {name:?} header"))
            .rect()
            .center()
            .x
    };
    assert!(
        centre(left) < centre(middle) && centre(middle) < centre(right),
        "{left:?}, {middle:?} and {right:?} are not in that order"
    );
}

#[test]
fn an_assigned_job_adds_to_pasted_stock() {
    let env = stock_environment();
    let c = make_plan(&env, "Hobgoblin C", 2, &[HOBGOBLIN_I], "Hobgoblin I\t1");
    assign(&env, 503, c);

    let mut harness = start(&env, login());
    click(&mut harness, "Hobgoblin C");
    assert_cells(
        &harness,
        "Hobgoblin I",
        &[("Need", "2"), ("Own", "2"), ("Make", "0")],
    );
    drop(harness);

    store(&env)
        .unassign_job(503)
        .expect("the job is unassigned");
    let mut harness = start(&env, login());
    click(&mut harness, "Hobgoblin C");
    assert_cells(
        &harness,
        "Hobgoblin I",
        &[("Need", "2"), ("Own", "1"), ("Make", "1")],
    );
}

#[test]
fn the_running_column_sits_after_own_and_shows_the_assigned_jobs() {
    let env = stock_environment();
    reserving_pair(&env);

    let mut harness = start(&env, login());
    click(&mut harness, "Hobgoblin B");
    assert_header_order(&harness, "Own", "Running", "Make");
    assert_cells(
        &harness,
        "Hobgoblin I",
        &[("Need", "2"), ("Own", "1"), ("Running", "1"), ("Make", "1")],
    );
    let tip = hover_cell(&mut harness, "Hobgoblin I", "Own");
    assert!(
        tip.iter()
            .any(|line| line == "0 in hangars, 1 from running jobs"),
        "the Own hover: {tip:?}"
    );

    click(&mut harness, "Hobgoblin A");
    for row in ["Hobgoblin II", "Hobgoblin I", "Tritanium"] {
        assert_eq!(
            cell_under(&harness, row, "Running"),
            None,
            "{row} shows Running"
        );
    }
    let tip = hover_cell(&mut harness, "Hobgoblin I", "Own");
    assert!(
        !tip.iter().any(|line| line.contains("from running jobs")),
        "the Own hover: {tip:?}"
    );
}

#[test]
fn running_is_used_before_the_hangar_units() {
    let env = stock_environment();
    let (a, b) = reserving_pair(&env);
    set_order(&env, &[b, a]);

    let mut harness = start(&env, login());
    click(&mut harness, "Hobgoblin B");
    assert_cells(
        &harness,
        "Hobgoblin I",
        &[("Own", "2"), ("Running", "1"), ("Make", "0")],
    );
    let tip = hover_cell(&mut harness, "Hobgoblin I", "Own");
    assert!(
        tip.iter()
            .any(|line| line == "1 in hangars, 1 from running jobs"),
        "the Own hover: {tip:?}"
    );
}

#[test]
fn running_follows_the_job_assigned_to_a_pasted_plan() {
    let env = stock_environment();
    reserving_pair(&env);
    let c = make_plan(&env, "Hobgoblin C", 2, &[HOBGOBLIN_I], "Hobgoblin I\t1");
    assign(&env, 503, c);

    let mut harness = start(&env, login());
    click(&mut harness, "Hobgoblin C");
    assert_cells(
        &harness,
        "Hobgoblin I",
        &[("Own", "2"), ("Running", "2"), ("Make", "0")],
    );
    let tip = hover_cell(&mut harness, "Hobgoblin I", "Own");
    assert!(
        tip.iter()
            .any(|line| line == "0 in hangars, 2 from running jobs"),
        "the Own hover: {tip:?}"
    );
    for row in ["Hobgoblin II", "Tritanium"] {
        assert_eq!(
            cell_under(&harness, row, "Running"),
            None,
            "{row} shows Running"
        );
    }
}

#[test]
fn the_running_snapshot_is_taken() {
    let env = stock_environment();
    reserving_pair(&env);
    let mut harness = start(&env, login());
    click(&mut harness, "Hobgoblin B");
    snapshot(&mut harness, "build_running");
}

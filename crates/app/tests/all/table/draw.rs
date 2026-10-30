use super::*;

/// Whether `label` is drawn anywhere in the last frame.
fn shows(harness: &Harness<'_>, label: &str) -> bool {
    harness.query_all_by_label(label).count() > 0
}

/// Four columns of 80 points, so a narrower panel drops some for a `+N columns` cell.
fn level_columns() -> Vec<Column> {
    [("a", "A"), ("b", "B"), ("c", "C"), ("d", "D")]
        .into_iter()
        .map(|(id, label)| Column {
            id,
            label: label.to_owned(),
            kind: ColumnKind::Text,
            width: 80.0,
        })
        .collect()
}

/// A row whose first and last cells say `first` and `last`, for reading its place on screen.
fn level_row_with(key: u64, first: &str, last: &str) -> Row {
    Row {
        key,
        depth: 0,
        cells: vec![
            Cell::text(first),
            Cell::text("b1"),
            Cell::text("c1"),
            Cell::text(last),
        ],
    }
}

/// One row, with the same cells in every level column.
fn level_row(key: u64) -> Row {
    level_row_with(key, "a1", "d1")
}

/// A table of the four level columns at `panel_width`, built with `level` and `start_sort`,
/// whose kept level is written to `seen` after each frame. The harness adds 8 points of margin
/// on each side.
fn level_harness(
    level: Option<Level>,
    panel_width: f32,
    start_sort: Option<(usize, Direction)>,
    rows: Vec<Row>,
    seen: &Arc<Mutex<Option<Level>>>,
) -> Harness<'static> {
    let seen = Arc::clone(seen);
    let columns = level_columns();
    let style = Style::eve_dense();
    Harness::builder()
        .with_size(vec2(panel_width + 16.0, 400.0))
        .build_ui(move |ui: &mut Ui| {
            style.apply(ui.ctx());
            let id = Id::new("level test");
            let table = Table::new(id, &columns).start(BTreeSet::new(), start_sort);
            let table = match level {
                Some(level) => table.level(level),
                None => table,
            };
            table.show(ui, &style, Density::Normal, &rows, None);
            if let Ok(mut seen) = seen.lock() {
                *seen = Some(table_level(ui, id, Level::Summary));
            }
        })
}

#[test]
fn the_levels_are_summary_standard_technical_in_order() {
    assert_eq!(
        Level::ALL.map(Level::label),
        ["Summary", "Standard", "Technical"]
    );
    assert_eq!(Level::Summary.next(), Level::Standard);
    assert_eq!(Level::Standard.next(), Level::Technical);
    assert_eq!(Level::Technical.next(), Level::Technical);
}

#[test]
fn a_standard_table_shows_the_columns_that_fit_and_a_summary_shows_fewer() {
    // 250 points hold two 80 point columns and leave the 90 point `+2 columns` cell.
    let seen = Arc::new(Mutex::new(None));
    let mut harness = level_harness(
        Some(Level::Standard),
        250.0,
        None,
        vec![level_row(1)],
        &seen,
    );
    harness.run();
    assert!(shows(&harness, "A"));
    assert!(shows(&harness, "B"));
    assert!(shows(&harness, "+2 columns"));
    assert!(!shows(&harness, "C"), "C is past the two that fit");
    assert!(!shows(&harness, "D"), "D is past the two that fit");

    // 100 points hold only the always-drawn first column.
    let seen = Arc::new(Mutex::new(None));
    let mut harness = level_harness(Some(Level::Summary), 100.0, None, vec![level_row(1)], &seen);
    harness.run();
    assert!(shows(&harness, "A"), "the first column never drops");
    assert!(shows(&harness, "+3 columns"));
    assert!(!shows(&harness, "B"));
}

#[test]
fn clicking_the_more_columns_cell_raises_the_level_to_technical() {
    let seen = Arc::new(Mutex::new(None));
    let mut harness = level_harness(
        Some(Level::Standard),
        250.0,
        None,
        vec![level_row(1)],
        &seen,
    );
    harness.run();
    harness.get_by_label("+2 columns").click();
    harness.run();
    assert_eq!(*seen.lock().unwrap(), Some(Level::Technical));
    for label in ["A", "B", "C", "D"] {
        assert!(shows(&harness, label), "{label} is drawn at Technical");
    }
    assert!(!shows(&harness, "+2 columns"), "Technical has no +N cell");
    assert!(!shows(&harness, "+3 columns"), "Technical has no +N cell");
}

#[test]
fn a_table_without_a_level_shows_every_column_and_no_more_cell() {
    let seen = Arc::new(Mutex::new(None));
    let mut harness = level_harness(None, 250.0, None, vec![level_row(1)], &seen);
    harness.run();
    for label in ["A", "B", "C", "D"] {
        assert!(shows(&harness, label), "{label} is drawn without a level");
    }
    assert!(!shows(&harness, "+2 columns"), "no level has no +N cell");
    assert!(!shows(&harness, "+3 columns"), "no level has no +N cell");
}

#[test]
fn a_sort_on_a_column_the_level_hides_is_dropped() {
    // By D ascending the rows would be three, one, two; the given order is one, two, three.
    let rows = vec![
        level_row_with(1, "row one", "m"),
        level_row_with(2, "row two", "z"),
        level_row_with(3, "row three", "a"),
    ];

    // Standard draws only A and B, so the start sort on D is dropped.
    let seen = Arc::new(Mutex::new(None));
    let mut harness = level_harness(
        Some(Level::Standard),
        250.0,
        Some((3, Direction::Ascending)),
        rows.clone(),
        &seen,
    );
    harness.run();
    let one = harness.get_by_label("row one").rect().center().y;
    let two = harness.get_by_label("row two").rect().center().y;
    let three = harness.get_by_label("row three").rect().center().y;
    assert!(
        one < two && two < three,
        "the hidden D sort leaves the given order"
    );

    // Technical draws D, so the same start sort applies.
    let seen = Arc::new(Mutex::new(None));
    let mut harness = level_harness(
        Some(Level::Technical),
        250.0,
        Some((3, Direction::Ascending)),
        rows,
        &seen,
    );
    harness.run();
    let one = harness.get_by_label("row one").rect().center().y;
    let two = harness.get_by_label("row two").rect().center().y;
    let three = harness.get_by_label("row three").rect().center().y;
    assert!(three < one && one < two, "the drawn D sort applies");
}

#[test]
fn a_big_table_lays_out_only_the_rows_in_view_and_scrolls_to_the_end() {
    let rows: Vec<Row> = (0..50_000u64)
        .map(|index| row(index, 0, &format!("Item {index}"), i128::from(index), None))
        .collect();
    let seen = Arc::new(Mutex::new(Seen::default()));
    let mut harness = table_harness(rows, None, Density::Normal, vec2(800.0, 600.0), None, &seen);
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
    // left of the Value column, and level with its text. The 28 px control is taller than the
    // 24 px Normal row and sits top-aligned in it, so its centre is 2 px below the text's.
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
    assert!(
        (pick.center().y - item.center().y).abs() < 2.5,
        "the checkbox still overlaps the row's text line: pick {pick:?}, item {item:?}"
    );
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

#[test]
fn the_theme_sets_14_px_text_and_28_px_controls() {
    let ctx = Context::default();
    Style::eve_dense().apply(&ctx);
    let style = ctx.global_style();
    let size = |text: TextStyle| style.text_styles[&text].size;
    assert_eq!(size(TextStyle::Body), 14.0);
    assert_eq!(size(TextStyle::Button), 14.0);
    assert_eq!(size(TextStyle::Small), 12.0);
    assert_eq!(size(TextStyle::Monospace), 13.0);
    assert_eq!(size(TextStyle::Heading), 20.0);
    assert_eq!(style.spacing.interact_size.y, 28.0);
    assert_eq!(style.spacing.button_padding, vec2(12.0, 4.0));
}

/// A 16 by 16 picture that is red everywhere.
const RED_PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x10, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0xf3, 0xff,
    0x61, 0x00, 0x00, 0x00, 0x19, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63, 0xf8, 0xcf, 0xc0, 0xf0,
    0x9f, 0x12, 0xcc, 0x30, 0x6a, 0xc0, 0xa8, 0x01, 0xa3, 0x06, 0x0c, 0x17, 0x03, 0x00, 0x30, 0xc4,
    0xfe, 0x10, 0x8a, 0xc6, 0x4c, 0xe5, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42,
    0x60, 0x82,
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

use super::*;

use super::headline::*;
use super::loading::*;

// The table tests below use M1-132's harness and write their records and prices straight to
// the app's store. Main and Alt are both in the login, and Main is the latest point.

/// Alt's record: 90,000 seconds before `NOW`, so its data is a day old at the latest point.
const ALT_AT: i64 = NOW - 90_000;

/// Main at `NOW` with the whole scenario and Alt at `ALT_AT` with 1,000 Tritanium, priced at
/// PAU 61,200.00 and Tritanium 3.89.
fn add_tables_scenario(store: &mut Store) {
    store
        .add_record(&load_record(
            MAIN,
            NOW,
            load_seen(
                Some("1,234,567.89"),
                &[(LOAD_PAU, 160)],
                Some("6,120,000.00"),
                Some("550,100.00"),
                &[(LOAD_PAU, 10)],
            ),
        ))
        .expect("Main's record is written");
    store
        .add_record(&load_record(
            ALT,
            ALT_AT,
            load_seen(None, &[(TRITANIUM, 1_000)], None, None, &[]),
        ))
        .expect("Alt's record is written");
    store
        .save_point_prices(&PointPrices {
            at: NOW,
            published_at: Some(PUBLISHED),
            rows: BTreeMap::from([
                ((LOAD_PAU, Side::Sell), load_row("61,200.00")),
                ((TRITANIUM, Side::Sell), load_row("3.89")),
            ]),
        })
        .expect("the copy is saved");
}

/// Opens the page with Main and Alt in the login and the table scenario in the store.
fn start_tables(data: &TempDir) -> Harness<'static, App> {
    let mut harness = start_with_characters(
        data,
        Box::new(EmptyForge),
        vec![character(MAIN, "Main"), character(ALT, "Alt")],
    );
    add_tables_scenario(&mut harness.state_mut().shared_mut().store);
    open_net_worth(&mut harness);
    harness.run();
    harness
}

/// The rectangle of the cell `text` on the row of `anchor`, when both are drawn.
fn cell_on_row(harness: &Harness<'static, App>, anchor: &str, text: &str) -> Option<egui::Rect> {
    let rows: Vec<f32> = harness
        .query_all_by_label(anchor)
        .map(|node| node.rect().center().y)
        .collect();
    harness
        .query_all_by_label(text)
        .find(|node| {
            let y = node.rect().center().y;
            rows.iter().any(|row| (y - row).abs() <= 3.0)
        })
        .map(|node| node.rect())
}

#[test]
fn the_by_character_table_lists_each_character_its_parts_and_its_data_age() {
    let dir = TempDir::new().expect("a temp folder");
    let mut harness = start_tables(&dir);

    assert!(shown(&harness, "Character"));
    assert!(shown(&harness, "Data age"));
    assert!(shown(&harness, "Other"));
    assert_eq!(harness.query_all_by_label("By character").count(), 1);

    assert!(on_row(
        &harness,
        "Main",
        &[
            "1,234,567.89",
            "9,792,000.00",
            "6,120,000.00",
            "550,100.00",
            "612,000.00",
            "0.00",
            "18,308,667.89",
            "0s",
        ]
    ));
    assert!(on_row(
        &harness,
        "Alt",
        &[
            "0.00", "3,890.00", "0.00", "0.00", "0.00", "0.00", "3,890.00", "1d",
        ]
    ));
    assert!(on_row(
        &harness,
        "9,795,890.00",
        &[
            "Total",
            "1,234,567.89",
            "6,120,000.00",
            "550,100.00",
            "612,000.00",
            "0.00",
            "18,312,557.89",
        ]
    ));

    // Alt's data is older than a day, so its age is amber; Main's is fresh.
    let anchor = harness
        .get_all_by_label("Alt")
        .next()
        .expect("Alt's row")
        .rect();
    harness.hover_at(pos2(anchor.min.x + 10.0, anchor.center().y));
    harness.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: vec2(-900.0, 0.0),
        phase: egui::TouchPhase::Move,
        modifiers: egui::Modifiers::NONE,
    });
    harness.run_steps(30);
    let stale = cell_on_row(&harness, "Alt", "1d").expect("Alt's age cell after the scroll");
    assert!(coloured(&mut harness, stale), "the stale age is amber");
    let fresh = cell_on_row(&harness, "Main", "0s").expect("Main's age cell");
    assert!(!coloured(&mut harness, fresh), "a fresh age is plain");
}

#[test]
fn the_right_now_table_shows_each_part_its_share_basis_and_age() {
    let dir = TempDir::new().expect("a temp folder");
    let harness = start_tables(&dir);

    assert!(shown(&harness, "Share"));
    assert!(shown(&harness, "Basis"));
    assert_eq!(harness.query_all_by_label("Right now").count(), 1);

    assert!(on_row(
        &harness,
        "6.74%",
        &["Wallet", "1,234,567.89", "wallet", "1d"]
    ));
    assert!(on_row(
        &harness,
        "53.49%",
        &["Assets", "9,795,890.00", "Forge sell, 5% price", "1d"]
    ));
    assert!(on_row(
        &harness,
        "33.42%",
        &["Sell orders", "6,120,000.00", "own price", "1d"]
    ));
    assert!(on_row(
        &harness,
        "3.00%",
        &["Buy escrow", "550,100.00", "escrow", "1d"]
    ));
    assert!(on_row(
        &harness,
        "3.34%",
        &["Industry jobs", "612,000.00", "Forge sell, 5% price", "1d"]
    ));
    assert!(on_row(&harness, "100.00%", &["Total", "18,312,557.89"]));
    // A part with nothing in it has no row, and the vault is not in this point.
    assert_eq!(label_count(&harness, "Implants"), 0);
    assert_eq!(label_count(&harness, "PLEX vault"), 0);
}

#[test]
fn unticking_a_part_drops_its_column_and_row_and_adds_the_rest_up_again() {
    let dir = TempDir::new().expect("a temp folder");
    let mut harness = start_tables(&dir);
    assert_eq!(label_count(&harness, "Sell orders"), 2);

    click_checkbox(&mut harness, "Sell orders");

    assert_eq!(label_count(&harness, "Sell orders"), 0);
    assert!(!shown(&harness, "6,120,000.00"));
    assert!(on_row(&harness, "Main", &["12,188,667.89"]));
    assert!(shown(&harness, "12,192,557.89"));
    assert!(on_row(
        &harness,
        "10.13%",
        &["Wallet", "1,234,567.89", "wallet"]
    ));
}

#[test]
fn without_points_the_two_tables_draw_nothing() {
    let dir = TempDir::new().expect("a temp folder");
    let mut harness = start(&dir);
    open_net_worth(&mut harness);

    assert!(!shown(&harness, "Character"));
    assert!(!shown(&harness, "Data age"));
    assert!(!shown(&harness, "Basis"));
    assert!(!shown(&harness, "100.00%"));
}

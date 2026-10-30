use super::*;

use super::loading::*;

/// Every label and value in the tree, in tree order.
fn labels(harness: &Harness<'static, App>) -> Vec<String> {
    harness
        .root()
        .children_recursive()
        .filter_map(|node| {
            let node = node.accesskit_node();
            node.label().or_else(|| node.value())
        })
        .collect()
}

/// How many plain labels show exactly `text`. A drop-down's picked value is not a label, so
/// this tells the headline's basis line apart from the price control that names the same rule.
pub(super) fn label_count(harness: &Harness<'static, App>, text: &str) -> usize {
    harness
        .query_all_by_value(text)
        .filter(|node| node.accesskit_node().role() == Role::Label)
        .count()
}

/// Adds the two-point record set and its copy: Main at `FIRST_AT` with 160 PAU and at `NOW`
/// with the whole scenario.
fn add_two_points(store: &mut Store) {
    add_main_first(store);
    add_main_latest(store);
    seed_history(store);
    store
        .save_point_prices(&load_copy(false))
        .expect("the copy is saved");
}

#[test]
fn the_headline_shows_the_last_point_and_the_changes_over_the_range() {
    let dir = TempDir::new().expect("a temp folder");
    let mut harness = start(&dir);
    add_two_points(&mut harness.state_mut().shared_mut().store);

    open_net_worth(&mut harness);
    assert!(shown(&harness, "Net worth 18.31 M ISK"));
    assert_eq!(
        hover(&mut harness, "Net worth 18.31 M ISK"),
        vec!["18,308,667.89 ISK".to_owned()]
    );
    assert!(shown(&harness, "Forge sell, 5% price · 9 Sep 01:30"));
    assert!(shown(&harness, "+7.71 M (+72.72%) since 2 Sep"));
    assert!(shown(
        &harness,
        "Price moves +192 K, everything else +7.52 M"
    ));
    assert!(shown(&harness, "+7.71 M (+72.72%) over 30 days"));

    click_combo(&mut harness, "Forge sell, 5% price", 0);
    click(&mut harness, "Forge buy, 5% price");
    harness.run_steps(2);
    assert!(has_combo(&harness, "Forge buy, 5% price"));
    assert!(shown(&harness, "Net worth 17.26 M ISK"));
    assert!(shown(&harness, "Forge buy, 5% price · 9 Sep 01:30"));
    assert!(shown(&harness, "+7.62 M (+79.01%) since 2 Sep"));
}

#[test]
fn the_change_is_since_yesterday_when_the_earlier_point_is_the_day_before() {
    let dir = TempDir::new().expect("a temp folder");
    let mut harness = start(&dir);
    {
        let store = &mut harness.state_mut().shared_mut().store;
        let before = NOW - 85_600;
        store
            .add_record(&load_record(
                MAIN,
                before,
                load_seen(Some("1,000,000.00"), &[(LOAD_PAU, 160)], None, None, &[]),
            ))
            .expect("the earlier record is written");
        store
            .save_point_prices(&PointPrices {
                at: before,
                published_at: None,
                rows: BTreeMap::from([((LOAD_PAU, Side::Sell), load_row("60,000.00"))]),
            })
            .expect("the earlier copy is saved");
        store
            .add_record(&load_record(
                MAIN,
                NOW,
                load_seen(Some("1,000,000.00"), &[(LOAD_PAU, 170)], None, None, &[]),
            ))
            .expect("the latest record is written");
        store
            .save_point_prices(&load_copy(false))
            .expect("the latest copy is saved");
    }

    open_net_worth(&mut harness);
    assert!(shown(&harness, "Net worth 11.40 M ISK"));
    assert!(shown(&harness, "+804 K (+7.58%) since yesterday"));
    assert!(shown(
        &harness,
        "Price moves +192 K, everything else +612 K"
    ));
    assert!(shown(&harness, "+804 K (+7.58%) over 30 days"));
}

#[test]
fn an_empty_range_says_so_and_a_later_record_shows_after_leaving_and_returning() {
    let dir = TempDir::new().expect("a temp folder");
    let mut harness = start(&dir);
    open_net_worth(&mut harness);
    assert!(shown(
        &harness,
        "No net worth points yet. Refresh your characters to record the first one."
    ));

    harness
        .state_mut()
        .shared_mut()
        .store
        .add_record(&load_record(
            MAIN,
            NOW,
            load_seen(Some("1,234,567.89"), &[], None, None, &[]),
        ))
        .expect("the record is written");

    harness
        .get_all_by_label("Today")
        .next()
        .expect("the Today rail entry")
        .click();
    harness.run();
    open_net_worth(&mut harness);

    assert!(shown(&harness, "Net worth 1.23 M ISK"));
    assert_eq!(
        label_count(&harness, "Forge sell, 5% price"),
        1,
        "the headline names the basis alone"
    );
    let drawn = labels(&harness);
    assert!(
        !drawn
            .iter()
            .any(|text| text.starts_with("Forge sell, 5% price ·")),
        "no publication time is named for a source without one"
    );
    assert!(!drawn.iter().any(|text| text.contains(" since ")));
    assert!(!drawn.iter().any(|text| text.contains(" over ")));
}

#[test]
fn an_unpriced_type_is_named_with_its_units_on_hover() {
    let dir = TempDir::new().expect("a temp folder");
    let mut harness = start(&dir);
    harness
        .state_mut()
        .shared_mut()
        .store
        .add_record(&load_record(
            MAIN,
            NOW,
            Snapshot::Seen(Holdings {
                wallet: Some(isk("1,000,000.00")),
                assets: BTreeMap::from([(587, 3)]),
                sell_orders: None,
                buy_escrow: None,
                jobs_output: BTreeMap::new(),
            }),
        ))
        .expect("the record is written");

    open_net_worth(&mut harness);
    assert!(shown(&harness, "Net worth 1.00 M ISK"));
    let warning = "1 item types have no price at this basis and are not counted.";
    assert!(shown(&harness, warning));
    assert_eq!(hover(&mut harness, warning), vec!["Rifter × 3".to_owned()]);
}

#[test]
fn a_failed_price_read_says_so_once_and_the_points_still_show() {
    let dir = TempDir::new().expect("a temp folder");
    let mut harness = start(&dir);
    add_two_points(&mut harness.state_mut().shared_mut().store);
    open_net_worth(&mut harness);
    assert!(shown(&harness, "Net worth 18.31 M ISK"));

    let db = dir.path().join("data").join("user.sqlite");
    rusqlite::Connection::open(&db)
        .expect("the database opens")
        .execute_batch("PRAGMA foreign_keys = OFF; DROP TABLE point_prices;")
        .expect("the copy table drops");

    harness
        .get_all_by_label("Today")
        .next()
        .expect("the Today rail entry")
        .click();
    harness.run();
    open_net_worth(&mut harness);
    harness.run_steps(20);

    let wanted = "Could not read the prices: database error: no such table: point_prices";
    let matching: Vec<String> = errors(&mut harness)
        .into_iter()
        .filter(|text| text == wanted)
        .collect();
    assert_eq!(matching.len(), 1, "{:?}", errors(&mut harness));
    assert!(
        labels(&harness)
            .iter()
            .any(|text| text.starts_with("Net worth ")),
        "the points still show"
    );
}

// The chart tests below use M1-132's harness and write their records and prices straight to
// the app's store. The chart's tooltip is read at the plot's right edge less one point, where
// the nearest dot is the latest one.

use super::*;

use super::loading::*;

/// The chart's 30-day scenario: Main records one a day at `NOW - k x 86,400` for k 0 to 29,
/// its wallet rising by 50,000.00 a day from 1,000,000.00 with 160 PAU; PAU's Forge history is
/// 60,000.00 on each of those days. No price copy is kept.
fn add_thirty_days(store: &mut Store) {
    let market = forge_id(store);
    let newest = NOW.div_euclid(86_400);
    for k in 0..30_i64 {
        let wallet = isk("1,000,000.00")
            .checked_add(
                isk("50,000.00")
                    .checked_mul_qty(u64::try_from(29 - k).expect("a small day count"))
                    .expect("a wallet step"),
            )
            .expect("a wallet");
        store
            .add_record(&load_record(
                MAIN,
                NOW - k * 86_400,
                Snapshot::Seen(Holdings {
                    wallet: Some(wallet),
                    assets: BTreeMap::from([(LOAD_PAU, 160)]),
                    sell_orders: None,
                    buy_escrow: None,
                    jobs_output: BTreeMap::new(),
                }),
            ))
            .expect("a day's record is written");
    }
    for day in (newest - 29)..=newest {
        store
            .record(
                market,
                day,
                &[(LOAD_PAU, Side::Sell, load_row("60,000.00"))],
            )
            .expect("a day's history is written");
    }
}

/// The tooltip of the chart's right edge less one point, mid height.
fn hover_chart(harness: &mut Harness<'static, App>) -> Vec<String> {
    harness.run();
    for _ in 0..4 {
        let plot = harness
            .query_all_by_label("Net worth chart")
            .next()
            .expect("the chart")
            .rect();
        let tip = hover_at(harness, pos2(plot.max.x - 1.0, plot.center().y));
        let stayed = harness
            .query_all_by_label("Net worth chart")
            .next()
            .expect("the chart")
            .rect()
            == plot;
        if !tip.is_empty() && stayed {
            return tip;
        }
        harness.remove_cursor();
        harness.run();
    }
    panic!("the chart tooltip did not show");
}

/// The tooltip of the chart's dot of day `k` of the thirty-day scenario, counting from the
/// latest day as 0. The plot's width holds the 29 steps between the first and last points.
fn hover_day(harness: &mut Harness<'static, App>, k: i64) -> Vec<String> {
    harness.run();
    let plot = harness
        .query_all_by_label("Net worth chart")
        .next()
        .expect("the chart")
        .rect();
    let days = u16::try_from(29 - k).expect("a day of the thirty-day scenario");
    let x = plot.min.x + plot.width() * f32::from(days) / 29.0;
    hover_at(harness, pos2(x, plot.center().y))
}

/// Whether the tooltip `lines` shows the exact line `text`.
fn has_line(lines: &[String], text: &str) -> bool {
    lines
        .iter()
        .any(|entry| entry.lines().any(|line| line == text))
}

/// Whether the tooltip `lines` shows any line of the carried characters.
fn partial(lines: &[String]) -> bool {
    lines
        .iter()
        .any(|entry| entry.lines().any(|line| line.starts_with("Partial:")))
}

#[test]
fn the_thirty_day_chart_draws_its_stack_and_names_the_day_on_hover() {
    let dir = TempDir::new().expect("a temp folder");
    let mut harness = start(&dir);
    add_thirty_days(&mut harness.state_mut().shared_mut().store);

    open_net_worth(&mut harness);
    let lines = [
        "2001-09-09 01:46",
        "Total 12,050,000.00 ISK",
        "Wallet 2,450,000.00",
        "Assets 9,600,000.00",
        "Sell orders 0.00",
        "Buy escrow 0.00",
        "Industry jobs 0.00",
        "Contract collateral 0.00",
        "Contracts 0.00",
        "Implants 0.00",
        "PLEX vault 0.00",
        "Forge sell, 5% price · 9 Sep",
    ];
    assert_eq!(hover_chart(&mut harness), vec![lines.join("\n")]);
    snapshot(&mut harness, "networth-30d");
}

#[test]
fn a_carried_character_marks_the_day_partial_and_is_named_under_the_chart() {
    let dir = TempDir::new().expect("a temp folder");
    let mut harness = start_with_characters(
        &dir,
        Box::new(EmptyForge),
        vec![character(MAIN, "Main"), character(ALT, "Alt")],
    );
    {
        let store = &mut harness.state_mut().shared_mut().store;
        add_thirty_days(store);
        store
            .add_record(&load_record(
                ALT,
                NOW - 691_200,
                load_seen(Some("500,000.00"), &[], None, None, &[]),
            ))
            .expect("the alt record is written");
    }

    open_net_worth(&mut harness);
    let lines = [
        "2001-09-09 01:46",
        "Total 12,550,000.00 ISK",
        "Wallet 2,950,000.00",
        "Assets 9,600,000.00",
        "Sell orders 0.00",
        "Buy escrow 0.00",
        "Industry jobs 0.00",
        "Contract collateral 0.00",
        "Contracts 0.00",
        "Implants 0.00",
        "PLEX vault 0.00",
        "Forge sell, 5% price · 9 Sep",
        "Partial: Alt carried from 8d before",
    ];
    assert_eq!(hover_chart(&mut harness), vec![lines.join("\n")]);
    assert!(shown(&harness, "Alt: last data 8d ago"));
    snapshot(&mut harness, "networth-partial");
}

/// The thirty-day chart with Alt's one record eight days before `NOW`: its data is a week or
/// more old at the first two days of the range.
fn start_with_carried_alt(dir: &TempDir) -> Harness<'static, App> {
    let mut harness = start_with_characters(
        dir,
        Box::new(EmptyForge),
        vec![character(MAIN, "Main"), character(ALT, "Alt")],
    );
    {
        let store = &mut harness.state_mut().shared_mut().store;
        add_thirty_days(store);
        store
            .add_record(&load_record(
                ALT,
                NOW - 691_200,
                load_seen(Some("500,000.00"), &[], None, None, &[]),
            ))
            .expect("the alt record is written");
    }
    harness
}

#[test]
fn a_carried_character_can_be_left_out_with_a_tick() {
    let dir = TempDir::new().expect("a temp folder");
    let mut harness = start_with_carried_alt(&dir);
    open_net_worth(&mut harness);

    assert!(shown(&harness, "Leave out Alt"));
    assert!(!ticked(&harness, "Leave out Alt"));
    assert!(shown(&harness, "Alt: last data 8d ago"));
    let tip = hover_day(&mut harness, 0);
    assert!(has_line(&tip, "Total 12,550,000.00 ISK"), "{tip:?}");
    let tip = hover_day(&mut harness, 1);
    assert!(has_line(&tip, "Total 12,500,000.00 ISK"), "{tip:?}");

    click_checkbox(&mut harness, "Leave out Alt");
    assert!(ticked(&harness, "Leave out Alt"));
    assert!(!shown(&harness, "Alt: last data"));
    let tip = hover_day(&mut harness, 0);
    assert!(has_line(&tip, "Total 12,050,000.00 ISK"), "{tip:?}");
    assert!(has_line(&tip, "Wallet 2,450,000.00"), "{tip:?}");
    assert!(!partial(&tip), "{tip:?}");
    let tip = hover_day(&mut harness, 1);
    assert!(has_line(&tip, "Total 12,000,000.00 ISK"), "{tip:?}");
    assert!(!partial(&tip), "{tip:?}");
    let tip = hover_day(&mut harness, 2);
    assert!(has_line(&tip, "Total 12,450,000.00 ISK"), "{tip:?}");
    assert!(
        has_line(&tip, "Partial: Alt carried from 6d before"),
        "{tip:?}"
    );

    click_checkbox(&mut harness, "Leave out Alt");
    assert!(!ticked(&harness, "Leave out Alt"));
    let tip = hover_day(&mut harness, 0);
    assert!(has_line(&tip, "Total 12,550,000.00 ISK"), "{tip:?}");
}

#[test]
fn a_left_out_character_stays_ticked_when_the_page_is_left_and_come_back_to() {
    let dir = TempDir::new().expect("a temp folder");
    let mut harness = start_with_carried_alt(&dir);
    open_net_worth(&mut harness);
    click_checkbox(&mut harness, "Leave out Alt");

    harness.get_by_label("Orders").click();
    harness.run();
    open_net_worth(&mut harness);

    assert!(ticked(&harness, "Leave out Alt"));
    let tip = hover_day(&mut harness, 0);
    assert!(has_line(&tip, "Total 12,050,000.00 ISK"), "{tip:?}");
}

#[test]
fn only_main_records_show_no_leave_out_tick_and_the_chart_as_before() {
    let dir = TempDir::new().expect("a temp folder");
    let mut harness = start(&dir);
    add_thirty_days(&mut harness.state_mut().shared_mut().store);
    open_net_worth(&mut harness);

    assert!(!labelled_with_prefix(&harness, "Leave out"));
    let tip = hover_day(&mut harness, 0);
    assert!(has_line(&tip, "Total 12,050,000.00 ISK"), "{tip:?}");
    assert!(!partial(&tip), "{tip:?}");
}

#[test]
fn without_the_wallet_tick_the_hover_names_the_rest_alone() {
    let dir = TempDir::new().expect("a temp folder");
    let mut harness = start(&dir);
    add_thirty_days(&mut harness.state_mut().shared_mut().store);
    open_net_worth(&mut harness);

    click_checkbox(&mut harness, "Wallet");
    assert!(!ticked(&harness, "Wallet"));
    let lines = [
        "2001-09-09 01:46",
        "Total 9,600,000.00 ISK",
        "Assets 9,600,000.00",
        "Sell orders 0.00",
        "Buy escrow 0.00",
        "Industry jobs 0.00",
        "Contract collateral 0.00",
        "Contracts 0.00",
        "Implants 0.00",
        "PLEX vault 0.00",
        "Forge sell, 5% price · 9 Sep",
    ];
    assert_eq!(hover_chart(&mut harness), vec![lines.join("\n")]);
}

#[test]
fn a_lone_point_draws_a_chart_and_an_empty_range_draws_none() {
    let dir = TempDir::new().expect("a temp folder");
    let mut harness = start(&dir);
    {
        let store = &mut harness.state_mut().shared_mut().store;
        store
            .add_record(&load_record(
                MAIN,
                NOW,
                Snapshot::Seen(Holdings {
                    wallet: Some(isk("2,450,000.00")),
                    assets: BTreeMap::from([(LOAD_PAU, 160)]),
                    sell_orders: None,
                    buy_escrow: None,
                    jobs_output: BTreeMap::new(),
                }),
            ))
            .expect("the only record is written");
        let market = forge_id(store);
        store
            .record(
                market,
                NOW.div_euclid(86_400),
                &[(LOAD_PAU, Side::Sell, load_row("60,000.00"))],
            )
            .expect("the day's history is written");
    }

    open_net_worth(&mut harness);
    let tip = hover_chart(&mut harness);
    assert!(
        tip.iter()
            .any(|text| text.lines().any(|line| line == "Total 12,050,000.00 ISK")),
        "{tip:?}"
    );

    let empty = TempDir::new().expect("a temp folder");
    let mut empty_harness = start(&empty);
    open_net_worth(&mut empty_harness);
    assert!(!shown(&empty_harness, "Net worth chart"));
}

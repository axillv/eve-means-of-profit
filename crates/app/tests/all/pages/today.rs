use super::*;

/// The six block titles of the Today page, in [`Block::ALL`] order.
const TODAY_TITLES: [&str; 6] = [
    "Jobs",
    "Plans",
    "Orders",
    "Warnings",
    "Net worth",
    "Characters",
];

/// The left edge of the Today page's content. The rail's Today entry shares its label with
/// the page heading, which is the rightmost.
fn today_left(harness: &Harness<'_, App>) -> f32 {
    harness
        .query_all_by_label("Today")
        .map(|node| node.rect().min.x)
        .fold(f32::NEG_INFINITY, f32::max)
}

/// The rect of the Today block title `label`, not the rail entry of the same name.
fn today_title(harness: &Harness<'_, App>, label: &str) -> Rect {
    let left = today_left(harness);
    harness
        .query_all_by_label(label)
        .find(|node| node.rect().min.x >= left - 1.0)
        .unwrap_or_else(|| panic!("no Today block title {label:?}"))
        .rect()
}

fn today_titles(harness: &Harness<'_, App>) -> Vec<Rect> {
    TODAY_TITLES
        .iter()
        .map(|label| today_title(harness, label))
        .collect()
}

#[test]
fn a_fresh_app_starts_on_today_and_every_empty_block_says_why() {
    let env = env();
    let mut harness = start(&env, FakeEve::new(Vec::new(), None), NOW);
    loaded(&mut harness);

    assert!(
        shown(&harness, "Warnings"),
        "Warnings shows without a rail click, so Today starts"
    );
    for text in [
        "Jobs to deliver and free slots show here once a character is added.",
        "No plans yet. Each plan shows its next step here.",
        "Market orders someone has beaten show here once a character is added.",
        "None. Price spikes and thin markets in your plans show here.",
        "Starts with your first character.",
        "None yet.",
    ] {
        assert!(shown(&harness, text), "{text:?} shows");
    }

    let titles = today_titles(&harness);
    let left = titles[0].min.x;
    for title in &titles[..4] {
        assert!((title.min.x - left).abs() < 1.0, "the first four are left");
    }
    let right = titles[4].min.x;
    assert!(right > left, "the last two are right of the first four");
    for title in &titles[4..] {
        assert!((title.min.x - right).abs() < 1.0, "the last two are right");
    }
    assert!((titles[4].top() - titles[0].top()).abs() < 0.5);
    for pair in titles.windows(2) {
        assert!(
            pair[1].top() > pair[0].top() || pair[1].min.x > pair[0].min.x,
            "titles are in Block::ALL order"
        );
    }

    for name in [
        "Jobs: Add character",
        "Plans: Pick a product",
        "Orders: Add character",
        "Net worth: Add character",
        "Characters: Add character",
    ] {
        assert_eq!(
            harness.query_all_by_label(name).count(),
            1,
            "{name} is shown once"
        );
    }
    assert_eq!(
        harness
            .query_all_by_label("Warnings: Add character")
            .count(),
        0,
        "Warnings has no button"
    );
    assert_eq!(
        harness
            .query_all_by_label("Warnings: Pick a product")
            .count(),
        0,
        "Warnings has no button"
    );
    assert!(!shown(&harness, "0 ready to deliver"));
    assert!(!shown(&harness, "0 beaten · 0 open"));
}

#[rustfmt::skip]
#[test]
fn the_blocks_keep_their_places_across_pages_and_characters() {
    let env = env();
    let mut harness = start(&env, FakeEve::new(Vec::new(), None), NOW);
    loaded(&mut harness);
    let before = today_titles(&harness);

    rail(&mut harness, "Build");
    rail(&mut harness, "Today");
    let after = today_titles(&harness);
    assert_eq!(after, before, "the block places do not move");

    let mut with_characters =
        start(&env, FakeEve::new(vec![character(MAIN, "Main")], None), NOW);
    loaded(&mut with_characters);
    let character_titles = today_titles(&with_characters);
    for (index, (with, without)) in character_titles.iter().zip(&before).enumerate() {
        // The characters block follows the Net worth block, whose body now has a line, so it
        // moves down; every other title keeps its place.
        if index == 5 {
            continue;
        }
        assert_eq!(with, without, "a character does not move block {index}");
    }
    assert!(
        character_titles[5].top() > character_titles[4].bottom(),
        "the characters block stays below the Net worth block"
    );
    for column in [&before[..4], &before[4..]] {
        let x = column[0].min.x;
        for title in column {
            assert!((title.min.x - x).abs() < 1.0, "one x per column");
        }
    }
}

#[test]
fn today_lists_the_ready_jobs_oldest_first() {
    let env = env();
    saved::save_character(&esi_dir(&env), &data_with(Some(ready_jobs()), None))
        .expect("Main's jobs saved");
    let mut harness = start(&env, FakeEve::new(vec![character(MAIN, "Main")], None), NOW);
    loaded(&mut harness);

    assert!(shown(&harness, "2 ready to deliver"));
    assert!(shown(&harness, "Deliver 5 × Crystalline Carbonide"));
    assert!(shown(&harness, "Home Fort · ready 1h"));
    assert!(shown(&harness, "Deliver 1 × Hobgoblin II Blueprint"));
    assert!(shown(&harness, "Home Fort · ready 10s"));
    assert!(
        crate::ui::top(&harness, "Deliver 5 × Crystalline Carbonide")
            < crate::ui::top(&harness, "Deliver 1 × Hobgoblin II Blueprint"),
        "the job that ended first is above"
    );

    crate::ui::click(&mut harness, "Deliver 5 × Crystalline Carbonide");
    assert!(
        shown(&harness, "4 jobs · 2 ready to deliver"),
        "a job row shows Jobs"
    );
}

#[test]
fn today_lists_the_beaten_orders_and_copies_the_price_to_type() {
    let env = env();
    saved::save_character(&esi_dir(&env), &data_with(None, Some(beaten_orders())))
        .expect("Main's orders saved");
    let mut harness =
        start_with_forge(&env, FakeEve::new(vec![character(MAIN, "Main")], None), NOW);
    loaded(&mut harness);
    hold_forge(&mut harness);

    assert!(shown(&harness, "3 beaten · 4 open"));
    assert!(shown(&harness, "Sell Particle Accelerator Unit beaten"));
    assert!(shown(&harness, "Buy Morphite beaten"));
    assert!(shown(&harness, "70,000.00, best 61,200.00"));
    assert!(shown(&harness, "61,500.00, best 61,200.00"));
    assert!(shown(&harness, "2,100.00, best 2,150.00"));
    assert!(
        crate::ui::top(&harness, "70,000.00, best 61,200.00")
            < crate::ui::top(&harness, "61,500.00, best 61,200.00"),
        "the widest gap is above"
    );
    assert!(
        crate::ui::top(&harness, "61,500.00, best 61,200.00")
            < crate::ui::top(&harness, "2,100.00, best 2,150.00"),
        "the smallest gap is last"
    );

    harness
        .get_all_by_label("Orders: Copy 61,190.00")
        .next()
        .expect("the corp PAU row's copy button")
        .click();
    harness.step();
    assert_eq!(copied_text(&harness).as_deref(), Some("61190.00"));
    assert!(shown(&harness, "Warnings"), "Today stays after a copy");

    crate::ui::click(&mut harness, "Buy Morphite beaten");
    assert!(
        shown(&harness, "4 orders · sell 9,711,000.00 · escrow 21,000.00"),
        "an order row shows Orders"
    );
}

#[test]
fn a_character_with_nothing_to_do_shows_zero_counts_and_no_rows() {
    let env = env();
    let mut harness = start(&env, FakeEve::new(vec![character(MAIN, "Main")], None), NOW);
    loaded(&mut harness);

    assert!(shown(&harness, "0 ready to deliver"));
    assert!(shown(&harness, "0 beaten · 0 open"));
    assert!(!shown(&harness, "Main"), "no rows show");
}

#[test]
fn a_filling_button_goes_to_its_page() {
    let env = env();
    let mut harness = start(&env, FakeEve::new(Vec::new(), None), NOW);
    loaded(&mut harness);

    crate::ui::click(&mut harness, "Plans: Pick a product");
    assert!(shown(&harness, "Steps"), "Pick a product shows Build");

    rail(&mut harness, "Today");
    crate::ui::click(&mut harness, "Jobs: Add character");
    assert!(
        shown(&harness, "jEve data"),
        "Add character shows Characters"
    );
}

#[test]
fn the_status_line_and_the_today_heading_each_refresh_all() {
    let env = env();
    saved::save_character(&esi_dir(&env), &main_data()).expect("Main's data saved");
    let eve = FakeEve::new(vec![character(MAIN, "Main")], None);
    let refreshes = eve.refreshes();
    let mut harness = start(&env, eve, NOW);
    loaded(&mut harness);
    settle_start(&mut harness, &refreshes);
    drop(price_marks(&mut harness));
    let base = *refreshes.lock().unwrap_or_else(PoisonError::into_inner);
    assert_eq!(base, 1);

    // The status line's button is the lower one.
    assert_eq!(refresh_buttons(&harness).len(), 2);
    refresh_buttons(&harness)
        .into_iter()
        .nth(1)
        .expect("the status line's button")
        .click();
    let mut marks = Vec::new();
    for _ in 0..500 {
        harness.step();
        marks.extend(price_marks(&mut harness));
        if *refreshes.lock().unwrap_or_else(PoisonError::into_inner) == base + 1
            && !marks.is_empty()
        {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        *refreshes.lock().unwrap_or_else(PoisonError::into_inner),
        base + 1
    );
    assert_eq!(marks, vec![true]);

    // The Today heading's button is the upper one.
    refresh_buttons(&harness)
        .into_iter()
        .next()
        .expect("the heading's button")
        .click();
    let mut marks = Vec::new();
    for _ in 0..500 {
        harness.step();
        marks.extend(price_marks(&mut harness));
        if *refreshes.lock().unwrap_or_else(PoisonError::into_inner) == base + 2
            && !marks.is_empty()
        {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        *refreshes.lock().unwrap_or_else(PoisonError::into_inner),
        base + 2
    );
    assert_eq!(marks, vec![true]);
}

#[test]
fn the_today_page_matches_its_snapshot() {
    let env = env();
    let mut harness = start(&env, FakeEve::new(Vec::new(), None), NOW);
    loaded(&mut harness);
    harness.remove_cursor();
    harness.run();
    harness.snapshot("today-empty");
}

#[test]
fn the_today_blocks_match_their_snapshot() {
    let env = env();
    saved::save_character(
        &esi_dir(&env),
        &data_with(Some(ready_jobs()), Some(beaten_orders())),
    )
    .expect("Main's jobs and orders saved");
    let mut harness =
        start_with_forge(&env, FakeEve::new(vec![character(MAIN, "Main")], None), NOW);
    loaded(&mut harness);
    hold_forge(&mut harness);
    harness.remove_cursor();
    harness.run();
    harness.snapshot("today-blocks");
}

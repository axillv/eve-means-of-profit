use super::*;

use eve_means_of_profit::rail;

#[test]
fn the_rail_lists_the_pages_and_each_shows_its_panels() {
    let env = env();
    let mut harness = start(&env, FakeEve::new(Vec::new(), None), NOW);
    loaded(&mut harness);
    let pages: [(&str, &[&str]); 10] = [
        ("Today", &["Today"]),
        ("What to build", &["What could I build?"]),
        ("Build", &["Plans", "Plan", "Steps", "Buy list"]),
        ("Stockpiles", &["Stockpiles", "Contents"]),
        ("Industry jobs", &["Industry jobs"]),
        ("Orders", &["Orders"]),
        ("Assets", &["Places", "Assets"]),
        ("Net worth", &["Net worth", "By character", "Right now"]),
        ("Characters", &["Characters", "jEve data"]),
        (
            "Settings",
            &["Facilities", "Freight services", "Price rule"],
        ),
    ];
    for (page, panels) in pages {
        rail(&mut harness, page);
        for panel in panels {
            // Once each (a tab bar or a second copy would show twice); a title that is also
            // a rail label shows once more, on the rail.
            let on_rail = [
                "Today",
                "Assets",
                "Net worth",
                "Orders",
                "Industry jobs",
                "Characters",
                "Stockpiles",
            ]
            .contains(panel);
            assert_eq!(
                harness.query_all_by_label(panel).count(),
                if on_rail { 2 } else { 1 },
                "{page}: {panel} is shown once"
            );
        }
    }
    rail(&mut harness, "Build");
    for panel in ["Plans", "Plan", "Steps", "Buy list"] {
        assert_eq!(harness.query_all_by_label(panel).count(), 1, "{panel}");
    }
}

#[test]
fn the_last_rail_entry_is_settings_and_shows_the_settings_panels() {
    let env = env();
    let mut harness = start(&env, FakeEve::new(Vec::new(), None), NOW);
    loaded(&mut harness);
    assert_eq!(Page::ALL.last().map(|page| page.label()), Some("Settings"));
    let left = rail_left(&harness);
    let settings: Vec<Rect> = harness
        .query_all_by_label("Settings")
        .filter(|node| (node.rect().min.x - left).abs() < 1.0)
        .map(|node| node.rect())
        .collect();
    assert_eq!(settings.len(), 1, "Settings is on the rail exactly once");
    assert!(
        harness.query_all_by_label("Setup").next().is_none(),
        "no entry is labelled Setup"
    );

    rail(&mut harness, "Settings");
    for panel in ["Facilities", "Freight services", "Price rule"] {
        assert!(shown(&harness, panel), "{panel} is shown");
    }
}

/// The rail's left edge. Every rail entry starts there; panel titles start to its right.
fn rail_left(harness: &Harness<'_, App>) -> f32 {
    harness.get_by_label("Build").rect().min.x
}

/// The rect of the rail entry named `label`, not a same-named page title.
fn rail_entry(harness: &Harness<'_, App>, label: &str) -> Rect {
    let left = rail_left(harness);
    harness
        .get_all_by_label(label)
        .find(|node| (node.rect().min.x - left).abs() < 1.0)
        .unwrap_or_else(|| panic!("no rail entry labelled {label:?}"))
        .rect()
}

#[test]
fn the_rail_groups_the_pages_and_wraps_only_at_spaces() {
    let env = env();
    let mut harness = start(&env, FakeEve::new(Vec::new(), None), NOW);
    loaded(&mut harness);

    let labels = [
        "Today",
        "What to build",
        "Build",
        "Stockpiles",
        "Industry jobs",
        "Orders",
        "Assets",
        "Net worth",
        "Characters",
        "Settings",
    ];
    let entries: Vec<Rect> = labels
        .iter()
        .map(|label| rail_entry(&harness, label))
        .collect();
    for pair in entries.windows(2) {
        assert!(
            pair[1].top() > pair[0].top(),
            "each rail entry is below the one before it"
        );
    }
    let left = rail_left(&harness);
    for label in labels {
        assert_eq!(
            harness
                .query_all_by_label(label)
                .filter(|node| (node.rect().min.x - left).abs() < 1.0)
                .count(),
            1,
            "{label} is on the rail exactly once"
        );
    }

    // Every entry is the same width; the rail adds the panel's padding around them and is
    // 172 px wide.
    let widest = entries.iter().map(|rect| rect.width()).fold(0.0, f32::max);
    assert_eq!(widest + 16.0, 172.0, "the rail is 172 px wide");
    assert_eq!(entries[4].width(), widest, "Industry jobs fits its label");

    let gap = |above: Rect, below: Rect| below.top() - above.bottom();
    let ordinary = gap(
        rail_entry(&harness, "Stockpiles"),
        rail_entry(&harness, "Industry jobs"),
    );
    for (above, below) in [("Orders", "Assets"), ("Net worth", "Characters")] {
        let grouped = gap(rail_entry(&harness, above), rail_entry(&harness, below));
        assert!(
            grouped >= ordinary + 16.0,
            "{above} to {below} is a group gap"
        );
    }
}

/// A plain harness that draws only the rail, with `show_with` and either its labels or its
/// icons.
fn rail_only(icons_only: bool) -> Harness<'static, Page> {
    let style = Style::eve_dense();
    Harness::builder()
        .with_size(vec2(400.0, 800.0))
        .build_ui_state(
            move |ui, page| {
                style.apply(ui.ctx());
                rail::show_with(ui, &style, page, icons_only);
            },
            Page::Build,
        )
}

/// The rail's labels, top to bottom.
fn rail_labels(harness: &Harness<'_, Page>) -> Vec<&'static str> {
    let mut labels: Vec<(&'static str, f32)> = Page::ALL
        .iter()
        .map(|page| {
            (
                page.label(),
                harness.get_by_label(page.label()).rect().top(),
            )
        })
        .collect();
    labels.sort_by(|left, right| left.1.total_cmp(&right.1));
    labels.into_iter().map(|(label, _)| label).collect()
}

#[test]
fn the_icons_only_rail_is_narrow_and_lists_the_pages_in_the_same_order() {
    let labelled = rail_only(false);
    let icons = rail_only(true);
    let expected = [
        "Today",
        "What to build",
        "Build",
        "Stockpiles",
        "Industry jobs",
        "Orders",
        "Assets",
        "Net worth",
        "Characters",
        "Settings",
    ];
    assert_eq!(rail_labels(&labelled), expected);
    assert_eq!(
        rail_labels(&icons),
        expected,
        "icons-only keeps the labels' order"
    );
    for label in expected {
        let rect = icons.get_by_label(label).rect();
        assert!(
            rect.width() <= 48.0,
            "{label} is at most 48 px wide, not {}",
            rect.width()
        );
    }
}

#[test]
fn hovering_an_icons_only_entry_shows_its_label() {
    let mut harness = rail_only(true);
    let tip = crate::ui::hover(&mut harness, "Orders");
    assert!(
        tip.iter().any(|text| text == "Orders"),
        "Orders is its hover text: {tip:?}"
    );

    let tip = crate::ui::hover(&mut harness, "What to build");
    assert!(
        tip.iter().any(|text| text == "What could I build?"),
        "What to build shows its full title: {tip:?}"
    );
}

#[test]
fn clicking_an_icons_only_entry_shows_its_page() {
    let mut harness = rail_only(true);
    harness.get_by_label("Net worth").click();
    harness.run();
    assert_eq!(*harness.state(), Page::NetWorth);
}

#[test]
fn an_empty_panel_draws_its_why_what_and_one_button_and_a_pageless_click_is_the_callers() {
    let style = Style::eve_dense();
    let mut harness = Harness::builder()
        .with_size(vec2(400.0, 800.0))
        .build_ui_state(
            move |ui, clicked: &mut bool| {
                style.apply(ui.ctx());
                *clicked |= empty::show(
                    ui,
                    &style,
                    "Why it is empty.",
                    "What to do about it.",
                    Some(Fill {
                        label: "Fill it",
                        page: None,
                    }),
                );
            },
            false,
        );
    harness.run();
    assert!(crate::ui::shown(&harness, "Why it is empty."));
    assert!(crate::ui::shown(&harness, "What to do about it."));
    assert_eq!(harness.get_all_by_role(Role::Button).count(), 1);
    harness.get_by_label("Fill it").click();
    harness.run();
    assert!(*harness.state(), "the click reaches the caller");
    assert_eq!(
        empty::take(&harness.ctx),
        None,
        "a fill without a page stores nothing"
    );
}

#[test]
fn a_fill_with_a_page_makes_the_next_rail_frame_draw_it_and_stores_nothing_after() {
    let style = Style::eve_dense();
    let mut harness = Harness::builder()
        .with_size(vec2(400.0, 800.0))
        .build_ui_state(
            move |ui, page: &mut Page| {
                style.apply(ui.ctx());
                rail::show_with(ui, &style, page, false);
                empty::show(
                    ui,
                    &style,
                    "Why it is empty.",
                    "What to do about it.",
                    Some(Fill {
                        label: "Go",
                        page: Some(Page::Characters),
                    }),
                );
            },
            Page::Today,
        );
    harness.run();
    assert_eq!(*harness.state(), Page::Today);
    harness.get_by_label("Go").click();
    harness.run();
    assert_eq!(
        *harness.state(),
        Page::Characters,
        "the next rail frame shows Characters"
    );
    assert_eq!(
        empty::take(&harness.ctx),
        None,
        "the rail takes the page once"
    );
}

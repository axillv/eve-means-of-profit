use super::*;

use eve_means_of_profit::rail;

#[test]
fn what_to_build_is_the_second_page_and_offers_a_character_when_empty() {
    let env = env();
    let mut harness = start(&env, FakeEve::new(Vec::new(), None), NOW);
    loaded(&mut harness);

    // The decider settled the spec's "nine pages" slip: the original nine plus What to
    // build make ten, with What to build second and Build third.
    assert_eq!(Page::ALL.len(), 10);
    assert_eq!(Page::ALL.get(1), Some(&Page::WhatToBuild));
    assert_eq!(Page::ALL.get(2), Some(&Page::Build));

    rail(&mut harness, "What to build");
    harness.run();
    assert!(shown(&harness, "What could I build?"));
    assert!(shown(&harness, "Profit a day"));
    assert!(shown(
        &harness,
        "Add a character to use your skills, blueprints and slots."
    ));
    assert!(!shown(&harness, "No products to rank yet."));

    harness.get_by_label("Add a character").click();
    harness.run();
    assert!(shown(&harness, "jEve data"), "the button opens Characters");

    rail(&mut harness, "What to build");
    harness.set_size(vec2(1280.0, 800.0));
    harness.run();
    harness.snapshot("what-to-build-empty");
}

#[test]
fn the_what_to_build_page_shows_its_four_views_and_its_filters() {
    let env = env();
    let mut harness = start(&env, FakeEve::new(Vec::new(), None), NOW);
    loaded(&mut harness);
    rail(&mut harness, "What to build");
    harness.run();

    for label in [
        "Best per day",
        "Best margin %",
        "Cheap to start",
        "With my blueprints",
    ] {
        assert!(shown(&harness, label), "{label} is shown");
    }
    assert!(
        combo_on_row(&harness, "Blueprint", "All"),
        "the Blueprint filter is shown on All"
    );
    let toggle = harness
        .query_all_by_label("Only what my skills allow")
        .find(|node| node.accesskit_node().role() == Role::CheckBox)
        .expect("the skills toggle is a checkbox");
    assert_ne!(
        toggle.accesskit_node().toggled(),
        Some(Toggled::True),
        "Only what my skills allow starts unchecked"
    );

    harness.set_size(vec2(1280.0, 800.0));
    harness.run();
    harness.snapshot("what-to-build-filters");
}

/// Whether a dropdown showing `value` sits on a row some node labelled `label` is on. A
/// combo box has no label of its own, only its value, so the label beside it anchors it.
fn combo_on_row(harness: &Harness<'_, App>, label: &str, value: &str) -> bool {
    fn find(node: &egui_kittest::Node<'_>, value: &str, rows: &[f32]) -> bool {
        let accesskit = node.accesskit_node();
        if accesskit.role() == Role::ComboBox
            && accesskit.value().as_deref() == Some(value)
            && rows
                .iter()
                .any(|row| (node.rect().center().y - row).abs() <= 3.0)
        {
            return true;
        }
        node.children().any(|child| find(&child, value, rows))
    }
    let rows: Vec<f32> = harness
        .query_all_by_label(label)
        .map(|node| node.rect().center().y)
        .collect();
    !rows.is_empty() && find(&harness.root(), value, &rows)
}

/// The PLEX market client that never answers: this page reads no PLEX.
struct NoPlex;

impl Http for NoPlex {
    #[rustfmt::skip]
    fn get(
        &self,
        _url: &str,
        _if_none_match: Option<&str>,
    ) -> Result<HttpResponse, MarketError> {
        Err(MarketError::Network("not in this test".to_owned()))
    }
}

/// The What to build page, the app state it draws with, and the folder its store lives in,
/// kept for as long as the harness.
struct WhatToBuildRig {
    page: WhatToBuildPage,
    cx: Shared,
    _dir: TempDir,
}

/// The page drawn on its own with `rows` in it: the app's feed makes no rows yet, so the
/// dead-end test gives the page rows and a shared state of its own.
fn what_to_build_harness(rows: Vec<RankRow>) -> Harness<'static, WhatToBuildRig> {
    let mut page = WhatToBuildPage::default();
    page.rows = rows;
    what_to_build_page_harness(page)
}

/// The page drawn on its own with the state `page` already holds, and the user having
/// answered `Own` so every row's assumption line says so.
fn what_to_build_page_harness(page: WhatToBuildPage) -> Harness<'static, WhatToBuildRig> {
    what_to_build_page_harness_with(page, Some(BlueprintDefault::Own))
}

/// The page drawn on its own with the state `page` already holds and `answer` saved, or none.
fn what_to_build_page_harness_with(
    page: WhatToBuildPage,
    answer: Option<BlueprintDefault>,
) -> Harness<'static, WhatToBuildRig> {
    let dir = TempDir::new().expect("a temp folder");
    let mut store = Store::open(&dir.path().join("user.sqlite"), NOW)
        .expect("the store opens")
        .store;
    if let Some(answer) = answer {
        store
            .set_blueprint_default(answer)
            .expect("the blueprint answer is saved");
    }
    let rig = WhatToBuildRig {
        page,
        cx: Shared {
            sde: None,
            store,
            worker: Worker::spawn(egui::Context::default(), Box::new(NoNetwork)),
            market: Arc::new(Prices::new(Box::new(EmptyForge))),
            plex_http: Arc::new(NoPlex),
            copy_notices: Arc::new(Mutex::new(Vec::new())),
            style: Style::eve_dense(),
            density: Density::Normal,
            rail_icons_only: false,
            refresh_while_open: false,
            data_dir: dir.path().to_path_buf(),
            notices: Vec::new(),
            clock: Box::new(|| NOW),
            session: Session::new(Box::new(NoEve)),
            selection: Selection::default(),
            industry: IndustryData::default(),
            claims: ClaimsState::default(),
        },
        _dir: dir,
    };
    Harness::builder()
        .with_size(vec2(1280.0, 800.0))
        .build_ui_state(
            move |ui, rig: &mut WhatToBuildRig| {
                rig.cx.style.apply(ui.ctx());
                let mut rail_page = Page::WhatToBuild;
                rail::show_with(ui, &rig.cx.style, &mut rail_page, rig.cx.rail_icons_only);
                egui::CentralPanel::default().show(ui, |ui| {
                    let opened = what_to_build::show(ui, &mut rig.page, &mut rig.cx);
                    assert!(opened.is_none(), "the test clicks no page button");
                });
            },
            rig,
        )
}

/// One row of the dead-ends fixture: 20 sales a day, so share 5 sells one unit, `profit_each`
/// a day for that unit, and `dead` saying why the ranking leaves the row out.
#[rustfmt::skip]
fn dead_row(
    name: &str,
    blueprint: Blueprint,
    profit_each: &str,
    dead: Option<DeadEnd>,
) -> RankRow {
    RankRow {
        type_id: DRONE,
        name: name.to_owned(),
        blueprint,
        dead,
        warnings: Vec::new(),
        profit_each: Some(Money::parse_isk(profit_each).expect("an amount")),
        sell_price: Some(Money::parse_isk("1,000.00").expect("an amount")),
        build_cost: None,
        capital: None,
        material_cost: None,
        job_cost: None,
        invention_cost: None,
        fees: None,
        middle: None,
        spread: None,
        sells_day: Some(20),
        capacity_day: Some(20),
        job_seconds: None,
        sell_orders: None,
        needs_skills: None,
    }
}

/// The five rows the dead-end test counts: A is ranked; D and H are bought blueprints with
/// no sales; E has no facility that can build it; G can only come from a contract.
fn dead_rows() -> Vec<RankRow> {
    let price = || Money::parse_isk("499,639.39").expect("an amount");
    vec![
        dead_row("A", Blueprint::Owned { me: 10 }, "300.00", None),
        dead_row(
            "D",
            Blueprint::Buy { price: price() },
            "200.00",
            Some(DeadEnd::NoSales),
        ),
        dead_row(
            "H",
            Blueprint::Buy { price: price() },
            "100.00",
            Some(DeadEnd::NoSales),
        ),
        dead_row(
            "E",
            Blueprint::Owned { me: 10 },
            "50.00",
            Some(DeadEnd::NoFacility),
        ),
        dead_row(
            "G",
            Blueprint::ContractsOnly,
            "10.00",
            Some(DeadEnd::NoBlueprint),
        ),
    ]
}

#[test]
fn the_dead_line_counts_the_rows_left_out_and_toggles_each_reason() {
    let mut harness = what_to_build_harness(dead_rows());
    harness.run();

    let parts = [
        "2 no sales in 30 days",
        "1 no facility can build it",
        "1 no blueprint for sale",
    ];
    for part in parts {
        assert!(crate::ui::shown(&harness, part), "{part} is shown");
    }
    assert!(crate::ui::shown(&harness, "A"), "the live row is ranked");
    assert!(!crate::ui::shown(&harness, "D"), "a dead row stays out");
    let live = crate::ui::find(&harness, "A").expect("the live row").rect();
    for part in parts {
        let shown = crate::ui::find(&harness, part)
            .unwrap_or_else(|| panic!("no node showing {part:?}"))
            .rect();
        assert!(
            shown.min.y > live.bottom(),
            "{part} is under the table's rows"
        );
    }

    harness.set_size(vec2(1280.0, 800.0));
    crate::ui::snapshot(&mut harness, "what-to-build-dead-ends");

    crate::ui::click(&mut harness, "2 no sales in 30 days");
    let live = crate::ui::find(&harness, "A").expect("the live row").rect();
    let dead = crate::ui::find(&harness, "D")
        .expect("its first row")
        .rect();
    assert!(crate::ui::shown(&harness, "H"), "both no-sales rows show");
    assert!(live.min.y < dead.min.y, "they come after the live row");
    assert!(
        crate::ui::shown(&harness, "1 no facility can build it"),
        "showing a reason changes no count"
    );
    assert!(
        !crate::ui::shown(&harness, "E"),
        "the other reasons stay out"
    );

    crate::ui::click(&mut harness, "2 no sales in 30 days");
    assert!(
        !crate::ui::shown(&harness, "D"),
        "the toggle hides its rows again"
    );
}

#[test]
fn the_blueprint_question_is_asked_once_and_saved() {
    let mut page = WhatToBuildPage::default();
    page.rows = vec![dead_row(
        "A",
        Blueprint::Default {
            me: 10,
            te: 20,
            assumed: true,
        },
        "300.00",
        None,
    )];
    let mut harness = what_to_build_page_harness_with(page, None);
    harness.run();

    assert!(
        crate::ui::shown(&harness, "Where do your blueprints come from?"),
        "a page with no answer asks the question"
    );
    for label in ["Copies", "Buy from NPCs", "Only my own"] {
        assert!(crate::ui::shown(&harness, label), "{label} is offered");
    }
    assert!(
        crate::ui::shown(
            &harness,
            "Blueprints you don't hold: copies at ME 10 / TE 20 (assumed)"
        ),
        "the no-answer line is the assumption"
    );

    harness.set_size(vec2(1280.0, 800.0));
    crate::ui::snapshot(&mut harness, "what-to-build-blueprints");

    harness.state_mut().page.blueprint_pick = BlueprintDefault::Copies { me: 8, te: 16 };
    harness.run();
    crate::ui::click(&mut harness, "Save");

    assert!(
        crate::ui::shown(
            &harness,
            "Blueprints you don't hold: copies at ME 8 / TE 16 (your answer)"
        ),
        "the saved line is the user's answer"
    );
    assert!(
        !crate::ui::shown(&harness, "Where do your blueprints come from?"),
        "the question closes once it is answered"
    );
    assert_eq!(
        harness
            .state_mut()
            .cx
            .store
            .blueprint_default()
            .expect("the answer reads back"),
        Some(BlueprintDefault::Copies { me: 8, te: 16 })
    );
}

/// One row of the fit fixture: a ranked product with `type_id` named `name`.
fn fit_row(type_id: u32, name: &str) -> RankRow {
    RankRow {
        type_id,
        ..dead_row(name, Blueprint::Owned { me: 10 }, "100.00", None)
    }
}

/// Ticks the tick box on the row labelled `name`.
fn tick_row(harness: &mut Harness<'_, WhatToBuildRig>, name: &str) {
    let y = crate::ui::find(harness, name)
        .unwrap_or_else(|| panic!("no row showing {name:?}"))
        .rect()
        .center()
        .y;
    harness
        .get_all_by_role(Role::CheckBox)
        .find(|node| (node.rect().center().y - y).abs() < 4.0)
        .unwrap_or_else(|| panic!("no tick on the row {name:?}"))
        .click();
    harness.run();
}

#[rustfmt::skip]
#[test]
fn the_fit_line_sits_under_the_table_for_the_ticked_rows() {
    let mut page = WhatToBuildPage::default();
    page.rows = vec![
        fit_row(1, "Fit one"),
        fit_row(2, "Fit two"),
        fit_row(3, "Fit three"),
    ];
    page.slots = vec![
        CharacterSlots {
            id: 1,
            free_manufacturing: 10,
        },
        CharacterSlots {
            id: 2,
            free_manufacturing: 10,
        },
    ];
    page.job_days = BTreeMap::from([(1, 3), (2, 2), (3, 4)]);
    let mut harness = what_to_build_page_harness(page);
    harness.run();

    let line =
        "These 3 need about 9 job-days; your 2 characters have 20 free manufacturing slots.";
    assert!(
        !crate::ui::shown(&harness, line),
        "no tick means no fit line"
    );

    for name in ["Fit one", "Fit two", "Fit three"] {
        tick_row(&mut harness, name);
    }
    assert!(crate::ui::shown(&harness, line), "the fit line is shown");
    let bottom = ["Fit one", "Fit two", "Fit three"]
        .iter()
        .map(|name| {
            crate::ui::find(&harness, name)
                .unwrap_or_else(|| panic!("no row showing {name:?}"))
                .rect()
                .bottom()
        })
        .fold(f32::MIN, f32::max);
    let line_rect = crate::ui::find(&harness, line)
        .expect("the fit line")
        .rect();
    assert!(
        line_rect.min.y > bottom,
        "the fit line is under the table's rows"
    );

    harness.set_size(vec2(1280.0, 800.0));
    crate::ui::snapshot(&mut harness, "what-to-build-fit");
}

#[test]
fn the_build_page_matches_its_snapshot() {
    let env = env();
    let mut harness = start(&env, FakeEve::new(Vec::new(), None), NOW);
    loaded(&mut harness);
    rail(&mut harness, "Build");
    harness.remove_cursor();
    harness.run();
    harness.snapshot("pages-build");
}

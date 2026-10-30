use super::*;

#[test]
fn the_assets_page_matches_its_snapshot() {
    let env = env();
    let mut harness = start(&env, FakeEve::new(Vec::new(), None), NOW);
    loaded(&mut harness);
    rail(&mut harness, "Assets");
    harness.run();
    harness.snapshot("pages-assets");
}

#[test]
fn the_assets_page_with_no_characters_says_why_and_offers_add_character() {
    let env = env();
    let mut harness = start(&env, FakeEve::new(Vec::new(), None), NOW);
    loaded(&mut harness);
    rail(&mut harness, "Assets");
    harness.run();

    for text in [
        "No characters yet.",
        "The app shows what you own, and where it sits, for each character you add.",
    ] {
        assert_eq!(
            harness.query_all_by_label(text).count(),
            2,
            "{text:?} shows in both panels"
        );
    }
    assert_eq!(
        harness.query_all_by_label("Add character").count(),
        2,
        "each panel offers one Add character"
    );
    assert!(!shown(&harness, "No assets yet."));
    assert!(!shown(&harness, "Refresh a character"));
    assert!(!shown(&harness, "The game data is not in yet."));

    // Both panels' buttons open the Characters page.
    for pick in [0, 1] {
        rail(&mut harness, "Assets");
        click_nth(&mut harness, "Add character", pick);
        assert!(
            shown(&harness, "jEve data"),
            "Add character {pick} shows the Characters page"
        );
    }
}

#[test]
fn a_character_without_saved_assets_offers_refresh_in_both_panels() {
    let env = env();
    let mut harness = start(&env, FakeEve::new(vec![character(MAIN, "Main")], None), NOW);
    loaded(&mut harness);
    rail(&mut harness, "Assets");
    harness.run();

    for text in [
        "No assets yet.",
        "Refresh a character on the Characters page to fetch what you own.",
    ] {
        assert_eq!(
            harness.query_all_by_label(text).count(),
            2,
            "{text:?} shows in both panels"
        );
    }
    assert_eq!(
        harness.query_all_by_label("Refresh a character").count(),
        2,
        "each panel offers one Refresh a character"
    );
    assert!(!shown(&harness, "No characters yet."));
    assert!(!shown(&harness, "Add character"));
    assert!(!shown(&harness, "The game data is not in yet."));

    // Both panels' buttons open the Characters page.
    for pick in [0, 1] {
        rail(&mut harness, "Assets");
        click_nth(&mut harness, "Refresh a character", pick);
        assert!(
            shown(&harness, "jEve data"),
            "Refresh a character {pick} shows the Characters page"
        );
    }
}

#[test]
fn j_eve_stock_without_game_data_says_so_and_offers_no_button() {
    let env = env();
    let jeve = jeve_profile();
    let mut harness = start(
        &env,
        FakeEve::new(Vec::new(), Some(jeve.path().to_path_buf())),
        NOW,
    );
    harness.state_mut().shared_mut().sde = None;
    loaded(&mut harness);
    rail(&mut harness, "Assets");
    harness.run();

    for text in [
        "The game data is not in yet.",
        "It downloads at start; the places and items show when it is in.",
    ] {
        assert_eq!(
            harness.query_all_by_label(text).count(),
            2,
            "{text:?} shows in both panels"
        );
    }
    assert!(!shown(&harness, "Add character"));
    assert!(!shown(&harness, "Refresh a character"));
    assert!(!shown(&harness, "No assets yet."));
    assert!(!shown(&harness, "No characters yet."));
}

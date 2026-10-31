use super::*;

use eve_means_of_profit::theme::Style;
use rusqlite::Connection;

/// Saves an empty plan named `name` in the env's store.
fn seed_plan(env: &Env, name: &str) {
    let mut store = Store::open(&env.data.join("user.sqlite"), NOW)
        .expect("the store opens")
        .store;
    store
        .create_plan(
            &mop_store::PlanRecord {
                id: 0,
                name: name.to_owned(),
                product: 0,
                quantity: 1,
                build: std::collections::BTreeSet::new(),
                me: std::collections::BTreeMap::new(),
                manufacturing: None,
                reactions: None,
                price_rule: PriceRule::SellFivePercent,
                route: None,
                service: None,
                packing: mop_core::freight::Packing::default(),
                pasted_stock: String::new(),
                created_at: NOW,
                updated_at: NOW,
            },
            NOW,
        )
        .expect("the plan is saved");
}

/// The number of plans in the running app's store.
fn plan_count(harness: &mut Harness<'_, App>) -> usize {
    harness
        .state_mut()
        .shared_mut()
        .store
        .plans()
        .expect("the plans")
        .len()
}

/// Clicks the `New plan` empty-state button of the panel titled `panel`, out of the up to four
/// such buttons a Build with no plan on screen draws.
fn click_new_plan_in(harness: &mut Harness<'_, App>, panel: &str) {
    let plan_left = harness.get_by_label("Plan").rect().min.x;
    // A card's header title sits one stroke and one sheet pad inside the panel, so the panel's
    // left edge is that much left of the title.
    let inset = Style::eve_dense().sizes.sheet_pad + 1.0;
    let buy_left = harness.get_by_label("Buy list").rect().min.x - inset;
    let steps_top = harness.get_by_label("Steps").rect().min.y;
    let node = harness
        .get_all_by_label("New plan")
        .find(|node| match panel {
            "Plans" => node.rect().min.x < plan_left,
            "Buy list" => node.rect().min.x >= buy_left,
            "Steps" => node.rect().min.x < buy_left && node.rect().min.y >= steps_top,
            _ => node.rect().min.x < buy_left && node.rect().min.y < steps_top,
        })
        .unwrap_or_else(|| panic!("the New plan button of {panel}"));
    node.click();
    harness.run_steps(6);
}

#[test]
fn plans_are_made_copied_renamed_and_deleted() {
    let env = make_env(&[]);
    let mut harness = start(&env);
    harness.run();
    click(&mut harness, "New plan");
    click(&mut harness, "New");
    assert!(shown(&harness, "New plan"));
    assert!(shown(&harness, "New plan 2"));

    click(&mut harness, "New plan");
    click(&mut harness, "Duplicate");
    assert!(shown(&harness, "New plan (copy)"));

    click(&mut harness, "New plan 2");
    click(&mut harness, "Delete");
    assert!(shown(&harness, "Delete plan New plan 2?"));
    click(&mut harness, "Cancel");
    assert!(shown(&harness, "New plan 2"));

    // The list is in the user's order, not by name: the renamed plan keeps its place between
    // its neighbours.
    let (first, copy) = (
        top_of(&harness, "New plan"),
        top_of(&harness, "New plan (copy)"),
    );
    assert!(first < top_of(&harness, "New plan 2") && top_of(&harness, "New plan 2") < copy);
    click(&mut harness, "Rename");
    type_into(&mut harness, "New plan 2", "Hobgoblin 10");
    click(&mut harness, "Rename");
    assert!(shown(&harness, "Hobgoblin 10"));
    let renamed = top_of(&harness, "Hobgoblin 10");
    assert!(first < renamed && renamed < copy);
}

#[test]
fn product_search_lists_only_types_with_a_recipe() {
    let env = make_env(&standard_facilities());
    let mut harness = start(&env);
    harness.run();
    click(&mut harness, "New plan");
    type_into(&mut harness, "", "hobgoblin");
    assert!(shown(&harness, "Hobgoblin I"));
    assert!(shown(&harness, "Hobgoblin II"));
    assert!(!shown(&harness, "Hobgoblin II Blueprint"));
}

#[test]
fn leaving_an_edited_plan_asks_before_it_goes() {
    let env = make_env(&standard_facilities());
    let mut harness = start(&env);
    harness.run();
    hobgoblin_ii_plan(&mut harness);
    click(&mut harness, "New");
    assert!(shown(&harness, "Save changes to New plan?"));
    click(&mut harness, "Cancel");
    assert!(shown(&harness, "PLAN  Hobgoblin II × 10 (unsaved)"));
}

#[test]
fn a_fresh_build_says_why_each_panel_is_empty_and_new_plan_makes_one() {
    let env = make_env(&standard_facilities());
    let mut harness = start(&env);
    harness.run();

    assert!(shown(&harness, "No plans yet."));
    assert!(shown(
        &harness,
        "A plan starts with a product and a quantity, then gives you a buy list to paste into EVE."
    ));
    assert_eq!(harness.get_all_by_label("No plan on screen.").count(), 3);
    for what in [
        "Pick one in the Plans list, or start a new plan with a product and a quantity.",
        "The steps, and what to build or buy, show when you pick a plan.",
        "What to buy, and the list to paste into EVE, shows when you pick a plan.",
    ] {
        assert!(shown(&harness, what), "{what:?} shows");
    }
    assert!(!shown(&harness, "No plan selected."));
    assert_eq!(harness.get_all_by_label("New plan").count(), 4);
    // None of the panels' own chrome, tables or footers shows.
    for text in ["Columns", "Only these:", "Save", "Pasted stock…"] {
        assert!(!shown(&harness, text), "{text:?} is not drawn");
    }

    click_new_plan_in(&mut harness, "Plans");
    assert!(!shown(&harness, "No plans yet."));
    assert!(!shown(&harness, "No plan on screen."));
    assert_eq!(plan_count(&mut harness), 1);
    assert!(shown(&harness, "PLAN  no product yet × 1"));
}

#[test]
fn a_saved_plan_with_none_selected_says_why_in_three_panels_and_any_new_plan_opens_one() {
    for panel in ["Plan", "Steps", "Buy list"] {
        let env = make_env(&standard_facilities());
        seed_plan(&env, "Old plan");
        let mut harness = start(&env);
        harness.run();

        assert!(!shown(&harness, "No plans yet."));
        assert_eq!(harness.get_all_by_label("No plan on screen.").count(), 3);
        assert!(shown(
            &harness,
            "Pick one in the Plans list, or start a new plan with a product and a quantity."
        ));
        assert!(shown(
            &harness,
            "The steps, and what to build or buy, show when you pick a plan."
        ));
        assert!(shown(
            &harness,
            "What to buy, and the list to paste into EVE, shows when you pick a plan."
        ));
        assert!(!shown(&harness, "No plan selected."));
        // The Plans panel keeps its list and toolbar.
        assert!(shown(&harness, "Old plan"));
        assert!(shown(&harness, "New"));

        click_new_plan_in(&mut harness, panel);
        assert!(
            !shown(&harness, "No plan on screen."),
            "a click in {panel} opens a plan"
        );
        assert_eq!(plan_count(&mut harness), 2);
        assert!(shown(&harness, "PLAN  no product yet × 1"));
    }
}

#[test]
fn a_store_error_in_the_plans_panel_is_not_empty() {
    let env = make_env(&[]);
    let mut harness = start(&env);
    harness.run();
    Connection::open(env.data.join("user.sqlite"))
        .expect("the database opens")
        .execute_batch("PRAGMA foreign_keys = OFF; DROP TABLE plans;")
        .expect("the plans table drops");
    harness.run();

    assert!(!shown(&harness, "No plans yet."));
    assert!(
        harness
            .query_all_by_label_contains("Could not read the plans")
            .next()
            .is_some(),
        "the panel keeps its own error"
    );
}

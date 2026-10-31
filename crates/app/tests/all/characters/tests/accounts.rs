use super::*;

/// Types into the text field that has this caption: clicks it, selects what it holds and types
/// `text` over it.
fn type_into(harness: &mut Harness<'_, App>, caption: &str, text: &str) {
    reveal_field(harness, caption);
    {
        let field = harness
            .query_all_by_label(caption)
            .find(|node| node.accesskit_node().role() == Role::TextInput)
            .unwrap_or_else(|| panic!("no text field captioned {caption}"));
        field.hover();
    }
    harness.run_steps(2);
    {
        let field = harness
            .query_all_by_label(caption)
            .find(|node| node.accesskit_node().role() == Role::TextInput)
            .unwrap_or_else(|| panic!("no text field captioned {caption}"));
        field.click();
    }
    harness.run_steps(2);
    harness.key_press_modifiers(Modifiers::COMMAND, Key::A);
    {
        let field = harness
            .query_all_by_label(caption)
            .find(|node| node.accesskit_node().role() == Role::TextInput)
            .unwrap_or_else(|| panic!("no text field captioned {caption}"));
        field.type_text(text);
    }
    harness.run_steps(3);
}

fn reveal_field(harness: &mut Harness<'_, App>, caption: &str) {
    {
        let field = harness
            .query_all_by_label(caption)
            .find(|node| node.accesskit_node().role() == Role::TextInput)
            .unwrap_or_else(|| panic!("no text field captioned {caption}"));
        field.scroll_to_me();
    }
    harness.run_steps(2);
}

/// The text the field with this caption holds.
fn field_text(harness: &Harness<'_, App>, caption: &str) -> Option<String> {
    harness
        .query_all_by_label(caption)
        .find(|node| node.accesskit_node().role() == Role::TextInput)
        .and_then(|node| node.value())
}

/// Clicks the `index`th button labelled `label`, counting from the top of the page.
fn click_nth(harness: &mut Harness<'_, App>, label: &str, index: usize) {
    {
        let target = harness
            .get_all_by_label(label)
            .filter(|node| node.accesskit_node().role() == Role::Button)
            .nth(index)
            .unwrap_or_else(|| panic!("no button labelled {label} number {index}"));
        target.click();
    }
    harness.run_steps(3);
}

/// Clicks the `index`th combo box that shows `value`, counting from the top of the page. A combo
/// box has no label of its own, only the value it shows.
fn click_combo(harness: &mut Harness<'_, App>, value: &str, index: usize) {
    let mut seen = 0;
    let found = reveal_combo_in(&harness.root(), value, index, &mut seen);
    assert!(found, "no combo box shows {value} number {index}");
    harness.run_steps(2);
    // A drop-down that was just closed by a pick is still open for a frame: move onto this one
    // and let that frame pass before the press, or the press only closes the old list.
    let mut seen = 0;
    hover_combo_in(&harness.root(), value, index, &mut seen);
    harness.run_steps(2);
    let mut seen = 0;
    click_combo_in(&harness.root(), value, index, &mut seen);
    harness.run_steps(3);
}

fn hover_combo_in(
    node: &egui_kittest::Node<'_>,
    value: &str,
    index: usize,
    seen: &mut usize,
) -> bool {
    let accesskit = node.accesskit_node();
    if accesskit.role() == Role::ComboBox && accesskit.value().as_deref() == Some(value) {
        if *seen == index {
            node.hover();
            return true;
        }
        *seen += 1;
    }
    node.children()
        .any(|child| hover_combo_in(&child, value, index, seen))
}

fn reveal_combo_in(
    node: &egui_kittest::Node<'_>,
    value: &str,
    index: usize,
    seen: &mut usize,
) -> bool {
    let accesskit = node.accesskit_node();
    if accesskit.role() == Role::ComboBox && accesskit.value().as_deref() == Some(value) {
        if *seen == index {
            node.scroll_to_me();
            return true;
        }
        *seen += 1;
    }
    node.children()
        .any(|child| reveal_combo_in(&child, value, index, seen))
}

fn click_combo_in(
    node: &egui_kittest::Node<'_>,
    value: &str,
    index: usize,
    seen: &mut usize,
) -> bool {
    let accesskit = node.accesskit_node();
    if accesskit.role() == Role::ComboBox && accesskit.value().as_deref() == Some(value) {
        if *seen == index {
            node.click();
            return true;
        }
        *seen += 1;
    }
    node.children()
        .any(|child| click_combo_in(&child, value, index, seen))
}

/// How many combo boxes show this value.
fn combos_showing(harness: &Harness<'_, App>, value: &str) -> usize {
    let mut seen = 0;
    count_combos_in(&harness.root(), value, &mut seen);
    seen
}

fn count_combos_in(node: &egui_kittest::Node<'_>, value: &str, seen: &mut usize) {
    let accesskit = node.accesskit_node();
    if accesskit.role() == Role::ComboBox && accesskit.value().as_deref() == Some(value) {
        *seen += 1;
    }
    node.children()
        .for_each(|child| count_combos_in(&child, value, seen));
}

/// The accounts the app's own store holds.
fn accounts_in(harness: &mut Harness<'_, App>) -> Vec<Account> {
    harness
        .state_mut()
        .shared_mut()
        .store
        .accounts()
        .expect("the accounts read")
}

/// Seeds an account through the store, with the characters given.
fn seed_account(env: &Env, name: &str, characters: &[u64]) -> i64 {
    let mut store = Store::open(&env.data.join("user.sqlite"), NOW)
        .expect("the store opens")
        .store;
    let account = store.add_account(name).expect("the account");
    for character in characters {
        store
            .set_account(*character, Some(account))
            .expect("a character in it");
    }
    account
}

/// Seeds a PLEX count through the store.
fn seed_plex(env: &Env, account: i64, count: u64, at: i64) {
    let mut store = Store::open(&env.data.join("user.sqlite"), NOW)
        .expect("the store opens")
        .store;
    store.set_plex(account, count, at).expect("the count");
}

/// The PLEX counts the app's own store holds for the account.
fn plex_counts_in(harness: &mut Harness<'_, App>, account: i64) -> Vec<(i64, u64)> {
    harness
        .state_mut()
        .shared_mut()
        .store
        .plex_counts(account)
        .expect("the counts read")
}

/// Empties the text field that has this caption: selects all of it and deletes it.
fn clear_field(harness: &mut Harness<'_, App>, caption: &str) {
    reveal_field(harness, caption);
    {
        let field = harness
            .query_all_by_label(caption)
            .find(|node| node.accesskit_node().role() == Role::TextInput)
            .unwrap_or_else(|| panic!("no text field captioned {caption}"));
        field.hover();
    }
    harness.run_steps(2);
    {
        let field = harness
            .query_all_by_label(caption)
            .find(|node| node.accesskit_node().role() == Role::TextInput)
            .unwrap_or_else(|| panic!("no text field captioned {caption}"));
        field.click();
    }
    harness.run_steps(2);
    harness.key_press_modifiers(Modifiers::COMMAND, Key::A);
    harness.key_press(Key::Backspace);
    harness.run_steps(3);
}

#[test]
fn a_fresh_store_shows_no_account_and_a_new_account_empties_the_field() {
    let env = env();
    let eve = FakeEve::new(&env, partial);
    let mut harness = open(&env, &eve);
    until(&mut harness, "the rows", |h| shown(h, "5m"));
    // Summary has no Account column, so no dropdown.
    assert_eq!(combos_showing(&harness, "(none)"), 0);
    choose_level(&mut harness, "Standard");
    // Both rows say (none) in the Account column.
    assert_eq!(combos_showing(&harness, "(none)"), 2);
    type_into(&mut harness, "New account", " Main acct ");
    press(&mut harness, "Add account", 0);
    let accounts = accounts_in(&mut harness);
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].name, "Main acct");
    assert!(accounts[0].characters.is_empty());
    assert_eq!(field_text(&harness, "New account").as_deref(), Some(""));
    // Both dropdowns still say (none), and both list the new account.
    assert_eq!(combos_showing(&harness, "(none)"), 2);
    click_combo(&mut harness, "(none)", 0);
    assert!(shown(&harness, "(none)"));
    assert!(shown(&harness, "Main acct"));
}

#[test]
fn picking_an_account_for_a_character_stores_it_and_a_new_app_shows_the_same() {
    let env = env();
    let eve = FakeEve::new(&env, partial);
    let mut harness = open(&env, &eve);
    choose_level(&mut harness, "Standard");
    type_into(&mut harness, "New account", "Main acct");
    press(&mut harness, "Add account", 0);
    // Rows sort by name, so Alt's dropdown is the first one saying (none).
    click_combo(&mut harness, "(none)", 0);
    click_nth(&mut harness, "Main acct", 0);
    until(&mut harness, "Alt's account", |h| {
        combos_showing(h, "Main acct") == 1
    });
    assert_eq!(combos_showing(&harness, "(none)"), 1);
    let accounts = accounts_in(&mut harness);
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].name, "Main acct");
    assert_eq!(accounts[0].characters, vec![ALT]);

    // A new app on the same data folder shows the same, once Standard is picked.
    let mut again = open(&env, &eve);
    choose_level(&mut again, "Standard");
    until(&mut again, "the accounts", |h| {
        combos_showing(h, "Main acct") == 1
    });
    assert_eq!(combos_showing(&again, "(none)"), 1);

    // Picking (none) for Alt empties the account.
    click_combo(&mut harness, "Main acct", 0);
    click_nth(&mut harness, "(none)", 0);
    until(&mut harness, "the account to go", |h| {
        combos_showing(h, "(none)") == 2
    });
    assert!(accounts_in(&mut harness)[0].characters.is_empty());
}

#[test]
fn a_taken_or_blank_account_name_is_an_error_and_stores_nothing() {
    let env = env();
    let eve = FakeEve::new(&env, partial);
    let mut harness = open(&env, &eve);
    type_into(&mut harness, "New account", " Main acct ");
    press(&mut harness, "Add account", 0);
    type_into(&mut harness, "New account", "Main acct");
    press(&mut harness, "Add account", 0);
    type_into(&mut harness, "New account", "  ");
    press(&mut harness, "Add account", 0);
    assert_eq!(
        notices(&mut harness),
        vec![
            "An account called \"Main acct\" already exists.".to_owned(),
            "Type a name for the account.".to_owned(),
        ]
    );
    let accounts = accounts_in(&mut harness);
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].name, "Main acct");
    assert!(accounts[0].characters.is_empty());
}

#[test]
fn an_account_with_three_other_characters_is_listed_but_disabled() {
    let env = env();
    let eve = FakeEve::new(&env, partial);
    let mut harness = open(&env, &eve);
    choose_level(&mut harness, "Standard");
    let full = {
        let store = &mut harness.state_mut().shared_mut().store;
        let full = store.add_account("Full").expect("the account");
        for character in [901, 902, 903] {
            store
                .set_account(character, Some(full))
                .expect("a character in it");
        }
        full
    };
    frames(&mut harness);
    // Rows sort by name, so Main's dropdown is the second one saying (none).
    click_combo(&mut harness, "(none)", 1);
    assert!(disabled(&harness, "Full"));
    harness
        .get_all_by_label("Full")
        .find(|node| node.accesskit_node().role() == Role::Button)
        .expect("the entry")
        .hover();
    frames(&mut harness);
    assert!(shown(&harness, "This account already has 3 characters."));

    // A character already in the account is allowed to stay in it: one of the three moves out
    // first, or a fourth would be refused.
    {
        let store = &mut harness.state_mut().shared_mut().store;
        store.set_account(903, None).expect("903 out");
        store.set_account(ALT, Some(full)).expect("Alt in Full");
    }
    frames(&mut harness);
    // Alt's dropdown now shows Full, and its Full entry is enabled.
    click_combo(&mut harness, "Full", 0);
    assert!(!disabled(&harness, "Full"));
}

#[test]
fn plex_counts_are_grouped_and_dates_read_both_ways() {
    assert_eq!(plex_text(0), "0");
    assert_eq!(plex_text(999), "999");
    assert_eq!(plex_text(1_250), "1,250");
    assert_eq!(plex_text(1_234_567), "1,234,567");
    assert_eq!(parse_date("1970-01-10"), Some(9));
    assert_eq!(parse_date("1970-02-30"), None);
    assert_eq!(parse_date("10 Jan"), None);
    assert_eq!(date_text(11), "1970-01-12");
}

#[test]
fn an_account_block_lists_its_characters_and_the_date_starts_today() {
    let env = env();
    seed_account(&env, "Main acct", &[MAIN, ALT]);
    seed_account(&env, "Spare", &[]);
    let eve = FakeEve::new(&env, partial);
    let mut harness = open(&env, &eve);
    until(&mut harness, "the account block", |h| {
        shown(h, "no PLEX count yet")
    });
    assert!(shown(&harness, "Main acct"));
    assert!(shown(&harness, "Alt, Main"));
    assert!(shown(&harness, "Rename Main acct"));
    assert!(shown(&harness, "PLEX in Main acct"));
    assert!(shown(&harness, "no characters"));
    assert_eq!(
        field_text(&harness, "Date (UTC)").as_deref(),
        Some("1970-01-12")
    );
}

#[test]
fn setting_plex_stores_the_count_and_clears_the_field() {
    let env = env();
    let main = seed_account(&env, "Main acct", &[MAIN, ALT]);
    let eve = FakeEve::new(&env, partial);
    let mut harness = open(&env, &eve);
    until(&mut harness, "the account block", |h| {
        shown(h, "no PLEX count yet")
    });
    type_into(&mut harness, "PLEX in Main acct", "1,250");
    press(&mut harness, "Set PLEX", 0);
    until(&mut harness, "the count line", |h| {
        shown(h, "1,250 PLEX, set 0s ago")
    });
    assert_eq!(plex_counts_in(&mut harness, main), vec![(NOW, 1_250)]);
    assert_eq!(
        field_text(&harness, "PLEX in Main acct").as_deref(),
        Some("")
    );
    assert_eq!(
        field_text(&harness, "Date (UTC)").as_deref(),
        Some("1970-01-12")
    );
}

#[test]
fn a_seeded_count_shows_its_age_and_a_new_count_is_added() {
    let env = env();
    let main = seed_account(&env, "Main acct", &[MAIN, ALT]);
    seed_plex(&env, main, 650, 740_800);
    let eve = FakeEve::new(&env, partial);
    let mut harness = open(&env, &eve);
    until(&mut harness, "the seeded count", |h| {
        shown(h, "650 PLEX, set 3d ago")
    });
    type_into(&mut harness, "PLEX in Main acct", "1,250");
    press(&mut harness, "Set PLEX", 0);
    until(&mut harness, "the new count", |h| {
        shown(h, "1,250 PLEX, set 0s ago")
    });
    assert_eq!(
        plex_counts_in(&mut harness, main),
        vec![(740_800, 650), (NOW, 1_250)]
    );
}

#[test]
fn bad_plex_counts_and_dates_are_errors_and_a_past_day_is_kept() {
    let env = env();
    let main = seed_account(&env, "Main acct", &[MAIN, ALT]);
    let eve = FakeEve::new(&env, partial);
    let mut harness = open(&env, &eve);
    until(&mut harness, "the account block", |h| {
        shown(h, "no PLEX count yet")
    });

    type_into(&mut harness, "PLEX in Main acct", "0");
    clear_field(&mut harness, "PLEX in Main acct");
    press(&mut harness, "Set PLEX", 0);
    assert_eq!(
        notices(&mut harness),
        vec!["Type the number of PLEX.".to_owned()]
    );
    clear_notices(&mut harness);

    type_into(&mut harness, "PLEX in Main acct", "12x");
    press(&mut harness, "Set PLEX", 0);
    assert_eq!(
        notices(&mut harness),
        vec!["Type a whole number of PLEX.".to_owned()]
    );
    clear_notices(&mut harness);

    type_into(&mut harness, "PLEX in Main acct", "900");
    type_into(&mut harness, "Date (UTC)", "1970-01-13");
    press(&mut harness, "Set PLEX", 0);
    assert_eq!(
        notices(&mut harness),
        vec!["The date cannot be in the future.".to_owned()]
    );
    clear_notices(&mut harness);

    type_into(&mut harness, "Date (UTC)", "1970-02-30");
    press(&mut harness, "Set PLEX", 0);
    assert_eq!(
        notices(&mut harness),
        vec!["Type the date as year-month-day, like 2026-10-18.".to_owned()]
    );
    clear_notices(&mut harness);

    type_into(&mut harness, "Date (UTC)", "10 Jan");
    press(&mut harness, "Set PLEX", 0);
    assert_eq!(
        notices(&mut harness),
        vec!["Type the date as year-month-day, like 2026-10-18.".to_owned()]
    );
    clear_notices(&mut harness);

    // A good count on a past day is kept at that day's 00:00 UTC.
    type_into(&mut harness, "PLEX in Main acct", "900");
    type_into(&mut harness, "Date (UTC)", "1970-01-10");
    press(&mut harness, "Set PLEX", 0);
    until(&mut harness, "the past count", |h| {
        shown(h, "900 PLEX, set 2d ago")
    });
    assert_eq!(plex_counts_in(&mut harness, main), vec![(777_600, 900)]);
    clear_notices(&mut harness);

    // The same date replaces its count instead of adding another.
    type_into(&mut harness, "PLEX in Main acct", "950");
    type_into(&mut harness, "Date (UTC)", "1970-01-10");
    press(&mut harness, "Set PLEX", 0);
    until(&mut harness, "the replaced count", |h| {
        shown(h, "950 PLEX, set 2d ago")
    });
    assert_eq!(plex_counts_in(&mut harness, main), vec![(777_600, 950)]);
    clear_notices(&mut harness);

    // 0 is a valid count, and an empty date means today.
    type_into(&mut harness, "PLEX in Main acct", "0");
    clear_field(&mut harness, "Date (UTC)");
    press(&mut harness, "Set PLEX", 0);
    until(&mut harness, "the zero count", |h| {
        shown(h, "0 PLEX, set 0s ago")
    });
    assert_eq!(
        plex_counts_in(&mut harness, main),
        vec![(777_600, 950), (NOW, 0)]
    );
}

#[test]
fn an_account_is_renamed_and_a_taken_or_blank_name_is_an_error() {
    let env = env();
    seed_account(&env, "Main acct", &[MAIN, ALT]);
    seed_account(&env, "Spare", &[]);
    let eve = FakeEve::new(&env, partial);
    let mut harness = open(&env, &eve);
    until(&mut harness, "the account block", |h| {
        shown(h, "no PLEX count yet")
    });

    type_into(&mut harness, "Rename Main acct", " Pilots ");
    press(&mut harness, "Rename", 0);
    until(&mut harness, "the new name", |h| shown(h, "Rename Pilots"));
    assert!(accounts_in(&mut harness).iter().any(|a| a.name == "Pilots"));
    assert_eq!(
        field_text(&harness, "Rename Pilots").as_deref(),
        Some("Pilots")
    );

    type_into(&mut harness, "Rename Pilots", "Spare");
    press(&mut harness, "Rename", 0);
    assert_eq!(
        notices(&mut harness),
        vec!["An account called \"Spare\" already exists.".to_owned()]
    );
    clear_notices(&mut harness);

    type_into(&mut harness, "Rename Pilots", "   ");
    press(&mut harness, "Rename", 0);
    assert_eq!(
        notices(&mut harness),
        vec!["Type a name for the account.".to_owned()]
    );
    assert!(accounts_in(&mut harness).iter().any(|a| a.name == "Pilots"));
}

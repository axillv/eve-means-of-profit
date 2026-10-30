use super::*;

use super::loading::*;

/// All eight part ticks, in the order the panel lists them.
fn part_labels() -> [&'static str; 8] {
    [
        "Wallet",
        "Assets",
        "Sell orders",
        "Buy escrow",
        "Industry jobs",
        "Contract collateral",
        "Contracts",
        "Implants",
    ]
}

#[test]
fn the_controls_start_at_the_defaults_and_a_pick_is_saved_at_once() {
    let dir = TempDir::new().expect("a temp folder");
    let mut harness = start(&dir);
    // A record and a copy already exist; picking a price must leave both alone.
    {
        let mut store = store_of(&dir.path().join("data"));
        add_main_first(&mut store);
        store
            .save_point_prices(&load_copy(false))
            .expect("the copy is saved");
    }
    assert_eq!(stored_records(&dir), 1);
    assert_eq!(copies(&dir).len(), 1);

    open_net_worth(&mut harness);
    assert!(
        has_combo(&harness, "30 days"),
        "the range starts at 30 days"
    );
    assert!(
        has_combo(&harness, "Forge sell, 5% price"),
        "the price starts at the app-wide default"
    );
    for part in part_labels() {
        assert!(ticked(&harness, part), "{part} is ticked");
    }
    assert!(ticked(&harness, "PLEX vault"), "the vault is ticked");
    assert!(
        errors(&mut harness).is_empty(),
        "{:?}",
        errors(&mut harness)
    );

    click_combo(&mut harness, "30 days", 0);
    click(&mut harness, "90 days");
    assert!(has_combo(&harness, "90 days"));
    assert_eq!(
        store_of(&dir.path().join("data"))
            .setting("net_worth.range")
            .expect("the range setting"),
        Some("90d".to_owned())
    );

    click_combo(&mut harness, "Forge sell, 5% price", 0);
    click(&mut harness, "Forge buy, 5% price");
    assert!(has_combo(&harness, "Forge buy, 5% price"));
    let store = store_of(&dir.path().join("data"));
    assert_eq!(
        store.value_price().expect("the saved price"),
        PriceRule::BuyFivePercent
    );
    assert!(
        errors(&mut harness).is_empty(),
        "{:?}",
        errors(&mut harness)
    );
    drop(store);
    assert_eq!(stored_records(&dir), 1, "the records are unchanged");
    assert_eq!(copies(&dir).len(), 1, "the price copies are unchanged");
    drop(harness);

    let mut restarted = start(&dir);
    open_net_worth(&mut restarted);
    assert!(
        has_combo(&restarted, "90 days"),
        "the range is still there after a restart"
    );
    assert!(has_combo(&restarted, "Forge buy, 5% price"));
}

#[test]
fn part_ticks_keep_the_basis_and_plex_is_saved_on_its_own() {
    let dir = TempDir::new().expect("a temp folder");
    let mut harness = start(&dir);
    {
        let mut store = store_of(&dir.path().join("data"));
        store
            .save_net_worth_settings(&NetWorthSettings {
                basis: PriceBasis::LowestSell,
                shown: all_parts(),
            })
            .expect("the basis is saved");
    }

    open_net_worth(&mut harness);
    for part in part_labels() {
        assert!(ticked(&harness, part), "{part} is ticked");
    }

    click_checkbox(&mut harness, "Sell orders");
    assert!(!ticked(&harness, "Sell orders"));
    let mut store = store_of(&dir.path().join("data"));
    let settings = store.net_worth_settings().expect("the settings read");
    assert_eq!(settings.basis, PriceBasis::LowestSell, "the basis is kept");
    let mut seven = all_parts();
    seven.remove(&Part::SellOrders);
    assert_eq!(settings.shown, seven);
    assert_eq!(settings.shown.len(), 7);
    drop(store);

    click_checkbox(&mut harness, "Sell orders");
    assert!(ticked(&harness, "Sell orders"));
    assert_eq!(
        store_of(&dir.path().join("data"))
            .net_worth_settings()
            .expect("the settings read")
            .shown,
        all_parts()
    );

    click_checkbox(&mut harness, "PLEX vault");
    assert!(!ticked(&harness, "PLEX vault"));
    assert_eq!(
        store_of(&dir.path().join("data"))
            .setting("net_worth.plex")
            .expect("the PLEX setting"),
        Some("0".to_owned())
    );

    click_checkbox(&mut harness, "PLEX vault");
    assert!(ticked(&harness, "PLEX vault"));
    assert_eq!(
        store_of(&dir.path().join("data"))
            .setting("net_worth.plex")
            .expect("the PLEX setting"),
        Some("1".to_owned())
    );
}

#[test]
fn a_failed_save_says_so_and_keeps_the_old_choice() {
    let dir = TempDir::new().expect("a temp folder");
    let mut harness = start(&dir);
    open_net_worth(&mut harness);
    assert!(has_combo(&harness, "30 days"));

    let db = dir.path().join("data").join("user.sqlite");
    rusqlite::Connection::open(&db)
        .expect("the database opens")
        .execute_batch("PRAGMA foreign_keys = OFF; DROP TABLE app_settings;")
        .expect("the settings table drops");

    click_combo(&mut harness, "30 days", 0);
    click(&mut harness, "7 days");
    assert!(
        has_combo(&harness, "30 days"),
        "the old range is still shown"
    );
    assert!(!has_combo(&harness, "7 days"));
    assert!(
        errors(&mut harness).iter().any(|text| {
            text.starts_with("Could not save the net worth settings: ")
                && text.contains("no such table: app_settings")
        }),
        "{:?}",
        errors(&mut harness)
    );
}

#[test]
fn a_failed_read_says_so_once_and_uses_the_defaults() {
    let dir = TempDir::new().expect("a temp folder");
    let mut harness = start(&dir);
    let db = dir.path().join("data").join("user.sqlite");
    rusqlite::Connection::open(&db)
        .expect("the database opens")
        .execute_batch("PRAGMA foreign_keys = OFF; DROP TABLE net_worth_settings;")
        .expect("the settings table drops");

    open_net_worth(&mut harness);
    harness.run_steps(4);
    let read_errors: Vec<String> = errors(&mut harness)
        .into_iter()
        .filter(|text| text.starts_with("Could not read the net worth settings: "))
        .collect();
    assert_eq!(read_errors.len(), 1, "{read_errors:?}");
    assert!(
        read_errors[0].contains("no such table: net_worth_settings"),
        "{read_errors:?}"
    );
    assert!(has_combo(&harness, "30 days"));
    assert!(has_combo(&harness, "Forge sell, 5% price"));
    for part in part_labels() {
        assert!(ticked(&harness, part), "{part} keeps its default");
    }
    assert!(ticked(&harness, "PLEX vault"));
}

// The headline tests below write their records and price copies straight to the app's store,
// then draw the Net worth page over them. The clock stays at NOW, so only leaving the page or
// changing a control reads the range again.

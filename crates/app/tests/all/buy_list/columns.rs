use super::*;

use super::copy::{copied_text, copy_multibuy};

/// Ticks or unticks the column labelled `label` through the Columns menu.
fn toggle_column(harness: &mut Harness<'_, App>, label: &str) {
    click(harness, "Columns");
    click(harness, label);
    harness.key_press(Key::Escape);
    harness.run();
}

/// Whether the checkbox labelled `label` is ticked.
fn checked(harness: &Harness<'_, App>, label: &str) -> bool {
    let node = harness
        .query_all_by_label(label)
        .find(|node| node.accesskit_node().toggled().is_some())
        .unwrap_or_else(|| panic!("no checkbox labelled {label}"));
    node.accesskit_node().toggled() == Some(egui::accesskit::Toggled::True)
}

/// The hidden columns saved for the buy list, as the store holds them.
fn saved_columns(env: &Env) -> Option<String> {
    let store = Store::open(&env.data.join("user.sqlite"), NOW)
        .expect("the store opens")
        .store;
    store
        .setting("columns.build_buy_list")
        .expect("the setting")
}

/// Saves `text` as the buy list's hidden columns, as an earlier session would have.
fn save_columns(env: &Env, text: &str) {
    let mut store = Store::open(&env.data.join("user.sqlite"), NOW)
        .expect("the store opens")
        .store;
    store
        .set_setting("columns.build_buy_list", text)
        .expect("the setting is saved");
}

#[test]
fn hidden_columns_leave_the_table() {
    let world = World::new();
    let env = make_env(&standard_services());
    let mut harness = start(&env, &world);
    harness.run();
    buy_list_for(&mut harness);
    settle(&mut harness, "Oldest: 4m");
    for column in ["Live", "Diff", "Age"] {
        toggle_column(&mut harness, column);
    }
    assert!(!shown(&harness, "Live"));
    assert!(!shown(&harness, "Diff"));
    assert!(!shown(&harness, "Age"));
    assert!(shown(&harness, "freight "));
    assert!(shown(&harness, "110 K"));
    assert!(shown(&harness, " · collateral 1.40 M · delivered 1.51 M"));
}

#[test]
fn hiding_need_own_and_price_removes_their_headers_cells_and_footer_sums() {
    let world = World::new();
    let env = make_env(&standard_services());
    let mut harness = start(&env, &world);
    harness.run();
    buy_list_for(&mut harness);
    settle(&mut harness, "Oldest: 4m");
    let names = [
        "Hobgoblin I",
        "Guidance Systems",
        "Robotics",
        "Morphite",
        "R.A.M.- Robotics",
        "Particle Accelerator Unit",
        "Datacore - Electronic Engineering",
        "Datacore - Graviton Physics",
    ];
    let tops_before: Vec<f32> = names
        .iter()
        .map(|row| row_anchor(&harness, row).1)
        .collect();
    let copy_before = copy_multibuy(&mut harness);
    for column in ["Need", "Own", "Price"] {
        toggle_column(&mut harness, column);
    }
    // The headers and row cells lose the three columns; the Steps panel has its own Need and
    // Own, so the Buy list header row is read from its Item column.
    let headers = line_cells(&harness, rightmost(&harness, "Item"));
    let header_texts: Vec<&str> = headers.iter().map(|(_, text)| text.as_str()).collect();
    assert_eq!(
        header_texts,
        ["Item", "Buy", "m³", "Value", "Live", "Diff", "Age"]
    );
    assert_eq!(
        cells(&harness, "Hobgoblin I"),
        [
            "Hobgoblin I",
            "10",
            "50.00",
            "275,000.00",
            "27,500.00",
            "4m",
        ]
    );
    // The footer keeps the Buy sum, the volume and the value under their columns.
    let footers = line_cells(&harness, harness.get_by_label("Total"));
    let footer_texts: Vec<&str> = footers.iter().map(|(_, text)| text.as_str()).collect();
    assert_eq!(footer_texts, ["Total", "66", "121.10", "1,395,936.00"]);
    assert!((footers[0].0.min.x - headers[0].0.min.x).abs() <= 1.0);
    for (footer, header) in [(1, 1), (2, 2), (3, 3)] {
        let off = (footers[footer].0.max.x - headers[header].0.max.x).abs();
        assert!(
            off <= 1.0,
            "{} is {off} points off its header",
            footers[footer].1
        );
    }
    // The multibuy copy and the row order do not depend on the shown columns.
    assert_eq!(copy_multibuy(&mut harness), copy_before);
    let tops_after: Vec<f32> = names
        .iter()
        .map(|row| row_anchor(&harness, row).1)
        .collect();
    assert_eq!(tops_after, tops_before);
}

#[test]
fn hidden_column_text_is_split_and_joined_on_commas() {
    use eve_means_of_profit::table::{hidden_from_text, hidden_to_text};

    assert_eq!(hidden_from_text("volume,share"), ["volume", "share"]);
    assert_eq!(hidden_from_text(""), Vec::<String>::new());
    assert_eq!(hidden_from_text(",volume,,"), ["volume"]);
    assert_eq!(
        hidden_to_text(&["volume".to_owned(), "share".to_owned()]),
        "volume,share"
    );
    assert_eq!(hidden_to_text(&[]), "");
}

#[test]
fn hidden_columns_are_saved_and_restored_by_a_new_app() {
    let world = World::new();
    let env = make_env(&standard_services());
    let mut harness = start(&env, &world);
    harness.run();
    buy_list_for(&mut harness);
    settle(&mut harness, "Oldest: 4m");

    toggle_column(&mut harness, "m³");
    assert!(!shown(&harness, "m³"));
    assert_eq!(saved_columns(&env).as_deref(), Some("volume"));

    drop(harness);
    let mut harness = start(&env, &world);
    harness.run();
    buy_list_for(&mut harness);
    settle(&mut harness, "Oldest: 4m");
    assert!(!shown(&harness, "m³"));
    click(&mut harness, "Columns");
    assert!(!checked(&harness, "m³"));
}

#[test]
fn showing_a_hidden_column_again_clears_the_saved_key() {
    let world = World::new();
    let env = make_env(&standard_services());
    save_columns(&env, "volume");
    let mut harness = start(&env, &world);
    harness.run();
    buy_list_for(&mut harness);
    settle(&mut harness, "Oldest: 4m");
    assert!(!shown(&harness, "m³"));

    toggle_column(&mut harness, "m³");
    assert!(shown(&harness, "m³"));
    assert_eq!(saved_columns(&env), None);
}

#[test]
fn an_unknown_hidden_id_is_ignored_and_dropped_at_the_next_save() {
    let world = World::new();
    let env = make_env(&standard_services());
    save_columns(&env, "nope");
    let mut harness = start(&env, &world);
    harness.run();
    buy_list_for(&mut harness);
    settle(&mut harness, "Oldest: 4m");
    for column in ["m³", "Live", "Diff", "Age"] {
        assert!(shown(&harness, column), "{column} is not drawn");
    }

    toggle_column(&mut harness, "m³");
    assert!(!shown(&harness, "m³"));
    assert_eq!(saved_columns(&env).as_deref(), Some("volume"));
}

#[cfg(unix)]
#[test]
fn a_failed_save_is_reported_and_the_column_stays_hidden() {
    use std::os::unix::fs::PermissionsExt;

    let world = World::new();
    let env = make_env(&standard_services());
    let mut harness = start(&env, &world);
    harness.run();
    buy_list_for(&mut harness);
    settle(&mut harness, "Oldest: 4m");
    // A data folder that cannot be written to makes the journal, and so the save, fail.
    let writable = fs::metadata(&env.data)
        .expect("the data folder")
        .permissions();
    let mut read_only = writable.clone();
    read_only.set_mode(0o555);
    fs::set_permissions(&env.data, read_only).expect("the data folder is read-only");

    toggle_column(&mut harness, "m³");
    fs::set_permissions(&env.data, writable).expect("the data folder is writable again");

    assert!(
        harness
            .query_all(by().label_contains("Could not save the settings:"))
            .next()
            .is_some()
    );
    assert!(!shown(&harness, "m³"));
    assert_eq!(saved_columns(&env), None);
}

#[test]
fn copy_as_tsv_leaves_out_the_hidden_columns_and_keeps_the_footer() {
    let world = World::new();
    let env = make_env(&standard_services());
    let mut harness = start(&env, &world);
    harness.run();
    buy_list_for(&mut harness);
    settle(&mut harness, "Oldest: 4m");
    for column in ["Live", "Diff", "Age"] {
        click(&mut harness, "Columns");
        click(&mut harness, column);
        harness.key_press(Key::Escape);
        harness.run();
    }
    // The copy is asked for in memory, as the panel's menu does, and the table makes it in the
    // next frame it draws.
    harness
        .ctx
        .data_mut(|data| data.insert_temp(copy_request(Id::new("Buy list")), true));
    harness.step();
    assert_eq!(
        copied_text(&harness).as_deref(),
        Some(
            "Item\tNeed\tOwn\tBuy\tPrice\tm³\tValue\n\
             Hobgoblin I\t10\t0\t10\t27500\t50\t275000\n\
             Guidance Systems\t10\t0\t10\t10450\t30\t104500\n\
             Robotics\t10\t0\t10\t39990\t30\t399900\n\
             Morphite\t10\t0\t10\t2150\t0.1\t21500\n\
             R.A.M.- Robotics\t10\t0\t10\t478.6\t0.4\t4786\n\
             Particle Accelerator Unit\t10\t0\t10\t30900\t10\t309000\n\
             Datacore - Electronic Engineering\t3\t0\t3\t50000\t0.3\t150000\n\
             Datacore - Graviton Physics\t3\t0\t3\t43750\t0.3\t131250\n\
             Total\t66\t0\t66\t\t121.1\t1395936"
        )
    );
}

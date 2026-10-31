use super::*;

#[test]
fn owned_rows_and_the_total_for_pau() {
    let env = env();
    with_main(&env);
    name_home_fort(&env);
    let jeve = jeve_profile();
    let eve = FakeEve {
        characters: vec![character(MAIN, "Main")],
        jeve: Some(jeve.path().to_path_buf()),
    };
    let (mut harness, _) = start(&env, eve, fresh_market(NOW - 240));
    loaded(&mut harness);
    select(&mut harness, PAU);

    assert!(shown(&harness, "Home Fort"));
    assert!(shown(&harness, "Alt (jEve)"));
    assert!(shown(&harness, JITA_NAME));
    assert!(!shown(&harness, "None owned."));
    // Rows in order: Main's 120 in Home Fort, Alt's 61 in Jita, Main's 40 in Jita, then the total.
    let home = centre_ys(&harness, "Home Fort");
    let alt = centre_ys(&harness, "Alt (jEve)");
    let jita = centre_ys(&harness, JITA_NAME);
    let total = centre_ys(&harness, "Total");
    assert!(
        on_row(&harness, "Home Fort", &["120", "120"]),
        "Main's row in Home Fort"
    );
    assert!(on_row(&harness, "Alt (jEve)", &["61", "60"]), "Alt's row");
    assert!(
        on_row(&harness, JITA_NAME, &["40", "40"]),
        "Main's row in Jita"
    );
    assert!(on_row(&harness, "Total", &["221", "220"]), "the footer");
    let first_alt = alt.iter().copied().fold(f32::MAX, f32::min);
    let last_jita = jita.iter().copied().fold(f32::MIN, f32::max);
    let first_total = total.iter().copied().fold(f32::MAX, f32::min);
    assert!(
        home.iter().any(|y| *y < first_alt),
        "Home Fort's row is first"
    );
    assert!(first_alt < last_jita, "Alt's row is above Main's Jita row");
    assert!(last_jita < first_total, "the footer is last");
}

/// The centre heights of the labels `text`, one per node with that label.
fn centre_ys(harness: &Harness<'_, App>, text: &str) -> Vec<f32> {
    harness
        .query_all_by_label(text)
        .map(|node| node.rect().center().y)
        .collect()
}

/// Whether each of `cells` is drawn on the same line as a label `anchor`.
fn on_row(harness: &Harness<'_, App>, anchor: &str, cells: &[&str]) -> bool {
    let rows = centre_ys(harness, anchor);
    cells.iter().all(|cell| {
        centre_ys(harness, cell)
            .iter()
            .any(|y| rows.iter().any(|row| (row - y).abs() < 3.0))
    })
}

/// Main's saved data with its job ending at `end`.
fn save_main_with_job_ending(env: &Env, end: i64) {
    let mut data = main_data(990_000, NOW);
    if let Some(jobs) = data.jobs.as_mut() {
        for job in &mut jobs.data {
            job.end = end;
        }
    }
    saved::save_character(&esi_dir(env), &data).expect("Main's data saved");
}

#[test]
fn a_job_that_has_ended_shows_ready() {
    let env = env();
    save_main_with_job_ending(&env, NOW);
    name_home_fort(&env);
    let (mut harness, _) = start(
        &env,
        FakeEve {
            characters: vec![character(MAIN, "Main")],
            jeve: None,
        },
        Answer::Down,
    );
    loaded(&mut harness);
    select(&mut harness, PAU);

    assert!(
        on_row(&harness, "Home Fort", &["ready"]),
        "a job that ends now is ready"
    );
    assert!(!shown(&harness, "in 0s"));
}

#[test]
fn the_counted_column_can_be_hidden_and_the_choice_is_saved() {
    let env = env();
    with_main(&env);
    name_home_fort(&env);
    let (mut harness, _) = start(
        &env,
        FakeEve {
            characters: vec![character(MAIN, "Main")],
            jeve: None,
        },
        Answer::Down,
    );
    loaded(&mut harness);
    select(&mut harness, PAU);
    assert!(shown(&harness, "Counted"));

    harness
        .get_all_by_label("Columns")
        .next()
        .expect("a Columns menu")
        .click();
    harness.run();
    harness
        .get_all_by_label("Counted")
        .last()
        .expect("the Counted tick")
        .click();
    harness.run();
    harness.key_press(egui::Key::Escape);
    harness.run();

    assert!(!shown(&harness, "Counted"), "the Counted column is hidden");
    let saved = harness
        .state_mut()
        .shared_mut()
        .store
        .setting("columns.item_owned")
        .expect("the setting reads");
    assert_eq!(saved.as_deref(), Some("counted"));
}

#[test]
fn running_job_row_for_pau() {
    let env = env();
    with_main(&env);
    name_home_fort(&env);
    let (mut harness, _) = start(
        &env,
        FakeEve {
            characters: vec![character(MAIN, "Main")],
            jeve: None,
        },
        Answer::Down,
    );
    loaded(&mut harness);
    select(&mut harness, PAU);

    assert!(shown(&harness, "Running jobs"));
    assert!(
        on_row(&harness, "Home Fort", &["10", "in 2h"]),
        "Main · Home Fort · 10 · in 2h"
    );
}

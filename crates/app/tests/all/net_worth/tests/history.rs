use super::*;

use super::loading::*;

// The jEve history tests below use M1-132's harness at the moment jEve's file was written,
// and a fake whose profile folder is under the temp folder.

/// The clock of the jEve history tests: day 20,371, a little after the file's last point.
const HISTORY_NOW: i64 = 1_760_100_000;
/// The owner id in the Tracker file: `Id(2112625428)`.
const JEVE_CHARACTER: u64 = 2_112_625_428;
/// M1-41's `totals.json`, verbatim: two points of one owner.
const TRACKER: &str = r#"{"2112625428":[{"date":1760000000000,"assets":2.252799953559E10,"escrows":0.0,"escrowstocover":0.0,"sellorders":0.0,"walletbalance":6.04675937223E9,"manufacturing":3258000.0,"contractcollateral":4.33E8,"contractvalue":1.6E7,"skillpoints":42298200000,"implants":0.30000000000000004},{"date":1760086400999,"assets":1.0,"escrows":0.0,"escrowstocover":0.0,"sellorders":0.0,"walletbalance":2.5,"manufacturing":0.0,"contractcollateral":0.0,"contractvalue":0.0}]}"#;

/// Makes `<temp>/jeve/profiles` and, when given, `<temp>/jeve/data/tracker.json`, and returns
/// the profiles folder the fake's `jeve_folder` gives.
fn jeve_folder(data: &TempDir, tracker: Option<&str>) -> PathBuf {
    let jeve = data.path().join("jeve");
    let profiles = jeve.join("profiles");
    fs::create_dir_all(&profiles).expect("the profiles folder");
    if let Some(text) = tracker {
        fs::create_dir_all(jeve.join("data")).expect("the jEve data folder");
        fs::write(jeve.join("data").join("tracker.json"), text).expect("the Tracker file");
    }
    profiles
}

/// Opens the harness at `HISTORY_NOW` with jEve's folder given.
fn start_history(data: &TempDir, jeve_folder: Option<PathBuf>) -> Harness<'static, App> {
    start_at(
        data,
        Box::new(EmptyForge),
        vec![character_at(MAIN, "Main", HISTORY_NOW)],
        HISTORY_NOW,
        jeve_folder,
    )
}

/// The records the store holds from `from` to `to`.
fn records_between(data: &TempDir, from: i64, to: i64) -> Vec<Record> {
    store_of(&data.path().join("data"))
        .records(from, to)
        .expect("records read")
}

/// The price copies the store holds from `from` to `to`.
fn copies_between(data: &TempDir, from: i64, to: i64) -> Vec<PointPrices> {
    store_of(&data.path().join("data"))
        .point_prices(from, to)
        .expect("the copies read")
}

/// Every notice's level and text, in the order they arrived.
fn notices(harness: &mut Harness<'static, App>) -> Vec<(Level, String)> {
    harness
        .state_mut()
        .shared_mut()
        .notices
        .iter()
        .map(|notice| (notice.level, notice.text.clone()))
        .collect()
}

/// Runs frames until a notice is exactly `text` at `level`.
fn wait_for_notice(harness: &mut Harness<'static, App>, level: Level, text: &str) {
    let wanted = text.to_owned();
    settle(harness, "the import notice", |h| {
        notices(h)
            .iter()
            .any(|(shown, notice)| *shown == level && *notice == wanted)
    });
}

#[test]
fn importing_jeve_history_adds_its_points_and_a_second_click_adds_none() {
    let dir = TempDir::new().expect("a temp folder");
    let profiles = jeve_folder(&dir, Some(TRACKER));
    let mut harness = start_history(&dir, Some(profiles));
    open_net_worth(&mut harness);
    assert!(
        notices(&mut harness).is_empty(),
        "{:?}",
        notices(&mut harness)
    );

    click(&mut harness, "Import jEve history");
    wait_for_notice(
        &mut harness,
        Level::Notice,
        "Imported 2 points from jEve (0 already there, 0 after the app's own data).",
    );

    let records = records_between(&dir, 0, HISTORY_NOW);
    assert_eq!(records.len(), 2);
    for record in &records {
        assert_eq!(record.character, CharacterKey::Id(JEVE_CHARACTER));
        assert!(matches!(record.snapshot, Snapshot::Imported(_)));
    }
    assert_eq!(records[0].at, 1_760_000_000);
    assert_eq!(records[1].at, 1_760_086_400);
    assert!(copies_between(&dir, 0, HISTORY_NOW).is_empty());

    // The page was invalidated: the two imported points are the headline's range.
    assert!(shown(&harness, "Net worth 3.50 ISK"));
    assert!(shown(&harness, "-29.03 B (-100.00%) since yesterday"));

    click(&mut harness, "Import jEve history");
    wait_for_notice(
        &mut harness,
        Level::Notice,
        "Imported 0 points from jEve (2 already there, 0 after the app's own data).",
    );
    assert_eq!(records_between(&dir, 0, HISTORY_NOW).len(), 2);
}

#[test]
fn an_imported_point_at_or_after_the_first_seen_record_is_skipped() {
    let dir = TempDir::new().expect("a temp folder");
    let profiles = jeve_folder(&dir, Some(TRACKER));
    let mut harness = start_history(&dir, Some(profiles));
    harness
        .state_mut()
        .shared_mut()
        .store
        .add_record(&load_record(
            JEVE_CHARACTER,
            1_760_050_000,
            load_seen(Some("1.00"), &[], None, None, &[]),
        ))
        .expect("the seen record is written");
    open_net_worth(&mut harness);

    click(&mut harness, "Import jEve history");
    wait_for_notice(
        &mut harness,
        Level::Notice,
        "Imported 1 points from jEve (0 already there, 1 after the app's own data).",
    );

    let imported: Vec<Record> = records_between(&dir, 0, HISTORY_NOW)
        .into_iter()
        .filter(|record| matches!(record.snapshot, Snapshot::Imported(_)))
        .collect();
    assert_eq!(imported.len(), 1);
    assert_eq!(imported[0].at, 1_760_000_000);
}

#[test]
fn a_missing_tracker_file_or_folder_says_so_and_writes_nothing() {
    let dir = TempDir::new().expect("a temp folder");
    let profiles = jeve_folder(&dir, None);
    let path = dir.path().join("jeve").join("data").join("tracker.json");
    let mut harness = start_history(&dir, Some(profiles));
    open_net_worth(&mut harness);
    click(&mut harness, "Import jEve history");
    wait_for_notice(
        &mut harness,
        Level::Notice,
        &format!("No jEve history found at {}.", path.display()),
    );
    assert!(records_between(&dir, 0, HISTORY_NOW).is_empty());
    assert!(copies_between(&dir, 0, HISTORY_NOW).is_empty());
    drop(harness);

    let other = TempDir::new().expect("a temp folder");
    let mut harness = start_history(&other, None);
    open_net_worth(&mut harness);
    click(&mut harness, "Import jEve history");
    wait_for_notice(
        &mut harness,
        Level::Notice,
        "No jEve folder found, so there is no history to import.",
    );
    assert!(records_between(&other, 0, HISTORY_NOW).is_empty());
    assert!(copies_between(&other, 0, HISTORY_NOW).is_empty());
}

#[test]
fn a_tracker_file_not_in_jeves_format_is_an_error_and_writes_nothing() {
    let dir = TempDir::new().expect("a temp folder");
    let profiles = jeve_folder(&dir, Some(r#"{"x":5}"#));
    let mut harness = start_history(&dir, Some(profiles));
    open_net_worth(&mut harness);
    click(&mut harness, "Import jEve history");
    settle(&mut harness, "the Tracker error", |h| {
        notices(h).iter().any(|(level, text)| {
            *level == Level::Error
                && text.starts_with(
                    "Could not import jEve history: the Tracker file is not in the format this app reads: ",
                )
        })
    });
    assert!(records_between(&dir, 0, HISTORY_NOW).is_empty());
    assert!(copies_between(&dir, 0, HISTORY_NOW).is_empty());
}

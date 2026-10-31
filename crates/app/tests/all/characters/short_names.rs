use super::*;

use eve_means_of_profit::characters::short_names;

/// A jEve profile holding only Main, with one asset at Jita 4-4, updated 700,000 s ago (3d).
fn fill_main_jeve(folder: &Path) {
    let connection = Connection::open(folder.join("#Default.db")).expect("database");
    connection
        .execute_batch(
            "CREATE TABLE owners (accountid TEXT, ownerid INTEGER, name TEXT, corp TEXT, show NUMERIC, \
             invalid NUMERIC, assetslastupdate INTEGER, assetsnextupdate INTEGER, scopes TEXT, \
             UNIQUE(accountid));
             CREATE TABLE assets (accountid TEXT, count INTEGER, flagid INTEGER, flagstring TEXT, \
             itemid INTEGER, typeid INTEGER, locationid INTEGER, singleton NUMERIC, rawquantity INTEGER, \
             UNIQUE(accountid, itemid));
             CREATE TABLE blueprints (accountid TEXT, itemid INTEGER, locationid INTEGER, typeid INTEGER, \
             flagid INTEGER, flagstring TEXT, quantity INTEGER, timeefficiency INTEGER, \
             materialefficiency INTEGER, runs NUMERIC, UNIQUE(accountid, itemid));
             INSERT INTO owners (accountid, ownerid, name, show, invalid, assetslastupdate, scopes) VALUES
              ('j1', 2112000001, 'Main', 1, 0, 700000000, 'esi-assets.read_assets.v1');
             INSERT INTO assets (accountid, itemid, typeid, locationid, flagid, flagstring, count, singleton, rawquantity) VALUES
              ('j1', 7001, 11481, 60003760, 4, 'Hangar', 60, 0, NULL);",
        )
        .expect("fixture builds");
    drop(connection);
}

/// Main's sell order of the Particle Accelerator Unit at Jita 4-4, open for 3 more days.
fn sell_order() -> Order {
    Order {
        order_id: 9_001,
        type_id: PAU,
        location_id: JITA_4_4,
        region_id: 10_000_002,
        is_buy: false,
        price: Money::from_ten_thousandths(615_000_000),
        volume_remain: 100,
        volume_total: 200,
        issued: 900_000,
        expires: NOW + 260_000,
        range: "station".to_owned(),
        escrow: None,
        min_volume: None,
        is_corporation: false,
    }
}

/// Main's active job, making one Particle Accelerator Unit at Home Fort.
fn active_job() -> Job {
    Job {
        job_id: 5_001,
        activity_id: 1,
        blueprint_type_id: RIFTER_BLUEPRINT,
        product_type_id: Some(PAU),
        runs: 1,
        status: JobStatus::Active,
        start: NOW - 86_400,
        end: NOW + 7_500,
        facility_id: HOME_FORT,
        station_id: HOME_FORT,
        output_location_id: HOME_FORT,
        installer_id: i64::try_from(MAIN).expect("an id that fits"),
        cost: None,
        licensed_runs: None,
    }
}

/// Main's asset, order and job: a row for every table with a Character column. Nothing is
/// fetched for the other characters, so their rows stay empty.
fn tables(data: &mut CharacterData) -> Results {
    let found = data.character_id == MAIN;
    if found {
        let asset = Asset {
            item_id: 7,
            type_id: PAU,
            location_id: HOME_FORT,
            flag: Some("Hangar".to_owned()),
            quantity: 3,
            singleton: false,
        };
        data.assets = Some(held(owned(MAIN, vec![asset]), NOW, None));
        data.orders = Some(held(vec![sell_order()], NOW, None));
        data.jobs = Some(held(vec![active_job()], NOW, None));
    }
    Kind::ALL
        .into_iter()
        .map(|kind| {
            let answer = if found {
                Ok(Refreshed::Fetched)
            } else {
                Ok(Refreshed::NotYet { until: NOW + 90 })
            };
            (kind, answer)
        })
        .collect()
}

const JITA_4_4: i64 = 60_003_760;

/// The Particle Accelerator Unit: a type the game data names, holds in the fixture and builds.
const PAU: u32 = 11_688;
/// The blueprint the fixture's manufacturing job uses.
const RIFTER_BLUEPRINT: u32 = 691;

/// The short name the app's own store holds for Main.
fn main_short_name(harness: &mut Harness<'_, App>) -> Option<String> {
    harness
        .state_mut()
        .shared_mut()
        .store
        .setting("character.short.2112000001")
        .expect("the setting reads")
}

/// The Short name field of the row whose character cell shows `anchor`: the text input drawn
/// in the same table row.
fn short_field<'a>(harness: &'a Harness<'a, App>, anchor: &str) -> egui_kittest::Node<'a> {
    let y = row_y(harness, anchor);
    harness
        .query_all(by().role(Role::TextInput))
        .find(|node| (node.rect().center().y - y).abs() < 12.0)
        .unwrap_or_else(|| panic!("no Short name field on the {anchor} row"))
}

/// Types into the Short name field of the row whose character cell shows `anchor`, selecting
/// what the field holds first.
fn type_short(harness: &mut Harness<'_, App>, anchor: &str, text: &str) {
    short_field(harness, anchor).hover();
    harness.run_steps(2);
    short_field(harness, anchor).click();
    harness.run_steps(2);
    harness.key_press_modifiers(Modifiers::COMMAND, Key::A);
    short_field(harness, anchor).type_text(text);
    harness.run_steps(3);
}

/// Empties the Short name field of the row whose character cell shows `anchor`.
fn clear_short(harness: &mut Harness<'_, App>, anchor: &str) {
    short_field(harness, anchor).hover();
    harness.run_steps(2);
    short_field(harness, anchor).click();
    harness.run_steps(2);
    harness.key_press_modifiers(Modifiers::COMMAND, Key::A);
    harness.key_press(Key::Backspace);
    harness.run_steps(3);
}

#[test]
fn a_short_name_is_saved_trimmed_shown_on_hover_and_cleared() {
    let env = env();
    let eve = FakeEve::new(&env, all_not_yet);
    let mut harness = open(&env, &eve);
    clear_notices(&mut harness);
    let ctx = harness.ctx.clone();

    // A short name is trimmed, saved under the character's key and drawn in its place.
    {
        let cx = harness.state_mut().shared_mut();
        assert!(short_names::set(&ctx, cx, MAIN, "  Indy 2  "));
        assert_eq!(
            cx.store
                .setting("character.short.2112000001")
                .expect("the setting reads")
                .as_deref(),
            Some("Indy 2")
        );
        assert_eq!(short_names::current(&ctx, cx, MAIN, "Main"), "Indy 2");
    }
    until(&mut harness, "the short name on Main's row", |h| {
        shown(h, "Indy 2")
    });
    assert_eq!(hover(&mut harness, "Indy 2"), vec!["Main".to_owned()]);

    // One over the long limit is refused: the saved name stands and the reason is said.
    let long = "1234567890123456789012345";
    assert_eq!(long.chars().count(), 25);
    {
        let cx = harness.state_mut().shared_mut();
        assert!(!short_names::set(&ctx, cx, MAIN, long));
        assert_eq!(
            cx.store
                .setting("character.short.2112000001")
                .expect("the setting reads")
                .as_deref(),
            Some("Indy 2")
        );
        assert_eq!(short_names::current(&ctx, cx, MAIN, "Main"), "Indy 2");
    }
    assert_eq!(
        notices(&mut harness),
        vec!["A short name can have at most 24 characters.".to_owned()]
    );
    clear_notices(&mut harness);

    // An empty one clears the key: the row draws the full name again, with nothing on hover.
    harness.remove_cursor();
    {
        let cx = harness.state_mut().shared_mut();
        assert!(short_names::set(&ctx, cx, MAIN, ""));
        assert_eq!(
            cx.store
                .setting("character.short.2112000001")
                .expect("the setting reads"),
            None
        );
        assert_eq!(short_names::current(&ctx, cx, MAIN, "Main"), "Main");
    }
    until(&mut harness, "the full name on Main's row", |h| {
        shown(h, "Main")
    });
    assert!(hover(&mut harness, "Main").is_empty());
}

#[test]
fn typing_a_short_name_saves_it_and_clearing_the_field_removes_it() {
    let env = env();
    let eve = FakeEve::new(&env, all_not_yet);
    // The player types the name into Main's Short name field and presses Enter.
    {
        let mut harness = open(&env, &eve);
        choose_level(&mut harness, "Standard");
        type_short(&mut harness, "Main", "Indy 2");
        harness.key_press(Key::Enter);
        until(&mut harness, "the row redrawn", |h| shown(h, "Indy 2"));
        assert_eq!(main_short_name(&mut harness).as_deref(), Some("Indy 2"));
        assert_eq!(hover(&mut harness, "Indy 2"), vec!["Main".to_owned()]);
    }

    // A new app reads the saved name back, once Standard shows the field.
    let mut harness = open(&env, &eve);
    choose_level(&mut harness, "Standard");
    until(&mut harness, "the short name after a restart", |h| {
        shown(h, "Indy 2")
    });

    // Clearing the field and pressing Enter clears the key: the full name is drawn again.
    clear_short(&mut harness, "Indy 2");
    harness.key_press(Key::Enter);
    until(&mut harness, "the full name on the row", |h| {
        shown(h, "Main")
    });
    assert_eq!(main_short_name(&mut harness), None);
    harness.remove_cursor();
    assert!(hover(&mut harness, "Main").is_empty());
}

#[test]
fn every_table_draws_the_short_name_with_the_full_name_on_hover() {
    let env = env();
    let jeve = TempDir::new().expect("a temp folder");
    fill_main_jeve(jeve.path());
    let mut eve = FakeEve::new(&env, tables);
    eve.jeve = Some(jeve.path().to_path_buf());
    let mut harness = open(&env, &eve);
    let ctx = harness.ctx.clone();
    clear_notices(&mut harness);
    {
        let cx = harness.state_mut().shared_mut();
        assert!(short_names::set(&ctx, cx, MAIN, "Indy 2"));
    }

    // The Characters page: the login table's row and the jEve table's row.
    until(&mut harness, "the short name in both tables", |h| {
        count(h, "Indy 2") >= 2
    });
    assert_eq!(hover(&mut harness, "Indy 2"), vec!["Main".to_owned()]);

    rail(&mut harness, "Orders");
    until(&mut harness, "Main's order row", |h| shown(h, "Indy 2"));
    assert_eq!(hover(&mut harness, "Indy 2"), vec!["Main".to_owned()]);

    rail(&mut harness, "Industry jobs");
    until(&mut harness, "Main's job row", |h| shown(h, "Indy 2"));
    assert_eq!(hover(&mut harness, "Indy 2"), vec!["Main".to_owned()]);

    rail(&mut harness, "Assets");
    until(&mut harness, "Main's asset row", |h| shown(h, "Indy 2"));
    assert_eq!(hover(&mut harness, "Indy 2"), vec!["Main".to_owned()]);

    // The Item panel's two tables: who holds the item, and the job making it.
    rail(&mut harness, "Build");
    harness.state_mut().shared_mut().selection.item = Some(PAU);
    frames(&mut harness);
    until(&mut harness, "the Owned and Running jobs rows", |h| {
        count(h, "Indy 2") >= 2
    });
    assert_eq!(hover(&mut harness, "Indy 2"), vec!["Main".to_owned()]);

    // Without a saved key every table draws the full name again, with nothing on hover.
    harness.remove_cursor();
    {
        let cx = harness.state_mut().shared_mut();
        assert!(short_names::set(&ctx, cx, MAIN, ""));
    }
    frames(&mut harness);
    rail(&mut harness, "Orders");
    until(&mut harness, "the full name on the order row", |h| {
        shown(h, "Main")
    });
    assert!(hover(&mut harness, "Main").is_empty());
}

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use profit_core::stock::{Asset, CharacterAssets, Places, StockScope, resolve};
use profit_jeve::{
    BlueprintKind, JeveCharacter, JeveData, JeveError, OwnedBlueprint, ReadOptions, read_profile,
};
use rusqlite::Connection;

#[cfg(test)]
mod tests {
    use super::*;

    const OWNERS_TABLE: &str = "CREATE TABLE IF NOT EXISTS owners (accountid TEXT, ownerid INTEGER, \
    name TEXT, corp TEXT, show NUMERIC, invalid NUMERIC, assetslastupdate INTEGER, \
    assetsnextupdate INTEGER, balancelastupdate INTEGER, balancenextupdate INTEGER, \
    marketordersnextupdate INTEGER, journalnextupdate INTEGER, transactionsnextupdate INTEGER, \
    industryjobsnextupdate INTEGER, contractsnextupdate INTEGER, locationsnextupdate INTEGER, \
    blueprintsnextupdate INTEGER, skillsnextupdate INTEGER, miningnextupdate INTEGER, \
    accountname TEXT, refreshtoken TEXT, scopes TEXT, structuresnextupdate INTEGER, \
    accountnextupdate INTEGER, loyaltypointsnextupdate INTEGER, npcstandingnextupdate INTEGER, \
    callbackurl TEXT, characterroles TEXT, UNIQUE(accountid));";

    const ASSETS_TABLE: &str = "CREATE TABLE IF NOT EXISTS assets (accountid TEXT, count INTEGER, \
    flagid INTEGER, flagstring TEXT, itemid INTEGER, typeid INTEGER, locationid INTEGER, \
    singleton NUMERIC, rawquantity INTEGER, UNIQUE(accountid, itemid));";

    const ASSETS_TABLE_WITHOUT_RAW: &str = "CREATE TABLE IF NOT EXISTS assets (accountid TEXT, \
    count INTEGER, flagid INTEGER, flagstring TEXT, itemid INTEGER, typeid INTEGER, \
    locationid INTEGER, singleton NUMERIC, UNIQUE(accountid, itemid));";

    const BLUEPRINTS_TABLE: &str = "CREATE TABLE IF NOT EXISTS blueprints (accountid TEXT, \
    itemid INTEGER, locationid INTEGER, typeid INTEGER, flagid INTEGER, flagstring TEXT, \
    quantity INTEGER, timeefficiency INTEGER, materialefficiency INTEGER, runs NUMERIC, \
    UNIQUE(accountid, itemid));";

    const ROWS: &str = "
INSERT INTO owners (accountid, ownerid, name, show, invalid, assetslastupdate, scopes, refreshtoken, callbackurl) VALUES
 ('a1', 90000001, 'Ana Ward', 1, 1, 1760651234567, 'esi-assets.read_assets.v1 esi-characters.read_blueprints.v1', 'SECRET-TOKEN-1', NULL),
 ('a2', 90000002, 'Bo Lin', 0, 0, NULL, 'esi-assets.read_assets.v1', 'SECRET-TOKEN-2', NULL),
 ('c1', 98000001, 'Lin Industries', 1, 0, 1760000000000, 'esi-assets.read_corporation_assets.v1 esi-characters.read_corporation_roles.v1', 'SECRET-TOKEN-3', 'SECRET-URL');
INSERT INTO assets (accountid, itemid, typeid, locationid, flagid, flagstring, count, singleton, rawquantity) VALUES
 ('a1', 1001, 34, 60003760, 4, 'Hangar', 1000, 0, NULL),
 ('a1', 1002, 587, 60003760, 4, 'Hangar', 1, 1, -1),
 ('a1', 1003, 2455, 60003760, 4, 'Hangar', 1, 0, -2),
 ('a1', 1004, 35, 1002, 5, 'Cargo', 300, 0, NULL),
 ('a1', 1005, 185, 1002, 27, 'HiSlot0', 100, 0, NULL),
 ('a1', 1006, 34, 30000142, 0, NULL, 50, 0, NULL),
 ('a2', 2001, 34, 60003760, 4, 'Hangar', 2000, 0, NULL),
 ('c1', 3001, 34, 60003760, 116, 'CorpSAG1', 999, 0, NULL),
 ('gone', 4001, 34, 60003760, 4, 'Hangar', 5, 0, NULL);
INSERT INTO blueprints (accountid, itemid, locationid, typeid, flagid, flagstring, quantity, timeefficiency, materialefficiency, runs) VALUES
 ('a1', 1003, 60003760, 2455, 4, 'Hangar', -2, 20, 10, 7),
 ('a1', 1007, 60003760, 2455, 4, 'Hangar', -1, 14, 9, -1),
 ('a2', 2002, 60003760, 691, 4, 'Hangar', 3, 0, 0, -1),
 ('c1', 3002, 60003760, 691, 116, 'CorpSAG1', -1, 0, 0, -1);
";

    const SECRETS: [&str; 5] = [
        "SECRET-TOKEN-1",
        "SECRET-TOKEN-2",
        "SECRET-TOKEN-3",
        "SECRET-URL",
        "esi-",
    ];

    /// A profile folder with `#Default.db` built from `sql`. The folder lives as long as this does.
    struct Fixture {
        _root: tempfile::TempDir,
        dir: PathBuf,
        path: PathBuf,
    }

    fn profile_with(sql: &str) -> Fixture {
        let root = tempfile::tempdir().expect("temp dir");
        let dir = root.path().join("profiles");
        fs::create_dir(&dir).expect("create profiles folder");
        let path = dir.join("#Default.db");
        let connection = Connection::open(&path).expect("create database");
        connection.execute_batch(sql).expect("build fixture");
        drop(connection);
        Fixture {
            _root: root,
            dir,
            path,
        }
    }

    /// The full fixture, then `extra` SQL applied on top of it.
    fn fixture(extra: &str) -> Fixture {
        profile_with(&format!(
            "{OWNERS_TABLE}{ASSETS_TABLE}{BLUEPRINTS_TABLE}{ROWS}{extra}"
        ))
    }

    fn read(fixture: &Fixture) -> JeveData {
        read_profile(&fixture.path, &ReadOptions::default()).expect("profile reads")
    }

    fn read_err(extra: &str) -> JeveError {
        let fixture = fixture(extra);
        read_profile(&fixture.path, &ReadOptions::default()).expect_err("profile is refused")
    }

    fn bad_value(error: &JeveError) -> Option<(&'static str, &'static str, i64)> {
        match error {
            JeveError::BadValue {
                table,
                column,
                item_id,
            } => Some((table, column, *item_id)),
            JeveError::Io { .. }
            | JeveError::NotJeve { .. }
            | JeveError::Busy
            | JeveError::Layout { .. }
            | JeveError::TooManyRows { .. }
            | JeveError::TrackerBadData { .. }
            | JeveError::TrackerNotJson { .. }
            | JeveError::Sqlite(_) => None,
        }
    }

    fn character(data: &JeveData, character_id: i64) -> &JeveCharacter {
        data.characters
            .iter()
            .find(|character| character.character_id == character_id)
            .expect("character is listed")
    }

    fn asset(
        item_id: i64,
        type_id: u32,
        location_id: i64,
        flag: Option<&str>,
        quantity: u64,
        singleton: bool,
    ) -> Asset {
        Asset {
            item_id,
            type_id,
            location_id,
            flag: flag.map(str::to_owned),
            quantity,
            singleton,
        }
    }

    #[test]
    fn characters_come_in_order_with_their_flags_and_times() {
        let data = read(&fixture(""));

        let ids: Vec<i64> = data.characters.iter().map(|c| c.character_id).collect();
        assert_eq!(ids, vec![90000001, 90000002]);

        let ana = &data.characters[0];
        assert_eq!(ana.name, "Ana Ward");
        assert!(ana.shown);
        assert!(ana.login_expired);
        assert_eq!(ana.assets_updated, Some(1_760_651_234));

        let bo = &data.characters[1];
        assert_eq!(bo.name, "Bo Lin");
        assert!(!bo.shown);
        assert!(!bo.login_expired);
        assert_eq!(bo.assets_updated, None);
    }

    #[test]
    fn ana_assets_are_exact() {
        let data = read(&fixture(""));

        assert_eq!(
            character(&data, 90000001).assets,
            CharacterAssets {
                character_id: 90000001,
                updated: Some(1_760_651_234),
                assets: vec![
                    asset(1001, 34, 60_003_760, Some("Hangar"), 1000, false),
                    asset(1002, 587, 60_003_760, Some("Hangar"), 1, true),
                    asset(1003, 2455, 60_003_760, Some("Hangar"), 1, true),
                    asset(1004, 35, 1002, Some("Cargo"), 300, false),
                    asset(1005, 185, 1002, Some("HiSlot0"), 100, false),
                    asset(1006, 34, 30_000_142, None, 50, false),
                ],
            }
        );
    }

    #[test]
    fn bo_assets_have_no_update_time() {
        let data = read(&fixture(""));

        assert_eq!(
            character(&data, 90000002).assets,
            CharacterAssets {
                character_id: 90000002,
                updated: None,
                assets: vec![asset(2001, 34, 60_003_760, Some("Hangar"), 2000, false)],
            }
        );
    }

    #[test]
    fn blueprints_are_read_as_originals_and_copies() {
        let data = read(&fixture(""));

        assert_eq!(
            character(&data, 90000001).blueprints,
            vec![
                OwnedBlueprint {
                    item_id: 1003,
                    type_id: 2455,
                    location_id: 60_003_760,
                    flag: Some("Hangar".to_owned()),
                    kind: BlueprintKind::Copy { runs: 7 },
                    stack: 1,
                    me: 10,
                    te: 20,
                },
                OwnedBlueprint {
                    item_id: 1007,
                    type_id: 2455,
                    location_id: 60_003_760,
                    flag: Some("Hangar".to_owned()),
                    kind: BlueprintKind::Original,
                    stack: 1,
                    me: 9,
                    te: 14,
                },
            ]
        );
        assert_eq!(
            character(&data, 90000002).blueprints,
            vec![OwnedBlueprint {
                item_id: 2002,
                type_id: 691,
                location_id: 60_003_760,
                flag: Some("Hangar".to_owned()),
                kind: BlueprintKind::Original,
                stack: 3,
                me: 0,
                te: 0,
            }]
        );
    }

    #[test]
    fn corporations_and_orphan_rows_are_counted_not_listed() {
        let data = read(&fixture(""));

        assert_eq!(data.skipped_corporations, 1);
        assert_eq!(data.orphan_rows, 1);
        assert_eq!(data.characters.len(), 2);
        assert!(data.characters.iter().all(|c| c.character_id != 98_000_001));
    }

    #[test]
    fn the_result_holds_no_secret() {
        let data = read(&fixture(""));

        let shown = format!("{data:?}");
        for secret in SECRETS {
            assert!(!shown.contains(secret), "result contains {secret:?}");
        }
    }

    #[test]
    fn stock_counts_match_the_stock_module() {
        let data = read(&fixture(""));
        let characters: Vec<CharacterAssets> = data
            .characters
            .iter()
            .map(|character| character.assets.clone())
            .collect();

        let owned = resolve(&characters)
            .owned(&StockScope {
                characters: [90_000_001, 90_000_002].into_iter().collect(),
                places: Places::All,
            })
            .expect("no overflow");

        assert_eq!(owned, BTreeMap::from([(34, 3_050), (35, 300)]));
    }

    #[test]
    fn reading_changes_neither_the_file_nor_the_folder() {
        let fixture = fixture("");
        let bytes = fs::read(&fixture.path).expect("read bytes");
        let modified = fs::metadata(&fixture.path)
            .and_then(|metadata| metadata.modified())
            .expect("modified time");
        let names = folder_names(&fixture.dir);

        read(&fixture);

        assert_eq!(fs::read(&fixture.path).expect("read bytes"), bytes);
        assert_eq!(
            fs::metadata(&fixture.path)
                .and_then(|metadata| metadata.modified())
                .expect("modified time"),
            modified
        );
        assert_eq!(folder_names(&fixture.dir), names);
    }

    fn folder_names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .expect("read folder")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        names.sort();
        names
    }

    #[test]
    fn update_times_round_down_to_whole_seconds() {
        let updated = |milliseconds: &str| {
            let fixture = fixture(&format!(
                "UPDATE owners SET assetslastupdate = {milliseconds} WHERE accountid = 'a1';"
            ));
            character(&read(&fixture), 90000001).assets_updated
        };

        assert_eq!(updated("999"), Some(0));
        assert_eq!(updated("-1"), Some(-1));
        assert_eq!(updated("1760651234999"), Some(1_760_651_234));
    }

    #[test]
    fn a_missing_column_names_the_first_one_missing() {
        let fixture = profile_with(&format!(
            "{OWNERS_TABLE}{ASSETS_TABLE_WITHOUT_RAW}{BLUEPRINTS_TABLE}"
        ));

        let error = read_profile(&fixture.path, &ReadOptions::default()).expect_err("layout");

        assert!(matches!(
            error,
            JeveError::Layout {
                table: "assets",
                column: Some("rawquantity"),
            }
        ));
    }

    #[test]
    fn a_missing_table_is_a_layout_error() {
        let fixture = profile_with(&format!("{OWNERS_TABLE}{ASSETS_TABLE}"));

        let error = read_profile(&fixture.path, &ReadOptions::default()).expect_err("layout");

        assert!(matches!(
            error,
            JeveError::Layout {
                table: "blueprints",
                column: None,
            }
        ));
    }

    #[test]
    fn an_empty_file_is_a_layout_error() {
        let root = tempfile::tempdir().expect("temp dir");
        let path = root.path().join("#Default.db");
        fs::write(&path, b"").expect("write empty file");

        let error = read_profile(&path, &ReadOptions::default()).expect_err("layout");

        assert!(matches!(
            error,
            JeveError::Layout {
                table: "owners",
                column: None,
            }
        ));
    }

    #[test]
    fn bad_values_are_refused_with_their_row() {
        let cases = [
            (
                "UPDATE assets SET typeid = -5 WHERE itemid = 1001;",
                ("assets", "typeid", 1001),
            ),
            (
                "UPDATE assets SET count = -1 WHERE itemid = 1001;",
                ("assets", "count", 1001),
            ),
            (
                "UPDATE assets SET locationid = NULL WHERE itemid = 1001;",
                ("assets", "locationid", 1001),
            ),
            (
                "UPDATE blueprints SET materialefficiency = 11 WHERE itemid = 1007;",
                ("blueprints", "materialefficiency", 1007),
            ),
            (
                "UPDATE blueprints SET timeefficiency = 21 WHERE itemid = 1007;",
                ("blueprints", "timeefficiency", 1007),
            ),
            (
                "UPDATE blueprints SET runs = 0 WHERE itemid = 1003;",
                ("blueprints", "runs", 1003),
            ),
            (
                "UPDATE blueprints SET runs = NULL WHERE itemid = 1007;",
                ("blueprints", "runs", 1007),
            ),
            (
                "UPDATE blueprints SET quantity = 0 WHERE itemid = 2002;",
                ("blueprints", "quantity", 2002),
            ),
            (
                "UPDATE blueprints SET quantity = -3 WHERE itemid = 2002;",
                ("blueprints", "quantity", 2002),
            ),
            (
                "UPDATE blueprints SET quantity = 4294967296 WHERE itemid = 2002;",
                ("blueprints", "quantity", 2002),
            ),
            (
                "UPDATE owners SET name = NULL WHERE accountid = 'a1';",
                ("owners", "name", 90_000_001),
            ),
        ];
        for (extra, expected) in cases {
            let error = read_err(extra);
            assert_eq!(bad_value(&error), Some(expected), "{extra}");
        }
    }

    #[test]
    fn rows_without_an_id_are_skipped_and_counted() {
        let data = read(&fixture(
            "UPDATE assets SET itemid = NULL WHERE itemid = 1006;",
        ));
        assert_eq!(data.unnamed_rows, 1);
        assert_eq!(character(&data, 90000001).assets.assets.len(), 5);

        let data = read(&fixture(
            "UPDATE owners SET ownerid = NULL WHERE accountid = 'a2';",
        ));
        assert_eq!(data.unnamed_rows, 1);
        assert_eq!(data.characters.len(), 1);
        // Bo's asset 2001 and blueprint 2002 now belong to no listed owner.
        assert_eq!(data.orphan_rows, 3);
    }

    #[test]
    fn a_missing_file_is_an_io_error_with_its_path() {
        let root = tempfile::tempdir().expect("temp dir");
        let path = root.path().join("#Default.db");

        let error = read_profile(&path, &ReadOptions::default()).expect_err("missing");

        assert!(matches!(error, JeveError::Io { path: found, .. } if found == path));
    }

    #[test]
    fn a_text_file_is_not_a_profile() {
        let root = tempfile::tempdir().expect("temp dir");
        let path = root.path().join("#Default.db");
        fs::write(&path, b"hello").expect("write text");

        let error = read_profile(&path, &ReadOptions::default()).expect_err("not a database");

        assert!(matches!(error, JeveError::NotJeve { path: found } if found == path));
    }

    #[test]
    fn a_locked_profile_is_busy_and_the_wait_is_short() {
        let fixture = fixture("");
        let locker = Connection::open(&fixture.path).expect("open locker");
        locker
            .execute_batch("BEGIN EXCLUSIVE;")
            .expect("take the lock");
        let options = ReadOptions {
            busy_timeout: Duration::from_millis(50),
        };

        let started = Instant::now();
        let error = read_profile(&fixture.path, &options).expect_err("locked");

        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(matches!(error, JeveError::Busy));
        locker.execute_batch("COMMIT;").expect("release the lock");
    }

    #[test]
    fn an_empty_asset_flag_and_raw_quantity_are_allowed() {
        let data = read(&fixture(""));

        let unflagged = character(&data, 90000001)
            .assets
            .assets
            .iter()
            .find(|asset| asset.item_id == 1006)
            .expect("row 1006 is read");
        assert_eq!(unflagged.flag, None);
    }
}

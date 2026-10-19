//! Acceptance tests for the import and the queries, on the build 3607705 fixture.

#[cfg(test)]
mod tests {
    use std::error::Error as _;
    use std::fs::{self, File};
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    use profit_core::industry::{Activity, Recipe};
    use profit_core::volume::Volume;
    use profit_sde::schema::SCHEMA_VERSION;
    use profit_sde::{ImportReport, Sde, SdeError, import};
    use rusqlite::Connection;
    use serde_json::{Value, json};
    use tempfile::TempDir;

    fn fixture() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sde-3607705")
    }

    /// A fresh copy of the fixture directory, which a test may change.
    fn fixture_copy(dir: &TempDir) -> PathBuf {
        let copy = dir.path().join("source");
        fs::create_dir(&copy).unwrap();
        for entry in fs::read_dir(fixture()).unwrap() {
            let entry = entry.unwrap();
            fs::copy(entry.path(), copy.join(entry.file_name())).unwrap();
        }
        copy
    }

    /// A zip with the fixture files at its root.
    fn fixture_zip(dir: &TempDir) -> PathBuf {
        let path = dir.path().join("sde.zip");
        let mut writer = zip::ZipWriter::new(File::create(&path).unwrap());
        for entry in fs::read_dir(fixture()).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name().to_string_lossy().into_owned();
            writer
                .start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(&fs::read(entry.path()).unwrap()).unwrap();
        }
        writer.finish().unwrap();
        path
    }

    /// Replaces one line of a copied `.jsonl` file. Lines count from 1.
    fn set_line(file: &Path, line: usize, text: &str) {
        let content = fs::read_to_string(file).unwrap();
        let mut lines: Vec<&str> = content.lines().collect();
        lines[line - 1] = text;
        fs::write(file, lines.join("\n") + "\n").unwrap();
    }

    /// Changes one line of a copied `.jsonl` file as JSON. Lines count from 1.
    fn edit_line(file: &Path, line: usize, edit: impl FnOnce(&mut Value)) {
        let content = fs::read_to_string(file).unwrap();
        let mut value: Value =
            serde_json::from_str(content.lines().nth(line - 1).unwrap()).unwrap();
        edit(&mut value);
        set_line(file, line, &value.to_string());
    }

    fn expected_report() -> ImportReport {
        ImportReport {
            build_number: 3_607_705,
            release_date: "2026-10-12T11:08:38Z".to_owned(),
            types: 76,
            groups: 27,
            categories: 9,
            recipes: 12,
            material_rows: 54,
            structures: 5,
            rigs: 6,
            skipped_unpublished: 1,
            duplicates: 0,
            systems: 88,
            stargates: 224,
            regions: 1,
        }
    }

    /// The fixture imported into a temporary database. Keep the directory alive while using the data.
    fn imported() -> (TempDir, Sde) {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("sde.db");
        import(&fixture(), &db).unwrap();
        let sde = Sde::open(&db).unwrap();
        (dir, sde)
    }

    fn expect_error(result: Result<ImportReport, SdeError>) -> SdeError {
        result.unwrap_err()
    }

    #[test]
    fn fixture_directory_imports_with_the_expected_report() {
        let dir = TempDir::new().unwrap();
        let report = import(&fixture(), &dir.path().join("sde.db")).unwrap();
        assert_eq!(report, expected_report());
    }

    #[test]
    fn a_zip_of_the_fixture_gives_the_same_report_and_answers() {
        let dir = TempDir::new().unwrap();
        let from_dir = dir.path().join("dir.db");
        let from_zip = dir.path().join("zip.db");
        let report_dir = import(&fixture(), &from_dir).unwrap();
        let report_zip = import(&fixture_zip(&dir), &from_zip).unwrap();
        assert_eq!(report_dir, report_zip);

        let by_dir = Sde::open(&from_dir).unwrap();
        let by_zip = Sde::open(&from_zip).unwrap();
        check_all(&by_dir);
        check_all(&by_zip);
        assert_eq!(by_dir.recipe_book().unwrap(), by_zip.recipe_book().unwrap());
    }

    #[test]
    fn a_type_in_an_unlisted_group_is_refused_with_its_line() {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("sde.db");
        let source = fixture_copy(&dir);
        edit_line(&source.join("types.jsonl"), 2, |value| {
            value["groupID"] = json!(999_999);
        });
        let error = expect_error(import(&source, &db));
        let SdeError::UnknownGroup {
            file,
            line,
            group_id,
        } = error
        else {
            panic!("expected an unknown group error, got {error:?}");
        };
        assert_eq!((file.as_str(), line, group_id), ("types.jsonl", 2, 999_999));
        assert!(!db.exists());
    }

    /// Every query the fixture answers, with the values the data must give.
    fn check_all(sde: &Sde) {
        check_build_reports_the_number_and_release_date(sde);
        check_item_587_is_the_rifter_and_haul_uses_the_packaged_volume(sde);
        check_item_16670_has_a_volume_of_a_hundredth(sde);
        check_unpublished_item_keeps_its_name_and_has_no_volume(sde);
        check_unknown_item_is_none(sde);
        check_find_exact_ignores_case_and_surrounding_spaces(sde);
        check_find_exact_needs_the_whole_name_and_a_non_empty_one(sde);
        check_search_ranks_whole_names_then_prefixes_then_words(sde);
        check_search_trims_and_ignores_case_and_limits_after_ranking(sde);
        check_search_finds_nothing_for_an_empty_or_absent_query(sde);
        check_recipe_book_holds_every_recipe(sde);
    }

    fn check_build_reports_the_number_and_release_date(sde: &Sde) {
        let build = sde.build().unwrap();
        assert_eq!(build.number, 3_607_705);
        assert_eq!(build.release_date, "2026-10-12T11:08:38Z");
    }

    #[test]
    fn build_reports_the_number_and_release_date() {
        let (_dir, sde) = imported();
        check_build_reports_the_number_and_release_date(&sde);
    }

    fn check_item_587_is_the_rifter_and_haul_uses_the_packaged_volume(sde: &Sde) {
        let item = sde.item(587).unwrap().unwrap();
        assert_eq!(item.name, "Rifter");
        assert_eq!(item.group_id, 25);
        assert_eq!(item.category_id, 6);
        assert!(item.published);
        assert_eq!(item.volume, Some(Volume::from_ten_thousandths(272_890_000)));
        assert_eq!(
            item.packaged_volume,
            Some(Volume::from_ten_thousandths(25_000_000))
        );
        assert_eq!(item.haul_volume(), item.packaged_volume);
    }

    #[test]
    fn item_587_is_the_rifter_and_haul_uses_the_packaged_volume() {
        let (_dir, sde) = imported();
        check_item_587_is_the_rifter_and_haul_uses_the_packaged_volume(&sde);
    }

    fn check_item_16670_has_a_volume_of_a_hundredth(sde: &Sde) {
        let item = sde.item(16670).unwrap().unwrap();
        assert_eq!(item.volume, Some(Volume::from_ten_thousandths(100)));
    }

    #[test]
    fn item_16670_has_a_volume_of_a_hundredth() {
        let (_dir, sde) = imported();
        check_item_16670_has_a_volume_of_a_hundredth(&sde);
    }

    fn check_unpublished_item_keeps_its_name_and_has_no_volume(sde: &Sde) {
        let item = sde.item(58735).unwrap().unwrap();
        assert_eq!(item.name, "Azbel");
        assert!(!item.published);
        assert_eq!(item.volume, None);
        assert_eq!(item.packaged_volume, None);
        assert_eq!(item.haul_volume(), None);
    }

    #[test]
    fn unpublished_item_keeps_its_name_and_has_no_volume() {
        let (_dir, sde) = imported();
        check_unpublished_item_keeps_its_name_and_has_no_volume(&sde);
    }

    fn check_unknown_item_is_none(sde: &Sde) {
        assert_eq!(sde.item(1).unwrap(), None);
    }

    #[test]
    fn unknown_item_is_none() {
        let (_dir, sde) = imported();
        check_unknown_item_is_none(&sde);
    }

    fn check_find_exact_ignores_case_and_surrounding_spaces(sde: &Sde) {
        assert_eq!(sde.find_exact("azbel").unwrap(), Some(35826));
        assert_eq!(sde.find_exact("  HOBGOBLIN II ").unwrap(), Some(2456));
    }

    #[test]
    fn find_exact_ignores_case_and_surrounding_spaces() {
        let (_dir, sde) = imported();
        check_find_exact_ignores_case_and_surrounding_spaces(&sde);
    }

    fn check_find_exact_needs_the_whole_name_and_a_non_empty_one(sde: &Sde) {
        assert_eq!(sde.find_exact("Hobgoblin").unwrap(), None);
        assert_eq!(sde.find_exact("").unwrap(), None);
    }

    #[test]
    fn find_exact_needs_the_whole_name_and_a_non_empty_one() {
        let (_dir, sde) = imported();
        check_find_exact_needs_the_whole_name_and_a_non_empty_one(&sde);
    }

    fn check_search_ranks_whole_names_then_prefixes_then_words(sde: &Sde) {
        assert_eq!(
            sde.search("hobgoblin", 10).unwrap(),
            vec![2454, 2456, 2455, 2457]
        );
        assert_eq!(
            sde.search("robotics", 10).unwrap(),
            vec![9848, 11481, 11887]
        );
        assert_eq!(
            sde.search("carb", 10).unwrap(),
            vec![16659, 46167, 16672, 16670, 46207, 46205, 16633]
        );
        assert_eq!(sde.search("azbel", 10).unwrap(), vec![35826]);
        assert_eq!(
            sde.search("ite", 10).unwrap(),
            vec![35, 11399, 16655, 46169, 46486, 46487]
        );
    }

    #[test]
    fn search_ranks_whole_names_then_prefixes_then_words() {
        let (_dir, sde) = imported();
        check_search_ranks_whole_names_then_prefixes_then_words(&sde);
    }

    fn check_search_trims_and_ignores_case_and_limits_after_ranking(sde: &Sde) {
        assert_eq!(
            sde.search("  HOBGOBLIN II  ", 10).unwrap(),
            vec![2456, 2457]
        );
        assert_eq!(sde.search("hobgoblin", 2).unwrap(), vec![2454, 2456]);
    }

    #[test]
    fn search_trims_and_ignores_case_and_limits_after_ranking() {
        let (_dir, sde) = imported();
        check_search_trims_and_ignores_case_and_limits_after_ranking(&sde);
    }

    fn check_search_finds_nothing_for_an_empty_or_absent_query(sde: &Sde) {
        assert!(sde.search("", 10).unwrap().is_empty());
        assert!(sde.search("zzz", 10).unwrap().is_empty());
    }

    #[test]
    fn search_finds_nothing_for_an_empty_or_absent_query() {
        let (_dir, sde) = imported();
        check_search_finds_nothing_for_an_empty_or_absent_query(&sde);
    }

    fn check_recipe_book_holds_every_recipe(sde: &Sde) {
        let book = sde.recipe_book().unwrap();
        assert_eq!(book.len(), 12);

        assert_eq!(
            book.get(2456),
            Some(&Recipe {
                activity: Activity::Manufacturing,
                output_per_run: 1,
                max_runs_per_job: 10,
                materials: vec![
                    (11399, 1),
                    (11688, 1),
                    (9834, 1),
                    (2454, 1),
                    (9848, 1),
                    (11481, 1),
                ],
                made_by_invention: true,
            })
        );
        assert_eq!(
            book.get(16670),
            Some(&Recipe {
                activity: Activity::Reaction,
                output_per_run: 10_000,
                max_runs_per_job: 1000,
                materials: vec![(4247, 5), (16655, 100), (16659, 100)],
                made_by_invention: false,
            })
        );
        let carbide = book.get(16672).unwrap();
        assert_eq!(carbide.activity, Activity::Reaction);
        assert_eq!(carbide.output_per_run, 10_000);
        assert_eq!(carbide.max_runs_per_job, 1000);
        assert_eq!(
            carbide.materials,
            vec![(4051, 5), (16657, 100), (16661, 100)]
        );
        assert_eq!(book.get(4247).map(|r| r.output_per_run), Some(40));
        assert_eq!(book.get(4247).map(|r| r.max_runs_per_job), Some(200));
        assert_eq!(book.get(11481).map(|r| r.output_per_run), Some(100));
        assert_eq!(book.get(11481).map(|r| r.max_runs_per_job), Some(200));
    }

    #[test]
    fn recipe_book_holds_every_recipe() {
        let (_dir, sde) = imported();
        check_recipe_book_holds_every_recipe(&sde);
    }

    #[test]
    fn a_failed_import_keeps_the_old_database() {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("sde.db");
        import(&fixture(), &db).unwrap();

        let source = fixture_copy(&dir);
        set_line(&source.join("types.jsonl"), 3, r#"{"_key": 36,"#);
        let error = expect_error(import(&source, &db));
        let SdeError::Json { file, line, .. } = error else {
            panic!("expected a JSON error, got {error:?}");
        };
        assert_eq!((file.as_str(), line), ("types.jsonl", 3));

        let sde = Sde::open(&db).unwrap();
        assert_eq!(sde.build().unwrap().number, 3_607_705);
        assert_eq!(sde.item(587).unwrap().unwrap().name, "Rifter");
        assert!(!dir.path().join("sde.db.new").exists());
    }

    #[test]
    fn a_type_without_english_name_is_refused_with_its_line() {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("sde.db");
        let source = fixture_copy(&dir);
        edit_line(&source.join("types.jsonl"), 1, |value| {
            value["name"].as_object_mut().unwrap().remove("en");
        });
        let error = expect_error(import(&source, &db));
        let SdeError::MissingEnglishName { file, line } = error else {
            panic!("expected a missing name error, got {error:?}");
        };
        assert_eq!((file.as_str(), line), ("types.jsonl", 1));
        assert!(!dir.path().join("sde.db.new").exists());
        assert!(!db.exists());
    }

    #[test]
    fn a_source_without_blueprints_is_refused() {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("sde.db");
        let source = fixture_copy(&dir);
        fs::remove_file(source.join("blueprints.jsonl")).unwrap();
        let error = expect_error(import(&source, &db));
        let SdeError::MissingFile(name) = error else {
            panic!("expected a missing file error, got {error:?}");
        };
        assert_eq!(name, "blueprints.jsonl");
        assert!(!dir.path().join("sde.db.new").exists());
    }

    #[test]
    fn a_volume_with_more_than_four_decimals_is_refused() {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("sde.db");
        let source = fixture_copy(&dir);
        edit_line(&source.join("types.jsonl"), 1, |value| {
            value["volume"] = serde_json::from_str("0.00025").unwrap();
        });
        let error = expect_error(import(&source, &db));
        let SdeError::BadNumber {
            file,
            line,
            field,
            text,
        } = error
        else {
            panic!("expected a bad number error, got {error:?}");
        };
        assert_eq!(
            (file.as_str(), line, field.as_str(), text.as_str()),
            ("types.jsonl", 1, "volume", "0.00025")
        );
        assert!(!dir.path().join("sde.db.new").exists());
    }

    #[test]
    fn a_stale_pending_database_is_replaced() {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("sde.db");
        fs::write(dir.path().join("sde.db.new"), b"left over").unwrap();
        import(&fixture(), &db).unwrap();
        assert!(!dir.path().join("sde.db.new").exists());
        assert_eq!(Sde::open(&db).unwrap().build().unwrap().number, 3_607_705);
    }

    #[test]
    fn a_stale_pending_database_is_removed_even_when_the_download_cannot_be_read() {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("sde.db");
        fs::write(dir.path().join("sde.db.new"), b"left over").unwrap();
        let source = fixture_copy(&dir);
        fs::remove_file(source.join("blueprints.jsonl")).unwrap();
        expect_error(import(&source, &db));
        assert!(!dir.path().join("sde.db.new").exists());
    }

    #[test]
    fn a_number_too_large_to_store_is_refused() {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("sde.db");
        let source = fixture_copy(&dir);
        edit_line(&source.join("blueprints.jsonl"), 1, |value| {
            value["activities"]["manufacturing"]["products"][0]["quantity"] = json!(u64::MAX);
        });
        let error = expect_error(import(&source, &db));
        let SdeError::TooLarge { value } = error else {
            panic!("expected a too large error, got {error:?}");
        };
        assert_eq!(value, u64::MAX);
        assert!(!dir.path().join("sde.db.new").exists());
    }

    #[test]
    fn open_refuses_a_missing_database() {
        let dir = TempDir::new().unwrap();
        let error = expect_error_open(&dir.path().join("missing.db"));
        assert!(matches!(error, SdeError::NotImported));
    }

    #[test]
    fn open_refuses_an_empty_file() {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("empty.db");
        fs::write(&db, b"").unwrap();
        let error = expect_error_open(&db);
        assert!(matches!(error, SdeError::NotImported));
    }

    #[test]
    fn open_refuses_another_schema_version() {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("sde.db");
        import(&fixture(), &db).unwrap();
        let conn = Connection::open(&db).unwrap();
        conn.pragma_update(None, "user_version", 99).unwrap();
        drop(conn);
        let error = expect_error_open(&db);
        assert!(matches!(error, SdeError::NotImported));
    }

    #[test]
    fn an_activity_without_materials_is_built_from_nothing() {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("sde.db");
        let source = fixture_copy(&dir);
        edit_line(&source.join("blueprints.jsonl"), 6, |value| {
            value["activities"]["manufacturing"]
                .as_object_mut()
                .unwrap()
                .remove("materials");
        });
        let report = import(&source, &db).unwrap();
        assert_eq!(report.recipes, 12);
        let book = Sde::open(&db).unwrap().recipe_book().unwrap();
        assert_eq!(book.get(4247).unwrap().materials, Vec::new());
    }

    #[test]
    fn a_published_activity_without_products_makes_no_recipe() {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("sde.db");
        let source = fixture_copy(&dir);
        edit_line(&source.join("blueprints.jsonl"), 6, |value| {
            value["activities"]["manufacturing"]
                .as_object_mut()
                .unwrap()
                .remove("products");
        });
        let report = import(&source, &db).unwrap();
        assert_eq!(report.recipes, 11);
        assert_eq!(report.skipped_unpublished, 1);
        let book = Sde::open(&db).unwrap().recipe_book().unwrap();
        assert_eq!(book.get(4247), None);
    }

    #[test]
    fn an_unpublished_activity_without_products_is_not_counted_as_skipped() {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("sde.db");
        let source = fixture_copy(&dir);
        // Line 9 is the unpublished test reaction.
        edit_line(&source.join("blueprints.jsonl"), 9, |value| {
            value["activities"]["reaction"]
                .as_object_mut()
                .unwrap()
                .remove("products");
        });
        let report = import(&source, &db).unwrap();
        assert_eq!(report.recipes, 12);
        assert_eq!(report.skipped_unpublished, 0);
    }

    #[test]
    fn a_recipe_without_a_run_limit_is_refused_with_its_line() {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("sde.db");
        let source = fixture_copy(&dir);
        edit_line(&source.join("blueprints.jsonl"), 6, |value| {
            value.as_object_mut().unwrap().remove("maxProductionLimit");
        });
        let error = expect_error(import(&source, &db));
        let SdeError::MissingField { file, line, field } = error else {
            panic!("expected a missing field error, got {error:?}");
        };
        assert_eq!(
            (file.as_str(), line, field.as_str()),
            ("blueprints.jsonl", 6, "maxProductionLimit")
        );
        assert!(!db.exists());
    }

    /// Adds a published blueprint with the given id that makes Tungsten Carbide (16672) from
    /// one unit of Tritanium, next to the real formula 46207.
    fn add_rival_blueprint(source: &Path, id: u32) {
        let file = source.join("blueprints.jsonl");
        let content = fs::read_to_string(&file).unwrap();
        let mut rival: Value = content
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .find(|value| value["_key"] == 46207)
            .unwrap();
        rival["_key"] = json!(id);
        rival["blueprintTypeID"] = json!(46205);
        rival["activities"]["reaction"]["materials"] = json!([{"typeID": 34, "quantity": 1}]);
        fs::write(&file, format!("{content}{rival}\n")).unwrap();
    }

    #[test]
    fn the_lowest_blueprint_id_wins_for_a_product_and_the_loss_is_counted() {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("sde.db");
        let source = fixture_copy(&dir);
        add_rival_blueprint(&source, 46100);
        let report = import(&source, &db).unwrap();
        assert_eq!(report.duplicates, 1);
        assert_eq!(report.recipes, 12);
        assert_eq!(report.material_rows, 52);
        let book = Sde::open(&db).unwrap().recipe_book().unwrap();
        assert_eq!(book.get(16672).unwrap().materials, vec![(34, 1)]);
    }

    #[test]
    fn a_higher_blueprint_id_loses_for_a_product_and_the_loss_is_counted() {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("sde.db");
        let source = fixture_copy(&dir);
        add_rival_blueprint(&source, 46300);
        let report = import(&source, &db).unwrap();
        assert_eq!(report.duplicates, 1);
        assert_eq!(report.recipes, 12);
        assert_eq!(report.material_rows, 54);
        let book = Sde::open(&db).unwrap().recipe_book().unwrap();
        assert_eq!(
            book.get(16672).unwrap().materials,
            vec![(4051, 5), (16657, 100), (16661, 100)]
        );
    }

    #[test]
    fn a_tie_on_blueprint_id_keeps_the_first_recipe_in_the_file() {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("sde.db");
        let source = fixture_copy(&dir);
        add_rival_blueprint(&source, 46207);
        let report = import(&source, &db).unwrap();
        assert_eq!(report.duplicates, 1);
        let book = Sde::open(&db).unwrap().recipe_book().unwrap();
        assert_eq!(
            book.get(16672).unwrap().materials,
            vec![(4051, 5), (16657, 100), (16661, 100)]
        );
    }

    #[test]
    fn the_database_records_its_version_build_and_import_time() {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("sde.db");
        let before = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        import(&fixture(), &db).unwrap();
        let after = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let conn = Connection::open(&db).unwrap();
        let user_version: u32 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(user_version, SCHEMA_VERSION);
        let (version, build, date, at): (u32, i64, String, i64) = conn
            .query_row(
                "SELECT schema_version, build_number, release_date, imported_at FROM meta",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        assert_eq!(build, 3_607_705);
        assert_eq!(date, "2026-10-12T11:08:38Z");
        let at = u64::try_from(at).unwrap();
        assert!(
            (before..=after).contains(&at),
            "{before} <= {at} <= {after}"
        );
    }

    #[test]
    fn a_pending_path_that_cannot_be_removed_is_an_io_error() {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("sde.db");
        let pending = dir.path().join("sde.db.new");
        fs::create_dir(&pending).unwrap();
        let error = expect_error(import(&fixture(), &db));
        let SdeError::Io { path, .. } = error else {
            panic!("expected an I/O error, got {error:?}");
        };
        assert_eq!(path, pending);
        assert!(!db.exists());
    }

    #[test]
    fn errors_name_the_file_and_line_in_their_text() {
        let file = || "types.jsonl".to_owned();
        let cases = [
            (
                SdeError::MissingFile("blueprints.jsonl".to_owned()),
                "the source has no blueprints.jsonl",
            ),
            (
                SdeError::Json {
                    file: file(),
                    line: 3,
                    message: "EOF while parsing".to_owned(),
                },
                "types.jsonl line 3: EOF while parsing",
            ),
            (
                SdeError::MissingField {
                    file: file(),
                    line: 4,
                    field: "groupID".to_owned(),
                },
                "types.jsonl line 4: missing groupID",
            ),
            (
                SdeError::MissingEnglishName {
                    file: file(),
                    line: 5,
                },
                "types.jsonl line 5: missing name.en",
            ),
            (
                SdeError::BadNumber {
                    file: file(),
                    line: 6,
                    field: "volume".to_owned(),
                    text: "0.00025".to_owned(),
                },
                "types.jsonl line 6: volume is not an exact number: 0.00025",
            ),
            (
                SdeError::UnknownType {
                    file: "blueprints.jsonl".to_owned(),
                    line: 7,
                    type_id: 9,
                },
                "blueprints.jsonl line 7: type 9 is not in types.jsonl",
            ),
            (
                SdeError::UnknownGroup {
                    file: file(),
                    line: 8,
                    group_id: 77,
                },
                "types.jsonl line 8: group 77 is not in groups.jsonl",
            ),
            (
                SdeError::TooLarge { value: u64::MAX },
                "the number 18446744073709551615 is too large to store",
            ),
            (
                SdeError::NotImported,
                "the static data is not imported with this version",
            ),
        ];
        for (error, text) in cases {
            assert_eq!(error.to_string(), text);
            assert!(error.source().is_none(), "{text}");
        }
    }

    #[test]
    fn wrapped_errors_show_and_chain_their_cause() {
        let io = SdeError::Io {
            path: PathBuf::from("/data/sde.zip"),
            source: std::io::Error::other("disk gone"),
        };
        assert_eq!(io.to_string(), "cannot read /data/sde.zip: disk gone");
        assert!(io.source().is_some());

        let zip = SdeError::Zip(zip::result::ZipError::FileNotFound);
        assert!(zip.to_string().starts_with("cannot read the zip archive: "));
        assert!(zip.source().is_some());

        let sqlite = SdeError::Sqlite(rusqlite::Error::QueryReturnedNoRows);
        assert!(sqlite.to_string().starts_with("database error: "));
        assert!(sqlite.source().is_some());
    }

    fn expect_error_open(db: &Path) -> SdeError {
        match Sde::open(db) {
            Ok(_) => panic!("the database should not open"),
            Err(error) => error,
        }
    }
}

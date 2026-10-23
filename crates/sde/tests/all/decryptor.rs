//! Acceptance tests for the decryptors and the structure job cost bonus, on the build 3607705 fixture.

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use mop_core::invention::Decryptor;
    use mop_sde::{Sde, SdeError, import};
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

    /// Changes one line of a copied `.jsonl` file as JSON. Lines count from 1.
    fn edit_line(file: &Path, line: usize, edit: impl FnOnce(&mut Value)) {
        let content = fs::read_to_string(file).unwrap();
        let mut lines: Vec<String> = content.lines().map(str::to_owned).collect();
        let mut value: Value = serde_json::from_str(&lines[line - 1]).unwrap();
        edit(&mut value);
        lines[line - 1] = value.to_string();
        fs::write(file, lines.join("\n") + "\n").unwrap();
    }

    /// The line of the `typeDogma.jsonl` record for `type_id`. Lines count from 1.
    fn dogma_line(source: &Path, type_id: u32) -> usize {
        let content = fs::read_to_string(source.join("typeDogma.jsonl")).unwrap();
        content
            .lines()
            .position(|line| {
                let record: Value = serde_json::from_str(line).unwrap();
                record["_key"] == json!(type_id)
            })
            .unwrap()
            + 1
    }

    /// Sets attribute `attribute` of type `type_id` to `value`, or removes it when `value` is `None`.
    fn edit_attribute(source: &Path, type_id: u32, attribute: u32, value: Option<&Value>) {
        let file = source.join("typeDogma.jsonl");
        let line = dogma_line(source, type_id);
        edit_line(&file, line, |record| {
            let attributes = record["dogmaAttributes"].as_array_mut().unwrap();
            let is_it = |entry: &Value| entry["attributeID"] == json!(attribute);
            match value {
                Some(new) => {
                    for entry in attributes.iter_mut().filter(|entry| is_it(entry)) {
                        entry["value"] = new.clone();
                    }
                }
                None => attributes.retain(|entry| !is_it(entry)),
            }
        });
    }

    /// The fixture imported into a temporary database. Keep the directory alive while using the data.
    fn imported() -> (TempDir, Sde) {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("sde.db");
        import(&fixture(), &db).unwrap();
        let sde = Sde::open(&db).unwrap();
        (dir, sde)
    }

    fn decryptor(type_id: u32, chance_hundredths: u32, me: i8, te: i8, runs: i8) -> Decryptor {
        Decryptor {
            type_id,
            chance_hundredths,
            me,
            te,
            runs,
        }
    }

    #[test]
    fn decryptors_are_the_eight_in_type_order() {
        let (_dir, sde) = imported();
        assert_eq!(
            sde.decryptors().unwrap(),
            vec![
                decryptor(34201, 120, 2, 10, 1),
                decryptor(34202, 180, -1, 4, 4),
                decryptor(34203, 60, -2, 2, 9),
                decryptor(34204, 150, 1, -2, 3),
                decryptor(34205, 110, 3, 6, 0),
                decryptor(34206, 100, 1, 8, 2),
                decryptor(34207, 190, 1, -2, 2),
                decryptor(34208, 90, 2, 0, 7),
            ]
        );
    }

    #[test]
    fn decryptor_by_type_id_gives_its_values_or_none() {
        let (_dir, sde) = imported();
        assert_eq!(
            sde.decryptor(34202).unwrap(),
            Some(Decryptor {
                type_id: 34202,
                chance_hundredths: 180,
                me: -1,
                te: 4,
                runs: 4,
            })
        );
        assert_eq!(sde.decryptor(2454).unwrap(), None);
    }

    #[test]
    fn structure_job_cost_is_the_bonus_in_hundredths_or_none() {
        let (_dir, sde) = imported();
        assert_eq!(sde.structure_job_cost(35825).unwrap(), Some(97));
        assert_eq!(sde.structure_job_cost(35826).unwrap(), Some(96));
        assert_eq!(sde.structure_job_cost(35827).unwrap(), Some(95));
        assert_eq!(sde.structure_job_cost(35835).unwrap(), None);
        assert_eq!(sde.structure_job_cost(35836).unwrap(), None);
        assert_eq!(sde.structure_job_cost(2454).unwrap(), None);
    }

    #[test]
    fn a_bad_me_value_fails_the_import_and_keeps_the_old_database() {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("sde.db");
        import(&fixture(), &db).unwrap();

        let source = fixture_copy(&dir);
        edit_attribute(&source, 34205, 1113, Some(&json!(3.5)));
        let error = import(&source, &db).unwrap_err();
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
            (
                "typeDogma.jsonl",
                dogma_line(&source, 34205),
                "attribute 1113",
                "3.5"
            )
        );
        assert!(Sde::open(&db).unwrap().decryptor(34205).unwrap().is_some());
        assert!(!dir.path().join("sde.db.new").exists());
    }

    #[test]
    fn a_chance_with_three_decimals_is_refused() {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("sde.db");
        let source = fixture_copy(&dir);
        edit_attribute(&source, 34201, 1112, Some(&json!(1.234)));
        let error = import(&source, &db).unwrap_err();
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
            (
                "typeDogma.jsonl",
                dogma_line(&source, 34201),
                "attribute 1112",
                "1.234"
            )
        );
        assert!(!db.exists());
    }

    #[test]
    fn a_version_four_database_is_not_read() {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("sde.db");
        import(&fixture(), &db).unwrap();
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.pragma_update(None, "user_version", 4).unwrap();
        drop(conn);
        assert!(matches!(Sde::open(&db), Err(SdeError::NotImported)));
    }

    #[test]
    fn a_chance_of_ten_is_accepted() {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("sde.db");
        let source = fixture_copy(&dir);
        edit_attribute(&source, 34201, 1112, Some(&json!(10.0)));
        import(&source, &db).unwrap();
        let sde = Sde::open(&db).unwrap();
        assert_eq!(
            sde.decryptor(34201).unwrap().unwrap().chance_hundredths,
            1000
        );
    }

    #[test]
    fn a_chance_above_ten_is_refused_with_its_text() {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("sde.db");
        let source = fixture_copy(&dir);
        edit_attribute(&source, 34201, 1112, Some(&json!(10.01)));
        let error = import(&source, &db).unwrap_err();
        let SdeError::BadNumber { field, text, .. } = error else {
            panic!("expected a bad number error, got {error:?}");
        };
        assert_eq!((field.as_str(), text.as_str()), ("attribute 1112", "10.01"));
        assert!(!db.exists());
    }

    #[test]
    fn a_decryptor_without_a_runs_record_is_refused() {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("sde.db");
        let source = fixture_copy(&dir);
        edit_attribute(&source, 34207, 1124, None);
        let error = import(&source, &db).unwrap_err();
        assert!(matches!(
            error,
            SdeError::IncompleteDecryptor {
                type_id: 34207,
                attribute: 1124
            }
        ));
        assert!(!db.exists());
    }

    #[test]
    fn zero_runs_is_a_value_and_imports() {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("sde.db");
        let source = fixture_copy(&dir);
        edit_attribute(&source, 34205, 1124, Some(&json!(0.0)));
        import(&source, &db).unwrap();
        let sde = Sde::open(&db).unwrap();
        assert_eq!(sde.decryptor(34205).unwrap().unwrap().runs, 0);
    }

    #[test]
    fn a_job_cost_with_three_decimals_is_refused() {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("sde.db");
        let source = fixture_copy(&dir);
        edit_attribute(&source, 35825, 2601, Some(&json!(0.955)));
        let error = import(&source, &db).unwrap_err();
        let SdeError::BadNumber { field, text, .. } = error else {
            panic!("expected a bad number error, got {error:?}");
        };
        assert_eq!((field.as_str(), text.as_str()), ("attribute 2601", "0.955"));
        assert!(!db.exists());
    }

    #[test]
    fn a_job_cost_above_one_is_refused() {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("sde.db");
        let source = fixture_copy(&dir);
        edit_attribute(&source, 35825, 2601, Some(&json!(1.01)));
        let error = import(&source, &db).unwrap_err();
        let SdeError::BadNumber { field, text, .. } = error else {
            panic!("expected a bad number error, got {error:?}");
        };
        assert_eq!((field.as_str(), text.as_str()), ("attribute 2601", "1.01"));
        assert!(!db.exists());
    }
}

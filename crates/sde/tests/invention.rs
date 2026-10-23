//! Acceptance tests for the invention activities, on the build 3607705 fixture.

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use mop_core::money::Rate;
    use mop_sde::invention::{InventionSkill, split_skills};
    use mop_sde::{InventionActivity, InventionProduct, Sde, SdeError, SkillRequirement, import};
    use rusqlite::Connection;
    use serde_json::Value;
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

    /// The line of the record with `key` in a `.jsonl` file. Lines count from 1.
    fn line_of(file: &Path, key: u64) -> usize {
        let content = fs::read_to_string(file).unwrap();
        content
            .lines()
            .position(|line| serde_json::from_str::<Value>(line).unwrap()["_key"] == key)
            .unwrap()
            + 1
    }

    /// Replaces `from` with `to` on one line of a `.jsonl` file. The text must occur once on that line.
    fn replace_on_line(file: &Path, line: usize, from: &str, to: &str) {
        let content = fs::read_to_string(file).unwrap();
        let mut lines: Vec<String> = content.lines().map(str::to_owned).collect();
        assert_eq!(
            lines[line - 1].matches(from).count(),
            1,
            "{from} on line {line}"
        );
        lines[line - 1] = lines[line - 1].replace(from, to);
        fs::write(file, lines.join("\n") + "\n").unwrap();
    }

    /// The fixture imported into a database in `dir`.
    fn imported(dir: &TempDir) -> PathBuf {
        let db = dir.path().join("sde.sqlite");
        import(&fixture(), &db).expect("the fixture imports");
        db
    }

    fn product(type_id: u32, ppm: u32, quantity: u32) -> InventionProduct {
        InventionProduct {
            type_id,
            probability: Rate::from_ppm(ppm),
            quantity,
        }
    }

    fn skill(type_id: u32, name: &str, level: u8) -> SkillRequirement {
        SkillRequirement {
            type_id,
            name: name.to_owned(),
            level,
        }
    }

    #[test]
    fn hobgoblin_one_makes_hobgoblin_two_from_two_datacores_and_three_skills() {
        let dir = TempDir::new().unwrap();
        let sde = Sde::open(&imported(&dir)).unwrap();
        assert_eq!(
            sde.invention(2455).unwrap(),
            Some(InventionActivity {
                blueprint: 2455,
                products: vec![product(2457, 340_000, 10)],
                materials: vec![(20419, 1), (20418, 1)],
                skills: vec![
                    skill(11453, "Electronic Engineering", 1),
                    skill(11446, "Graviton Physics", 1),
                    skill(23121, "Gallente Encryption Methods", 1),
                ],
            })
        );
    }

    #[test]
    fn blueprint_2194_uses_three_of_each_datacore_and_the_amarr_encryption_skill() {
        let dir = TempDir::new().unwrap();
        let sde = Sde::open(&imported(&dir)).unwrap();
        assert_eq!(
            sde.invention(2194).unwrap(),
            Some(InventionActivity {
                blueprint: 2194,
                products: vec![product(2196, 340_000, 10)],
                materials: vec![(20419, 3), (20418, 3)],
                skills: vec![
                    skill(11453, "Electronic Engineering", 1),
                    skill(11446, "Graviton Physics", 1),
                    skill(23087, "Amarr Encryption Methods", 1),
                ],
            })
        );
    }

    #[test]
    fn blueprint_2477_uses_the_minmatar_encryption_skill() {
        let dir = TempDir::new().unwrap();
        let sde = Sde::open(&imported(&dir)).unwrap();
        assert_eq!(
            sde.invention(2477).unwrap(),
            Some(InventionActivity {
                blueprint: 2477,
                products: vec![product(2479, 340_000, 10)],
                materials: vec![(20419, 3), (20418, 3)],
                skills: vec![
                    skill(11453, "Electronic Engineering", 1),
                    skill(11446, "Graviton Physics", 1),
                    skill(21791, "Minmatar Encryption Methods", 1),
                ],
            })
        );
    }

    #[test]
    fn rifter_691_keeps_both_of_its_products_in_file_order() {
        let dir = TempDir::new().unwrap();
        let sde = Sde::open(&imported(&dir)).unwrap();
        assert_eq!(
            sde.invention(691).unwrap(),
            Some(InventionActivity {
                blueprint: 691,
                products: vec![product(11372, 300_000, 1), product(11401, 300_000, 1)],
                materials: vec![(20172, 2), (20424, 2)],
                skills: vec![
                    skill(11445, "Minmatar Starship Engineering", 1),
                    skill(11452, "Mechanical Engineering", 1),
                    skill(21791, "Minmatar Encryption Methods", 1),
                ],
            })
        );
    }

    #[test]
    fn a_blueprint_without_an_invention_activity_has_none() {
        let dir = TempDir::new().unwrap();
        let sde = Sde::open(&imported(&dir)).unwrap();
        // 2457 is the product of 2455 and has no invention activity of its own.
        assert_eq!(sde.invention(2457).unwrap(), None);
        assert_eq!(sde.invention(2456).unwrap(), None);
        assert_eq!(sde.invention(1).unwrap(), None);
    }

    #[test]
    fn a_version_five_database_is_not_imported() {
        let dir = TempDir::new().unwrap();
        let db = imported(&dir);
        let conn = Connection::open(&db).unwrap();
        conn.pragma_update(None, "user_version", 5).unwrap();
        drop(conn);
        assert!(matches!(Sde::open(&db), Err(SdeError::NotImported)));
    }

    /// The fields of a `BadNumber` error, or a panic when it is another error.
    fn bad_number(error: SdeError) -> (String, usize, String, String) {
        let SdeError::BadNumber {
            file,
            line,
            field,
            text,
        } = error
        else {
            panic!("expected BadNumber, got {error:?}");
        };
        (file, line, field, text)
    }

    /// The fields of a `BadReference` error, or a panic when it is another error.
    fn bad_reference(error: SdeError) -> (String, usize, String, u32) {
        let SdeError::BadReference {
            file,
            line,
            field,
            id,
        } = error
        else {
            panic!("expected BadReference, got {error:?}");
        };
        (file, line, field, id)
    }

    #[test]
    fn a_probability_with_seven_decimals_is_refused_and_the_old_database_stays() {
        let dir = TempDir::new().unwrap();
        let db = imported(&dir);
        let source = fixture_copy(&dir);
        let file = source.join("blueprints.jsonl");
        let line = line_of(&file, 2455);
        replace_on_line(
            &file,
            line,
            r#""probability": 0.34,"#,
            r#""probability": 0.3400005,"#,
        );
        let error = import(&source, &db).expect_err("seven decimals are refused");
        let (file, bad_line, field, text) = bad_number(error);
        assert_eq!(
            (file.as_str(), bad_line, field.as_str(), text.as_str()),
            ("blueprints.jsonl", line, "probability", "0.3400005")
        );
        assert!(!dir.path().join("sde.sqlite.new").exists());
        let sde = Sde::open(&db).unwrap();
        assert!(sde.invention(2455).unwrap().is_some());
    }

    #[test]
    fn an_activity_whose_product_has_no_chance_is_not_kept_and_not_counted() {
        let dir = TempDir::new().unwrap();
        let source = fixture_copy(&dir);
        let file = source.join("blueprints.jsonl");
        let line = line_of(&file, 2455);
        replace_on_line(&file, line, r#""probability": 0.34, "#, "");
        let db = dir.path().join("sde.sqlite");
        let report = import(&source, &db).expect("the rest of the fixture imports");
        assert_eq!(report.invention_activities, 3);
        let sde = Sde::open(&db).unwrap();
        assert_eq!(sde.invention(2455).unwrap(), None);
        assert!(sde.invention(2194).unwrap().is_some());
    }

    #[test]
    fn a_probability_above_one_is_refused_with_its_line() {
        let dir = TempDir::new().unwrap();
        let source = fixture_copy(&dir);
        let file = source.join("blueprints.jsonl");
        let line = line_of(&file, 2455);
        replace_on_line(
            &file,
            line,
            r#""probability": 0.34,"#,
            r#""probability": 1.5,"#,
        );
        let error = import(&source, &dir.path().join("sde.sqlite")).expect_err("1.5 is refused");
        let (file, bad_line, field, text) = bad_number(error);
        assert_eq!(
            (file.as_str(), bad_line, field.as_str(), text.as_str()),
            ("blueprints.jsonl", line, "probability", "1.5")
        );
    }

    #[test]
    fn a_product_quantity_of_zero_is_refused_with_its_line() {
        let dir = TempDir::new().unwrap();
        let source = fixture_copy(&dir);
        let file = source.join("blueprints.jsonl");
        let line = line_of(&file, 2194);
        replace_on_line(
            &file,
            line,
            r#""quantity": 10, "typeID": 2196"#,
            r#""quantity": 0, "typeID": 2196"#,
        );
        let error = import(&source, &dir.path().join("sde.sqlite")).expect_err("0 is refused");
        let (file, bad_line, field, text) = bad_number(error);
        assert_eq!(
            (file.as_str(), bad_line, field.as_str(), text.as_str()),
            ("blueprints.jsonl", line, "quantity", "0")
        );
    }

    #[test]
    fn a_skill_without_a_type_record_is_refused_with_its_line() {
        let dir = TempDir::new().unwrap();
        let source = fixture_copy(&dir);
        // Drop the Gallente Encryption Methods record from the types file.
        let types = source.join("types.jsonl");
        let content = fs::read_to_string(&types).unwrap();
        let kept: Vec<&str> = content
            .lines()
            .filter(|line| serde_json::from_str::<Value>(line).unwrap()["_key"] != 23121)
            .collect();
        fs::write(&types, kept.join("\n") + "\n").unwrap();
        let line = line_of(&source.join("blueprints.jsonl"), 2455);
        let error = import(&source, &dir.path().join("sde.sqlite")).expect_err("no record");
        let (file, bad_line, field, id) = bad_reference(error);
        assert_eq!(
            (file.as_str(), bad_line, field.as_str(), id),
            ("blueprints.jsonl", line, "typeID", 23121)
        );
    }

    fn invention_skill(type_id: u32, name: &str, encryption: bool) -> InventionSkill {
        InventionSkill {
            type_id,
            name: name.to_owned(),
            encryption,
        }
    }

    fn activity_with_skills(skills: Vec<SkillRequirement>) -> InventionActivity {
        InventionActivity {
            blueprint: 1,
            products: vec![product(2, 340_000, 1)],
            materials: vec![],
            skills,
        }
    }

    #[test]
    fn hobgoblin_two_is_made_by_the_hobgoblin_one_invention_line_for_its_blueprint() {
        let dir = TempDir::new().unwrap();
        let sde = Sde::open(&imported(&dir)).unwrap();
        assert_eq!(
            sde.invention_for(2456).unwrap(),
            Some((
                InventionActivity {
                    blueprint: 2455,
                    products: vec![product(2457, 340_000, 10)],
                    materials: vec![(20419, 1), (20418, 1)],
                    skills: vec![
                        skill(11453, "Electronic Engineering", 1),
                        skill(11446, "Graviton Physics", 1),
                        skill(23121, "Gallente Encryption Methods", 1),
                    ],
                },
                product(2457, 340_000, 10),
            ))
        );
    }

    #[test]
    fn a_product_whose_blueprint_no_invention_makes_has_no_invention() {
        let dir = TempDir::new().unwrap();
        let sde = Sde::open(&imported(&dir)).unwrap();
        // The fixture has no Wolf or Jaguar blueprint, so the Rifter's invention makes nothing for them.
        assert_eq!(sde.invention_for(11371).unwrap(), None);
        assert_eq!(sde.invention_for(2454).unwrap(), None);
        assert_eq!(sde.invention_for(1).unwrap(), None);
    }

    #[test]
    fn invention_skills_are_the_distinct_skills_sorted_by_name() {
        let dir = TempDir::new().unwrap();
        let sde = Sde::open(&imported(&dir)).unwrap();
        assert_eq!(
            sde.invention_skills().unwrap(),
            vec![
                invention_skill(23087, "Amarr Encryption Methods", true),
                invention_skill(11453, "Electronic Engineering", false),
                invention_skill(23121, "Gallente Encryption Methods", true),
                invention_skill(11446, "Graviton Physics", false),
                invention_skill(11452, "Mechanical Engineering", false),
                invention_skill(21791, "Minmatar Encryption Methods", true),
                invention_skill(11445, "Minmatar Starship Engineering", false),
            ]
        );
    }

    #[test]
    fn split_skills_puts_the_encryption_skill_last_and_keeps_the_science_order() {
        let dir = TempDir::new().unwrap();
        let sde = Sde::open(&imported(&dir)).unwrap();
        let hobgoblin = sde.invention(2455).unwrap().unwrap();
        assert_eq!(split_skills(&hobgoblin), Some((11453, 11446, 23121)));
        let rifter = sde.invention(691).unwrap().unwrap();
        assert_eq!(split_skills(&rifter), Some((11445, 11452, 21791)));
    }

    #[test]
    fn split_skills_is_none_without_exactly_one_encryption_skill_among_three() {
        let two = activity_with_skills(vec![
            skill(11453, "Electronic Engineering", 1),
            skill(23121, "Gallente Encryption Methods", 1),
        ]);
        assert_eq!(split_skills(&two), None);
        let none = activity_with_skills(vec![
            skill(11453, "Electronic Engineering", 1),
            skill(11446, "Graviton Physics", 1),
            skill(11452, "Mechanical Engineering", 1),
        ]);
        assert_eq!(split_skills(&none), None);
    }

    #[test]
    fn split_skills_finds_the_encryption_skill_wherever_it_is_in_the_order() {
        let first = activity_with_skills(vec![
            skill(23087, "Amarr Encryption Methods", 1),
            skill(11453, "Electronic Engineering", 1),
            skill(11446, "Graviton Physics", 1),
        ]);
        assert_eq!(split_skills(&first), Some((11453, 11446, 23087)));
        let middle = activity_with_skills(vec![
            skill(11453, "Electronic Engineering", 1),
            skill(23121, "Gallente Encryption Methods", 1),
            skill(11446, "Graviton Physics", 1),
        ]);
        assert_eq!(split_skills(&middle), Some((11453, 11446, 23121)));
    }
}

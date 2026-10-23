//! Acceptance tests for the structures, rigs and material modifiers, on the build 3607705 fixture.

#[cfg(test)]
mod tests {
    use std::error::Error as _;
    use std::fs;
    use std::path::{Path, PathBuf};

    use mop_core::industry::{Activity, IndustryError, Modifier, job_quantity};
    use mop_sde::facility::{Facility, FacilityError, Rig, Security, Structure, StructureKind};
    use mop_sde::{Sde, SdeError, import};
    use rusqlite::Connection;
    use serde_json::{Value, json};
    use tempfile::TempDir;

    const RAITARU: u32 = 35825;
    const AZBEL: u32 = 35826;
    const SOTIYO: u32 = 35827;
    const ATHANOR: u32 = 35835;
    const TATARA: u32 = 35836;
    const NOT_A_STRUCTURE: u32 = 2454;
    const HOBGOBLIN: u32 = 2454;
    const PARTICLE_ACCELERATOR: u32 = 11688;
    const RIFTER: u32 = 587;
    const CRYSTALLINE_CARBONIDE: u32 = 16670;
    const CRYSTALLITE_ALLOY: u32 = 16655;
    const SMALL_SHIP_RIG_I: u32 = 37154;
    const DRONE_RIG_I: u32 = 37156;
    const DRONE_RIG_II: u32 = 37157;
    const COMPOSITE_RIG_II: u32 = 46487;

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

    /// The fixture imported into a temporary database. Keep the directory alive while using the data.
    fn imported() -> (TempDir, Sde) {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("sde.db");
        import(&fixture(), &db).unwrap();
        let sde = Sde::open(&db).unwrap();
        (dir, sde)
    }

    fn facility(structure: Option<u32>, security: Security, rigs: &[u32]) -> Facility {
        Facility {
            structure,
            security,
            rigs: rigs.to_vec(),
        }
    }

    /// The modifier of `product` for a manufacturing job at `me`.
    fn manufacturing(sde: &Sde, facility: &Facility, product: u32, me: u8) -> String {
        sde.modifier(facility, Activity::Manufacturing, product, me)
            .unwrap()
            .to_decimal_string()
    }

    fn reaction(sde: &Sde, facility: &Facility, product: u32) -> String {
        sde.modifier(facility, Activity::Reaction, product, 0)
            .unwrap()
            .to_decimal_string()
    }

    fn quantity(base: u64, runs: u64, modifier: Modifier) -> u64 {
        job_quantity(base, runs, modifier).unwrap()
    }

    fn error_of(result: Result<impl std::fmt::Debug, FacilityError>) -> FacilityError {
        result.unwrap_err()
    }

    #[test]
    fn structures_are_the_published_engineering_complexes_and_refineries() {
        let (_dir, sde) = imported();
        let structure = |type_id, name: &str, kind, rig_size| Structure {
            type_id,
            name: name.to_owned(),
            kind,
            rig_size,
        };
        assert_eq!(
            sde.structures().unwrap(),
            vec![
                structure(RAITARU, "Raitaru", StructureKind::EngineeringComplex, 2),
                structure(AZBEL, "Azbel", StructureKind::EngineeringComplex, 3),
                structure(SOTIYO, "Sotiyo", StructureKind::EngineeringComplex, 4),
                structure(ATHANOR, "Athanor", StructureKind::Refinery, 2),
                structure(TATARA, "Tatara", StructureKind::Refinery, 3),
            ]
        );
    }

    #[test]
    fn rigs_for_lists_the_rigs_that_fit_and_give_the_activity_a_bonus() {
        let (_dir, sde) = imported();
        let ids = |rigs: &[Rig]| rigs.iter().map(|r| r.type_id).collect::<Vec<_>>();
        let bonuses = |rigs: &[Rig]| {
            rigs.iter()
                .map(|r| r.bonus_tenths_percent)
                .collect::<Vec<_>>()
        };

        let drones = sde.rigs_for(RAITARU, Activity::Manufacturing).unwrap();
        assert_eq!(ids(&drones), vec![37154, 37155, 37156, 37157]);
        assert_eq!(bonuses(&drones), vec![-20, -24, -20, -24]);
        assert_eq!(
            ids(&sde.rigs_for(ATHANOR, Activity::Reaction).unwrap()),
            vec![46486, 46487]
        );

        assert!(
            sde.rigs_for(RAITARU, Activity::Reaction)
                .unwrap()
                .is_empty()
        );
        assert!(sde.rigs_for(TATARA, Activity::Reaction).unwrap().is_empty());
        assert!(
            sde.rigs_for(AZBEL, Activity::Manufacturing)
                .unwrap()
                .is_empty()
        );
        assert!(matches!(
            error_of(sde.rigs_for(NOT_A_STRUCTURE, Activity::Manufacturing)),
            FacilityError::NotAStructure(2454)
        ));
    }

    #[test]
    fn drone_rig_two_in_each_security() {
        let (_dir, sde) = imported();
        let set = facility(Some(RAITARU), Security::Null, &[DRONE_RIG_II]);
        assert_eq!(manufacturing(&sde, &set, HOBGOBLIN, 10), "0.8460936");
        let m = sde
            .modifier(&set, Activity::Manufacturing, HOBGOBLIN, 10)
            .unwrap();
        assert_eq!(quantity(509, 10, m), 4307);

        let low = facility(Some(RAITARU), Security::Low, &[DRONE_RIG_II]);
        assert_eq!(manufacturing(&sde, &low, HOBGOBLIN, 10), "0.8503704");

        let high = facility(Some(RAITARU), Security::High, &[DRONE_RIG_II]);
        assert_eq!(manufacturing(&sde, &high, HOBGOBLIN, 10), "0.869616");
    }

    #[test]
    fn drone_rig_one_in_low_security_gives_exact_quantities() {
        let (_dir, sde) = imported();
        let set = facility(Some(RAITARU), Security::Low, &[DRONE_RIG_I]);
        let m = sde
            .modifier(&set, Activity::Manufacturing, HOBGOBLIN, 10)
            .unwrap();
        assert_eq!(m.to_decimal_string(), "0.857142");
        assert_eq!(quantity(509, 10, m), 4363);
        assert_eq!(quantity(7, 10, m), 60);
        assert_eq!(quantity(4, 10, m), 35);
        assert_eq!(quantity(2, 10, m), 18);
    }

    #[test]
    fn a_rig_covers_only_the_products_its_filter_names() {
        let (_dir, sde) = imported();
        let null_drone = facility(Some(RAITARU), Security::Null, &[DRONE_RIG_II]);
        assert_eq!(
            manufacturing(&sde, &null_drone, PARTICLE_ACCELERATOR, 10),
            "0.891"
        );
        let m = sde
            .modifier(
                &null_drone,
                Activity::Manufacturing,
                PARTICLE_ACCELERATOR,
                10,
            )
            .unwrap();
        assert_eq!(quantity(31, 10, m), 277);

        let small_ship = facility(Some(RAITARU), Security::Null, &[SMALL_SHIP_RIG_I]);
        assert_eq!(manufacturing(&sde, &small_ship, RIFTER, 10), "0.853578");
        let m = sde
            .modifier(&small_ship, Activity::Manufacturing, RIFTER, 10)
            .unwrap();
        assert_eq!(quantity(6000, 30, m), 153645);

        let drone_only = facility(Some(RAITARU), Security::Null, &[DRONE_RIG_II]);
        assert_eq!(manufacturing(&sde, &drone_only, RIFTER, 10), "0.891");

        let both = facility(
            Some(RAITARU),
            Security::Null,
            &[SMALL_SHIP_RIG_I, DRONE_RIG_II],
        );
        assert_eq!(manufacturing(&sde, &both, RIFTER, 10), "0.853578");
    }

    #[test]
    fn npc_station_has_no_structure_bonus() {
        let (_dir, sde) = imported();
        let npc = facility(None, Security::High, &[]);
        assert_eq!(manufacturing(&sde, &npc, HOBGOBLIN, 10), "0.9");
        assert_eq!(manufacturing(&sde, &npc, HOBGOBLIN, 0), "1");
    }

    #[test]
    fn reactions_at_a_refinery_in_low_and_null_security() {
        let (_dir, sde) = imported();
        let null = facility(Some(ATHANOR), Security::Null, &[COMPOSITE_RIG_II]);
        assert_eq!(reaction(&sde, &null, CRYSTALLINE_CARBONIDE), "0.9736");
        let m = sde
            .modifier(&null, Activity::Reaction, CRYSTALLINE_CARBONIDE, 0)
            .unwrap();
        assert_eq!(quantity(100, 1, m), 98);
        assert_eq!(quantity(100, 10, m), 974);
        assert_eq!(reaction(&sde, &null, CRYSTALLITE_ALLOY), "0.9736");

        let low = facility(Some(ATHANOR), Security::Low, &[COMPOSITE_RIG_II]);
        assert_eq!(reaction(&sde, &low, CRYSTALLINE_CARBONIDE), "0.976");

        let bare = facility(Some(ATHANOR), Security::Null, &[]);
        assert_eq!(reaction(&sde, &bare, CRYSTALLINE_CARBONIDE), "1");
    }

    #[test]
    fn activities_that_a_place_does_not_offer_are_refused() {
        let (_dir, sde) = imported();
        let not_offered =
            |facility: Facility, activity| error_of(sde.check_facility(&facility, activity));
        assert!(matches!(
            not_offered(
                facility(Some(ATHANOR), Security::High, &[]),
                Activity::Reaction
            ),
            FacilityError::ActivityNotAvailable {
                activity: Activity::Reaction,
                structure: Some(ATHANOR),
                security: Security::High,
            }
        ));
        assert!(matches!(
            not_offered(facility(None, Security::Low, &[]), Activity::Reaction),
            FacilityError::ActivityNotAvailable {
                structure: None,
                ..
            }
        ));
        assert!(matches!(
            not_offered(
                facility(Some(RAITARU), Security::Null, &[]),
                Activity::Reaction
            ),
            FacilityError::ActivityNotAvailable { .. }
        ));
        assert!(matches!(
            not_offered(
                facility(Some(ATHANOR), Security::Null, &[]),
                Activity::Manufacturing
            ),
            FacilityError::ActivityNotAvailable { .. }
        ));
    }

    #[test]
    fn set_up_errors_name_the_first_failed_check() {
        let (_dir, sde) = imported();
        let check =
            |facility: Facility, activity| error_of(sde.check_facility(&facility, activity));
        assert!(matches!(
            check(
                facility(Some(NOT_A_STRUCTURE), Security::Null, &[]),
                Activity::Manufacturing
            ),
            FacilityError::NotAStructure(2454)
        ));
        assert!(matches!(
            check(
                facility(Some(TATARA), Security::Null, &[COMPOSITE_RIG_II]),
                Activity::Reaction
            ),
            FacilityError::RigDoesNotFit { rig: 46487, .. }
        ));
        assert!(matches!(
            check(
                facility(Some(AZBEL), Security::Null, &[DRONE_RIG_II]),
                Activity::Manufacturing
            ),
            FacilityError::RigDoesNotFit { rig: 37157, .. }
        ));
        assert!(matches!(
            check(
                facility(Some(RAITARU), Security::Null, &[COMPOSITE_RIG_II]),
                Activity::Manufacturing
            ),
            FacilityError::RigDoesNotFit { rig: 46487, .. }
        ));
        assert!(matches!(
            check(
                facility(None, Security::High, &[DRONE_RIG_II]),
                Activity::Manufacturing
            ),
            FacilityError::RigDoesNotFit {
                rig: 37157,
                structure: None
            }
        ));
        assert!(matches!(
            check(
                facility(Some(RAITARU), Security::Null, &[DRONE_RIG_I, DRONE_RIG_II]),
                Activity::Manufacturing
            ),
            FacilityError::RigGroupTwice(37157)
        ));
        assert!(matches!(
            check(
                facility(
                    Some(RAITARU),
                    Security::Null,
                    &[SMALL_SHIP_RIG_I, DRONE_RIG_I, DRONE_RIG_II, DRONE_RIG_II]
                ),
                Activity::Manufacturing
            ),
            FacilityError::TooManyRigs(4)
        ));
        assert!(matches!(
            check(
                facility(Some(RAITARU), Security::Null, &[NOT_A_STRUCTURE]),
                Activity::Manufacturing
            ),
            FacilityError::NotARig(2454)
        ));
    }

    #[test]
    fn modifier_errors_come_after_the_set_up_checks() {
        let (_dir, sde) = imported();
        let athanor = facility(Some(ATHANOR), Security::Null, &[]);
        assert!(matches!(
            error_of(sde.modifier(&athanor, Activity::Reaction, CRYSTALLINE_CARBONIDE, 2)),
            FacilityError::MeOnReaction(2)
        ));
        let raitaru = facility(Some(RAITARU), Security::Null, &[]);
        assert!(matches!(
            error_of(sde.modifier(&raitaru, Activity::Manufacturing, 1, 10)),
            FacilityError::UnknownProduct(1)
        ));
        assert!(matches!(
            error_of(sde.modifier(&raitaru, Activity::Manufacturing, HOBGOBLIN, 11)),
            FacilityError::Industry(IndustryError::MeOutOfRange(11))
        ));
        assert!(matches!(
            error_of(sde.modifier(&raitaru, Activity::Manufacturing, 1, 11)),
            FacilityError::Industry(IndustryError::MeOutOfRange(11))
        ));
    }

    #[test]
    fn a_wrapped_error_names_its_cause() {
        let error = FacilityError::Industry(IndustryError::ZeroMaxRuns);
        assert!(error.source().is_some());
        let error = FacilityError::NotARig(2454);
        assert!(error.source().is_none());
    }

    #[test]
    fn failure_messages_read_as_sentences() {
        let error = FacilityError::RigDoesNotFit {
            rig: 46487,
            structure: Some(RAITARU),
        };
        assert_eq!(error.to_string(), "rig 46487 does not fit structure 35825");
        let error = FacilityError::ActivityNotAvailable {
            activity: Activity::Reaction,
            structure: None,
            security: Security::Low,
        };
        assert_eq!(
            error.to_string(),
            "the reaction activity is not offered at an NPC station in low security space"
        );
    }

    #[test]
    fn a_rig_bonus_with_too_many_decimals_fails_the_import_and_keeps_the_old_database() {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("sde.db");
        let before = import(&fixture(), &db).unwrap();
        let source = fixture_copy(&dir);
        // Line 9 of the attribute file is type 37157.
        edit_line(&source.join("typeDogma.jsonl"), 9, |value| {
            for attribute in value["dogmaAttributes"].as_array_mut().unwrap() {
                if attribute["attributeID"] == json!(2594) {
                    attribute["value"] = json!(-2.45);
                }
            }
        });
        let SdeError::BadNumber {
            file,
            line,
            field,
            text,
        } = import(&source, &db).unwrap_err()
        else {
            panic!("the import should refuse the bad number");
        };
        assert_eq!(file, "typeDogma.jsonl");
        assert_eq!(line, 9);
        assert_eq!(field, "attribute 2594");
        assert_eq!(text, "-2.45");
        let kept = Sde::open(&db).unwrap().build().unwrap();
        assert_eq!(kept.number, before.build_number);
    }

    #[test]
    fn a_bonus_written_with_an_exponent_is_refused_as_text() {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("sde.db");
        let source = fixture_copy(&dir);
        // Line 9 of the attribute file is type 37157. The literal text 1e3 is not a plain decimal.
        edit_line(&source.join("typeDogma.jsonl"), 9, |value| {
            for attribute in value["dogmaAttributes"].as_array_mut().unwrap() {
                if attribute["attributeID"] == json!(2594) {
                    attribute["value"] = serde_json::from_str("1e3").unwrap();
                }
            }
        });
        let SdeError::BadNumber {
            file, line, field, ..
        } = import(&source, &db).unwrap_err()
        else {
            panic!("the import should refuse the bad number");
        };
        assert_eq!(file, "typeDogma.jsonl");
        assert_eq!(line, 9);
        assert_eq!(field, "attribute 2594");
    }

    /// The rigs of `fixture_copy` with line 9 (type 37157) changed by `edit`, imported and listed.
    fn rig_ids_after_editing_drone_rig_two(edit: impl FnOnce(&mut Value)) -> Vec<u32> {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("sde.db");
        let source = fixture_copy(&dir);
        edit_line(&source.join("typeDogma.jsonl"), 9, edit);
        import(&source, &db).unwrap();
        let sde = Sde::open(&db).unwrap();
        sde.rigs_for(RAITARU, Activity::Manufacturing)
            .unwrap()
            .iter()
            .map(|rig| rig.type_id)
            .collect()
    }

    #[test]
    fn a_rig_without_its_bonus_is_left_out_not_refused() {
        let ids = rig_ids_after_editing_drone_rig_two(|value| {
            value["dogmaAttributes"]
                .as_array_mut()
                .unwrap()
                .retain(|attribute| attribute["attributeID"] != json!(2594));
        });
        assert_eq!(ids, vec![37154, 37155, 37156]);
    }

    #[test]
    fn a_rig_without_its_security_values_is_left_out_not_refused() {
        let ids = rig_ids_after_editing_drone_rig_two(|value| {
            value["dogmaAttributes"]
                .as_array_mut()
                .unwrap()
                .retain(|attribute| attribute["attributeID"] != json!(2357));
        });
        assert_eq!(ids, vec![37154, 37155, 37156]);
    }

    #[test]
    fn a_rig_with_two_filters_covers_the_products_of_both() {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("sde.db");
        let source = fixture_copy(&dir);
        // Line 8 of the modifier file is type 37157. Filter 5 names the small ship groups, the Rifter's.
        edit_line(&source.join("industryModifierSources.jsonl"), 8, |value| {
            value["manufacturing"]["material"] = json!([
                {"dogmaAttributeID": 2542, "filterID": 1},
                {"dogmaAttributeID": 2542, "filterID": 5}
            ]);
        });
        import(&source, &db).unwrap();
        let sde = Sde::open(&db).unwrap();
        let set = facility(Some(RAITARU), Security::Null, &[DRONE_RIG_II]);
        assert_eq!(manufacturing(&sde, &set, RIFTER, 10), "0.8460936");
        assert_eq!(manufacturing(&sde, &set, PARTICLE_ACCELERATOR, 10), "0.891");
    }

    #[test]
    fn a_structure_takes_three_rigs() {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("sde.db");
        let source = fixture_copy(&dir);
        // Line 66 of the type file is type 37155: give it a group of its own, so three groups fit.
        edit_line(&source.join("types.jsonl"), 66, |value| {
            value["groupID"] = json!(1934);
        });
        import(&source, &db).unwrap();
        let sde = Sde::open(&db).unwrap();
        let three = facility(
            Some(RAITARU),
            Security::Null,
            &[SMALL_SHIP_RIG_I, 37155, DRONE_RIG_I],
        );
        sde.check_facility(&three, Activity::Manufacturing).unwrap();
    }

    #[test]
    fn a_rig_that_fits_another_group_is_refused() {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("sde.db");
        let source = fixture_copy(&dir);
        // Attribute 1299 is the engineering complex group that drone rig II fits.
        edit_line(&source.join("typeDogma.jsonl"), 9, |value| {
            for attribute in value["dogmaAttributes"].as_array_mut().unwrap() {
                if attribute["attributeID"] == json!(1299) {
                    attribute["value"] = json!(1000.0);
                }
            }
        });
        import(&source, &db).unwrap();
        let sde = Sde::open(&db).unwrap();
        let set = facility(Some(RAITARU), Security::Null, &[DRONE_RIG_II]);
        assert!(matches!(
            error_of(sde.check_facility(&set, Activity::Manufacturing)),
            FacilityError::RigDoesNotFit {
                rig: DRONE_RIG_II,
                ..
            }
        ));
    }

    #[test]
    fn a_database_of_another_version_is_not_read() {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("sde.db");
        import(&fixture(), &db).unwrap();
        Connection::open(&db)
            .unwrap()
            .pragma_update(None, "user_version", 1)
            .unwrap();
        assert!(matches!(Sde::open(&db), Err(SdeError::NotImported)));
    }

    #[test]
    fn a_rig_category_type_without_a_material_entry_is_not_read() {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("sde.db");
        let source = fixture_copy(&dir);
        // Type 37157 loses its material entry and its attribute record: it is no rig, so no error.
        edit_line(&source.join("industryModifierSources.jsonl"), 8, |value| {
            *value = json!({"_key": 37157});
        });
        let dogma = source.join("typeDogma.jsonl");
        let kept: Vec<String> = fs::read_to_string(&dogma)
            .unwrap()
            .lines()
            .enumerate()
            .filter(|(index, _)| *index != 8)
            .map(|(_, line)| line.to_owned())
            .collect();
        fs::write(&dogma, kept.join("\n") + "\n").unwrap();
        import(&source, &db).unwrap();
        let sde = Sde::open(&db).unwrap();
        assert_eq!(
            sde.rigs_for(RAITARU, Activity::Manufacturing)
                .unwrap()
                .len(),
            3
        );
    }

    #[test]
    fn a_rig_without_a_filter_covers_every_product() {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("sde.db");
        let source = fixture_copy(&dir);
        // Line 8 of the modifier file is type 37157.
        edit_line(&source.join("industryModifierSources.jsonl"), 8, |value| {
            value["manufacturing"]["material"][0]
                .as_object_mut()
                .unwrap()
                .remove("filterID");
        });
        import(&source, &db).unwrap();
        let sde = Sde::open(&db).unwrap();
        let set = facility(Some(RAITARU), Security::Null, &[DRONE_RIG_II]);
        assert_eq!(
            manufacturing(&sde, &set, PARTICLE_ACCELERATOR, 10),
            "0.8460936"
        );
    }
}

//! Acceptance tests for the station names: on the build 3607705 fixture, and on a copy of it whose
//! station and system files are the stations fixture's.

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::{Path, PathBuf};

    use mop_sde::{Sde, SdeError, Station, import};
    use rusqlite::Connection;
    use serde_json::{Value, json};
    use tempfile::TempDir;

    /// The files of the stations fixture that take the place of the main fixture's in a copy.
    const STATION_FILES: [&str; 7] = [
        "npcStations.jsonl",
        "npcCorporations.jsonl",
        "stationOperations.jsonl",
        "mapMoons.jsonl",
        "mapPlanets.jsonl",
        "mapStars.jsonl",
        "mapSolarSystems.jsonl",
    ];

    fn fixture() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sde-3607705")
    }

    fn stations_fixture() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/stations")
    }

    /// A copy of the main fixture in which the stations fixture's station and system files replace
    /// the main ones. The stargates are emptied: the stations fixture lists only six systems.
    fn stations_source(dir: &TempDir) -> PathBuf {
        let copy = dir.path().join("source");
        fs::create_dir(&copy).unwrap();
        for entry in fs::read_dir(fixture()).unwrap() {
            let entry = entry.unwrap();
            fs::copy(entry.path(), copy.join(entry.file_name())).unwrap();
        }
        for name in STATION_FILES {
            fs::copy(stations_fixture().join(name), copy.join(name)).unwrap();
        }
        fill_system_fields(&copy.join("mapSolarSystems.jsonl"));
        fs::write(copy.join("mapStargates.jsonl"), "").unwrap();
        copy
    }

    /// The stations fixture's systems have no region and no security status, which the import needs.
    /// They are put in the main fixture's one region, with security 0.0; no name depends on either.
    fn fill_system_fields(file: &Path) {
        let content = fs::read_to_string(file).unwrap();
        let mut text = String::new();
        for line in content.lines() {
            let mut value: Value = serde_json::from_str(line).unwrap();
            value["regionID"] = json!(10_000_002);
            value["securityStatus"] = json!(0.0);
            text.push_str(&value.to_string());
            text.push('\n');
        }
        fs::write(file, text).unwrap();
    }

    /// Changes the record of a copied `.jsonl` file whose `_key` is `key`, as JSON.
    fn edit_record(file: &Path, key: i64, edit: impl Fn(&mut Value)) {
        let content = fs::read_to_string(file).unwrap();
        let mut text = String::new();
        for line in content.lines() {
            let mut value: Value = serde_json::from_str(line).unwrap();
            if value["_key"] == json!(key) {
                edit(&mut value);
            }
            text.push_str(&value.to_string());
            text.push('\n');
        }
        fs::write(file, text).unwrap();
    }

    /// Imports `source` into a database in `dir` and opens it.
    fn import_and_open(source: &Path, dir: &TempDir) -> Sde {
        let db = dir.path().join("sde.db");
        import(source, &db).unwrap();
        Sde::open(&db).unwrap()
    }

    /// The names that ESI gives the seven stations of the stations fixture.
    fn expected_names() -> BTreeMap<i64, String> {
        let text = fs::read_to_string(stations_fixture().join("expected.json")).unwrap();
        let names: BTreeMap<String, String> = serde_json::from_str(&text).unwrap();
        names
            .into_iter()
            .map(|(id, name)| (id.parse().unwrap(), name))
            .collect()
    }

    #[test]
    fn the_main_fixture_station_has_its_name_and_system() {
        let dir = TempDir::new().unwrap();
        let sde = import_and_open(&fixture(), &dir);
        assert_eq!(
            sde.station(60003760).unwrap(),
            Some(Station {
                id: 60003760,
                name: "Jita IV - Moon 4 - Caldari Navy Assembly Plant".to_owned(),
                system_id: 30000142,
            })
        );
        assert_eq!(sde.station(1).unwrap(), None);
    }

    #[test]
    fn each_of_the_seven_stations_gets_its_expected_name() {
        let dir = TempDir::new().unwrap();
        let sde = import_and_open(&stations_source(&dir), &dir);
        let expected = expected_names();
        assert_eq!(expected.len(), 7);
        let ids: Vec<i64> = expected.keys().copied().collect();
        assert_eq!(sde.station_names(&ids).unwrap(), expected);
        for (id, name) in &expected {
            let station = sde.station(*id).unwrap().unwrap();
            assert_eq!(&station.name, name);
        }
    }

    #[test]
    fn station_names_leaves_out_an_id_that_is_not_a_station() {
        let dir = TempDir::new().unwrap();
        let sde = import_and_open(&stations_source(&dir), &dir);
        let names = sde.station_names(&[60003760, 1, 60015187]).unwrap();
        assert_eq!(names.len(), 2);
        assert!(!names.contains_key(&1));
        assert_eq!(
            names.get(&60015187).map(String::as_str),
            Some("Zarzakh - Deathless Custodians - The Fulcrum")
        );
    }

    #[test]
    fn planet_numbers_are_written_as_roman_numerals() {
        for (number, numeral) in [
            (1, "I"),
            (4, "IV"),
            (9, "IX"),
            (14, "XIV"),
            (19, "XIX"),
            (40, "XL"),
            (50, "L"),
        ] {
            let dir = TempDir::new().unwrap();
            let source = stations_source(&dir);
            edit_record(&source.join("mapPlanets.jsonl"), 40176121, |value| {
                value["celestialIndex"] = json!(number);
            });
            let sde = import_and_open(&source, &dir);
            let station = sde.station(60000010).unwrap().unwrap();
            assert_eq!(
                station.name,
                format!("Annaro {numeral} - CBD Corporation Storage")
            );
        }
    }

    #[test]
    fn a_planet_number_outside_one_to_fifty_is_refused() {
        for number in [0, 51] {
            let dir = TempDir::new().unwrap();
            let source = stations_source(&dir);
            edit_record(&source.join("mapPlanets.jsonl"), 40176121, |value| {
                value["celestialIndex"] = json!(number);
            });
            let error = import(&source, &dir.path().join("sde.db")).unwrap_err();
            let SdeError::BadNumber {
                file,
                line,
                field,
                text,
            } = error
            else {
                panic!("expected a bad number, got {error:?}");
            };
            assert_eq!(file, "mapPlanets.jsonl");
            assert_eq!(line, 5);
            assert_eq!(field, "celestialIndex");
            assert_eq!(text, number.to_string());
        }
    }

    #[test]
    fn a_station_whose_owner_is_not_listed_is_refused() {
        let dir = TempDir::new().unwrap();
        let source = stations_source(&dir);
        edit_record(&source.join("npcStations.jsonl"), 60003760, |value| {
            value["ownerID"] = json!(999_999);
        });
        let error = import(&source, &dir.path().join("sde.db")).unwrap_err();
        let SdeError::BadReference {
            file, field, id, ..
        } = error
        else {
            panic!("expected a bad reference, got {error:?}");
        };
        assert_eq!(
            (file.as_str(), field.as_str(), id),
            ("npcStations.jsonl", "ownerID", 999_999)
        );
    }

    #[test]
    fn an_orbit_in_no_map_file_is_refused() {
        let dir = TempDir::new().unwrap();
        let source = stations_source(&dir);
        edit_record(&source.join("npcStations.jsonl"), 60003760, |value| {
            value["orbitID"] = json!(1);
        });
        let error = import(&source, &dir.path().join("sde.db")).unwrap_err();
        let SdeError::BadReference {
            file, field, id, ..
        } = error
        else {
            panic!("expected a bad reference, got {error:?}");
        };
        assert_eq!(
            (file.as_str(), field.as_str(), id),
            ("npcStations.jsonl", "orbitID", 1)
        );
    }

    #[test]
    fn a_source_without_station_operations_is_refused() {
        let dir = TempDir::new().unwrap();
        let source = stations_source(&dir);
        fs::remove_file(source.join("stationOperations.jsonl")).unwrap();
        let error = import(&source, &dir.path().join("sde.db")).unwrap_err();
        let SdeError::MissingFile(name) = error else {
            panic!("expected a missing file, got {error:?}");
        };
        assert_eq!(name, "stationOperations.jsonl");
    }

    #[test]
    fn a_unique_name_without_english_is_refused_with_its_line() {
        let dir = TempDir::new().unwrap();
        let source = stations_source(&dir);
        edit_record(&source.join("mapMoons.jsonl"), 40009087, |value| {
            value["uniqueName"] = json!({ "de": "Jita IV - Moon 4" });
        });
        let error = import(&source, &dir.path().join("sde.db")).unwrap_err();
        let SdeError::MissingEnglishName { file, line } = error else {
            panic!("expected a missing English name, got {error:?}");
        };
        assert_eq!((file.as_str(), line), ("mapMoons.jsonl", 2));
    }

    #[test]
    fn a_database_of_the_previous_version_is_not_read() {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("sde.db");
        import(&fixture(), &db).unwrap();
        let conn = Connection::open(&db).unwrap();
        conn.pragma_update(None, "user_version", 3).unwrap();
        drop(conn);
        assert!(matches!(Sde::open(&db), Err(SdeError::NotImported)));
    }

    #[test]
    fn a_partial_station_name_finds_the_station_first() {
        let dir = TempDir::new().unwrap();
        let sde = import_and_open(&stations_source(&dir), &dir);
        let found = sde.search_stations("jita iv - moon 4", 10).unwrap();
        assert_eq!(
            found[0],
            (
                60003760,
                "Jita IV - Moon 4 - Caldari Navy Assembly Plant".to_owned()
            )
        );
    }

    #[test]
    fn a_whole_station_name_in_capitals_finds_that_station_first() {
        let dir = TempDir::new().unwrap();
        let sde = import_and_open(&stations_source(&dir), &dir);
        let found = sde
            .search_stations("JITA IV - MOON 4 - CALDARI NAVY ASSEMBLY PLANT", 10)
            .unwrap();
        assert_eq!(found[0].0, 60003760);
        assert!(
            sde.search_stations("no such station anywhere", 10)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn an_exact_station_name_finds_that_station_before_a_longer_containing_one() {
        let dir = TempDir::new().unwrap();
        let source = stations_source(&dir);
        edit_record(&source.join("npcCorporations.jsonl"), 1000438, |value| {
            value["name"]["en"] = json!("Jita IV - Moon 4 - Caldari Navy Assembly Plant");
        });
        let sde = import_and_open(&source, &dir);
        assert_eq!(
            sde.search_stations("jita iv - moon 4 - caldari navy assembly plant", 10)
                .unwrap(),
            vec![
                (
                    60003760,
                    "Jita IV - Moon 4 - Caldari Navy Assembly Plant".to_owned()
                ),
                (
                    60015187,
                    "Zarzakh - Jita IV - Moon 4 - Caldari Navy Assembly Plant - The Fulcrum"
                        .to_owned()
                ),
            ]
        );
    }

    #[test]
    fn an_exact_station_name_outranks_a_longer_containing_one() {
        let dir = TempDir::new().unwrap();
        let source = stations_source(&dir);
        edit_record(&source.join("npcCorporations.jsonl"), 1000438, |value| {
            value["name"]["en"] = json!("Jita IV - Moon 4 - Caldari Navy Assembly Plant");
        });
        let sde = import_and_open(&source, &dir);
        assert_eq!(
            sde.search_stations("jita iv - moon 4 - caldari navy assembly plant", 10)
                .unwrap(),
            vec![
                (
                    60003760,
                    "Jita IV - Moon 4 - Caldari Navy Assembly Plant".to_owned()
                ),
                (
                    60015187,
                    "Zarzakh - Jita IV - Moon 4 - Caldari Navy Assembly Plant - The Fulcrum"
                        .to_owned()
                ),
            ]
        );
    }

    #[test]
    fn an_exact_station_name_comes_before_a_longer_prefix_match() {
        let dir = TempDir::new().unwrap();
        let source = stations_source(&dir);
        let q = "Jita IV - Moon 4 - Caldari Navy Assembly Plant";
        edit_record(&source.join("mapMoons.jsonl"), 40008952, |value| {
            value["uniqueName"] = json!({ "en": q });
        });
        let sde = import_and_open(&source, &dir);
        let found = sde.search_stations(q, 10).unwrap();
        assert!(found.len() >= 2);
        assert_eq!(found[0], (60003760, q.to_owned()));
        assert_eq!(
            found[1],
            (
                60003454,
                "Jita IV - Moon 4 - Caldari Navy Assembly Plant - Caldari Business Tribunal"
                    .to_owned()
            )
        );
        assert_eq!(
            sde.search_stations(q, 1).unwrap(),
            vec![(60003760, q.to_owned())]
        );
    }

    #[test]
    fn a_blank_query_or_a_zero_limit_finds_nothing() {
        let dir = TempDir::new().unwrap();
        let sde = import_and_open(&stations_source(&dir), &dir);
        assert!(sde.search_stations("", 10).unwrap().is_empty());
        assert!(sde.search_stations("   ", 10).unwrap().is_empty());
        assert!(sde.search_stations("jita", 0).unwrap().is_empty());
    }
}

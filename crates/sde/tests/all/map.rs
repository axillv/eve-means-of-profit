//! Acceptance tests for the map: systems, names, search and jump counts, on the build 3607705 fixture.

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use mop_sde::{MapError, RouteKind, Sde, SdeError, SolarSystem, import};
    use rusqlite::Connection;
    use serde_json::{Value, json};
    use tempfile::TempDir;

    const JITA: u32 = 30000142;
    const PERIMETER: u32 = 30000144;
    const KIAINTI: u32 = 30000189;
    const VASALA: u32 = 30000190;
    const OBE: u32 = 30000205;

    /// The line of Jita in the system file. Lines count from 1.
    const JITA_LINE: usize = 24;

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

    /// Replaces text inside one line of a copied `.jsonl` file, keeping the rest of the line as it
    /// is. Lines count from 1.
    fn replace_text(file: &Path, line: usize, from: &str, to: &str) {
        let content = fs::read_to_string(file).unwrap();
        let mut lines: Vec<String> = content.lines().map(str::to_owned).collect();
        assert!(lines[line - 1].contains(from), "line {line} has no {from}");
        lines[line - 1] = lines[line - 1].replace(from, to);
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

    fn system(sde: &Sde, id: u32) -> SolarSystem {
        sde.system(id).unwrap().unwrap()
    }

    #[test]
    fn a_version_two_database_is_not_read() {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("old.db");
        let conn = Connection::open(&db).unwrap();
        conn.pragma_update(None, "user_version", 2).unwrap();
        drop(conn);
        let Err(error) = Sde::open(&db) else {
            panic!("a version two database was read");
        };
        assert!(matches!(error, SdeError::NotImported));
    }

    #[test]
    fn jita_has_its_name_region_and_security() {
        let (_dir, sde) = imported();
        let jita = system(&sde, JITA);
        assert_eq!(jita.name, "Jita");
        assert_eq!(jita.region_id, 10_000_002);
        assert_eq!(jita.region_name, "The Forge");
        assert_eq!(jita.security_millionths, 945_913);
        assert!(jita.is_high_sec());
    }

    #[test]
    fn perimeter_and_obe_have_their_security() {
        let (_dir, sde) = imported();
        let perimeter = system(&sde, PERIMETER);
        assert_eq!(perimeter.security_millionths, 949_000);
        assert!(perimeter.is_high_sec());
        let obe = system(&sde, OBE);
        assert_eq!(obe.security_millionths, 346_931);
        assert!(!obe.is_high_sec());
    }

    #[test]
    fn an_unknown_system_is_none() {
        let (_dir, sde) = imported();
        assert_eq!(sde.system(1).unwrap(), None);
    }

    #[test]
    fn find_system_ignores_case_and_surrounding_spaces() {
        let (_dir, sde) = imported();
        assert_eq!(sde.find_system("jita").unwrap(), Some(JITA));
        assert_eq!(sde.find_system("  NEW CALDARI ").unwrap(), Some(30000145));
    }

    #[test]
    fn find_system_needs_the_whole_name_and_a_non_empty_one() {
        let (_dir, sde) = imported();
        assert_eq!(sde.find_system("Jit").unwrap(), None);
        assert_eq!(sde.find_system("").unwrap(), None);
    }

    #[test]
    fn search_systems_ranks_like_type_search() {
        let (_dir, sde) = imported();
        assert_eq!(sde.search_systems("jita", 10).unwrap(), vec![JITA]);
        assert_eq!(sde.search_systems("per", 10).unwrap(), vec![PERIMETER]);
        assert_eq!(
            sde.search_systems("ou", 10).unwrap(),
            vec![30000135, 30000193]
        );
        assert_eq!(sde.search_systems("caldari", 10).unwrap(), vec![30000145]);
        assert_eq!(
            sde.search_systems("a", 10).unwrap(),
            vec![
                30000200, 30000163, 30000166, 30000132, 30000147, 30000185, 30000178, 30000125,
                30021407, 30000142,
            ]
        );
    }

    #[test]
    fn search_regions_matches_names_case_free_and_by_later_word() {
        let (_dir, sde) = imported();
        assert_eq!(
            sde.search_regions("for", 10).unwrap(),
            vec![(10000002, "The Forge".to_string())]
        );
        assert_eq!(
            sde.search_regions("FORGE", 10).unwrap(),
            vec![(10000002, "The Forge".to_string())]
        );
    }

    #[test]
    fn search_regions_finds_a_region_by_its_whole_name() {
        // The shared fixture holds only The Forge, so Domain is added to a copy of it.
        let dir = TempDir::new().unwrap();
        let source = fixture_copy(&dir);
        let regions = source.join("mapRegions.jsonl");
        let mut content = fs::read_to_string(&regions).unwrap();
        content.push_str("\n{\"_key\": 10000043, \"name\": {\"en\": \"Domain\"}}\n");
        fs::write(&regions, content).unwrap();
        let db = dir.path().join("sde.db");
        import(&source, &db).unwrap();
        let sde = Sde::open(&db).unwrap();
        assert_eq!(
            sde.search_regions("domain", 10).unwrap(),
            vec![(10000043, "Domain".to_string())]
        );
    }

    #[test]
    fn search_regions_finds_nothing_for_a_blank_or_zero_limit_query() {
        let (_dir, sde) = imported();
        assert!(sde.search_regions("", 10).unwrap().is_empty());
        assert!(sde.search_regions("   ", 10).unwrap().is_empty());
        assert!(sde.search_regions("for", 0).unwrap().is_empty());
    }

    #[test]
    fn search_systems_finds_nothing_for_an_empty_or_absent_query() {
        let (_dir, sde) = imported();
        assert!(sde.search_systems("", 10).unwrap().is_empty());
        assert!(sde.search_systems("zzz", 10).unwrap().is_empty());
    }

    #[test]
    fn jumps_from_jita_by_shortest_and_high_sec_routes() {
        let (_dir, sde) = imported();
        let map = sde.star_map().unwrap();
        let cases = [
            (JITA, Some(0), Some(0)),
            (PERIMETER, Some(1), Some(1)),
            (KIAINTI, Some(11), Some(12)),
            (VASALA, Some(11), Some(13)),
            (OBE, Some(16), None),
        ];
        for (to, shortest, high) in cases {
            assert_eq!(
                map.jumps(JITA, to, RouteKind::Shortest).unwrap(),
                shortest,
                "shortest to {to}"
            );
            assert_eq!(
                map.jumps(JITA, to, RouteKind::HighSecOnly).unwrap(),
                high,
                "high sec to {to}"
            );
        }
    }

    #[test]
    fn jumps_are_the_same_both_ways() {
        let (_dir, sde) = imported();
        let map = sde.star_map().unwrap();
        assert_eq!(
            map.jumps(KIAINTI, JITA, RouteKind::Shortest).unwrap(),
            Some(11)
        );
        assert_eq!(
            map.jumps(KIAINTI, JITA, RouteKind::HighSecOnly).unwrap(),
            Some(12)
        );
    }

    #[test]
    fn a_low_sec_end_has_no_high_sec_route() {
        let (_dir, sde) = imported();
        let map = sde.star_map().unwrap();
        assert_eq!(map.jumps(OBE, OBE, RouteKind::Shortest).unwrap(), Some(0));
        assert_eq!(map.jumps(OBE, OBE, RouteKind::HighSecOnly).unwrap(), None);
        assert_eq!(map.jumps(OBE, JITA, RouteKind::HighSecOnly).unwrap(), None);
        assert_eq!(map.jumps(JITA, OBE, RouteKind::HighSecOnly).unwrap(), None);
        // Walvalin is low sec with high sec gates to Kiainti and Vasala; its own end still rules.
        assert_eq!(
            map.jumps(30000191, JITA, RouteKind::HighSecOnly).unwrap(),
            None
        );
    }

    #[test]
    fn map_error_says_which_system_is_unknown() {
        assert_eq!(
            MapError::UnknownSystem(1).to_string(),
            "system 1 is not in the map"
        );
    }

    #[test]
    fn an_unknown_from_system_is_reported_before_an_unknown_to_system() {
        let (_dir, sde) = imported();
        let map = sde.star_map().unwrap();
        assert_eq!(
            map.jumps(1, JITA, RouteKind::Shortest),
            Err(MapError::UnknownSystem(1))
        );
        assert_eq!(
            map.jumps(JITA, 2, RouteKind::HighSecOnly),
            Err(MapError::UnknownSystem(2))
        );
        assert_eq!(
            map.jumps(1, 2, RouteKind::Shortest),
            Err(MapError::UnknownSystem(1))
        );
    }

    #[test]
    fn a_negative_security_text_in_exponent_form_is_read() {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("sde.db");
        let source = fixture_copy(&dir);
        replace_text(
            &source.join("mapSolarSystems.jsonl"),
            JITA_LINE,
            "\"securityStatus\": 0.945913",
            "\"securityStatus\": -9e-05",
        );
        import(&source, &db).unwrap();
        let sde = Sde::open(&db).unwrap();
        assert_eq!(system(&sde, JITA).security_millionths, -90);
    }

    #[test]
    fn a_trailing_zero_past_six_decimals_is_still_exact() {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("sde.db");
        let source = fixture_copy(&dir);
        replace_text(
            &source.join("mapSolarSystems.jsonl"),
            JITA_LINE,
            "\"securityStatus\": 0.945913",
            "\"securityStatus\": 0.9459130",
        );
        import(&source, &db).unwrap();
        let sde = Sde::open(&db).unwrap();
        assert_eq!(system(&sde, JITA).security_millionths, 945_913);
    }

    #[test]
    fn a_long_run_of_trailing_zeros_is_still_exact() {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("sde.db");
        let source = fixture_copy(&dir);
        replace_text(
            &source.join("mapSolarSystems.jsonl"),
            JITA_LINE,
            "\"securityStatus\": 0.945913",
            &format!("\"securityStatus\": 0.945913{}", "0".repeat(40)),
        );
        import(&source, &db).unwrap();
        let sde = Sde::open(&db).unwrap();
        assert_eq!(system(&sde, JITA).security_millionths, 945_913);
    }

    #[test]
    fn security_with_a_digit_past_six_decimals_is_refused() {
        assert_security_refused("0.9459135");
    }

    #[test]
    fn security_outside_the_range_is_refused() {
        assert_security_refused("1.5");
    }

    /// Imports a copy whose Jita has `text` as its security. The import is refused with the file,
    /// line and field named, and the old database is still there, unchanged.
    fn assert_security_refused(text: &str) {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("sde.db");
        import(&fixture(), &db).unwrap();
        let source = fixture_copy(&dir);
        replace_text(
            &source.join("mapSolarSystems.jsonl"),
            JITA_LINE,
            "\"securityStatus\": 0.945913",
            &format!("\"securityStatus\": {text}"),
        );
        let error = import(&source, &db).unwrap_err();
        let SdeError::BadNumber {
            file,
            line,
            field,
            text: shown,
        } = error
        else {
            panic!("expected a bad number error, got {error:?}");
        };
        assert_eq!(
            (file.as_str(), line, field.as_str(), shown.as_str()),
            ("mapSolarSystems.jsonl", JITA_LINE, "securityStatus", text)
        );
        let sde = Sde::open(&db).unwrap();
        assert_eq!(system(&sde, JITA).security_millionths, 945_913);
        assert!(!dir.path().join("sde.db.new").exists());
    }

    #[test]
    fn a_gate_to_a_missing_system_is_refused_with_its_line() {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("sde.db");
        import(&fixture(), &db).unwrap();
        let source = fixture_copy(&dir);
        edit_line(&source.join("mapStargates.jsonl"), 1, |value| {
            value["destination"]["solarSystemID"] = json!(30_009_999);
        });
        let error = import(&source, &db).unwrap_err();
        let SdeError::BadReference {
            file,
            line,
            field,
            id,
        } = error
        else {
            panic!("expected a bad reference error, got {error:?}");
        };
        assert_eq!(
            (file.as_str(), line, field.as_str(), id),
            (
                "mapStargates.jsonl",
                1,
                "destination.solarSystemID",
                30_009_999
            )
        );
        assert!(!dir.path().join("sde.db.new").exists());
        let sde = Sde::open(&db).unwrap();
        assert_eq!(system(&sde, JITA).security_millionths, 945_913);
    }
}

use std::collections::BTreeSet;
use std::fs::File;
use std::path::PathBuf;

use profit_core::industry::TypeId;
use profit_core::stockpile::{Characters, Filter, Place, Source, Target};
use profit_jeve::{
    ImportedStockpile, JeveError, StockpileImport, read_stockpiles, read_stockpiles_file,
};

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = include_str!("../fixtures/stockpiles/sample.xml");
    const MAIN: i64 = 2_112_000_001;
    const ALT: i64 = 2_112_000_002;
    const JITA_4_4: i64 = 60_003_760;

    fn type_name(id: TypeId) -> Option<String> {
        match id {
            34 => Some("Tritanium".to_owned()),
            35 => Some("Pyerite".to_owned()),
            2456 => Some("Hobgoblin II".to_owned()),
            2457 => Some("Hobgoblin II Blueprint".to_owned()),
            _ => None,
        }
    }

    fn import(xml: &str, existing: &[&str]) -> Result<StockpileImport, JeveError> {
        let existing: Vec<String> = existing.iter().map(|name| (*name).to_owned()).collect();
        read_stockpiles(xml, &existing, &type_name)
    }

    fn sources(listed: &[Source]) -> BTreeSet<Source> {
        listed.iter().copied().collect()
    }

    fn only(characters: &[i64]) -> Characters {
        Characters::Only(characters.iter().copied().collect())
    }

    fn every_source() -> BTreeSet<Source> {
        sources(&[
            Source::Assets,
            Source::SellOrders,
            Source::BuyOrders,
            Source::Jobs,
        ])
    }

    fn default_filter() -> Filter {
        Filter {
            characters: Characters::All,
            place: Place::Everywhere,
            sources: every_source(),
        }
    }

    fn target(type_id: TypeId, units: u64, ignore_multiplier: bool) -> Target {
        Target {
            type_id,
            units,
            ignore_multiplier,
        }
    }

    /// The notes as (stockpile, what) pairs, in the order they were given.
    fn notes(result: &StockpileImport) -> Vec<(String, String)> {
        result
            .left_out
            .iter()
            .map(|note| (note.stockpile.clone(), note.what.clone()))
            .collect()
    }

    fn note(stockpile: &str, what: &str) -> (String, String) {
        (stockpile.to_owned(), what.to_owned())
    }

    /// A settings file holding one stockpile element with `body` inside it.
    fn one(stockpile: &str) -> String {
        format!("<settings><stockpiles>{stockpile}</stockpiles></settings>")
    }

    fn only_stockpile(result: &StockpileImport) -> &ImportedStockpile {
        assert_eq!(result.stockpiles.len(), 1);
        &result.stockpiles[0]
    }

    fn failure(xml: &str) -> String {
        match import(xml, &[]) {
            Err(error) => error.to_string(),
            Ok(_) => panic!("the import should have failed"),
        }
    }

    #[test]
    fn sample_gives_three_stockpiles_with_their_targets_and_filters() {
        let result = import(SAMPLE, &["kit"]).expect("import");

        assert_eq!(result.stockpiles.len(), 3);
        assert_eq!(
            result.stockpiles[0],
            ImportedStockpile {
                name: "Kit (2)".to_owned(),
                group: Some("Drones".to_owned()),
                multiplier: 1,
                targets: vec![target(34, 10_000_000, false), target(2456, 3, true)],
                includes: vec![],
                filters: vec![Filter {
                    characters: only(&[MAIN]),
                    place: Place::Location(JITA_4_4),
                    sources: sources(&[Source::Assets, Source::SellOrders]),
                }],
            }
        );
        assert_eq!(
            result.stockpiles[1],
            ImportedStockpile {
                name: "Weekly".to_owned(),
                group: None,
                multiplier: 2,
                targets: vec![target(35, 7, true)],
                includes: vec![("Kit (2)".to_owned(), 3)],
                filters: vec![default_filter()],
            }
        );
        assert_eq!(
            result.stockpiles[2],
            ImportedStockpile {
                name: "Half".to_owned(),
                group: None,
                multiplier: 2,
                targets: vec![target(34, 150, false)],
                includes: vec![("Weekly".to_owned(), 1)],
                filters: vec![Filter {
                    characters: Characters::All,
                    place: Place::Everywhere,
                    sources: sources(&[Source::Assets, Source::Jobs]),
                }],
            }
        );
    }

    #[test]
    fn sample_lists_what_was_left_out_in_order() {
        let result = import(SAMPLE, &["kit"]).expect("import");

        assert_eq!(
            notes(&result),
            vec![
                note("Kit (2)", "named \"Kit (2)\": \"Kit\" is taken"),
                note("Kit (2)", "Hobgoblin II: 2.5 units rounded up to 3"),
                note(
                    "Kit (2)",
                    "Hobgoblin II Blueprint (copy): blueprint copy row left out"
                ),
                note(
                    "Kit (2)",
                    "Hobgoblin II Blueprint: blueprint runs row left out"
                ),
                note(
                    "Kit (2)",
                    "type 99999999 is not in the game data; row left out"
                ),
                note("Kit (2)", "filter 1: containers left out"),
                note(
                    "Kit (2)",
                    "filter 2: constellation 20000020 is not supported; filter left out"
                ),
                note(
                    "Kit (2)",
                    "filter 3: exclude filter left out (counts more than in jEve)"
                ),
                note("Kit (2)", "filter 4: contracts left out"),
                note(
                    "Kit (2)",
                    "filter 4: no source the app counts; filter left out"
                ),
                note(
                    "Weekly",
                    "includes \"Missing\", which is not in the file; left out"
                ),
                note("Half", "multiplier 1.5 rounded up to 2"),
                note("Half", "Tritanium listed twice; units added"),
                note("Half", "includes \"Weekly\" 0.5 times; rounded up to 1"),
            ]
        );
    }

    #[test]
    fn taken_names_get_the_next_free_number() {
        let result = import(SAMPLE, &["Kit", "Kit (2)"]).expect("import");

        assert_eq!(result.stockpiles[0].name, "Kit (3)");
        assert_eq!(
            result.stockpiles[1].includes,
            vec![("Kit (3)".to_owned(), 3)]
        );
    }

    #[test]
    fn a_free_name_comes_in_without_a_note() {
        let result = import(SAMPLE, &[]).expect("import");

        assert_eq!(result.stockpiles[0].name, "Kit");
        assert!(
            !notes(&result)
                .iter()
                .any(|(_, what)| what.starts_with("named"))
        );
    }

    #[test]
    fn names_taken_by_an_earlier_stockpile_of_the_file_get_a_number() {
        let xml = one(
            r#"<stockpile name="Kit"/><stockpile name="kit"><item typeid="34" minimum="1"/></stockpile>"#,
        );

        let result = import(&xml, &[]).expect("import");

        assert_eq!(result.stockpiles[0].name, "Kit");
        assert_eq!(result.stockpiles[1].name, "kit (2)");
        assert_eq!(
            notes(&result),
            vec![note("kit (2)", "named \"kit (2)\": \"kit\" is taken")]
        );
    }

    #[test]
    fn a_subpile_matches_its_stockpile_ignoring_capitals() {
        let xml = one(
            r#"<stockpile name="Kit"><item typeid="34" minimum="1"/></stockpile><stockpile name="Weekly"><subpile name="kit" minimum="2"/></stockpile>"#,
        );

        let result = import(&xml, &[]).expect("import");

        assert_eq!(result.stockpiles[1].includes, vec![("Kit".to_owned(), 2)]);
        assert!(notes(&result).is_empty());
    }

    #[test]
    fn the_result_never_shows_text_outside_the_stockpiles() {
        let result = import(SAMPLE, &["kit"]).expect("import");

        assert!(!format!("{result:?}").contains("secret-do-not-read"));
        for note in notes(&result) {
            assert!(!note.1.contains("secret-do-not-read"));
        }
    }

    #[test]
    fn a_file_is_read_the_same_as_its_text() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path: PathBuf = dir.path().join("stockpiles.xml");
        std::fs::write(&path, SAMPLE).expect("write");

        let from_file = read_stockpiles_file(&path, &["kit".to_owned()], &type_name).expect("file");

        assert_eq!(from_file, import(SAMPLE, &["kit"]).expect("import"));
    }

    #[test]
    fn a_file_of_exactly_50_mb_is_read_not_refused() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("edge.xml");
        let file = File::create(&path).expect("create");
        file.set_len(50 * 1024 * 1024).expect("size");

        let error = read_stockpiles_file(&path, &[], &type_name).expect_err("not XML");

        assert_eq!(
            error.to_string(),
            "cannot read the stockpiles: not an XML file"
        );
    }

    #[test]
    fn a_filter_with_only_one_job_day_limit_says_so() {
        let xml = one(
            r#"<stockpile name="Jobs"><stockpilefilter locationid="0" inventory="true" sellorders="false" buyorders="false" jobs="true" jobsdaysmore="3"/></stockpile>"#,
        );

        let result = import(&xml, &[]).expect("import");

        assert_eq!(
            notes(&result),
            vec![note("Jobs", "filter 1: job days left out")]
        );
    }

    #[test]
    fn a_filter_with_a_singleton_choice_says_so() {
        let xml = one(
            r#"<stockpile name="Single"><stockpilefilter locationid="0" inventory="true" sellorders="false" buyorders="false" jobs="false" singleton="true"/></stockpile>"#,
        );

        let result = import(&xml, &[]).expect("import");

        assert_eq!(
            notes(&result),
            vec![note("Single", "filter 1: singleton choice left out")]
        );
    }

    #[test]
    fn an_old_filter_for_character_zero_counts_every_character() {
        let xml = one(
            r#"<stockpile name="Old" characterid="0" locationid="0" inventory="true" sellorders="false" buyorders="false" jobs="false"/>"#,
        );

        let result = import(&xml, &[]).expect("import");

        assert_eq!(
            only_stockpile(&result).filters[0].characters,
            Characters::All
        );
        assert!(notes(&result).is_empty());
    }

    #[test]
    fn a_location_too_big_for_an_id_is_not_understood() {
        let xml = one(
            r#"<stockpile name="Huge"><stockpilefilter locationid="99999999999999999999999" inventory="true" sellorders="false" buyorders="false" jobs="false"/></stockpile>"#,
        );

        let result = import(&xml, &[]).expect("import");

        assert!(only_stockpile(&result).filters.is_empty());
        assert_eq!(
            notes(&result),
            vec![
                note(
                    "Huge",
                    "filter 1: place 99999999999999999999999 is not understood; filter left out"
                ),
                note("Huge", "no filter left: counts nothing until you add one"),
            ]
        );
    }

    #[test]
    fn a_type_too_big_for_an_id_is_not_in_the_game_data() {
        let xml = one(
            r#"<stockpile name="Huge"><item typeid="99999999999999999999999" minimum="1"/></stockpile>"#,
        );

        let result = import(&xml, &[]).expect("import");

        assert!(only_stockpile(&result).targets.is_empty());
        assert_eq!(
            notes(&result),
            vec![note(
                "Huge",
                "type 99999999999999999999999 is not in the game data; row left out"
            )]
        );
    }

    #[test]
    fn a_singleton_switch_set_to_false_says_nothing() {
        let xml = one(
            r#"<stockpile name="Single"><stockpilefilter locationid="0" inventory="true" sellorders="false" buyorders="false" jobs="false" singleton="false"/></stockpile>"#,
        );

        let result = import(&xml, &[]).expect("import");

        assert_eq!(only_stockpile(&result).filters.len(), 1);
        assert!(notes(&result).is_empty());
    }

    #[test]
    fn a_total_row_is_skipped_even_when_its_other_attributes_are_bad() {
        let xml = one(
            r#"<stockpile name="Total"><item typeid="0" minimum="not-read" runs="maybe"/><item typeid="34" minimum="2"/></stockpile>"#,
        );

        let result = import(&xml, &[]).expect("import");

        assert_eq!(only_stockpile(&result).targets, vec![target(34, 2, false)]);
        assert!(notes(&result).is_empty());
    }

    #[test]
    fn a_type_that_is_not_a_number_fails_the_import() {
        let xml = one(r#"<stockpile name="Kit"><item typeid="abc" minimum="1"/></stockpile>"#);

        assert_eq!(
            failure(&xml),
            "cannot read the stockpiles: stockpile \"Kit\": \"abc\" is not a number"
        );
    }

    #[test]
    fn a_location_that_is_not_a_number_fails_the_import() {
        let xml = one(
            r#"<stockpile name="Kit"><stockpilefilter locationid="abc" inventory="true" sellorders="false" buyorders="false" jobs="false"/></stockpile>"#,
        );

        assert_eq!(
            failure(&xml),
            "cannot read the stockpiles: stockpile \"Kit\": \"abc\" is not a number"
        );
    }

    #[test]
    fn a_file_over_50_mb_is_refused() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("big.xml");
        let file = File::create(&path).expect("create");
        file.set_len(50 * 1024 * 1024 + 1).expect("size");

        let error = read_stockpiles_file(&path, &[], &type_name).expect_err("too big");

        assert_eq!(
            error.to_string(),
            "cannot read the stockpiles: the file is larger than 50 MB"
        );
    }

    #[test]
    fn an_old_single_filter_comes_in_as_one_filter() {
        let xml = one(
            r#"<stockpile name="Old" characterid="2112000002" locationid="30000142" inventory="true" sellorders="false" buyorders="false" jobs="true"/>"#,
        );

        let result = import(&xml, &[]).expect("import");

        assert_eq!(
            only_stockpile(&result).filters,
            vec![Filter {
                characters: only(&[ALT]),
                place: Place::System(30_000_142),
                sources: sources(&[Source::Assets, Source::Jobs]),
            }]
        );
        assert!(notes(&result).is_empty());
    }

    #[test]
    fn a_stockpile_with_no_filter_counts_everything() {
        let xml = one(r#"<stockpile name="Plain"><item typeid="34" minimum="5.0"/></stockpile>"#);

        let result = import(&xml, &[]).expect("import");

        assert_eq!(only_stockpile(&result).filters, vec![default_filter()]);
        assert!(notes(&result).is_empty());
    }

    #[test]
    fn a_location_0_filter_counts_everywhere() {
        let xml = one(
            r#"<stockpile name="Everywhere"><stockpilefilter locationid="0" inventory="true" sellorders="false" buyorders="false" jobs="false"/></stockpile>"#,
        );

        let result = import(&xml, &[]).expect("import");

        assert_eq!(
            only_stockpile(&result).filters,
            vec![Filter {
                characters: Characters::All,
                place: Place::Everywhere,
                sources: sources(&[Source::Assets]),
            }]
        );
    }

    #[test]
    fn a_region_filter_and_a_structure_filter_keep_their_place() {
        let xml = one(
            r#"<stockpile name="Places"><stockpilefilter locationid="10000002" inventory="true" sellorders="false" buyorders="false" jobs="false"/><stockpilefilter locationid="1035466617946" inventory="true" sellorders="false" buyorders="false" jobs="false"/></stockpile>"#,
        );

        let result = import(&xml, &[]).expect("import");

        assert_eq!(
            only_stockpile(&result).filters,
            vec![
                Filter {
                    characters: Characters::All,
                    place: Place::Region(10_000_002),
                    sources: sources(&[Source::Assets]),
                },
                Filter {
                    characters: Characters::All,
                    place: Place::Location(1_035_466_617_946),
                    sources: sources(&[Source::Assets]),
                },
            ]
        );
        assert!(notes(&result).is_empty());
    }

    #[test]
    fn a_bad_location_drops_only_its_filter() {
        let xml = one(
            r#"<stockpile name="Odd"><stockpilefilter locationid="42" inventory="true" sellorders="false" buyorders="false" jobs="false"/><stockpilefilter locationid="0" inventory="true" sellorders="false" buyorders="false" jobs="false"/></stockpile>"#,
        );

        let result = import(&xml, &[]).expect("import");

        assert_eq!(only_stockpile(&result).filters.len(), 1);
        assert_eq!(
            notes(&result),
            vec![note(
                "Odd",
                "filter 1: place 42 is not understood; filter left out"
            )]
        );
    }

    #[test]
    fn a_filter_with_only_inventory_on_has_no_notes() {
        let xml = one(
            r#"<stockpile name="Switch"><stockpilefilter locationid="0" inventory="true" sellorders="false" buyorders="false" jobs="false"/></stockpile>"#,
        );

        let result = import(&xml, &[]).expect("import");

        assert_eq!(only_stockpile(&result).filters.len(), 1);
        assert_eq!(
            only_stockpile(&result).filters[0].sources,
            sources(&[Source::Assets])
        );
        assert!(notes(&result).is_empty());
    }

    #[test]
    fn a_filter_without_its_inventory_switch_fails_the_import() {
        let xml = one(
            r#"<stockpile name="Kit"><stockpilefilter locationid="0" sellorders="false" buyorders="false" jobs="false"/></stockpile>"#,
        );

        assert_eq!(
            failure(&xml),
            "cannot read the stockpiles: stockpile \"Kit\": a filter has no inventory"
        );
    }

    #[test]
    fn a_stockpile_with_nothing_left_of_its_filters_says_so() {
        let xml = one(
            r#"<stockpile name="Kit"><stockpilefilter locationid="20000020" inventory="true" sellorders="false" buyorders="false" jobs="false"/><stockpilefilter locationid="30000142" exclude="true" inventory="true" sellorders="false" buyorders="false" jobs="false"/></stockpile>"#,
        );

        let result = import(&xml, &[]).expect("import");

        assert!(only_stockpile(&result).filters.is_empty());
        assert_eq!(
            notes(&result),
            vec![
                note(
                    "Kit",
                    "filter 1: constellation 20000020 is not supported; filter left out"
                ),
                note(
                    "Kit",
                    "filter 2: exclude filter left out (counts more than in jEve)"
                ),
                note("Kit", "no filter left: counts nothing until you add one"),
            ]
        );
    }

    #[test]
    fn a_loop_of_subpiles_leaves_out_the_one_that_closes_it() {
        let xml = one(
            r#"<stockpile name="A"><item typeid="34" minimum="1"/><subpile name="B" minimum="1"/></stockpile><stockpile name="B"><item typeid="35" minimum="1"/><subpile name="A" minimum="1"/></stockpile>"#,
        );

        let result = import(&xml, &[]).expect("import");

        assert_eq!(result.stockpiles[0].includes, vec![("B".to_owned(), 1)]);
        assert!(result.stockpiles[1].includes.is_empty());
        assert_eq!(
            notes(&result),
            vec![note(
                "B",
                "includes \"A\", which would make a loop; left out"
            )]
        );
    }

    #[test]
    fn a_huge_multiplier_comes_in_at_1_and_is_listed() {
        let xml = one(
            r#"<stockpile name="Ammo" multiplier="5000000000"><item typeid="34" minimum="10"/></stockpile>"#,
        );

        let result = import(&xml, &[]).expect("import");

        assert_eq!(only_stockpile(&result).multiplier, 1);
        assert_eq!(
            notes(&result),
            vec![note("Ammo", "multiplier 5000000000 is too big; set to 1")]
        );
    }

    #[test]
    fn a_huge_subpile_count_comes_in_at_1_and_is_listed() {
        let xml = one(
            r#"<stockpile name="Kit"><item typeid="34" minimum="1"/></stockpile><stockpile name="Weekly"><subpile name="Kit" minimum="5000000000"/></stockpile>"#,
        );

        let result = import(&xml, &[]).expect("import");

        assert_eq!(result.stockpiles[1].includes, vec![("Kit".to_owned(), 1)]);
        assert_eq!(
            notes(&result),
            vec![note(
                "Weekly",
                "includes \"Kit\" 5000000000 times is too big; set to 1"
            )]
        );
    }

    #[test]
    fn an_item_minimum_too_big_for_the_app_is_left_out() {
        let xml = one(r#"<stockpile name="Big"><item typeid="34" minimum="1.0E20"/></stockpile>"#);

        let result = import(&xml, &[]).expect("import");

        assert!(only_stockpile(&result).targets.is_empty());
        assert_eq!(
            notes(&result),
            vec![note(
                "Big",
                "Tritanium: 1.0E20 units is out of range; row left out"
            )]
        );
    }

    #[test]
    fn a_multiplier_of_zero_becomes_1() {
        let xml = one(
            r#"<stockpile name="Zero" multiplier="0.0"><item typeid="34" minimum="4"/></stockpile>"#,
        );

        let result = import(&xml, &[]).expect("import");

        assert_eq!(only_stockpile(&result).multiplier, 1);
        assert_eq!(notes(&result), vec![note("Zero", "multiplier 0 set to 1")]);
    }

    #[test]
    fn a_negative_multiplier_and_target_are_read_as_zero() {
        let xml = one(
            r#"<stockpile name="Negative" multiplier="-2"><item typeid="34" minimum="-1"/></stockpile>"#,
        );

        let result = import(&xml, &[]).expect("import");

        assert_eq!(only_stockpile(&result).multiplier, 1);
        assert!(only_stockpile(&result).targets.is_empty());
        assert_eq!(
            notes(&result),
            vec![
                note("Negative", "multiplier 0 set to 1"),
                note("Negative", "Tritanium: target 0 left out"),
            ]
        );
    }

    #[test]
    fn a_zero_target_and_a_zero_subpile_are_left_out() {
        let xml = one(
            r#"<stockpile name="Zero"><item typeid="34" minimum="0.0"/><subpile name="Zero" minimum="0"/></stockpile>"#,
        );

        let result = import(&xml, &[]).expect("import");

        assert!(only_stockpile(&result).targets.is_empty());
        assert_eq!(
            notes(&result),
            vec![
                note("Zero", "Tritanium: target 0 left out"),
                note("Zero", "includes \"Zero\" 0 times; left out"),
            ]
        );
    }

    #[test]
    fn a_type_listed_twice_with_different_ignore_settings_keeps_the_first_row() {
        let xml = one(
            r#"<stockpile name="Mixed"><item typeid="34" minimum="4"/><item typeid="34" minimum="9" ignoremultiplier="true"/></stockpile>"#,
        );

        let result = import(&xml, &[]).expect("import");

        assert_eq!(only_stockpile(&result).targets, vec![target(34, 4, false)]);
        assert_eq!(
            notes(&result),
            vec![note("Mixed", "Tritanium listed twice; second row left out")]
        );
    }

    #[test]
    fn a_repeated_type_whose_units_overflow_keeps_the_first_row() {
        let xml = one(
            r#"<stockpile name="Full"><item typeid="34" minimum="18446744073709551615"/><item typeid="34" minimum="1"/></stockpile>"#,
        );

        let result = import(&xml, &[]).expect("import");

        assert_eq!(
            only_stockpile(&result).targets,
            vec![target(34, u64::MAX, false)]
        );
        assert_eq!(
            notes(&result),
            vec![note("Full", "Tritanium listed twice; second row left out")]
        );
    }

    #[test]
    fn a_bad_yes_or_no_value_fails_the_import() {
        let xml = one(
            r#"<stockpile name="Kit"><stockpilefilter locationid="0" inventory="yes" sellorders="false" buyorders="false" jobs="false"/></stockpile>"#,
        );

        assert_eq!(
            failure(&xml),
            "cannot read the stockpiles: stockpile \"Kit\": filter 1 inventory is \"yes\", not true or false"
        );
    }

    #[test]
    fn an_item_without_a_type_fails_the_import() {
        let xml = one(r#"<stockpile name="Kit"><item minimum="1"/></stockpile>"#);

        assert_eq!(
            failure(&xml),
            "cannot read the stockpiles: stockpile \"Kit\": a row has no typeid"
        );
    }

    #[test]
    fn a_number_that_is_not_one_fails_the_import() {
        let xml = one(r#"<stockpile name="Kit"><item typeid="34" minimum="1,5"/></stockpile>"#);

        assert_eq!(
            failure(&xml),
            "cannot read the stockpiles: stockpile \"Kit\": \"1,5\" is not a number"
        );
    }

    #[test]
    fn a_stockpile_without_a_name_fails_the_import() {
        let xml = one(r#"<stockpile><item typeid="34" minimum="1"/></stockpile>"#);

        assert_eq!(
            failure(&xml),
            "cannot read the stockpiles: a stockpile has no name"
        );
    }

    #[test]
    fn a_file_that_is_not_xml_fails_the_import() {
        assert_eq!(
            failure("hello"),
            "cannot read the stockpiles: not an XML file"
        );
    }

    #[test]
    fn a_file_of_another_kind_fails_the_import() {
        assert_eq!(
            failure("<foo/>"),
            "cannot read the stockpiles: not a jEve settings or stockpile file"
        );
    }

    #[test]
    fn a_settings_file_without_stockpiles_fails_the_import() {
        assert_eq!(
            failure("<settings/>"),
            "cannot read the stockpiles: the file holds no stockpiles"
        );
    }
}

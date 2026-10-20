use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use profit_core::money::Money;
use profit_core::net_worth::{CharacterKey, Part, Record, Snapshot};
use profit_jeve::{JeveError, read_tracker, tracker_path};

#[cfg(test)]
mod tests {
    use super::*;

    /// A point with every field jEve requires and no optional ones.
    const POINT: &str = r#"{"date":1000,"assets":0.0,"escrows":0.0,"escrowstocover":0.0,"sellorders":0.0,"walletbalance":0.0,"manufacturing":0.0,"contractcollateral":0.0,"contractvalue":0.0}"#;

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/tracker")
            .join(name)
    }

    /// A Tracker file holding `text`. The folder is removed when the returned guard is dropped.
    fn tracker_file(text: &str) -> (tempfile::TempDir, PathBuf) {
        let root = tempfile::tempdir().expect("temp dir");
        let path = root.path().join("tracker.json");
        fs::write(&path, text).expect("write tracker");
        (root, path)
    }

    fn read_text(text: &str) -> Result<Option<Vec<Record>>, JeveError> {
        let (_root, path) = tracker_file(text);
        read_tracker(&path)
    }

    fn records(text: &str) -> Vec<Record> {
        read_text(text).expect("read").expect("the file exists")
    }

    fn values(record: &Record) -> &BTreeMap<Part, Money> {
        match &record.snapshot {
            Snapshot::Imported(values) => values,
            Snapshot::Seen(_) | Snapshot::Removed => {
                panic!("expected imported values, got {:?}", record.snapshot)
            }
        }
    }

    fn value(record: &Record, part: Part) -> Money {
        values(record)
            .get(&part)
            .copied()
            .expect("every part is present")
    }

    fn isk(text: &str) -> Money {
        Money::parse_isk(text).expect("valid ISK")
    }

    fn bad_data(result: Result<Option<Vec<Record>>, JeveError>) -> (String, usize, &'static str) {
        match result {
            Err(JeveError::TrackerBadData {
                owner,
                index,
                field,
            }) => (owner, index, field),
            other => panic!("expected bad data, got {other:?}"),
        }
    }

    #[test]
    fn filters_fixture_reads_the_balance_and_asset_lists() {
        let records = records(&fs::read_to_string(fixture("filters.json")).expect("fixture"));

        assert_eq!(records.len(), 1);
        let record = &records[0];
        assert_eq!(record.character, CharacterKey::Name("TEST-NAME".to_owned()));
        assert_eq!(record.at, 1_552_492_124);
        assert_eq!(value(record, Part::Wallet), isk("10.00"));
        assert_eq!(value(record, Part::Assets), isk("9.00"));
        assert_eq!(value(record, Part::SellOrders), isk("8.00"));
        assert_eq!(value(record, Part::BuyEscrow), isk("5.00"));
        assert_eq!(value(record, Part::IndustryJobs), isk("7.00"));
        assert_eq!(value(record, Part::ContractCollateral), isk("3.00"));
        assert_eq!(value(record, Part::Contracts), isk("4.00"));
        assert_eq!(value(record, Part::Implants), Money::ZERO);
    }

    #[test]
    fn totals_fixture_reads_every_point_in_order() {
        let records = records(&fs::read_to_string(fixture("totals.json")).expect("fixture"));

        assert_eq!(records.len(), 2);
        let first = &records[0];
        assert_eq!(first.character, CharacterKey::Id(2_112_625_428));
        assert_eq!(first.at, 1_760_000_000);
        assert_eq!(
            value(first, Part::Wallet),
            Money::from_ten_thousandths(60_467_593_722_300)
        );
        assert_eq!(
            value(first, Part::Assets),
            Money::from_ten_thousandths(225_279_995_355_900)
        );
        assert_eq!(
            value(first, Part::IndustryJobs),
            Money::from_ten_thousandths(32_580_000_000)
        );
        assert_eq!(
            value(first, Part::ContractCollateral),
            Money::from_ten_thousandths(4_330_000_000_000)
        );
        assert_eq!(
            value(first, Part::Contracts),
            Money::from_ten_thousandths(160_000_000_000)
        );
        assert_eq!(
            value(first, Part::Implants),
            Money::from_ten_thousandths(3_000)
        );
        assert_eq!(value(first, Part::SellOrders), Money::ZERO);
        assert_eq!(value(first, Part::BuyEscrow), Money::ZERO);

        let second = &records[1];
        assert_eq!(second.character, CharacterKey::Id(2_112_625_428));
        assert_eq!(second.at, 1_760_086_400);
        assert_eq!(value(second, Part::Wallet), isk("2.50"));
        assert_eq!(value(second, Part::Assets), isk("1.00"));
    }

    #[test]
    fn the_balance_list_is_summed_and_wins_over_the_total() {
        let text = r#"{"1":[{"date":1000,"assets":50.0,"escrows":0.0,"escrowstocover":0.0,"sellorders":0.0,"walletbalance":99.0,"manufacturing":0.0,"contractcollateral":0.0,"contractvalue":0.0,"balance":[{"id":"1000","value":1.25},{"id":"1001","value":2.5}],"asset":[]}]}"#;

        let records = records(text);

        assert_eq!(value(&records[0], Part::Wallet), isk("3.75"));
        assert_eq!(value(&records[0], Part::Assets), Money::ZERO);
    }

    #[test]
    fn a_missing_required_field_names_the_owner_point_and_field() {
        // The second point of the totals fixture, without sellorders.
        let text = format!(
            r#"{{"2112625428":[{POINT},{{"date":1760086400999,"assets":1.0,"escrows":0.0,"escrowstocover":0.0,"walletbalance":2.5,"manufacturing":0.0,"contractcollateral":0.0,"contractvalue":0.0}}]}}"#
        );
        assert_eq!(
            bad_data(read_text(&text)),
            ("2112625428".to_owned(), 1, "sellorders")
        );

        let text = format!(
            r#"{{"2112625428":[{}]}}"#,
            POINT.replace(r#","escrowstocover":0.0"#, "")
        );
        assert_eq!(
            bad_data(read_text(&text)),
            ("2112625428".to_owned(), 0, "escrowstocover")
        );
    }

    #[test]
    fn a_value_that_is_not_a_number_is_bad_data() {
        let text = format!(
            r#"{{"1":[{}]}}"#,
            POINT.replace(r#""walletbalance":0.0"#, r#""walletbalance":"abc""#)
        );

        assert_eq!(
            bad_data(read_text(&text)),
            ("1".to_owned(), 0, "walletbalance")
        );
    }

    #[test]
    fn a_negative_date_is_bad_data() {
        let text = format!(
            r#"{{"1":[{}]}}"#,
            POINT.replace(r#""date":1000"#, r#""date":-1"#)
        );

        assert_eq!(bad_data(read_text(&text)), ("1".to_owned(), 0, "date"));
    }

    #[test]
    fn a_value_that_does_not_fit_is_bad_data() {
        let text = format!(
            r#"{{"1":[{}]}}"#,
            POINT.replace(
                r#""walletbalance":0.0"#,
                r#""walletbalance":922337203685477.5808"#,
            )
        );

        assert_eq!(
            bad_data(read_text(&text)),
            ("1".to_owned(), 0, "walletbalance")
        );
    }

    #[test]
    fn a_top_level_that_is_not_an_object_is_not_json() {
        match read_text("[]") {
            Err(JeveError::TrackerNotJson { reason }) => {
                assert!(reason.contains("an object of owners"), "{reason}");
            }
            other => panic!("expected not JSON, got {other:?}"),
        }
    }

    #[test]
    fn a_folder_in_place_of_the_file_is_an_io_error() {
        let root = tempfile::tempdir().expect("temp dir");

        assert!(matches!(
            read_tracker(root.path()),
            Err(JeveError::Io { .. })
        ));
    }

    #[test]
    fn an_owner_that_is_not_a_list_is_not_json() {
        assert!(matches!(
            read_text(r#"{"x":5}"#),
            Err(JeveError::TrackerNotJson { .. })
        ));
    }

    #[test]
    fn a_missing_file_is_none_and_an_empty_object_has_no_records() {
        let root = tempfile::tempdir().expect("temp dir");
        assert_eq!(
            read_tracker(&root.path().join("missing.json")).expect("read"),
            None
        );
        assert_eq!(read_text("{}").expect("read"), Some(Vec::new()));
    }

    #[test]
    fn reading_leaves_the_fixture_bytes_unchanged() {
        for name in ["filters.json", "totals.json"] {
            let path = fixture(name);
            let before = fs::read(&path).expect("fixture");

            read_tracker(&path).expect("read");

            assert_eq!(fs::read(&path).expect("fixture"), before, "{name}");
        }
    }

    #[test]
    fn owner_keys_that_are_not_ids_are_names() {
        let text =
            format!(r#"{{"12ab":[{POINT}],"99999999999999999999":[{POINT}],"7":[{POINT}]}}"#);

        let keys: Vec<CharacterKey> = records(&text)
            .into_iter()
            .map(|record| record.character)
            .collect();

        assert_eq!(
            keys,
            vec![
                CharacterKey::Name("12ab".to_owned()),
                CharacterKey::Name("99999999999999999999".to_owned()),
                CharacterKey::Id(7),
            ]
        );
    }

    #[test]
    fn owner_keys_with_a_sign_or_nothing_are_names() {
        let text = format!(r#"{{"+5":[{POINT}],"":[{POINT}]}}"#);

        let keys: Vec<CharacterKey> = records(&text)
            .into_iter()
            .map(|record| record.character)
            .collect();

        assert_eq!(
            keys,
            vec![
                CharacterKey::Name("+5".to_owned()),
                CharacterKey::Name(String::new()),
            ]
        );
    }

    #[test]
    fn owners_come_out_in_file_order() {
        let text = format!(r#"{{"b":[{POINT}],"2":[{POINT}],"a":[{POINT}]}}"#);

        let keys: Vec<CharacterKey> = records(&text)
            .into_iter()
            .map(|record| record.character)
            .collect();

        assert_eq!(
            keys,
            vec![
                CharacterKey::Name("b".to_owned()),
                CharacterKey::Id(2),
                CharacterKey::Name("a".to_owned()),
            ]
        );
    }

    #[test]
    fn tracker_path_is_inside_the_data_folder() {
        assert_eq!(
            tracker_path(Path::new("/home/u/.jeveassets")),
            PathBuf::from("/home/u/.jeveassets/data/tracker.json")
        );
    }
}

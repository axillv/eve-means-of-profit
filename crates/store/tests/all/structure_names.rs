use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use mop_store::{Store, StoreError};
use rusqlite::Connection;

#[cfg(test)]
mod tests {
    use super::*;

    const RAITARU: i64 = 1_035_466_617_946;
    const RAITARU_2: i64 = 1_035_466_617_947;

    fn open_temp() -> (tempfile::TempDir, PathBuf, Store) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("eve.db");
        let store = Store::open(&path, 1_000).expect("open").store;
        (dir, path, store)
    }

    /// The stored `set_at` of a structure, read straight from the file.
    fn set_at_of(path: &Path, id: i64) -> Option<i64> {
        let conn = Connection::open(path).expect("open");
        conn.query_row(
            "SELECT set_at FROM structure_names WHERE structure_id = ?1",
            [id],
            |row| row.get(0),
        )
        .ok()
    }

    fn row_count(path: &Path) -> i64 {
        let conn = Connection::open(path).expect("open");
        conn.query_row("SELECT count(*) FROM structure_names", [], |row| row.get(0))
            .expect("count")
    }

    #[test]
    fn a_new_file_has_no_names_and_no_version_number() {
        let (_dir, path, _store) = open_temp();
        let conn = Connection::open(&path).expect("open");
        let version: u32 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("version");
        assert_eq!(version, 0);
        assert_eq!(row_count(&path), 0);
    }

    #[test]
    fn a_name_is_trimmed_and_a_second_one_replaces_it() {
        let (_dir, path, mut store) = open_temp();

        store
            .set_structure_name(RAITARU, "  Home Raitaru  ", 1_792_404_000)
            .expect("set");
        assert_eq!(
            store.structure_name(RAITARU).expect("read"),
            Some("Home Raitaru".to_owned())
        );
        assert_eq!(set_at_of(&path, RAITARU), Some(1_792_404_000));

        store
            .set_structure_name(RAITARU, "Raitaru 2", 1_792_404_100)
            .expect("replace");
        assert_eq!(
            store.structure_name(RAITARU).expect("read"),
            Some("Raitaru 2".to_owned())
        );
        assert_eq!(row_count(&path), 1);
        assert_eq!(set_at_of(&path, RAITARU), Some(1_792_404_100));
    }

    #[test]
    fn a_blank_name_removes_the_name() {
        let (_dir, _path, mut store) = open_temp();
        store
            .set_structure_name(RAITARU, "Home Raitaru", 1)
            .expect("set");

        store.set_structure_name(RAITARU, "   ", 1).expect("clear");
        assert_eq!(store.structure_name(RAITARU).expect("read"), None);

        store
            .set_structure_name(RAITARU, "   ", 1)
            .expect("clear again");
        assert_eq!(store.structure_name(RAITARU).expect("read"), None);
    }

    #[test]
    fn a_hundred_characters_are_kept_and_one_hundred_and_one_are_refused() {
        let (_dir, _path, mut store) = open_temp();
        let longest = "Ж".repeat(100);
        store.set_structure_name(RAITARU, &longest, 1).expect("set");
        assert_eq!(store.structure_name(RAITARU).expect("read"), Some(longest));

        let too_long = "a".repeat(101);
        assert!(matches!(
            store.set_structure_name(RAITARU, &too_long, 2),
            Err(StoreError::TooLarge)
        ));
        assert_eq!(
            store.structure_name(RAITARU).expect("read"),
            Some("Ж".repeat(100))
        );
    }

    #[test]
    fn only_structure_ids_can_be_named() {
        let (_dir, path, mut store) = open_temp();

        assert!(matches!(
            store.set_structure_name(60_003_760, "Jita", 1),
            Err(StoreError::NotAStructure(60_003_760))
        ));
        assert!(matches!(
            store.set_structure_name(999_999_999_999, "Edge", 1),
            Err(StoreError::NotAStructure(999_999_999_999))
        ));
        assert_eq!(store.structure_name(60_003_760).expect("read"), None);
        assert_eq!(row_count(&path), 0);

        store
            .set_structure_name(1_000_000_000_000, "Edge", 1)
            .expect("the first structure id is accepted");
        assert_eq!(
            store.structure_name(1_000_000_000_000).expect("read"),
            Some("Edge".to_owned())
        );
    }

    #[test]
    fn every_name_is_listed_in_id_order_and_kept_across_reopening() {
        let (_dir, path, mut store) = open_temp();
        store
            .set_structure_name(RAITARU_2, "Second", 1)
            .expect("set");
        store
            .set_structure_name(RAITARU, "Home Raitaru", 1)
            .expect("set");
        drop(store);

        let mut reopened = Store::open(&path, 5_000).expect("reopen").store;
        let expected = BTreeMap::from([
            (RAITARU, "Home Raitaru".to_owned()),
            (RAITARU_2, "Second".to_owned()),
        ]);
        assert_eq!(reopened.structure_names().expect("list"), expected);
        assert_eq!(
            reopened
                .structure_names()
                .expect("list")
                .keys()
                .collect::<Vec<_>>(),
            vec![&RAITARU, &RAITARU_2]
        );
    }
}

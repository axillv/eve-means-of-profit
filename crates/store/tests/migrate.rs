use std::path::Path;

use profit_store::Store;
use rusqlite::Connection;

#[cfg(test)]
mod tests {
    use super::*;

    fn version_of(path: &Path) -> u32 {
        let conn = Connection::open(path).expect("open");
        conn.pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("version")
    }

    fn migration_rows(path: &Path) -> Vec<(u32, String, i64)> {
        let conn = Connection::open(path).expect("open");
        let mut stmt = conn
            .prepare("SELECT version, name, applied_at FROM migrations ORDER BY version")
            .expect("prepare");
        stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .expect("query")
            .collect::<rusqlite::Result<_>>()
            .expect("rows")
    }

    #[test]
    fn new_file_is_at_version_one_with_no_backup() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("eve.db");

        drop(Store::open(&path, 1_000).expect("open"));

        assert_eq!(version_of(&path), 1);
        assert_eq!(
            migration_rows(&path),
            vec![(1, "initial".to_owned(), 1_000)]
        );
        assert_eq!(std::fs::read_dir(dir.path()).expect("list").count(), 1);
    }

    #[test]
    fn reopening_applies_nothing_and_writes_no_backup() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("eve.db");
        drop(Store::open(&path, 1_000).expect("first open"));
        let before = std::fs::read(&path).expect("read");

        drop(Store::open(&path, 2_000).expect("second open"));

        assert_eq!(
            migration_rows(&path),
            vec![(1, "initial".to_owned(), 1_000)]
        );
        assert_eq!(std::fs::read(&path).expect("read"), before);
        assert_eq!(std::fs::read_dir(dir.path()).expect("list").count(), 1);
    }

    #[test]
    fn newer_schema_is_refused_and_the_file_is_untouched() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("eve.db");
        drop(Store::open(&path, 1_000).expect("open"));
        Connection::open(&path)
            .expect("open raw")
            .pragma_update(None, "user_version", 99)
            .expect("set version");
        let before = std::fs::read(&path).expect("read");

        let err = Store::open(&path, 2_000).expect_err("must refuse");

        assert!(matches!(
            err,
            profit_store::StoreError::NewerSchema {
                found: 99,
                known: 1
            }
        ));
        assert_eq!(std::fs::read(&path).expect("read"), before);
    }
}

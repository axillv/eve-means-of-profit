#[cfg(test)]
mod tests {
    use mop_store::{Store, StoreError};
    use rusqlite::Connection;
    use rusqlite::types::FromSql;
    use std::path::{Path, PathBuf};

    fn raw(path: &Path) -> Connection {
        Connection::open(path).expect("open")
    }

    fn scalar<T: FromSql>(path: &Path, sql: &str) -> T {
        raw(path)
            .query_row(sql, [], |row| row.get(0))
            .expect("scalar")
    }

    fn table_exists(path: &Path, name: &str) -> bool {
        let found: i64 = scalar(
            path,
            &format!("SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = '{name}'"),
        );
        found == 1
    }

    fn aside_of(path: &Path) -> PathBuf {
        let mut aside = path.as_os_str().to_owned();
        aside.push(".old-test");
        PathBuf::from(aside)
    }

    /// The layout text a new file holds, read from a file made in a folder of its own.
    fn layout_of_a_new_file() -> String {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("reference.sqlite");
        drop(Store::open(&path, 1_000).expect("open").store);
        scalar(&path, "SELECT layout FROM meta WHERE id = 1")
    }

    /// A file as an earlier test build wrote it: a migrations table, version 8 and one plan.
    fn earlier_test_build_file(path: &Path) {
        raw(path)
            .execute_batch(
                "CREATE TABLE migrations (
                    version INTEGER PRIMARY KEY, name TEXT NOT NULL, applied_at INTEGER NOT NULL
                );
                INSERT INTO migrations (version, name, applied_at) VALUES (1, 'initial', 1000);
                CREATE TABLE plans (id INTEGER PRIMARY KEY, name TEXT NOT NULL);
                INSERT INTO plans (id, name) VALUES (1, 'Hobgoblin 10');
                PRAGMA user_version = 8;",
            )
            .expect("earlier file");
    }

    #[test]
    fn a_new_file_gets_the_layout_and_nothing_is_set_aside() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("eve.db");

        let opened = Store::open(&path, 1_000).expect("open");

        assert_eq!(opened.started_fresh, None);
        drop(opened);
        assert_eq!(scalar::<i64>(&path, "SELECT count(*) FROM meta"), 1);
        assert_eq!(
            scalar::<String>(&path, "SELECT layout FROM meta"),
            layout_of_a_new_file()
        );
        assert_eq!(scalar::<i64>(&path, "SELECT created_at FROM meta"), 1_000);
        assert_eq!(scalar::<u32>(&path, "PRAGMA user_version"), 0);
        assert!(!table_exists(&path, "migrations"));
        assert!(!aside_of(&path).exists());
        assert_eq!(std::fs::read_dir(dir.path()).expect("list").count(), 1);
    }

    #[test]
    fn a_reopened_file_is_opened_as_it_is_and_keeps_what_was_saved() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("eve.db");
        let mut store = Store::open(&path, 1_000).expect("open").store;
        store
            .set_setting("columns.build_buy_list", "volume,share")
            .expect("set");
        drop(store);

        let reopened = Store::open(&path, 2_000).expect("reopen");

        assert_eq!(reopened.started_fresh, None);
        assert_eq!(
            reopened
                .store
                .setting("columns.build_buy_list")
                .expect("setting"),
            Some("volume,share".to_owned())
        );
        assert_eq!(scalar::<i64>(&path, "SELECT created_at FROM meta"), 1_000);
        assert!(!aside_of(&path).exists());
    }

    #[test]
    fn an_empty_file_is_treated_as_a_new_one() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("eve.db");
        std::fs::write(&path, b"").expect("write");

        let opened = Store::open(&path, 1_000).expect("open");

        assert_eq!(opened.started_fresh, None);
        drop(opened);
        assert_eq!(scalar::<i64>(&path, "SELECT count(*) FROM meta"), 1);
        assert!(!aside_of(&path).exists());
    }

    #[test]
    fn an_earlier_test_build_file_is_set_aside_unchanged_and_a_new_one_starts() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("eve.db");
        earlier_test_build_file(&path);
        let before = std::fs::read(&path).expect("read");

        let opened = Store::open(&path, 2_000).expect("open");

        assert_eq!(opened.started_fresh, Some(aside_of(&path)));
        drop(opened);
        assert_eq!(std::fs::read(aside_of(&path)).expect("old file"), before);
        assert_eq!(scalar::<i64>(&path, "SELECT count(*) FROM plans"), 0);
        assert_eq!(
            scalar::<String>(&path, "SELECT layout FROM meta"),
            layout_of_a_new_file()
        );
        assert!(!table_exists(&path, "migrations"));
    }

    #[test]
    fn a_second_earlier_file_replaces_the_one_set_aside_before() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("eve.db");
        earlier_test_build_file(&path);
        drop(Store::open(&path, 2_000).expect("first"));
        std::fs::remove_file(&path).expect("remove");
        earlier_test_build_file(&path);
        raw(&path)
            .execute("INSERT INTO plans (id, name) VALUES (2, 'Avatar 1')", [])
            .expect("second plan");
        let second = std::fs::read(&path).expect("read");

        let opened = Store::open(&path, 3_000).expect("second");

        assert_eq!(opened.started_fresh, Some(aside_of(&path)));
        drop(opened);
        assert_eq!(std::fs::read(aside_of(&path)).expect("old file"), second);
    }

    #[test]
    fn a_layout_that_differs_by_one_character_is_set_aside() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("eve.db");
        drop(Store::open(&path, 1_000).expect("open"));
        raw(&path)
            .execute("UPDATE meta SET layout = layout || ' '", [])
            .expect("change layout");
        let before = std::fs::read(&path).expect("read");

        let opened = Store::open(&path, 2_000).expect("open");

        assert_eq!(opened.started_fresh, Some(aside_of(&path)));
        drop(opened);
        assert_eq!(std::fs::read(aside_of(&path)).expect("old file"), before);
    }

    #[test]
    fn a_meta_table_of_another_shape_is_set_aside() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("eve.db");
        raw(&path)
            .execute_batch("CREATE TABLE meta (note TEXT); INSERT INTO meta VALUES ('x');")
            .expect("old file");

        let opened = Store::open(&path, 2_000).expect("open");

        assert_eq!(opened.started_fresh, Some(aside_of(&path)));
    }

    #[test]
    fn a_file_that_is_not_a_database_is_refused_and_left_as_it_is() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("eve.db");
        std::fs::write(&path, b"hello").expect("write");

        let err = Store::open(&path, 1_000).expect_err("must refuse");

        assert!(matches!(err, StoreError::Sqlite(_)), "{err}");
        assert_eq!(std::fs::read(&path).expect("read"), b"hello");
        assert!(!aside_of(&path).exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_folder_that_cannot_be_written_gives_an_io_error() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("tempdir");
        let folder = dir.path().join("locked");
        std::fs::create_dir(&folder).expect("folder");
        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o555)).expect("lock");

        let err = Store::open(&folder.join("eve.db"), 1_000).expect_err("must refuse");

        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o755)).expect("unlock");
        assert!(matches!(err, StoreError::Io { .. }), "{err}");
    }

    #[test]
    fn a_path_below_a_plain_file_gives_an_io_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("plain");
        std::fs::write(&file, b"hello").expect("file");

        let err = Store::open(&file.join("eve.db"), 1_000).expect_err("must refuse");

        assert!(matches!(err, StoreError::Io { .. }), "{err}");
        assert_eq!(std::fs::read(&file).expect("read"), b"hello");
    }
}

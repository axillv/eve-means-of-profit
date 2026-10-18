#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::path::PathBuf;

    use profit_jeve::JeveError;

    #[test]
    fn messages_name_the_problem() {
        let cases = [
            (JeveError::Busy, "the profile is locked"),
            (
                JeveError::NotJeve {
                    path: PathBuf::from("/p/#Default.db"),
                },
                "/p/#Default.db is not a SQLite database",
            ),
            (
                JeveError::Layout {
                    table: "assets",
                    column: Some("rawquantity"),
                },
                "the assets table has no rawquantity column",
            ),
            (
                JeveError::Layout {
                    table: "blueprints",
                    column: None,
                },
                "there is no blueprints table",
            ),
            (
                JeveError::BadValue {
                    table: "assets",
                    column: "typeid",
                    item_id: 1001,
                },
                "the typeid value in the assets table, row 1001",
            ),
            (
                JeveError::TooManyRows { table: "assets" },
                "the assets table has more rows",
            ),
        ];
        for (error, expected) in cases {
            assert!(
                error.to_string().contains(expected),
                "{error} does not contain {expected:?}"
            );
        }
    }

    #[test]
    fn io_message_names_the_path_and_the_cause() {
        let error = JeveError::Io {
            path: PathBuf::from("/p/#Default.db"),
            source: std::io::Error::other("denied"),
        };

        assert_eq!(error.to_string(), "could not read /p/#Default.db: denied");
    }

    #[test]
    fn only_io_and_sqlite_errors_have_a_cause() {
        let io = JeveError::Io {
            path: PathBuf::new(),
            source: std::io::Error::other("denied"),
        };
        assert!(io.source().is_some());
        assert!(
            JeveError::from(rusqlite::Error::QueryReturnedNoRows)
                .source()
                .is_some()
        );
        assert!(JeveError::Busy.source().is_none());
    }
}

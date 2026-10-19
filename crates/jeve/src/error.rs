use std::fmt;
use std::path::PathBuf;

/// Why a jEveAssets profile could not be listed or read.
#[derive(Debug)]
pub enum JeveError {
    /// A file or folder could not be read.
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    /// The file is not a SQLite database.
    NotJeve { path: PathBuf },
    /// Another program holds the file; jEveAssets may be writing it.
    Busy,
    /// The profile lacks a table, or a column in a table, that this app reads.
    Layout {
        table: &'static str,
        column: Option<&'static str>,
    },
    /// A value is empty or outside what this app accepts.
    BadValue {
        table: &'static str,
        column: &'static str,
        item_id: i64,
    },
    /// A table has more rows than the counts in this app can hold.
    TooManyRows { table: &'static str },
    /// A point of the Tracker history has a missing, non-numeric or out-of-range value.
    TrackerBadData {
        owner: String,
        index: usize,
        field: &'static str,
    },
    /// The Tracker file is not JSON, or not the object-of-lists shape jEveAssets writes.
    TrackerNotJson { reason: String },
    /// Any other SQLite error.
    Sqlite(rusqlite::Error),
}

impl fmt::Display for JeveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "could not read {}: {source}", path.display()),
            Self::NotJeve { path } => write!(
                f,
                "{} is not a SQLite database, so it is not a jEveAssets profile",
                path.display()
            ),
            Self::Busy => write!(
                f,
                "the profile is locked, so jEveAssets may be writing it; try again in a moment"
            ),
            Self::Layout {
                table,
                column: Some(column),
            } => write!(
                f,
                "this app does not know this jEveAssets profile layout: the {table} table has no {column} column"
            ),
            Self::Layout {
                table,
                column: None,
            } => write!(
                f,
                "this app does not know this jEveAssets profile layout: there is no {table} table"
            ),
            Self::BadValue {
                table,
                column,
                item_id,
            } => write!(
                f,
                "the {column} value in the {table} table, row {item_id}, is empty or not one this app accepts"
            ),
            Self::TooManyRows { table } => {
                write!(f, "the {table} table has more rows than this app can count")
            }
            Self::TrackerBadData {
                owner,
                index,
                field,
            } => write!(
                f,
                "the Tracker history of {owner} has a missing, non-numeric or out-of-range {field} value at point {}",
                // The index is below a list length, which is at most isize::MAX, so index + 1 cannot saturate.
                index.saturating_add(1)
            ),
            Self::TrackerNotJson { reason } => write!(
                f,
                "the Tracker file is not in the format this app reads: {reason}"
            ),
            Self::Sqlite(error) => write!(f, "could not read the profile: {error}"),
        }
    }
}

impl std::error::Error for JeveError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Sqlite(error) => Some(error),
            Self::NotJeve { .. }
            | Self::Busy
            | Self::Layout { .. }
            | Self::BadValue { .. }
            | Self::TooManyRows { .. }
            | Self::TrackerBadData { .. }
            | Self::TrackerNotJson { .. } => None,
        }
    }
}

impl From<rusqlite::Error> for JeveError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error)
    }
}

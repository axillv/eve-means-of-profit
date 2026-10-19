//! What a store call can fail with.

use std::fmt;
use std::path::PathBuf;

pub type Result<T> = std::result::Result<T, StoreError>;

#[derive(Debug)]
pub enum StoreError {
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    Sqlite(rusqlite::Error),
    /// The file was written by a later version of the app; it is left untouched.
    NewerSchema {
        found: u32,
        known: u32,
    },
    Migration {
        version: u32,
        message: String,
    },
    EmptyName,
    NameTaken(String),
    RouteExists,
    /// The names of the plans that still use the item.
    InUse {
        plans: Vec<String>,
    },
    NotFound,
    TooLarge,
    /// The id is not a structure, so it cannot take a name the user typed.
    NotAStructure(i64),
    /// A stockpile the store will not keep, with the reason the user can read.
    InvalidStockpile(String),
    /// The names along an include loop, the first one repeated at the end.
    StockpileLoop(Vec<String>),
    /// A stored value that Rust cannot turn back into its type.
    BadData(String),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::Sqlite(err) => write!(f, "database error: {err}"),
            Self::NewerSchema { found, known } => write!(
                f,
                "the file is from a newer version (schema {found}, this version knows {known})"
            ),
            Self::Migration { version, message } => {
                write!(f, "upgrade to version {version} failed: {message}")
            }
            Self::EmptyName => write!(f, "the name is empty"),
            Self::NameTaken(name) => write!(f, "the name \"{name}\" is already used"),
            Self::RouteExists => write!(f, "that route already exists"),
            Self::InUse { plans } => write!(f, "still used by: {}", plans.join(", ")),
            Self::NotFound => write!(f, "not found"),
            Self::TooLarge => write!(f, "the number is too large to store"),
            Self::NotAStructure(id) => write!(f, "{id} is not a structure"),
            Self::InvalidStockpile(reason) => write!(f, "{reason}"),
            Self::StockpileLoop(names) => {
                write!(f, "includes itself: {}", names.join(" → "))
            }
            Self::BadData(what) => write!(f, "stored data cannot be read: {what}"),
        }
    }
}

impl std::error::Error for StoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Sqlite(err) => Some(err),
            Self::NewerSchema { .. }
            | Self::Migration { .. }
            | Self::EmptyName
            | Self::NameTaken(_)
            | Self::RouteExists
            | Self::InUse { .. }
            | Self::NotFound
            | Self::TooLarge
            | Self::NotAStructure(_)
            | Self::InvalidStockpile(_)
            | Self::StockpileLoop(_)
            | Self::BadData(_) => None,
        }
    }
}

impl From<rusqlite::Error> for StoreError {
    fn from(err: rusqlite::Error) -> Self {
        Self::Sqlite(err)
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use super::*;

    #[test]
    fn messages_name_the_thing_that_failed() {
        let taken = StoreError::NameTaken("Hobgoblin 10".to_owned()).to_string();
        assert!(taken.contains("Hobgoblin 10"), "{taken}");
        let in_use = StoreError::InUse {
            plans: vec!["Avatar 1".to_owned(), "Hobgoblin 10".to_owned()],
        }
        .to_string();
        assert_eq!(in_use, "still used by: Avatar 1, Hobgoblin 10");
        let newer = StoreError::NewerSchema {
            found: 99,
            known: 1,
        }
        .to_string();
        assert!(newer.contains("99") && newer.contains('1'), "{newer}");
    }

    #[test]
    fn database_errors_keep_their_cause() {
        let cause = rusqlite::Error::InvalidQuery;
        let err = StoreError::from(cause);
        assert!(err.source().is_some());
        assert!(StoreError::NotFound.source().is_none());
    }
}

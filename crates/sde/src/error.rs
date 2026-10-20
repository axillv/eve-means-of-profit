use std::fmt;
use std::path::PathBuf;

use profit_core::industry::TypeId;

/// Why the static data could not be imported or read.
///
/// Errors from a source file name the file, and the line where there is one. Lines count from 1.
#[derive(Debug)]
pub enum SdeError {
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    Zip(zip::result::ZipError),
    /// The source has no file with this name.
    MissingFile(String),
    Json {
        file: String,
        line: usize,
        message: String,
    },
    MissingField {
        file: String,
        line: usize,
        field: String,
    },
    MissingEnglishName {
        file: String,
        line: usize,
    },
    BadNumber {
        file: String,
        line: usize,
        field: String,
        text: String,
    },
    /// A blueprint refers to a type that the source does not list.
    UnknownType {
        file: String,
        line: usize,
        type_id: TypeId,
    },
    /// A type belongs to a group that the source does not list.
    UnknownGroup {
        file: String,
        line: usize,
        group_id: u32,
    },
    /// A map record refers to a system or region that the download does not list.
    BadReference {
        file: String,
        line: usize,
        field: String,
        id: u32,
    },
    /// A structure or rig is in the download's lists but has no record in the attribute file.
    NoDogma {
        file: String,
        type_id: TypeId,
    },
    /// A decryptor has no record for one of its four attributes.
    IncompleteDecryptor {
        type_id: TypeId,
        attribute: u32,
    },
    /// A number is too large to store in the database.
    TooLarge {
        value: u64,
    },
    Sqlite(rusqlite::Error),
    /// The database is missing, empty, or was written with another schema version.
    NotImported,
}

impl fmt::Display for SdeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "cannot read {}: {source}", path.display()),
            Self::Zip(error) => write!(f, "cannot read the zip archive: {error}"),
            Self::MissingFile(name) => write!(f, "the source has no {name}"),
            Self::Json {
                file,
                line,
                message,
            } => write!(f, "{file} line {line}: {message}"),
            Self::MissingField { file, line, field } => {
                write!(f, "{file} line {line}: missing {field}")
            }
            Self::MissingEnglishName { file, line } => {
                write!(f, "{file} line {line}: missing name.en")
            }
            Self::BadNumber {
                file,
                line,
                field,
                text,
            } => write!(
                f,
                "{file} line {line}: {field} is not an exact number: {text}"
            ),
            Self::UnknownType {
                file,
                line,
                type_id,
            } => write!(
                f,
                "{file} line {line}: type {type_id} is not in types.jsonl"
            ),
            Self::UnknownGroup {
                file,
                line,
                group_id,
            } => write!(
                f,
                "{file} line {line}: group {group_id} is not in groups.jsonl"
            ),
            Self::BadReference {
                file,
                line,
                field,
                id,
            } => write!(f, "{file} line {line}: {field} {id} is not in the download"),
            Self::NoDogma { file, type_id } => {
                write!(f, "{file}: type {type_id} has no attribute record")
            }
            Self::IncompleteDecryptor { type_id, attribute } => {
                write!(f, "decryptor {type_id} has no attribute {attribute}")
            }
            Self::TooLarge { value } => write!(f, "the number {value} is too large to store"),
            Self::Sqlite(error) => write!(f, "database error: {error}"),
            Self::NotImported => f.write_str("the static data is not imported with this version"),
        }
    }
}

impl std::error::Error for SdeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Zip(error) => Some(error),
            Self::Sqlite(error) => Some(error),
            Self::MissingFile(_)
            | Self::Json { .. }
            | Self::MissingField { .. }
            | Self::MissingEnglishName { .. }
            | Self::BadNumber { .. }
            | Self::UnknownType { .. }
            | Self::UnknownGroup { .. }
            | Self::BadReference { .. }
            | Self::NoDogma { .. }
            | Self::IncompleteDecryptor { .. }
            | Self::TooLarge { .. }
            | Self::NotImported => None,
        }
    }
}

impl From<rusqlite::Error> for SdeError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error)
    }
}

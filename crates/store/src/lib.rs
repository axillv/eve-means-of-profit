//! The user's own data: plans, facilities, freight routes, settings, net worth history and
//! stockpiles, in one SQLite file.
//!
//! [`Store::open`] creates the file or upgrades it. Each method runs in one transaction. Names are
//! trimmed and unique ignoring case. Money, volumes and rates are whole numbers.

pub mod costs;
pub mod error;
pub mod facilities;
pub mod freight;
pub mod migrate;
pub mod net_worth;
pub mod plans;
pub mod price_history;
pub mod reservations;
pub mod settings;
pub mod stockpiles;
pub mod structure_names;

use rusqlite::Transaction;

pub use costs::{FacilityCosts, FeeSettings};
pub use error::{Result, StoreError};
pub use facilities::FacilityRow;
pub use freight::{RouteRow, ServiceRow};
pub use migrate::Store;
pub use net_worth::{ImportReport, NetWorthSettings, PriceHistory};
pub use plans::{PlanRecord, PlanSummary, SellMode};
pub use reservations::PlanStock;
pub(crate) use reservations::copy_scope;
pub use settings::{Density, Settings};

/// Trims a name and refuses an empty one.
pub(crate) fn clean_name(name: &str) -> Result<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(StoreError::EmptyName);
    }
    Ok(trimmed.to_owned())
}

/// True when another row (not `except`) already has this name, ignoring case.
pub(crate) fn is_taken(taken: &[(i64, String)], name: &str, except: Option<i64>) -> bool {
    let key = name.to_lowercase();
    taken
        .iter()
        .any(|(id, other)| Some(*id) != except && other.to_lowercase() == key)
}

/// Refuses `name` when another row already has it.
pub(crate) fn ensure_free(taken: &[(i64, String)], name: &str, except: Option<i64>) -> Result<()> {
    if is_taken(taken, name, except) {
        return Err(StoreError::NameTaken(name.to_owned()));
    }
    Ok(())
}

/// The (id, name) pairs a query returns, for name checks and lists.
pub(crate) fn id_names(
    tx: &Transaction<'_>,
    sql: &str,
    params: impl rusqlite::Params,
) -> Result<Vec<(i64, String)>> {
    let mut stmt = tx.prepare(sql)?;
    let rows = stmt.query_map(params, |row| Ok((row.get(0)?, row.get(1)?)))?;
    rows.collect::<rusqlite::Result<_>>()
        .map_err(StoreError::from)
}

/// A foreign key that points at nothing: the row is refused, which the caller reports as not found.
pub(crate) fn missing_reference(err: rusqlite::Error) -> StoreError {
    let missing = matches!(
        &err,
        rusqlite::Error::SqliteFailure(failure, _)
            if failure.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_FOREIGNKEY
    );
    if missing {
        StoreError::NotFound
    } else {
        StoreError::from(err)
    }
}

/// Reads a stored number as the type it belongs to; a value that does not fit is bad data.
pub(crate) fn from_stored<T: TryFrom<i64>>(
    value: i64,
    table: &str,
    row: i64,
    column: &str,
) -> Result<T> {
    T::try_from(value).ok().ok_or_else(|| {
        StoreError::BadData(format!(
            "{table} row {row}, column {column}: {value} is out of range"
        ))
    })
}

/// A stored code that names no value of its type.
pub(crate) fn bad_code(table: &str, row: i64, column: &str, code: i64) -> StoreError {
    StoreError::BadData(format!(
        "{table} row {row}, column {column}: unknown code {code}"
    ))
}

/// A count as the database stores it. Anything above the largest stored integer is refused.
pub(crate) fn to_stored_u64(value: u64) -> Result<i64> {
    i64::try_from(value).ok().ok_or(StoreError::TooLarge)
}

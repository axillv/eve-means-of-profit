//! Opens the database file and brings its schema up to date. Each upgrade is its own transaction.
//!
//! The data is the user's own and cannot be fetched again, so before an upgrade the file is
//! copied aside as `<file>.before-v<version>`.

use std::path::{Path, PathBuf};

use rusqlite::{Connection, params};

use crate::error::{Result, StoreError};

/// Every schema change, oldest first: (version, name, SQL). Version 1 creates the first schema and
/// version 2 the net worth history and version 3 the structure names the user typed.
pub(crate) const MIGRATIONS: &[(u32, &str, &str)] = &[
    (1, "initial", INITIAL_SCHEMA),
    (2, "net worth", NET_WORTH_SCHEMA),
    (3, "structure names", STRUCTURE_NAMES_SCHEMA),
];

const INITIAL_SCHEMA: &str = "
CREATE TABLE facilities (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL CHECK (length(name) > 0),
    structure INTEGER CHECK (structure >= 0),
    security INTEGER NOT NULL
);
CREATE TABLE facility_rigs (
    facility_id INTEGER NOT NULL REFERENCES facilities (id) ON DELETE CASCADE,
    position INTEGER NOT NULL CHECK (position >= 0),
    type_id INTEGER NOT NULL CHECK (type_id >= 0),
    PRIMARY KEY (facility_id, position)
);
CREATE TABLE routes (
    id INTEGER PRIMARY KEY,
    from_system INTEGER NOT NULL CHECK (from_system >= 0),
    to_system INTEGER NOT NULL CHECK (to_system >= 0),
    UNIQUE (from_system, to_system)
);
CREATE TABLE services (
    id INTEGER PRIMARY KEY,
    route_id INTEGER NOT NULL REFERENCES routes (id) ON DELETE CASCADE,
    name TEXT NOT NULL CHECK (length(name) > 0),
    per_m3 INTEGER CHECK (per_m3 >= 0),
    per_jump INTEGER CHECK (per_jump >= 0),
    collateral_ppm INTEGER CHECK (collateral_ppm >= 0),
    minimum INTEGER CHECK (minimum >= 0),
    max_m3 INTEGER CHECK (max_m3 >= 0),
    max_collateral INTEGER CHECK (max_collateral >= 0),
    route_kind INTEGER NOT NULL,
    jumps_override INTEGER CHECK (jumps_override >= 0)
);
CREATE TABLE plans (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL CHECK (length(name) > 0),
    product INTEGER NOT NULL CHECK (product >= 0),
    quantity INTEGER NOT NULL CHECK (quantity >= 1),
    manufacturing INTEGER REFERENCES facilities (id),
    reactions INTEGER REFERENCES facilities (id),
    price_rule INTEGER NOT NULL,
    route INTEGER REFERENCES routes (id),
    service INTEGER REFERENCES services (id) ON DELETE SET NULL,
    packing INTEGER NOT NULL,
    pasted_stock TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE TABLE plan_build (
    plan_id INTEGER NOT NULL REFERENCES plans (id) ON DELETE CASCADE,
    type_id INTEGER NOT NULL CHECK (type_id >= 0),
    PRIMARY KEY (plan_id, type_id)
);
CREATE TABLE plan_me (
    plan_id INTEGER NOT NULL REFERENCES plans (id) ON DELETE CASCADE,
    type_id INTEGER NOT NULL CHECK (type_id >= 0),
    me INTEGER NOT NULL CHECK (me BETWEEN 0 AND 10),
    PRIMARY KEY (plan_id, type_id)
);
CREATE TABLE settings (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    default_price_rule INTEGER NOT NULL,
    density INTEGER NOT NULL,
    last_sde_check INTEGER
);
";

const NET_WORTH_SCHEMA: &str = "
CREATE TABLE item_sets (
    id INTEGER PRIMARY KEY
);
CREATE TABLE item_set_items (
    set_id INTEGER NOT NULL REFERENCES item_sets (id),
    type_id INTEGER NOT NULL CHECK (type_id >= 0),
    units INTEGER NOT NULL CHECK (units >= 0),
    PRIMARY KEY (set_id, type_id)
);
CREATE TABLE net_worth_records (
    id INTEGER PRIMARY KEY,
    character_id INTEGER CHECK (character_id >= 0),
    character_name TEXT CHECK (length(character_name) > 0),
    at INTEGER NOT NULL,
    kind INTEGER NOT NULL CHECK (kind BETWEEN 0 AND 2),
    wallet INTEGER CHECK (wallet >= 0),
    sell_orders INTEGER CHECK (sell_orders >= 0),
    buy_escrow INTEGER CHECK (buy_escrow >= 0),
    assets_set INTEGER REFERENCES item_sets (id),
    jobs_set INTEGER REFERENCES item_sets (id),
    CHECK ((character_id IS NULL) <> (character_name IS NULL))
);
CREATE INDEX net_worth_records_by_at ON net_worth_records (at);
CREATE INDEX net_worth_records_by_character ON net_worth_records (character_id, character_name, at);
CREATE TABLE net_worth_imported (
    record_id INTEGER NOT NULL REFERENCES net_worth_records (id),
    part INTEGER NOT NULL CHECK (part BETWEEN 0 AND 7),
    amount INTEGER NOT NULL CHECK (amount >= 0),
    PRIMARY KEY (record_id, part)
) WITHOUT ROWID;
CREATE TABLE item_prices (
    day INTEGER NOT NULL,
    type_id INTEGER NOT NULL CHECK (type_id >= 0),
    lowest_sell INTEGER CHECK (lowest_sell >= 0),
    highest_buy INTEGER CHECK (highest_buy >= 0),
    average INTEGER CHECK (average >= 0),
    PRIMARY KEY (day, type_id)
) WITHOUT ROWID;
CREATE INDEX item_prices_by_type ON item_prices (type_id, day);
CREATE TABLE net_worth_settings (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    basis INTEGER NOT NULL CHECK (basis BETWEEN 0 AND 3)
);
CREATE TABLE net_worth_shown (
    id INTEGER PRIMARY KEY,
    part INTEGER NOT NULL UNIQUE CHECK (part BETWEEN 0 AND 7)
);
";

const STRUCTURE_NAMES_SCHEMA: &str = "
CREATE TABLE structure_names (
    structure_id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    set_at INTEGER NOT NULL
);
";

/// An open database. Every method on it runs in one transaction.
#[derive(Debug)]
pub struct Store {
    pub(crate) conn: Connection,
}

impl Store {
    /// Opens the file at `path`, creating it when missing, and upgrades its schema to the
    /// newest version this app knows. `now` is the Unix time the upgrades are recorded with.
    pub fn open(path: &Path, now: i64) -> Result<Store> {
        open_with(path, now, MIGRATIONS)
    }
}

pub(crate) fn open_with(path: &Path, now: i64, migrations: &[(u32, &str, &str)]) -> Result<Store> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "foreign_keys", true)?;
    let known = migrations.iter().map(|m| m.0).max().unwrap_or(0);
    let found: u32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if found > known {
        return Err(StoreError::NewerSchema { found, known });
    }
    let pending: Vec<_> = migrations.iter().filter(|m| m.0 > found).collect();
    if pending.is_empty() {
        return Ok(Store { conn });
    }
    let mut store = Store { conn };
    if found >= 1 {
        backup(path, known)?;
    }
    for (version, name, sql) in pending {
        apply(&mut store.conn, *version, name, sql, now)?;
    }
    Ok(store)
}

/// Copies the file byte for byte to `<file>.before-v<target>`.
fn backup(path: &Path, target: u32) -> Result<()> {
    let mut copy = path.as_os_str().to_owned();
    copy.push(format!(".before-v{target}"));
    std::fs::copy(path, PathBuf::from(&copy)).map_err(|source| StoreError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(())
}

/// Runs one migration and records it. A failure rolls back that migration only.
fn apply(conn: &mut Connection, version: u32, name: &str, sql: &str, now: i64) -> Result<()> {
    let tx = conn.transaction()?;
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS migrations (
            version INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            applied_at INTEGER NOT NULL
        );",
    )?;
    tx.execute_batch(sql).map_err(|err| StoreError::Migration {
        version,
        message: err.to_string(),
    })?;
    tx.execute(
        "INSERT INTO migrations (version, name, applied_at) VALUES (?1, ?2, ?3)",
        params![version, name, now],
    )?;
    tx.pragma_update(None, "user_version", version)?;
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXTRA: &str = "CREATE TABLE extra (x INTEGER);";
    const BROKEN: &str = "CREATE TABLE broken (;";

    fn version_of(path: &Path) -> u32 {
        let conn = Connection::open(path).expect("open");
        conn.pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("version")
    }

    fn migration_rows(path: &Path) -> i64 {
        let conn = Connection::open(path).expect("open");
        conn.query_row("SELECT count(*) FROM migrations", [], |row| row.get(0))
            .expect("rows")
    }

    /// The (version, name, applied_at) rows of the migrations table, oldest first.
    fn migrations_of(path: &Path) -> Vec<(u32, String, i64)> {
        let conn = Connection::open(path).expect("open");
        conn.prepare("SELECT version, name, applied_at FROM migrations ORDER BY version")
            .expect("prepare")
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .expect("query")
            .collect::<rusqlite::Result<_>>()
            .expect("rows")
    }

    #[test]
    fn upgrade_keeps_a_copy_of_the_old_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("eve.db");
        drop(open_with(&path, 1_000, &[(1, "initial", INITIAL_SCHEMA)]).expect("v1"));
        let before = std::fs::read(&path).expect("read");

        let two: &[(u32, &str, &str)] = &[(1, "initial", INITIAL_SCHEMA), (2, "extra", EXTRA)];
        drop(open_with(&path, 2_000, two).expect("upgrade"));

        let copy = dir.path().join("eve.db.before-v2");
        assert_eq!(std::fs::read(&copy).expect("backup"), before);
        assert_eq!(version_of(&path), 2);
        assert_eq!(migration_rows(&path), 2);
    }

    #[test]
    fn failed_upgrade_keeps_the_old_version_and_its_backup() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("eve.db");
        let mut store = open_with(&path, 1_000, &[(1, "initial", INITIAL_SCHEMA)]).expect("v1");
        let plan = crate::PlanRecord {
            id: 0,
            name: "Hobgoblin 10".to_owned(),
            product: 2456,
            quantity: 10,
            build: std::collections::BTreeSet::from([2454]),
            me: std::collections::BTreeMap::from([(2456, 2)]),
            manufacturing: None,
            reactions: None,
            price_rule: profit_market::book::PriceRule::LowestSell,
            route: None,
            service: None,
            packing: profit_core::freight::Packing::FillOnly,
            pasted_stock: "Tritanium\t1,000".to_owned(),
            created_at: 1_000,
            updated_at: 1_000,
        };
        let id = store.create_plan(&plan, 1_000).expect("plan");
        drop(store);

        let broken: &[(u32, &str, &str)] = &[(1, "initial", INITIAL_SCHEMA), (2, "broken", BROKEN)];
        let err = open_with(&path, 2_000, broken).expect_err("must fail");
        let mut reopened = open_with(&path, 3_000, &[(1, "initial", INITIAL_SCHEMA)]).expect("v1");
        assert_eq!(
            reopened.plan(id).expect("plan"),
            Some(crate::PlanRecord { id, ..plan })
        );
        assert!(matches!(err, StoreError::Migration { version: 2, .. }));
        assert_eq!(version_of(&path), 1);
        assert_eq!(migration_rows(&path), 1);
        assert!(dir.path().join("eve.db.before-v2").exists());
    }

    fn hobgoblin() -> crate::PlanRecord {
        crate::PlanRecord {
            id: 0,
            name: "Hobgoblin 10".to_owned(),
            product: 2456,
            quantity: 10,
            build: std::collections::BTreeSet::from([2454]),
            me: std::collections::BTreeMap::from([(2456, 2)]),
            manufacturing: None,
            reactions: None,
            price_rule: profit_market::book::PriceRule::LowestSell,
            route: None,
            service: None,
            packing: profit_core::freight::Packing::FillOnly,
            pasted_stock: "Tritanium\t1,000".to_owned(),
            created_at: 1_000,
            updated_at: 1_000,
        }
    }

    #[test]
    fn a_version_one_file_is_kept_aside_and_upgraded() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("eve.db");
        let plan = hobgoblin();
        let mut store = open_with(&path, 1_000, &[(1, "initial", INITIAL_SCHEMA)]).expect("v1");
        let id = store.create_plan(&plan, 1_000).expect("plan");
        drop(store);
        let before = std::fs::read(&path).expect("read");

        let mut upgraded = Store::open(&path, 5_000).expect("upgrade");

        assert_eq!(
            std::fs::read(dir.path().join("eve.db.before-v3")).expect("backup"),
            before
        );
        assert_eq!(version_of(&path), 3);
        assert_eq!(
            migrations_of(&path),
            vec![
                (1, "initial".to_owned(), 1_000),
                (2, "net worth".to_owned(), 5_000),
                (3, "structure names".to_owned(), 5_000)
            ]
        );
        assert_eq!(
            upgraded.plan(id).expect("plan"),
            Some(crate::PlanRecord { id, ..plan })
        );
    }

    #[test]
    fn a_version_two_file_is_kept_aside_and_upgraded_to_three() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("eve.db");
        let plan = hobgoblin();
        let two: &[(u32, &str, &str)] = &[
            (1, "initial", INITIAL_SCHEMA),
            (2, "net worth", NET_WORTH_SCHEMA),
        ];
        let mut store = open_with(&path, 1_000, two).expect("v2");
        let id = store.create_plan(&plan, 1_000).expect("plan");
        drop(store);
        let before = std::fs::read(&path).expect("read");
        let backup = dir.path().join("eve.db.before-v3");

        let mut upgraded = Store::open(&path, 1_792_404_000).expect("upgrade");

        assert_eq!(std::fs::read(&backup).expect("backup"), before);
        assert_eq!(version_of(&path), 3);
        assert_eq!(
            migrations_of(&path).last(),
            Some(&(3, "structure names".to_owned(), 1_792_404_000))
        );
        assert_eq!(migrations_of(&path).len(), 3);
        assert_eq!(
            upgraded.plan(id).expect("plan"),
            Some(crate::PlanRecord { id, ..plan })
        );
        drop(upgraded);

        std::fs::remove_file(&backup).expect("remove");
        drop(Store::open(&path, 1_792_404_500).expect("reopen"));
        assert!(!backup.exists());
    }

    #[test]
    fn a_new_file_starts_at_the_newest_version_without_a_backup() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("eve.db");

        drop(Store::open(&path, 1_000).expect("open"));

        assert_eq!(version_of(&path), 3);
        assert!(!dir.path().join("eve.db.before-v3").exists());
    }
}

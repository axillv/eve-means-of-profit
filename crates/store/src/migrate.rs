//! Opens the database file and creates its layout. Until the first release there are no numbered
//! steps: a file whose stored layout differs from [`LAYOUT`] is set aside, not upgraded.

use std::fs::OpenOptions;
use std::io;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, params};

use crate::error::{Result, StoreError};

/// The whole layout of the user database. Before the first release it is edited freely. Every file
/// stores the text it was made with, and [`Store::open`] sets aside any file whose text differs
/// from this one, so an edit here means each earlier file starts fresh once. Numbered steps return
/// at the first release, when this text becomes the first of them.
const LAYOUT: &str = "
CREATE TABLE meta (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    layout TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE TABLE facilities (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL CHECK (length(name) > 0),
    structure INTEGER CHECK (structure >= 0),
    security INTEGER NOT NULL,
    system INTEGER CHECK (system IS NULL OR system > 0),
    tax_ppm INTEGER CHECK (tax_ppm IS NULL OR tax_ppm BETWEEN 0 AND 1000000)
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
    updated_at INTEGER NOT NULL,
    position INTEGER NOT NULL DEFAULT 0,
    reserve INTEGER NOT NULL DEFAULT 0 CHECK (reserve IN (0, 1)),
    stock_source INTEGER NOT NULL DEFAULT 0 CHECK (stock_source IN (0, 1)),
    sell_mode INTEGER NOT NULL DEFAULT 0 CHECK (sell_mode IN (0, 1))
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
CREATE TABLE plan_stock_characters (
    plan_id INTEGER NOT NULL REFERENCES plans (id) ON DELETE CASCADE,
    character_id INTEGER NOT NULL CHECK (character_id > 0),
    PRIMARY KEY (plan_id, character_id)
) WITHOUT ROWID;
CREATE TABLE plan_stock_places (
    plan_id INTEGER NOT NULL REFERENCES plans (id) ON DELETE CASCADE,
    location_id INTEGER NOT NULL,
    PRIMARY KEY (plan_id, location_id)
) WITHOUT ROWID;
CREATE TABLE plan_manual_prices (
    plan INTEGER NOT NULL REFERENCES plans (id),
    type_id INTEGER NOT NULL,
    per_unit INTEGER NOT NULL,
    set_at INTEGER NOT NULL,
    PRIMARY KEY (plan, type_id)
) WITHOUT ROWID;
CREATE TABLE job_assignments (
    job_id INTEGER PRIMARY KEY,
    plan_id INTEGER NOT NULL REFERENCES plans (id) ON DELETE CASCADE,
    assigned_at INTEGER NOT NULL
);
CREATE TABLE app_settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
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
CREATE TABLE structure_names (
    structure_id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    set_at INTEGER NOT NULL
);
CREATE TABLE stockpiles (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL CHECK (length(name) > 0),
    group_name TEXT,
    multiplier INTEGER NOT NULL CHECK (multiplier >= 1),
    position INTEGER NOT NULL CHECK (position >= 0),
    short_mode INTEGER NOT NULL DEFAULT 0 CHECK (short_mode IN (0, 1))
);
CREATE TABLE stockpile_targets (
    stockpile INTEGER NOT NULL REFERENCES stockpiles (id) ON DELETE CASCADE,
    type INTEGER NOT NULL CHECK (type >= 0),
    units INTEGER NOT NULL CHECK (units >= 1),
    ignore_multiplier INTEGER NOT NULL CHECK (ignore_multiplier IN (0, 1)),
    PRIMARY KEY (stockpile, type)
) WITHOUT ROWID;
CREATE TABLE stockpile_includes (
    parent INTEGER NOT NULL REFERENCES stockpiles (id) ON DELETE CASCADE,
    child INTEGER NOT NULL REFERENCES stockpiles (id) ON DELETE RESTRICT,
    count INTEGER NOT NULL CHECK (count >= 1),
    PRIMARY KEY (parent, child),
    CHECK (parent <> child)
) WITHOUT ROWID;
CREATE TABLE stockpile_filters (
    id INTEGER PRIMARY KEY,
    stockpile INTEGER NOT NULL REFERENCES stockpiles (id) ON DELETE CASCADE,
    position INTEGER NOT NULL CHECK (position >= 0),
    place_kind INTEGER NOT NULL CHECK (place_kind BETWEEN 0 AND 3),
    place_id INTEGER CHECK (place_id >= 0),
    assets INTEGER NOT NULL CHECK (assets IN (0, 1)),
    sell_orders INTEGER NOT NULL CHECK (sell_orders IN (0, 1)),
    buy_orders INTEGER NOT NULL CHECK (buy_orders IN (0, 1)),
    jobs INTEGER NOT NULL CHECK (jobs IN (0, 1)),
    CHECK ((place_kind = 0) = (place_id IS NULL)),
    CHECK (assets + sell_orders + buy_orders + jobs >= 1)
);
CREATE TABLE stockpile_filter_characters (
    filter INTEGER NOT NULL REFERENCES stockpile_filters (id) ON DELETE CASCADE,
    character INTEGER NOT NULL CHECK (character >= 0),
    PRIMARY KEY (filter, character)
) WITHOUT ROWID;
CREATE TABLE fee_settings (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    accounting INTEGER NOT NULL CHECK (accounting BETWEEN 0 AND 5),
    broker_relations INTEGER NOT NULL CHECK (broker_relations BETWEEN 0 AND 5),
    faction_standing INTEGER NOT NULL CHECK (faction_standing BETWEEN -1000 AND 1000),
    corporation_standing INTEGER NOT NULL CHECK (corporation_standing BETWEEN -1000 AND 1000),
    broker_override_ppm INTEGER CHECK (broker_override_ppm IS NULL OR broker_override_ppm BETWEEN 0 AND 1000000)
);
CREATE TABLE history_markets (
    id INTEGER PRIMARY KEY,
    kind INTEGER NOT NULL,
    place INTEGER NOT NULL,
    daily_days INTEGER NOT NULL,
    total_days INTEGER,
    enabled INTEGER NOT NULL,
    UNIQUE (kind, place)
);
INSERT INTO history_markets (kind, place, daily_days, total_days, enabled)
    VALUES (0, 10000002, 90, NULL, 1);
CREATE TABLE price_history (
    market INTEGER NOT NULL REFERENCES history_markets (id),
    type_id INTEGER NOT NULL,
    side INTEGER NOT NULL,
    day INTEGER NOT NULL,
    min INTEGER NOT NULL,
    max INTEGER NOT NULL,
    weighted_average INTEGER NOT NULL,
    median INTEGER NOT NULL,
    five_percent INTEGER NOT NULL,
    volume INTEGER NOT NULL,
    orders INTEGER NOT NULL,
    PRIMARY KEY (market, type_id, side, day)
) WITHOUT ROWID;
CREATE TABLE history_settings (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    limit_bytes INTEGER,
    auto_refresh INTEGER NOT NULL
);
INSERT INTO history_settings (id, limit_bytes, auto_refresh) VALUES (1, NULL, 1);
";

/// An open database. Every method on it runs in one transaction.
#[derive(Debug)]
pub struct Store {
    pub(crate) conn: Connection,
}

/// The store, and what happened to the file that was there before.
#[derive(Debug)]
pub struct OpenedStore {
    pub store: Store,
    /// Where an earlier file was set aside when the app started fresh; `None` when the file was
    /// opened or created as it is.
    pub started_fresh: Option<PathBuf>,
}

impl Store {
    /// Opens the file at `path`, creating it when it is missing or empty. A file made with another
    /// layout (an earlier test build) is renamed to `<path>.old-test`, replacing an older one there,
    /// and a new file is created. `now` is the Unix time a new file is recorded with.
    pub fn open(path: &Path, now: i64) -> Result<OpenedStore> {
        if !exists_with_content(path)? {
            return Ok(OpenedStore {
                store: create_layout(path, now)?,
                started_fresh: None,
            });
        }
        let store = Store {
            conn: open_connection(path)?,
        };
        if is_current(&store.conn)? {
            return Ok(OpenedStore {
                store,
                started_fresh: None,
            });
        }
        drop(store);
        let aside = aside_path(path);
        std::fs::rename(path, &aside).map_err(|source| io_error(path, source))?;
        match create_layout(path, now) {
            Ok(store) => Ok(OpenedStore {
                store,
                started_fresh: Some(aside),
            }),
            Err(err) => {
                std::fs::rename(&aside, path).map_err(|source| io_error(path, source))?;
                Err(err)
            }
        }
    }
}

/// False for a missing file and for an empty one; any other file has content.
fn exists_with_content(path: &Path) -> Result<bool> {
    match std::fs::metadata(path) {
        Ok(meta) => Ok(meta.len() > 0),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(io_error(path, source)),
    }
}

fn open_connection(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "foreign_keys", true)?;
    Ok(conn)
}

/// True when the file was made with this exact layout text: no version number, and a `meta` row
/// that holds [`LAYOUT`]. A file that is not a database fails the first query and is not touched.
fn is_current(conn: &Connection) -> Result<bool> {
    let version: u32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version != 0 {
        return Ok(false);
    }
    let meta_tables: i64 = conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'meta'",
        [],
        |row| row.get(0),
    )?;
    if meta_tables == 0 {
        return Ok(false);
    }
    let columns: i64 = conn.query_row(
        "SELECT count(*) FROM pragma_table_info('meta') WHERE name IN ('id', 'layout')",
        [],
        |row| row.get(0),
    )?;
    if columns != 2 {
        return Ok(false);
    }
    let stored: Option<String> = conn
        .query_row(
            "SELECT CAST(layout AS TEXT) FROM meta WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .optional()?;
    Ok(stored.as_deref() == Some(LAYOUT))
}

/// Creates the file (or fills an empty one) with [`LAYOUT`] and records the layout and `now`.
fn create_layout(path: &Path, now: i64) -> Result<Store> {
    OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(|source| io_error(path, source))?;
    let mut conn = open_connection(path)?;
    let tx = conn.transaction()?;
    tx.execute_batch(LAYOUT)?;
    tx.execute(
        "INSERT INTO meta (id, layout, created_at) VALUES (1, ?1, ?2)",
        params![LAYOUT, now],
    )?;
    tx.commit()?;
    Ok(Store { conn })
}

fn aside_path(path: &Path) -> PathBuf {
    let mut aside = path.as_os_str().to_owned();
    aside.push(".old-test");
    PathBuf::from(aside)
}

fn io_error(path: &Path, source: io::Error) -> StoreError {
    StoreError::Io {
        path: path.to_path_buf(),
        source,
    }
}

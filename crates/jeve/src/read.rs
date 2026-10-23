//! Reads a jEveAssets profile without writing to it: characters, their assets and blueprints.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use mop_core::industry::TypeId;
use mop_core::stock::{Asset, CharacterAssets, CharacterId, ItemId, LocationId};
use rusqlite::{Connection, ErrorCode, OpenFlags, Row};

use crate::JeveError;

/// The columns the reader selects, per table. The first one missing from the file is the error.
const REQUIRED: [(&str, &[&str]); 3] = [
    (
        "owners",
        &[
            "accountid",
            "ownerid",
            "name",
            "show",
            "invalid",
            "assetslastupdate",
            "scopes",
        ],
    ),
    (
        "assets",
        &[
            "accountid",
            "itemid",
            "typeid",
            "locationid",
            "flagstring",
            "count",
            "singleton",
            "rawquantity",
        ],
    ),
    (
        "blueprints",
        &[
            "accountid",
            "itemid",
            "typeid",
            "locationid",
            "flagstring",
            "quantity",
            "materialefficiency",
            "timeefficiency",
            "runs",
        ],
    ),
];

// Only the columns above are named. The scope check reads the owner's scopes text inside the
// expression; the text itself never leaves the database.
const OWNERS_SQL: &str = "SELECT accountid, ownerid, name, show, invalid, assetslastupdate, \
    instr(scopes, 'esi-characters.read_corporation_roles.v1') > 0 FROM owners";
const ASSETS_SQL: &str = "SELECT accountid, itemid, typeid, locationid, flagstring, count, \
    singleton, rawquantity FROM assets";
const BLUEPRINTS_SQL: &str = "SELECT accountid, itemid, typeid, locationid, flagstring, quantity, \
    materialefficiency, timeefficiency, runs FROM blueprints";

/// How long to wait for another program to release the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadOptions {
    pub busy_timeout: Duration,
}

impl Default for ReadOptions {
    fn default() -> Self {
        Self {
            busy_timeout: Duration::from_secs(2),
        }
    }
}

/// Everything the profile holds for the characters it lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JeveData {
    /// Sorted by character id. Corporations are not listed. One entry per owner row: two rows
    /// for the same character id (two logins holding it) both appear, and stock counts keep one
    /// copy of each item.
    pub characters: Vec<JeveCharacter>,
    /// Corporation owners left out, with their rows.
    pub skipped_corporations: u32,
    /// Asset and blueprint rows whose owner is not in the profile.
    pub orphan_rows: u32,
    /// Rows with no id (owner, item) that cannot be named, left out. Only a damaged file has them.
    pub unnamed_rows: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JeveCharacter {
    pub character_id: CharacterId,
    pub name: String,
    pub shown: bool,
    /// The login no longer works; the data is still in the profile.
    pub login_expired: bool,
    /// When jEveAssets last fetched the assets, Unix seconds; `None` = never.
    pub assets_updated: Option<i64>,
    pub assets: CharacterAssets,
    pub blueprints: Vec<OwnedBlueprint>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedBlueprint {
    pub item_id: ItemId,
    pub type_id: TypeId,
    pub location_id: LocationId,
    pub flag: Option<String>,
    pub kind: BlueprintKind,
    /// How many blueprints the row stands for: a stack of originals, or 1.
    pub stack: u32,
    /// Material efficiency, 0 to 10.
    pub me: u8,
    /// Time efficiency, 0 to 20.
    pub te: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlueprintKind {
    Original,
    Copy { runs: u32 },
}

/// One owner per account id. `None` marks a corporation, whose rows are skipped.
type Owners = BTreeMap<String, Option<JeveCharacter>>;

/// The result while it is being read, before sorting.
struct Loaded {
    owners: Owners,
    skipped_corporations: u32,
    orphan_rows: u32,
    unnamed_rows: u32,
}

/// Reads the profile at `path`. The file is opened read-only and never changed.
pub fn read_profile(path: &Path, options: &ReadOptions) -> Result<JeveData, JeveError> {
    std::fs::metadata(path).map_err(|source| JeveError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    open(path, options)
        .and_then(|mut connection| read_all(&mut connection))
        .map_err(|error| classify(path, error))
}

fn open(path: &Path, options: &ReadOptions) -> Result<Connection, JeveError> {
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let connection = Connection::open_with_flags(path, flags)?;
    connection.busy_timeout(options.busy_timeout)?;
    Ok(connection)
}

/// Turns the SQLite failures that mean "not a profile" or "file busy" into their own errors.
fn classify(path: &Path, error: JeveError) -> JeveError {
    if let JeveError::Sqlite(rusqlite::Error::SqliteFailure(failure, _)) = &error {
        if failure.code == ErrorCode::NotADatabase {
            return JeveError::NotJeve {
                path: path.to_path_buf(),
            };
        }
        if matches!(
            failure.code,
            ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked
        ) {
            return JeveError::Busy;
        }
    }
    error
}

/// Reads all tables inside one read transaction, so they come from the same moment even while
/// jEveAssets is writing the file.
fn read_all(connection: &mut Connection) -> Result<JeveData, JeveError> {
    let transaction = connection.transaction()?;
    let data = read_tables(&transaction)?;
    transaction.commit()?;
    Ok(data)
}

fn read_tables(connection: &Connection) -> Result<JeveData, JeveError> {
    check_layout(connection)?;
    let mut loaded = read_owners(connection)?;
    read_assets(connection, &mut loaded)?;
    read_blueprints(connection, &mut loaded)?;

    let mut characters: Vec<JeveCharacter> = loaded.owners.into_values().flatten().collect();
    for character in &mut characters {
        character.assets.assets.sort_by_key(|asset| asset.item_id);
        character
            .blueprints
            .sort_by_key(|blueprint| blueprint.item_id);
    }
    characters.sort_by_key(|character| character.character_id);
    Ok(JeveData {
        characters,
        skipped_corporations: loaded.skipped_corporations,
        orphan_rows: loaded.orphan_rows,
        unnamed_rows: loaded.unnamed_rows,
    })
}

fn check_layout(connection: &Connection) -> Result<(), JeveError> {
    for (table, columns) in REQUIRED {
        let present = table_columns(connection, table)?;
        if present.is_empty() {
            return Err(JeveError::Layout {
                table,
                column: None,
            });
        }
        let missing = columns
            .iter()
            .copied()
            .find(|column| !present.iter().any(|name| name.as_str() == *column));
        if let Some(column) = missing {
            return Err(JeveError::Layout {
                table,
                column: Some(column),
            });
        }
    }
    Ok(())
}

fn table_columns(connection: &Connection, table: &str) -> rusqlite::Result<Vec<String>> {
    let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
    let names = statement.query_map([], |row| row.get::<_, String>(1))?;
    names.collect()
}

fn query_rows<T>(
    connection: &Connection,
    sql: &str,
    map: impl FnMut(&Row<'_>) -> rusqlite::Result<T>,
) -> Result<Vec<T>, JeveError> {
    let mut statement = connection.prepare(sql)?;
    let rows = statement.query_map([], map)?;
    rows.collect::<rusqlite::Result<Vec<T>>>()
        .map_err(JeveError::from)
}

/// Adds one to a row count, failing rather than wrapping when the count no longer fits.
fn bump(counter: &mut u32, table: &'static str) -> Result<(), JeveError> {
    *counter = counter
        .checked_add(1)
        .ok_or(JeveError::TooManyRows { table })?;
    Ok(())
}

fn bad_value(table: &'static str, column: &'static str, item_id: i64) -> JeveError {
    JeveError::BadValue {
        table,
        column,
        item_id,
    }
}

/// A value that must be present; an empty one is a bad value of the row.
fn cell<T>(
    value: Option<T>,
    table: &'static str,
    column: &'static str,
    item_id: i64,
) -> Result<T, JeveError> {
    value.ok_or_else(|| bad_value(table, column, item_id))
}

fn type_id_of(value: Option<i64>, table: &'static str, item_id: i64) -> Result<TypeId, JeveError> {
    value
        .and_then(|type_id| TypeId::try_from(type_id).ok())
        .ok_or_else(|| bad_value(table, "typeid", item_id))
}

struct OwnerRow {
    accountid: Option<String>,
    ownerid: Option<i64>,
    name: Option<String>,
    show: Option<i64>,
    invalid: Option<i64>,
    assetslastupdate: Option<i64>,
    corporation: Option<i64>,
}

impl OwnerRow {
    fn read(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            accountid: row.get(0)?,
            ownerid: row.get(1)?,
            name: row.get(2)?,
            show: row.get(3)?,
            invalid: row.get(4)?,
            assetslastupdate: row.get(5)?,
            corporation: row.get(6)?,
        })
    }
}

fn read_owners(connection: &Connection) -> Result<Loaded, JeveError> {
    let mut loaded = Loaded {
        owners: BTreeMap::new(),
        skipped_corporations: 0,
        orphan_rows: 0,
        unnamed_rows: 0,
    };
    for row in query_rows(connection, OWNERS_SQL, OwnerRow::read)? {
        let Some(character_id) = row.ownerid else {
            bump(&mut loaded.unnamed_rows, "owners")?;
            continue;
        };
        let account_id = cell(row.accountid, "owners", "accountid", character_id)?;
        let corporation = cell(row.corporation, "owners", "scopes", character_id)? > 0;
        if corporation {
            bump(&mut loaded.skipped_corporations, "owners")?;
            loaded.owners.insert(account_id, None);
            continue;
        }
        let character = JeveCharacter {
            character_id,
            name: cell(row.name, "owners", "name", character_id)?,
            shown: cell(row.show, "owners", "show", character_id)? != 0,
            login_expired: cell(row.invalid, "owners", "invalid", character_id)? != 0,
            assets_updated: row.assetslastupdate.map(|ms| ms.div_euclid(1000)),
            assets: CharacterAssets {
                character_id,
                updated: row.assetslastupdate.map(|ms| ms.div_euclid(1000)),
                assets: Vec::new(),
            },
            blueprints: Vec::new(),
        };
        loaded.owners.insert(account_id, Some(character));
    }
    Ok(loaded)
}

struct AssetRow {
    accountid: Option<String>,
    itemid: Option<i64>,
    typeid: Option<i64>,
    locationid: Option<i64>,
    flagstring: Option<String>,
    count: Option<i64>,
    singleton: Option<i64>,
    rawquantity: Option<i64>,
}

impl AssetRow {
    fn read(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            accountid: row.get(0)?,
            itemid: row.get(1)?,
            typeid: row.get(2)?,
            locationid: row.get(3)?,
            flagstring: row.get(4)?,
            count: row.get(5)?,
            singleton: row.get(6)?,
            rawquantity: row.get(7)?,
        })
    }
}

fn read_assets(connection: &Connection, loaded: &mut Loaded) -> Result<(), JeveError> {
    for mut row in query_rows(connection, ASSETS_SQL, AssetRow::read)? {
        let Some(item_id) = row.itemid else {
            bump(&mut loaded.unnamed_rows, "assets")?;
            continue;
        };
        let account = cell(row.accountid.take(), "assets", "accountid", item_id)?;
        match loaded.owners.get_mut(&account) {
            None => bump(&mut loaded.orphan_rows, "assets")?,
            Some(None) => {}
            Some(Some(character)) => character.assets.assets.push(asset_from(row, item_id)?),
        }
    }
    Ok(())
}

fn asset_from(row: AssetRow, item_id: ItemId) -> Result<Asset, JeveError> {
    let type_id = type_id_of(row.typeid, "assets", item_id)?;
    let quantity = cell(
        row.count.and_then(|count| u64::try_from(count).ok()),
        "assets",
        "count",
        item_id,
    )?;
    let location_id = cell(row.locationid, "assets", "locationid", item_id)?;
    let singleton = cell(row.singleton, "assets", "singleton", item_id)? != 0;
    Ok(Asset {
        item_id,
        type_id,
        location_id,
        flag: row.flagstring,
        quantity,
        // A row with a raw quantity is an assembled single or a blueprint copy, even when the
        // singleton flag says otherwise.
        singleton: singleton || row.rawquantity.is_some(),
    })
}

struct BlueprintRow {
    accountid: Option<String>,
    itemid: Option<i64>,
    typeid: Option<i64>,
    locationid: Option<i64>,
    flagstring: Option<String>,
    quantity: Option<i64>,
    materialefficiency: Option<i64>,
    timeefficiency: Option<i64>,
    runs: Option<i64>,
}

impl BlueprintRow {
    fn read(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            accountid: row.get(0)?,
            itemid: row.get(1)?,
            typeid: row.get(2)?,
            locationid: row.get(3)?,
            flagstring: row.get(4)?,
            quantity: row.get(5)?,
            materialefficiency: row.get(6)?,
            timeefficiency: row.get(7)?,
            runs: row.get(8)?,
        })
    }
}

fn read_blueprints(connection: &Connection, loaded: &mut Loaded) -> Result<(), JeveError> {
    for mut row in query_rows(connection, BLUEPRINTS_SQL, BlueprintRow::read)? {
        let Some(item_id) = row.itemid else {
            bump(&mut loaded.unnamed_rows, "blueprints")?;
            continue;
        };
        let account = cell(row.accountid.take(), "blueprints", "accountid", item_id)?;
        match loaded.owners.get_mut(&account) {
            None => bump(&mut loaded.orphan_rows, "blueprints")?,
            Some(None) => {}
            Some(Some(character)) => character.blueprints.push(blueprint_from(row, item_id)?),
        }
    }
    Ok(())
}

fn blueprint_from(row: BlueprintRow, item_id: ItemId) -> Result<OwnedBlueprint, JeveError> {
    let type_id = type_id_of(row.typeid, "blueprints", item_id)?;
    let location_id = cell(row.locationid, "blueprints", "locationid", item_id)?;
    let quantity = cell(row.quantity, "blueprints", "quantity", item_id)?;
    let runs = cell(row.runs, "blueprints", "runs", item_id)?;
    let me = cell(
        row.materialefficiency
            .and_then(|me| u8::try_from(me).ok())
            .filter(|me| *me <= 10),
        "blueprints",
        "materialefficiency",
        item_id,
    )?;
    let te = cell(
        row.timeefficiency
            .and_then(|te| u8::try_from(te).ok())
            .filter(|te| *te <= 20),
        "blueprints",
        "timeefficiency",
        item_id,
    )?;
    let (kind, stack) = match quantity {
        -1 => (BlueprintKind::Original, 1),
        -2 => (
            BlueprintKind::Copy {
                runs: copy_runs(runs, item_id)?,
            },
            1,
        ),
        _ => (
            BlueprintKind::Original,
            cell(
                u32::try_from(quantity).ok().filter(|stack| *stack > 0),
                "blueprints",
                "quantity",
                item_id,
            )?,
        ),
    };
    Ok(OwnedBlueprint {
        item_id,
        type_id,
        location_id,
        flag: row.flagstring,
        kind,
        stack,
        me,
        te,
    })
}

fn copy_runs(runs: i64, item_id: ItemId) -> Result<u32, JeveError> {
    cell(
        u32::try_from(runs).ok().filter(|runs| *runs >= 1),
        "blueprints",
        "runs",
        item_id,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reader_connection_refuses_writes() {
        let root = tempfile::tempdir().expect("temp dir");
        let path = root.path().join("probe.db");
        Connection::open(&path)
            .expect("create database")
            .execute_batch("CREATE TABLE probe (a)")
            .expect("create table");
        let connection = open(&path, &ReadOptions::default()).expect("open reader");

        connection
            .execute_batch("CREATE TABLE written (a)")
            .expect_err("a write through the reader fails");
    }
}

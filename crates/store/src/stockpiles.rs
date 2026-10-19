//! Stockpiles: named lists of targets, the filters that say which stock counts toward them, and the
//! stockpiles they include.

use std::collections::{BTreeMap, BTreeSet};

use profit_core::reserve::ShortMode;
use profit_core::stockpile::{
    Characters, Filter, Include, Place, Source, Stockpile, Target, include_loop,
};
use rusqlite::{OptionalExtension, Transaction, params};

use crate::error::{Result, StoreError};
use crate::migrate::Store;
use crate::{
    bad_code, clean_name, ensure_free, from_stored, id_names, missing_reference, to_stored_u64,
};

const TABLE: &str = "stockpiles";
const FILTERS: &str = "stockpile_filters";

fn invalid(reason: &str) -> StoreError {
    StoreError::InvalidStockpile(reason.to_owned())
}

/// The stored code of a stockpile's Short mode.
fn short_mode_code(mode: ShortMode) -> i64 {
    match mode {
        ShortMode::AfterPlans => 0,
        ShortMode::RightNow => 1,
    }
}

/// The stored kind and place id of a filter's place. Everywhere has no place id.
fn place_code(place: Place) -> (i64, Option<i64>) {
    match place {
        Place::Everywhere => (0, None),
        Place::Location(id) => (1, Some(id)),
        Place::System(id) => (2, Some(i64::from(id))),
        Place::Region(id) => (3, Some(i64::from(id))),
    }
}

fn place_from(kind: i64, place_id: Option<i64>, filter: i64) -> Result<Place> {
    let needed = || {
        place_id.ok_or_else(|| {
            StoreError::BadData(format!("{FILTERS} row {filter}, column place_id: missing"))
        })
    };
    Ok(match kind {
        0 => Place::Everywhere,
        1 => Place::Location(needed()?),
        2 => Place::System(from_stored(needed()?, FILTERS, filter, "place_id")?),
        3 => Place::Region(from_stored(needed()?, FILTERS, filter, "place_id")?),
        other => return Err(bad_code(FILTERS, filter, "place_kind", other)),
    })
}

/// Refuses what the store cannot keep, before anything is written.
fn check(stockpile: &Stockpile) -> Result<()> {
    if stockpile.multiplier == 0 {
        return Err(invalid("multiplier must be 1 or more"));
    }
    let mut listed = BTreeSet::new();
    for target in &stockpile.targets {
        if target.units == 0 {
            return Err(invalid("units must be 1 or more"));
        }
        if !listed.insert(target.type_id) {
            return Err(invalid(&format!("type {} is listed twice", target.type_id)));
        }
    }
    if stockpile.includes.iter().any(|include| include.count == 0) {
        return Err(invalid("count must be 1 or more"));
    }
    for filter in &stockpile.filters {
        if filter.sources.is_empty() {
            return Err(invalid("a filter needs at least one source"));
        }
        // Storing no characters means all of them, so a character filter with none would widen.
        if let Characters::Only(set) = &filter.characters
            && set.is_empty()
        {
            return Err(invalid("a character filter needs at least one character"));
        }
    }
    Ok(())
}

fn stockpile_names(tx: &Transaction<'_>) -> Result<Vec<(i64, String)>> {
    id_names(tx, "SELECT id, name FROM stockpiles", [])
}

fn read_targets(tx: &Transaction<'_>, id: i64) -> Result<Vec<Target>> {
    let mut stmt = tx.prepare(
        "SELECT type, units, ignore_multiplier FROM stockpile_targets \
         WHERE stockpile = ?1 ORDER BY type",
    )?;
    let raw = stmt
        .query_map(params![id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, bool>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    raw.into_iter()
        .map(|(type_id, units, ignore_multiplier)| {
            Ok(Target {
                type_id: from_stored(type_id, "stockpile_targets", id, "type")?,
                units: from_stored(units, "stockpile_targets", id, "units")?,
                ignore_multiplier,
            })
        })
        .collect()
}

fn read_includes(tx: &Transaction<'_>, id: i64) -> Result<Vec<Include>> {
    let mut stmt =
        tx.prepare("SELECT child, count FROM stockpile_includes WHERE parent = ?1 ORDER BY child")?;
    let raw = stmt
        .query_map(params![id], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    raw.into_iter()
        .map(|(stockpile_id, count)| {
            Ok(Include {
                stockpile_id,
                count: from_stored(count, "stockpile_includes", id, "count")?,
            })
        })
        .collect()
}

fn read_characters(tx: &Transaction<'_>, filter: i64) -> Result<Characters> {
    let mut stmt = tx.prepare(
        "SELECT character FROM stockpile_filter_characters WHERE filter = ?1 ORDER BY character",
    )?;
    let raw = stmt
        .query_map(params![filter], |row| row.get::<_, i64>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if raw.is_empty() {
        return Ok(Characters::All);
    }
    let set = raw
        .into_iter()
        .map(|character| {
            from_stored(
                character,
                "stockpile_filter_characters",
                filter,
                "character",
            )
        })
        .collect::<Result<BTreeSet<_>>>()?;
    Ok(Characters::Only(set))
}

/// The columns of one filter row, before they are turned back into Rust types.
struct RawFilter {
    id: i64,
    place_kind: i64,
    place_id: Option<i64>,
    sources: BTreeSet<Source>,
}

/// The sources a filter row lists, from its four flag columns.
fn listed_sources(row: &rusqlite::Row<'_>) -> rusqlite::Result<BTreeSet<Source>> {
    let flags = [
        (3, Source::Assets),
        (4, Source::SellOrders),
        (5, Source::BuyOrders),
        (6, Source::Jobs),
    ];
    let mut listed = BTreeSet::new();
    for (column, source) in flags {
        if row.get::<_, bool>(column)? {
            listed.insert(source);
        }
    }
    Ok(listed)
}

fn read_filters(tx: &Transaction<'_>, id: i64) -> Result<Vec<Filter>> {
    let mut stmt = tx.prepare(
        "SELECT id, place_kind, place_id, assets, sell_orders, buy_orders, jobs \
         FROM stockpile_filters WHERE stockpile = ?1 ORDER BY position, id",
    )?;
    let raw = stmt
        .query_map(params![id], |row| {
            Ok(RawFilter {
                id: row.get(0)?,
                place_kind: row.get(1)?,
                place_id: row.get(2)?,
                sources: listed_sources(row)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    raw.into_iter()
        .map(|filter| {
            Ok(Filter {
                characters: read_characters(tx, filter.id)?,
                place: place_from(filter.place_kind, filter.place_id, filter.id)?,
                sources: filter.sources,
            })
        })
        .collect()
}

fn read_stockpile(tx: &Transaction<'_>, id: i64) -> Result<Option<Stockpile>> {
    let row = tx
        .query_row(
            "SELECT name, group_name, multiplier FROM stockpiles WHERE id = ?1",
            params![id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()?;
    let Some((name, group, multiplier)) = row else {
        return Ok(None);
    };
    Ok(Some(Stockpile {
        id,
        name,
        group,
        multiplier: from_stored(multiplier, TABLE, id, "multiplier")?,
        targets: read_targets(tx, id)?,
        includes: read_includes(tx, id)?,
        filters: read_filters(tx, id)?,
    }))
}

/// Every stockpile in list order: by position, then id.
fn read_stockpiles(tx: &Transaction<'_>) -> Result<Vec<Stockpile>> {
    let mut stmt = tx.prepare("SELECT id FROM stockpiles ORDER BY position, id")?;
    let ids = stmt
        .query_map([], |row| row.get::<_, i64>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    ids.into_iter()
        .map(|id| read_stockpile(tx, id)?.ok_or(StoreError::NotFound))
        .collect()
}

/// Replaces the targets, includes and filters of a stockpile.
fn write_parts(tx: &Transaction<'_>, id: i64, stockpile: &Stockpile) -> Result<()> {
    tx.execute(
        "DELETE FROM stockpile_targets WHERE stockpile = ?1",
        params![id],
    )?;
    tx.execute(
        "DELETE FROM stockpile_includes WHERE parent = ?1",
        params![id],
    )?;
    tx.execute(
        "DELETE FROM stockpile_filters WHERE stockpile = ?1",
        params![id],
    )?;
    for target in &stockpile.targets {
        tx.execute(
            "INSERT INTO stockpile_targets (stockpile, type, units, ignore_multiplier) \
             VALUES (?1, ?2, ?3, ?4)",
            params![
                id,
                i64::from(target.type_id),
                to_stored_u64(target.units)?,
                target.ignore_multiplier
            ],
        )?;
    }
    for include in &stockpile.includes {
        tx.execute(
            "INSERT INTO stockpile_includes (parent, child, count) VALUES (?1, ?2, ?3)",
            params![id, include.stockpile_id, i64::from(include.count)],
        )
        .map_err(missing_reference)?;
    }
    for (position, filter) in (0_i64..).zip(&stockpile.filters) {
        write_filter(tx, id, position, filter)?;
    }
    Ok(())
}

fn write_filter(tx: &Transaction<'_>, id: i64, position: i64, filter: &Filter) -> Result<()> {
    let (kind, place_id) = place_code(filter.place);
    tx.execute(
        "INSERT INTO stockpile_filters (stockpile, position, place_kind, place_id, assets, \
            sell_orders, buy_orders, jobs) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            id,
            position,
            kind,
            place_id,
            filter.sources.contains(&Source::Assets),
            filter.sources.contains(&Source::SellOrders),
            filter.sources.contains(&Source::BuyOrders),
            filter.sources.contains(&Source::Jobs)
        ],
    )?;
    let filter_id = tx.last_insert_rowid();
    if let Characters::Only(set) = &filter.characters {
        for character in set {
            tx.execute(
                "INSERT INTO stockpile_filter_characters (filter, character) VALUES (?1, ?2)",
                params![filter_id, character],
            )?;
        }
    }
    Ok(())
}

impl Store {
    /// Every stockpile, in list order.
    pub fn stockpiles(&mut self) -> Result<Vec<Stockpile>> {
        let tx = self.conn.transaction()?;
        let all = read_stockpiles(&tx)?;
        tx.commit()?;
        Ok(all)
    }

    /// The stockpile with this id, if there is one.
    pub fn stockpile(&mut self, id: i64) -> Result<Option<Stockpile>> {
        let tx = self.conn.transaction()?;
        let stockpile = read_stockpile(&tx, id)?;
        tx.commit()?;
        Ok(stockpile)
    }

    /// Adds an empty stockpile at the end of the list, with multiplier 1. Returns its id.
    pub fn create_stockpile(&mut self, name: &str, group: Option<&str>) -> Result<i64> {
        let tx = self.conn.transaction()?;
        let name = clean_name(name)?;
        ensure_free(&stockpile_names(&tx)?, &name, None)?;
        tx.execute(
            "INSERT INTO stockpiles (name, group_name, multiplier, position) \
             VALUES (?1, ?2, 1, (SELECT COALESCE(MAX(position) + 1, 0) FROM stockpiles))",
            params![name, group],
        )?;
        let id = tx.last_insert_rowid();
        tx.commit()?;
        Ok(id)
    }

    /// Saves a stockpile over the one with its id, parts and all. Nothing is written when a check
    /// fails: the name, a value out of range, an unknown include, or an include loop.
    pub fn save_stockpile(&mut self, stockpile: &Stockpile) -> Result<()> {
        let tx = self.conn.transaction()?;
        let name = clean_name(&stockpile.name)?;
        ensure_free(&stockpile_names(&tx)?, &name, Some(stockpile.id))?;
        check(stockpile)?;

        let stored = read_stockpiles(&tx)?;
        let mut all: BTreeMap<i64, Stockpile> = stored.into_iter().map(|s| (s.id, s)).collect();
        if !all.contains_key(&stockpile.id) {
            return Err(StoreError::NotFound);
        }
        for include in &stockpile.includes {
            if !all.contains_key(&include.stockpile_id) {
                return Err(StoreError::NotFound);
            }
        }
        let saved = Stockpile {
            name: name.clone(),
            ..stockpile.clone()
        };
        all.insert(saved.id, saved.clone());
        if let Some(path) = include_loop(saved.id, &all) {
            let names = path
                .iter()
                .map(|id| {
                    all.get(id)
                        .map_or_else(|| id.to_string(), |s| s.name.clone())
                })
                .collect();
            return Err(StoreError::StockpileLoop(names));
        }

        tx.execute(
            "UPDATE stockpiles SET name = ?2, group_name = ?3, multiplier = ?4 WHERE id = ?1",
            params![saved.id, name, saved.group, i64::from(saved.multiplier)],
        )?;
        write_parts(&tx, saved.id, &saved)?;
        tx.commit()?;
        Ok(())
    }

    /// Deletes a stockpile with its targets and filters. Refused while another stockpile includes
    /// it; `InUse` then names those stockpiles.
    pub fn delete_stockpile(&mut self, id: i64) -> Result<()> {
        let tx = self.conn.transaction()?;
        let mut users = id_names(
            &tx,
            "SELECT p.id, p.name FROM stockpile_includes i \
             JOIN stockpiles p ON p.id = i.parent WHERE i.child = ?1",
            params![id],
        )?;
        if !users.is_empty() {
            users.sort_by_cached_key(|(id, name)| (name.to_lowercase(), *id));
            return Err(StoreError::InUse {
                plans: users.into_iter().map(|(_, name)| name).collect(),
            });
        }
        let removed = tx.execute("DELETE FROM stockpiles WHERE id = ?1", params![id])?;
        if removed == 0 {
            return Err(StoreError::NotFound);
        }
        tx.commit()?;
        Ok(())
    }

    /// Puts the stockpiles in the given order. The list must name every stockpile once; otherwise
    /// nothing is written.
    pub fn set_stockpile_order(&mut self, order: &[i64]) -> Result<()> {
        let tx = self.conn.transaction()?;
        let known: BTreeSet<i64> = stockpile_names(&tx)?
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        let asked: BTreeSet<i64> = order.iter().copied().collect();
        if order.len() != known.len() || asked != known {
            return Err(StoreError::NotFound);
        }
        for (position, id) in (0_i64..).zip(order) {
            tx.execute(
                "UPDATE stockpiles SET position = ?2 WHERE id = ?1",
                params![id, position],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Whether a stockpile's Short counts stock after the plans' claims or right now. An unknown
    /// stockpile, or one holding a mode this app cannot name, is refused.
    pub fn short_mode(&mut self, stockpile_id: i64) -> Result<ShortMode> {
        let tx = self.conn.transaction()?;
        let stored: Option<i64> = tx
            .query_row(
                "SELECT short_mode FROM stockpiles WHERE id = ?1",
                params![stockpile_id],
                |row| row.get(0),
            )
            .optional()?;
        let mode = match stored {
            None => return Err(StoreError::NotFound),
            Some(0) => ShortMode::AfterPlans,
            Some(1) => ShortMode::RightNow,
            Some(code) => return Err(bad_code(TABLE, stockpile_id, "short_mode", code)),
        };
        tx.commit()?;
        Ok(mode)
    }

    /// Sets whether a stockpile's Short counts stock after the plans' claims or right now. An
    /// unknown stockpile is refused.
    pub fn set_short_mode(&mut self, stockpile_id: i64, mode: ShortMode) -> Result<()> {
        let tx = self.conn.transaction()?;
        let changed = tx.execute(
            "UPDATE stockpiles SET short_mode = ?2 WHERE id = ?1",
            params![stockpile_id, short_mode_code(mode)],
        )?;
        if changed == 0 {
            return Err(StoreError::NotFound);
        }
        tx.commit()?;
        Ok(())
    }
}

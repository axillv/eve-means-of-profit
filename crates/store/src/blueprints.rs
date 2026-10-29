//! Each character's blueprints with their material and time efficiency, kept for the
//! what-to-build feed. A save replaces the character's whole list with the one just fetched.

use std::collections::BTreeSet;

use mop_core::industry::TypeId;
use mop_core::stock::{ItemId, LocationId};
use rusqlite::{OptionalExtension, Transaction, params};

use crate::error::{Result, StoreError};
use crate::migrate::Store;
use crate::{bad_code, from_stored, to_stored_u64};

const TABLE: &str = "blueprints";
/// The most material efficiency a blueprint can have.
const MAX_ME: i64 = 10;
/// The most time efficiency a blueprint can have.
const MAX_TE: i64 = 20;

/// One blueprint the character owns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlueprintRow {
    pub item_id: ItemId,
    pub type_id: TypeId,
    pub location_id: LocationId,
    pub flag: Option<String>,
    pub stack: u32,
    /// The runs a copy has left; `None` for an original.
    pub runs: Option<u32>,
    pub me: u8,
    pub te: u8,
}

/// One character's blueprints and the time they were fetched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredBlueprints {
    pub character_id: u64,
    pub fetched_at: i64,
    pub rows: Vec<BlueprintRow>,
}

impl Store {
    /// Saves one character's whole blueprint list and the time it was fetched, replacing the list
    /// saved before. Every row is checked first: a row the store will not keep refuses the save
    /// and nothing is written, so the previous list stays.
    pub fn save_blueprints(
        &mut self,
        character_id: u64,
        fetched_at: i64,
        rows: &[BlueprintRow],
    ) -> Result<()> {
        let character = to_stored_u64(character_id)?;
        for row in rows {
            ensure_kept(row)?;
        }
        let mut seen = BTreeSet::new();
        for row in rows {
            if !seen.insert(row.item_id) {
                return Err(StoreError::BadData(format!(
                    "blueprint item {} is listed twice",
                    row.item_id
                )));
            }
        }
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO blueprint_fetches (character_id, fetched_at) VALUES (?1, ?2) \
             ON CONFLICT (character_id) DO UPDATE SET fetched_at = ?2",
            params![character, fetched_at],
        )?;
        tx.execute(
            "DELETE FROM blueprints WHERE character_id = ?1",
            params![character],
        )?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO blueprints (character_id, item_id, type_id, location_id, flag, \
                 stack, runs, me, te) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )?;
            for row in rows {
                stmt.execute(params![
                    character,
                    row.item_id,
                    i64::from(row.type_id),
                    row.location_id,
                    row.flag,
                    i64::from(row.stack),
                    row.runs.map(i64::from),
                    i64::from(row.me),
                    i64::from(row.te),
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// The character's blueprints and the time they were fetched; `None` when no fetch was saved.
    /// The rows come by item id.
    pub fn blueprints(&mut self, character_id: u64) -> Result<Option<StoredBlueprints>> {
        let character = to_stored_u64(character_id)?;
        let tx = self.conn.transaction()?;
        let fetched_at: Option<i64> = tx
            .query_row(
                "SELECT fetched_at FROM blueprint_fetches WHERE character_id = ?1",
                params![character],
                |row| row.get(0),
            )
            .optional()?;
        let Some(fetched_at) = fetched_at else {
            return Ok(None);
        };
        let rows = rows_of(&tx, character)?;
        tx.commit()?;
        Ok(Some(StoredBlueprints {
            character_id,
            fetched_at,
            rows,
        }))
    }

    /// Every character's saved blueprints, by character id; each's rows by item id.
    pub fn all_blueprints(&mut self) -> Result<Vec<StoredBlueprints>> {
        let tx = self.conn.transaction()?;
        let mut all = Vec::new();
        {
            let mut stmt = tx.prepare(
                "SELECT character_id, fetched_at FROM blueprint_fetches ORDER BY character_id",
            )?;
            let found =
                stmt.query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))?;
            for entry in found {
                let (character, fetched_at) = entry?;
                let character_id = from_stored(character, TABLE, character, "character_id")?;
                let rows = rows_of(&tx, character)?;
                all.push(StoredBlueprints {
                    character_id,
                    fetched_at,
                    rows,
                });
            }
        }
        tx.commit()?;
        Ok(all)
    }
}

/// A row the store will keep, or `BadData` naming the blueprint and the reason.
fn ensure_kept(row: &BlueprintRow) -> Result<()> {
    let bad =
        |reason: String| StoreError::BadData(format!("blueprint item {}: {reason}", row.item_id));
    if i64::from(row.me) > MAX_ME {
        return Err(bad(format!(
            "material efficiency {} is above {MAX_ME}",
            row.me
        )));
    }
    if i64::from(row.te) > MAX_TE {
        return Err(bad(format!("time efficiency {} is above {MAX_TE}", row.te)));
    }
    if row.stack == 0 {
        return Err(bad("a stack holds no blueprints".to_owned()));
    }
    if let Some(runs) = row.runs {
        if runs == 0 {
            return Err(bad("a copy has no runs left".to_owned()));
        }
        if row.stack != 1 {
            return Err(bad("a copy is never a stack".to_owned()));
        }
    }
    Ok(())
}

/// The character's stored blueprints, by item id.
fn rows_of(tx: &Transaction<'_>, character: i64) -> Result<Vec<BlueprintRow>> {
    let mut stmt = tx.prepare(
        "SELECT item_id, type_id, location_id, flag, stack, runs, me, te \
         FROM blueprints WHERE character_id = ?1 ORDER BY item_id",
    )?;
    let rows = stmt.query_map(params![character], |row| {
        Ok(StoredRow {
            item_id: row.get(0)?,
            type_id: row.get(1)?,
            location_id: row.get(2)?,
            flag: row.get(3)?,
            stack: row.get(4)?,
            runs: row.get(5)?,
            me: row.get(6)?,
            te: row.get(7)?,
        })
    })?;
    rows.map(|row| row?.into_row()).collect()
}

/// One row as the database holds it, before it is turned back into Rust types.
struct StoredRow {
    item_id: i64,
    type_id: i64,
    location_id: i64,
    flag: Option<String>,
    stack: i64,
    runs: Option<i64>,
    me: i64,
    te: i64,
}

impl StoredRow {
    /// The row's blueprint. A value the table's checks should have refused is `BadData` naming
    /// the column it came from; nothing is clamped to fit.
    fn into_row(self) -> Result<BlueprintRow> {
        let stack = stored_stack(self.stack, self.item_id)?;
        let runs = self
            .runs
            .map(|runs| stored_runs(runs, self.item_id))
            .transpose()?;
        if runs.is_some() && stack != 1 {
            return Err(StoreError::BadData(format!(
                "{TABLE} row {}, column runs: a copy is stored as a stack of {stack}",
                self.item_id
            )));
        }
        Ok(BlueprintRow {
            item_id: self.item_id,
            type_id: from_stored(self.type_id, TABLE, self.item_id, "type_id")?,
            location_id: self.location_id,
            flag: self.flag,
            stack,
            runs,
            me: stored_level(self.me, self.item_id, "me", MAX_ME)?,
            te: stored_level(self.te, self.item_id, "te", MAX_TE)?,
        })
    }
}

/// A stored stack size that holds at least one blueprint.
fn stored_stack(value: i64, item: i64) -> Result<u32> {
    u32::try_from(value)
        .ok()
        .filter(|stack| *stack >= 1)
        .ok_or_else(|| bad_code(TABLE, item, "stack", value))
}

/// A stored run count of a copy: at least one run.
fn stored_runs(value: i64, item: i64) -> Result<u32> {
    u32::try_from(value)
        .ok()
        .filter(|runs| *runs >= 1)
        .ok_or_else(|| bad_code(TABLE, item, "runs", value))
}

/// A stored efficiency level within its range; anything else names the column it came from.
fn stored_level(value: i64, item: i64, column: &'static str, max: i64) -> Result<u8> {
    u8::try_from(value)
        .ok()
        .filter(|level| i64::from(*level) <= max)
        .ok_or_else(|| bad_code(TABLE, item, column, value))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    const CHARACTER: u64 = 90_000_001;
    const OTHER: u64 = 90_000_002;

    fn open_temp() -> (tempfile::TempDir, PathBuf, Store) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("eve.db");
        let store = Store::open(&path, 1_000).expect("open").store;
        (dir, path, store)
    }

    fn row(item_id: ItemId, type_id: TypeId, me: u8, te: u8) -> BlueprintRow {
        BlueprintRow {
            item_id,
            type_id,
            location_id: 60_003_760,
            flag: Some("Hangar".to_owned()),
            stack: 1,
            runs: None,
            me,
            te,
        }
    }

    /// The three rows of the acceptance: a copy and two originals, one of them a stack of three.
    fn fetched_rows() -> Vec<BlueprintRow> {
        vec![
            BlueprintRow {
                item_id: 1003,
                type_id: 2455,
                location_id: 60_003_760,
                flag: Some("Hangar".to_owned()),
                stack: 1,
                runs: Some(7),
                me: 10,
                te: 20,
            },
            BlueprintRow {
                item_id: 1007,
                type_id: 2455,
                location_id: 60_003_760,
                flag: Some("Hangar".to_owned()),
                stack: 1,
                runs: None,
                me: 9,
                te: 14,
            },
            BlueprintRow {
                item_id: 2002,
                type_id: 691,
                location_id: 60_003_760,
                flag: Some("Hangar".to_owned()),
                stack: 3,
                runs: None,
                me: 0,
                te: 0,
            },
        ]
    }

    #[test]
    fn a_save_is_read_back_by_item_id() {
        let (_dir, _path, mut store) = open_temp();
        // Written out of order; the read comes by item id.
        let mut rows = fetched_rows();
        rows.reverse();
        store
            .save_blueprints(CHARACTER, 1_000, &rows)
            .expect("save");
        let stored = store.blueprints(CHARACTER).expect("read").expect("some");
        assert_eq!(stored.character_id, CHARACTER);
        assert_eq!(stored.fetched_at, 1_000);
        assert_eq!(stored.rows, fetched_rows());
        assert_eq!(store.all_blueprints().expect("all").len(), 1);
    }

    #[test]
    fn a_new_save_replaces_the_rows_and_an_empty_one_keeps_the_time() {
        let (_dir, _path, mut store) = open_temp();
        store
            .save_blueprints(CHARACTER, 1_000, &fetched_rows())
            .expect("save");
        let kept = row(1003, 2455, 9, 20);
        store
            .save_blueprints(CHARACTER, 2_000, std::slice::from_ref(&kept))
            .expect("save again");
        let stored = store.blueprints(CHARACTER).expect("read").expect("some");
        assert_eq!(stored.fetched_at, 2_000);
        assert_eq!(stored.rows, vec![kept]);
        store
            .save_blueprints(CHARACTER, 3_000, &[])
            .expect("save empty");
        let stored = store.blueprints(CHARACTER).expect("read").expect("some");
        assert_eq!(stored.fetched_at, 3_000);
        assert!(stored.rows.is_empty());
    }

    #[test]
    fn characters_are_kept_apart_and_a_stranger_has_none() {
        let (_dir, _path, mut store) = open_temp();
        store
            .save_blueprints(CHARACTER, 1_000, &fetched_rows())
            .expect("save");
        let one = row(2003, 691, 0, 0);
        store
            .save_blueprints(OTHER, 1_500, std::slice::from_ref(&one))
            .expect("save other");
        let stored = store.blueprints(OTHER).expect("read").expect("some");
        assert_eq!(stored.character_id, OTHER);
        assert_eq!(stored.fetched_at, 1_500);
        assert_eq!(stored.rows, vec![one]);
        let all = store.all_blueprints().expect("all");
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].character_id, CHARACTER);
        assert_eq!(all[1].character_id, OTHER);
        assert!(store.blueprints(999).expect("read").is_none());
    }

    #[test]
    fn a_row_the_store_refuses_keeps_the_previous_list() {
        let (_dir, _path, mut store) = open_temp();
        store
            .save_blueprints(CHARACTER, 1_000, &fetched_rows())
            .expect("save");
        let too_much_me = row(3001, 34, 11, 0);
        let too_much_te = row(3001, 34, 0, 21);
        let mut no_stack = row(3001, 34, 0, 0);
        no_stack.stack = 0;
        let mut no_runs = row(3001, 34, 0, 0);
        no_runs.runs = Some(0);
        let mut stacked_copy = row(3001, 34, 0, 0);
        stacked_copy.runs = Some(7);
        stacked_copy.stack = 2;
        let mut twice = fetched_rows();
        twice.push(row(1003, 2455, 5, 5));
        for bad in [too_much_me, too_much_te, no_stack, no_runs, stacked_copy] {
            let refused = store.save_blueprints(CHARACTER, 2_000, std::slice::from_ref(&bad));
            assert!(
                matches!(refused, Err(StoreError::BadData(_))),
                "{refused:?}"
            );
            let stored = store.blueprints(CHARACTER).expect("read").expect("some");
            assert_eq!(stored.fetched_at, 1_000);
            assert_eq!(stored.rows, fetched_rows());
        }
        let refused = store.save_blueprints(CHARACTER, 2_000, &twice);
        assert!(
            matches!(refused, Err(StoreError::BadData(_))),
            "{refused:?}"
        );
        let stored = store.blueprints(CHARACTER).expect("read").expect("some");
        assert_eq!(stored.fetched_at, 1_000);
        assert_eq!(stored.rows, fetched_rows());
    }

    #[test]
    fn a_stored_value_outside_the_checks_is_bad_data() {
        let (_dir, _path, mut store) = open_temp();
        store
            .save_blueprints(CHARACTER, 1_000, &fetched_rows())
            .expect("save");
        // A table without the checks stands for a file another build wrote.
        store
            .conn
            .execute_batch(
                "DROP TABLE blueprints;
                 CREATE TABLE blueprints (
                     character_id INTEGER NOT NULL,
                     item_id INTEGER NOT NULL,
                     type_id INTEGER NOT NULL,
                     location_id INTEGER NOT NULL,
                     flag TEXT,
                     stack INTEGER NOT NULL,
                     runs INTEGER,
                     me INTEGER NOT NULL,
                     te INTEGER NOT NULL,
                     PRIMARY KEY (character_id, item_id)
                 )",
            )
            .expect("recreate");
        let bad = |stack: i64, runs: Option<i64>, me: i64, te: i64| {
            let runs = runs.map_or_else(|| "NULL".to_owned(), |runs| runs.to_string());
            format!(
                "INSERT INTO blueprints (character_id, item_id, type_id, location_id, flag, \
                 stack, runs, me, te) VALUES ({CHARACTER}, 4001, 34, 60003760, 'Hangar', \
                 {stack}, {runs}, {me}, {te})"
            )
        };
        for (insert, column) in [
            (bad(0, None, 0, 0), "stack"),
            (bad(1, Some(0), 0, 0), "runs"),
            (bad(1, None, 11, 0), "me"),
            (bad(1, None, 0, 21), "te"),
            (bad(2, Some(7), 0, 0), "runs"),
        ] {
            store.conn.execute_batch(&insert).expect("insert");
            let refused = store.blueprints(CHARACTER);
            assert!(
                matches!(&refused, Err(StoreError::BadData(text)) if text.contains(column)),
                "{refused:?}"
            );
            store
                .conn
                .execute("DELETE FROM blueprints WHERE item_id = 4001", [])
                .expect("delete");
        }
    }
}

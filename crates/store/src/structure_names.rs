//! Names the user types for structures that EVE does not name. They are the user's own data: kept
//! for good, not tied to a character, so removing a character leaves them in place.

use std::collections::BTreeMap;

use rusqlite::{OptionalExtension, params};

use crate::error::{Result, StoreError};
use crate::migrate::Store;

/// Structure ids start here; smaller ids are stations and systems, which the game data names.
const FIRST_STRUCTURE: i64 = 1_000_000_000_000;

/// The longest name, in characters, after trimming.
const MAX_CHARS: usize = 100;

impl Store {
    /// Sets the name the user typed for a structure. The name is trimmed; an empty one removes the
    /// name, so EVE's name shows again. `now` is the Unix time the name is saved with.
    pub fn set_structure_name(&mut self, id: i64, name: &str, now: i64) -> Result<()> {
        if id < FIRST_STRUCTURE {
            return Err(StoreError::NotAStructure(id));
        }
        let name = name.trim();
        if name.chars().count() > MAX_CHARS {
            return Err(StoreError::TooLarge);
        }
        let tx = self.conn.transaction()?;
        if name.is_empty() {
            tx.execute(
                "DELETE FROM structure_names WHERE structure_id = ?1",
                params![id],
            )?;
        } else {
            tx.execute(
                "INSERT INTO structure_names (structure_id, name, set_at) VALUES (?1, ?2, ?3) \
                 ON CONFLICT (structure_id) DO UPDATE SET name = ?2, set_at = ?3",
                params![id, name, now],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// The name the user typed for a structure, or `None` when there is none.
    pub fn structure_name(&mut self, id: i64) -> Result<Option<String>> {
        let tx = self.conn.transaction()?;
        let name = tx
            .query_row(
                "SELECT name FROM structure_names WHERE structure_id = ?1",
                params![id],
                |row| row.get(0),
            )
            .optional()?;
        tx.commit()?;
        Ok(name)
    }

    /// Every name the user typed, by structure id.
    pub fn structure_names(&mut self) -> Result<BTreeMap<i64, String>> {
        let tx = self.conn.transaction()?;
        let names = {
            let mut stmt =
                tx.prepare("SELECT structure_id, name FROM structure_names ORDER BY structure_id")?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })?;
            rows.collect::<rusqlite::Result<BTreeMap<_, _>>>()?
        };
        tx.commit()?;
        Ok(names)
    }
}

//! Each character's blueprints with their material and time efficiency, kept for the
//! what-to-build feed. A save replaces the character's whole list with the one just fetched.

use std::collections::BTreeSet;

use mop_core::industry::TypeId;
use mop_core::money::Money;
use mop_core::stock::{ItemId, LocationId};
use rusqlite::{OptionalExtension, Transaction, params};

use crate::error::{Result, StoreError};
use crate::migrate::Store;
use crate::{bad_code, from_stored, to_stored_u64};

const TABLE: &str = "blueprints";
const OVERRIDES: &str = "blueprint_overrides";
const PLAN_BLUEPRINTS: &str = "plan_blueprints";
const KEY_BLUEPRINT_DEFAULT: &str = "blueprint_default";
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

/// A blueprint whose ME and TE the user set by hand, and what they paid for it if anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlueprintOverride {
    pub blueprint: TypeId,
    pub me: u8,
    pub te: u8,
    pub price_paid: Option<Money>,
    pub set_at: i64,
}

/// A blueprint saved with a plan, with the ME, TE and price the plan's copy holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlueprintCopy {
    pub blueprint: TypeId,
    pub me: u8,
    pub te: u8,
    pub price_paid: Option<Money>,
    pub set_at: i64,
}

/// Where the blueprints a product needs come from when the character holds none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlueprintDefault {
    /// Buy them from NPCs.
    Npc,
    /// Make or buy copies at this ME and TE.
    Copies { me: u8, te: u8 },
    /// Use only the character's own blueprints.
    Own,
}

impl Default for BlueprintDefault {
    fn default() -> Self {
        Self::Copies { me: 10, te: 20 }
    }
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

    /// The saved first-visit answer, or `None` until one is given. A stored answer that cannot be
    /// read is `BadData`.
    pub fn blueprint_default(&self) -> Result<Option<BlueprintDefault>> {
        match self.setting(KEY_BLUEPRINT_DEFAULT)? {
            None => Ok(None),
            Some(text) => default_from_text(&text).map(Some),
        }
    }

    /// Saves the first-visit answer. An answer the table would not keep is `BadData` and nothing is
    /// written, so an earlier answer stays.
    pub fn set_blueprint_default(&mut self, answer: BlueprintDefault) -> Result<()> {
        let text = default_text(answer)?;
        self.set_setting(KEY_BLUEPRINT_DEFAULT, &text)
    }

    /// Every blueprint's override, by blueprint id.
    pub fn overrides(&mut self) -> Result<Vec<BlueprintOverride>> {
        let tx = self.conn.transaction()?;
        let found = read_overrides(&tx)?;
        tx.commit()?;
        Ok(found)
    }

    /// One blueprint's override, or `None` when it has none.
    pub fn override_for(&mut self, blueprint: TypeId) -> Result<Option<BlueprintOverride>> {
        let tx = self.conn.transaction()?;
        let found = read_override(&tx, blueprint)?;
        tx.commit()?;
        Ok(found)
    }

    /// Saves a blueprint's override, replacing the one saved before. A value outside the table's
    /// checks is `BadData` and nothing is written.
    pub fn set_override(&mut self, value: &BlueprintOverride) -> Result<()> {
        check_override(
            OVERRIDES,
            value.blueprint,
            value.me,
            value.te,
            value.price_paid,
        )?;
        self.conn.execute(
            "INSERT INTO blueprint_overrides (blueprint_id, me, te, price_paid, set_at) \
             VALUES (?1, ?2, ?3, ?4, ?5) \
             ON CONFLICT (blueprint_id) DO UPDATE SET me = excluded.me, te = excluded.te, \
                 price_paid = excluded.price_paid, set_at = excluded.set_at",
            params![
                i64::from(value.blueprint),
                i64::from(value.me),
                i64::from(value.te),
                value.price_paid.map(Money::ten_thousandths),
                value.set_at
            ],
        )?;
        Ok(())
    }

    /// Removes a blueprint's override. Removing one that was never saved is not an error.
    pub fn clear_override(&mut self, blueprint: TypeId) -> Result<()> {
        self.conn.execute(
            "DELETE FROM blueprint_overrides WHERE blueprint_id = ?1",
            params![i64::from(blueprint)],
        )?;
        Ok(())
    }

    /// A plan's saved blueprint copies, by blueprint id.
    pub fn plan_blueprints(&mut self, plan: i64) -> Result<Vec<BlueprintCopy>> {
        let tx = self.conn.transaction()?;
        let found = read_plan_blueprints(&tx, plan)?;
        tx.commit()?;
        Ok(found)
    }

    /// Replaces a plan's blueprint copies with `rows`, in blueprint id order. Every row and the
    /// duplicates are checked first; an unknown plan is `NotFound`. Nothing is written when the
    /// rows are refused. An empty list clears the plan's copies.
    pub fn save_plan_blueprints(&mut self, plan: i64, rows: &[BlueprintCopy]) -> Result<()> {
        for row in rows {
            check_override(
                PLAN_BLUEPRINTS,
                row.blueprint,
                row.me,
                row.te,
                row.price_paid,
            )?;
        }
        let mut seen = BTreeSet::new();
        for row in rows {
            if !seen.insert(row.blueprint) {
                return Err(StoreError::BadData(format!(
                    "plan {plan}: blueprint {} is listed twice",
                    row.blueprint
                )));
            }
        }
        let tx = self.conn.transaction()?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM plans WHERE id = ?1)",
            params![plan],
            |row| row.get(0),
        )?;
        if !exists {
            return Err(StoreError::NotFound);
        }
        tx.execute(
            "DELETE FROM plan_blueprints WHERE plan_id = ?1",
            params![plan],
        )?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO plan_blueprints (plan_id, blueprint_id, me, te, price_paid, set_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;
            for row in rows {
                stmt.execute(params![
                    plan,
                    i64::from(row.blueprint),
                    i64::from(row.me),
                    i64::from(row.te),
                    row.price_paid.map(Money::ten_thousandths),
                    row.set_at
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
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

/// The text an answer is stored as under `blueprint_default`.
fn default_text(answer: BlueprintDefault) -> Result<String> {
    match answer {
        BlueprintDefault::Npc => Ok("npc".to_owned()),
        BlueprintDefault::Own => Ok("own".to_owned()),
        BlueprintDefault::Copies { me, te } => {
            check_levels("blueprint_default", me, te)?;
            Ok(format!("copies:{me}:{te}"))
        }
    }
}

/// A stored answer that names no answer.
fn bad_default(text: &str, why: impl std::fmt::Display) -> StoreError {
    StoreError::BadData(format!(
        "setting {KEY_BLUEPRINT_DEFAULT} has the value {text:?}, which is not valid: {why}"
    ))
}

/// The answer saved under `blueprint_default`; a value that names no answer is `BadData`.
fn default_from_text(text: &str) -> Result<BlueprintDefault> {
    match text {
        "npc" => Ok(BlueprintDefault::Npc),
        "own" => Ok(BlueprintDefault::Own),
        _ => {
            let copies = text
                .strip_prefix("copies:")
                .ok_or_else(|| bad_default(text, "not npc, own or copies"))?;
            let (me, te) = copies
                .split_once(':')
                .ok_or_else(|| bad_default(text, "copies needs an ME and a TE"))?;
            let me: i64 = me.parse().map_err(|error| bad_default(text, error))?;
            let te: i64 = te.parse().map_err(|error| bad_default(text, error))?;
            let me = u8::try_from(me)
                .ok()
                .filter(|me| i64::from(*me) <= MAX_ME)
                .ok_or_else(|| bad_default(text, format!("ME {me} is above {MAX_ME}")))?;
            let te = u8::try_from(te)
                .ok()
                .filter(|te| i64::from(*te) <= MAX_TE)
                .ok_or_else(|| bad_default(text, format!("TE {te} is above {MAX_TE}")))?;
            Ok(BlueprintDefault::Copies { me, te })
        }
    }
}

/// Refuses ME and TE above the most a blueprint can have.
fn check_levels(name: &str, me: u8, te: u8) -> Result<()> {
    if i64::from(me) > MAX_ME {
        return Err(StoreError::BadData(format!(
            "{name}: material efficiency {me} is above {MAX_ME}"
        )));
    }
    if i64::from(te) > MAX_TE {
        return Err(StoreError::BadData(format!(
            "{name}: time efficiency {te} is above {MAX_TE}"
        )));
    }
    Ok(())
}

/// Refuses a value the override and plan-copy tables would not keep.
fn check_override(
    table: &str,
    blueprint: TypeId,
    me: u8,
    te: u8,
    price_paid: Option<Money>,
) -> Result<()> {
    check_levels(&format!("{table} blueprint {blueprint}"), me, te)?;
    if price_paid.is_some_and(|price| price < Money::ZERO) {
        return Err(StoreError::BadData(format!(
            "{table} blueprint {blueprint}: the price paid is below zero"
        )));
    }
    Ok(())
}

/// A stored ME and TE pair within the checks.
fn stored_levels(table: &str, row: i64, me: i64, te: i64) -> Result<(u8, u8)> {
    Ok((
        stored_level(table, me, row, "me", MAX_ME)?,
        stored_level(table, te, row, "te", MAX_TE)?,
    ))
}

/// A stored price; a negative one is `BadData` naming the column.
fn stored_price(table: &str, row: i64, price: Option<i64>) -> Result<Option<Money>> {
    match price {
        Some(price) if price < 0 => Err(bad_code(table, row, "price_paid", price)),
        Some(price) => Ok(Some(Money::from_ten_thousandths(price))),
        None => Ok(None),
    }
}

/// One stored override row.
fn stored_override(
    table: &str,
    blueprint: i64,
    me: i64,
    te: i64,
    price: Option<i64>,
    set_at: i64,
) -> Result<BlueprintOverride> {
    let blueprint = from_stored(blueprint, table, blueprint, "blueprint_id")?;
    let (me, te) = stored_levels(table, i64::from(blueprint), me, te)?;
    Ok(BlueprintOverride {
        blueprint,
        me,
        te,
        price_paid: stored_price(table, i64::from(blueprint), price)?,
        set_at,
    })
}

/// Every stored override, by blueprint id.
fn read_overrides(tx: &Transaction<'_>) -> Result<Vec<BlueprintOverride>> {
    let mut stmt = tx.prepare(
        "SELECT blueprint_id, me, te, price_paid, set_at \
         FROM blueprint_overrides ORDER BY blueprint_id",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, Option<i64>>(3)?,
            row.get::<_, i64>(4)?,
        ))
    })?;
    let mut found = Vec::new();
    for row in rows {
        let (blueprint, me, te, price, set_at) = row?;
        found.push(stored_override(
            OVERRIDES, blueprint, me, te, price, set_at,
        )?);
    }
    Ok(found)
}

/// The stored override of one blueprint, if there is one.
fn read_override(tx: &Transaction<'_>, blueprint: TypeId) -> Result<Option<BlueprintOverride>> {
    let row = tx
        .query_row(
            "SELECT blueprint_id, me, te, price_paid, set_at \
             FROM blueprint_overrides WHERE blueprint_id = ?1",
            params![i64::from(blueprint)],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            },
        )
        .optional()?;
    row.map(|(blueprint, me, te, price, set_at)| {
        stored_override(OVERRIDES, blueprint, me, te, price, set_at)
    })
    .transpose()
}

/// A plan's stored blueprint copies, by blueprint id.
fn read_plan_blueprints(tx: &Transaction<'_>, plan: i64) -> Result<Vec<BlueprintCopy>> {
    let mut stmt = tx.prepare(
        "SELECT blueprint_id, me, te, price_paid, set_at \
         FROM plan_blueprints WHERE plan_id = ?1 ORDER BY blueprint_id",
    )?;
    let rows = stmt.query_map(params![plan], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, Option<i64>>(3)?,
            row.get::<_, i64>(4)?,
        ))
    })?;
    let mut found = Vec::new();
    for row in rows {
        let (blueprint, me, te, price, set_at) = row?;
        let blueprint = from_stored(blueprint, PLAN_BLUEPRINTS, blueprint, "blueprint_id")?;
        let (me, te) = stored_levels(PLAN_BLUEPRINTS, i64::from(blueprint), me, te)?;
        found.push(BlueprintCopy {
            blueprint,
            me,
            te,
            price_paid: stored_price(PLAN_BLUEPRINTS, i64::from(blueprint), price)?,
            set_at,
        });
    }
    Ok(found)
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
            me: stored_level(TABLE, self.me, self.item_id, "me", MAX_ME)?,
            te: stored_level(TABLE, self.te, self.item_id, "te", MAX_TE)?,
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
fn stored_level(table: &str, value: i64, row: i64, column: &'static str, max: i64) -> Result<u8> {
    u8::try_from(value)
        .ok()
        .filter(|level| i64::from(*level) <= max)
        .ok_or_else(|| bad_code(table, row, column, value))
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

    fn isk(amount: i64) -> Money {
        Money::from_ten_thousandths(amount * 10_000)
    }

    fn answer(me: u8, te: u8, price: Option<Money>, set_at: i64) -> BlueprintOverride {
        BlueprintOverride {
            blueprint: 2455,
            me,
            te,
            price_paid: price,
            set_at,
        }
    }

    fn copy(blueprint: TypeId, me: u8, te: u8, price: Option<Money>, set_at: i64) -> BlueprintCopy {
        BlueprintCopy {
            blueprint,
            me,
            te,
            price_paid: price,
            set_at,
        }
    }

    fn plan_record(product: TypeId) -> crate::plans::PlanRecord {
        crate::plans::PlanRecord {
            id: 0,
            name: "Test plan".to_owned(),
            product,
            quantity: 10,
            build: BTreeSet::new(),
            me: std::collections::BTreeMap::new(),
            manufacturing: None,
            reactions: None,
            price_rule: mop_market::book::PriceRule::default(),
            route: None,
            service: None,
            packing: mop_core::freight::Packing::default(),
            pasted_stock: String::new(),
            created_at: 0,
            updated_at: 0,
        }
    }

    #[test]
    fn the_first_visit_answer_is_unset_until_saved_and_a_bad_one_keeps_the_last() {
        let (_dir, _path, mut store) = open_temp();
        assert_eq!(store.blueprint_default().expect("fresh"), None);

        store
            .set_blueprint_default(BlueprintDefault::Npc)
            .expect("npc");
        assert_eq!(
            store.blueprint_default().expect("npc"),
            Some(BlueprintDefault::Npc)
        );

        store
            .set_blueprint_default(BlueprintDefault::Copies { me: 8, te: 16 })
            .expect("copies");
        assert_eq!(
            store.blueprint_default().expect("copies"),
            Some(BlueprintDefault::Copies { me: 8, te: 16 })
        );
        assert_eq!(
            store
                .setting("blueprint_default")
                .expect("read the setting"),
            Some("copies:8:16".to_owned())
        );

        store
            .set_blueprint_default(BlueprintDefault::Own)
            .expect("own");
        assert_eq!(
            store.blueprint_default().expect("own"),
            Some(BlueprintDefault::Own)
        );

        let refused = store.set_blueprint_default(BlueprintDefault::Copies { me: 11, te: 16 });
        assert!(
            matches!(refused, Err(StoreError::BadData(_))),
            "{refused:?}"
        );
        assert_eq!(
            store.blueprint_default().expect("still own"),
            Some(BlueprintDefault::Own)
        );

        store
            .set_setting("blueprint_default", "copies:8:21")
            .expect("bad stored answer");
        assert!(matches!(
            store.blueprint_default(),
            Err(StoreError::BadData(_))
        ));
    }

    #[test]
    fn an_override_is_upserted_cleared_and_refused_before_writing() {
        let (_dir, _path, mut store) = open_temp();
        store
            .set_override(&answer(8, 16, Some(isk(500_000)), 1_000))
            .expect("first");
        assert_eq!(
            store.overrides().expect("one row"),
            vec![answer(8, 16, Some(isk(500_000)), 1_000)]
        );
        assert_eq!(
            store.override_for(2455).expect("by blueprint"),
            Some(answer(8, 16, Some(isk(500_000)), 1_000))
        );

        store
            .set_override(&answer(9, 14, None, 2_000))
            .expect("replace");
        assert_eq!(
            store.overrides().expect("still one row"),
            vec![answer(9, 14, None, 2_000)]
        );

        let before = store.overrides().expect("before");
        for bad in [
            answer(11, 14, None, 3_000),
            answer(9, 21, None, 3_000),
            answer(9, 14, Some(Money::from_ten_thousandths(-10_000)), 3_000),
        ] {
            let refused = store.set_override(&bad);
            assert!(
                matches!(refused, Err(StoreError::BadData(_))),
                "{refused:?}"
            );
            assert_eq!(store.overrides().expect("unchanged"), before);
        }

        store.clear_override(2455).expect("clear");
        assert!(store.overrides().expect("empty").is_empty());
        assert_eq!(store.override_for(2455).expect("gone"), None);
        store
            .clear_override(2456)
            .expect("clearing an absent one is fine");
    }

    #[test]
    fn a_stored_override_outside_the_checks_is_bad_data() {
        let (_dir, _path, mut store) = open_temp();
        // A table without the checks stands for a file another build wrote.
        store
            .conn
            .execute_batch(
                "DROP TABLE blueprint_overrides;
                 CREATE TABLE blueprint_overrides (
                     blueprint_id INTEGER PRIMARY KEY,
                     me INTEGER NOT NULL,
                     te INTEGER NOT NULL,
                     price_paid INTEGER,
                     set_at INTEGER NOT NULL
                 )",
            )
            .expect("recreate");
        for (me, te, price) in [(11, 0, "NULL"), (0, 21, "NULL"), (0, 0, "-1")] {
            store
                .conn
                .execute(
                    &format!(
                        "INSERT OR REPLACE INTO blueprint_overrides \
                         (blueprint_id, me, te, price_paid, set_at) \
                         VALUES (2455, {me}, {te}, {price}, 1000)"
                    ),
                    [],
                )
                .expect("insert");
            assert!(
                matches!(store.overrides(), Err(StoreError::BadData(_))),
                "me {me}, te {te}, price {price}"
            );
            assert!(
                matches!(store.override_for(2455), Err(StoreError::BadData(_))),
                "me {me}, te {te}, price {price}"
            );
        }
    }

    #[test]
    fn a_plans_copies_are_saved_in_blueprint_order_and_cleared() {
        let (_dir, _path, mut store) = open_temp();
        let plan = store
            .create_plan(&plan_record(2454), 1_000)
            .expect("create");
        assert_eq!(plan, 1);

        let first = copy(2455, 8, 16, None, 2_000);
        let second = copy(2457, 10, 20, Some(isk(1)), 2_001);
        store
            .save_plan_blueprints(plan, &[first, second])
            .expect("save");
        assert_eq!(
            store.plan_blueprints(plan).expect("read"),
            vec![first, second]
        );

        let before = store.plan_blueprints(plan).expect("before");
        let refused = store.save_plan_blueprints(plan, &[first, first]);
        assert!(
            matches!(refused, Err(StoreError::BadData(_))),
            "{refused:?}"
        );
        assert_eq!(store.plan_blueprints(plan).expect("unchanged"), before);
        let refused = store.save_plan_blueprints(999, &[first]);
        assert!(matches!(refused, Err(StoreError::NotFound)), "{refused:?}");
        assert_eq!(store.plan_blueprints(plan).expect("unchanged"), before);

        store.save_plan_blueprints(plan, &[]).expect("clear");
        assert!(store.plan_blueprints(plan).expect("empty").is_empty());

        store
            .save_plan_blueprints(plan, &[first, second])
            .expect("save again");
        store.delete_plan(plan).expect("delete");
        assert!(
            store
                .plan_blueprints(plan)
                .expect("the copies went with the plan")
                .is_empty()
        );
    }

    #[test]
    fn a_plans_stored_copy_outside_the_checks_is_bad_data() {
        let (_dir, _path, mut store) = open_temp();
        let plan = store
            .create_plan(&plan_record(2454), 1_000)
            .expect("create");
        store
            .conn
            .execute_batch(
                "DROP TABLE plan_blueprints;
                 CREATE TABLE plan_blueprints (
                     plan_id INTEGER NOT NULL,
                     blueprint_id INTEGER NOT NULL,
                     me INTEGER NOT NULL,
                     te INTEGER NOT NULL,
                     price_paid INTEGER,
                     set_at INTEGER NOT NULL,
                     PRIMARY KEY (plan_id, blueprint_id)
                 )",
            )
            .expect("recreate");
        for (me, te, price) in [(11, 0, "NULL"), (0, 21, "NULL"), (0, 0, "-1")] {
            store
                .conn
                .execute(
                    &format!(
                        "INSERT OR REPLACE INTO plan_blueprints \
                         (plan_id, blueprint_id, me, te, price_paid, set_at) \
                         VALUES ({plan}, 2455, {me}, {te}, {price}, 1000)"
                    ),
                    [],
                )
                .expect("insert");
            assert!(
                matches!(store.plan_blueprints(plan), Err(StoreError::BadData(_))),
                "me {me}, te {te}, price {price}"
            );
        }
    }

    #[test]
    fn a_plan_copy_outside_the_checks_is_refused_before_writing() {
        let (_dir, _path, mut store) = open_temp();
        let plan = store
            .create_plan(&plan_record(2454), 1_000)
            .expect("create");
        let kept = copy(2455, 8, 16, None, 2_000);
        store
            .save_plan_blueprints(plan, std::slice::from_ref(&kept))
            .expect("save");
        for bad in [
            copy(2457, 11, 20, None, 3_000),
            copy(2457, 10, 21, None, 3_000),
            copy(2457, 10, 20, Some(Money::from_ten_thousandths(-1)), 3_000),
        ] {
            let refused = store.save_plan_blueprints(plan, &[bad]);
            assert!(
                matches!(refused, Err(StoreError::BadData(_))),
                "{refused:?}"
            );
            assert_eq!(store.plan_blueprints(plan).expect("unchanged"), vec![kept]);
        }
    }
}

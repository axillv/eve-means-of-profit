//! Plan reservations: where each plan's stock comes from, the plans that reserve, and the running
//! jobs assigned to plans.

use std::collections::{BTreeMap, BTreeSet};

use profit_core::stock::{CharacterId, Places, StockScope};
use rusqlite::{OptionalExtension, Transaction, params};

use crate::error::{Result, StoreError};
use crate::migrate::Store;
use crate::{bad_code, missing_reference};

const TABLE: &str = "plans";

/// Where a plan's stock comes from: only the pasted list, or the characters and places of a scope.
///
/// The stock itself is not stored here; this is what reservation logic needs to know.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanStock {
    /// The plan reads stock from what the user pasted on it.
    Pasted,
    /// The plan reads stock from the chosen characters and places.
    Characters(StockScope),
}

/// The stock source of a plan, as the single stored code tells it.
fn source_of(tx: &Transaction<'_>, plan_id: i64) -> Result<Option<i64>> {
    tx.query_row(
        "SELECT stock_source FROM plans WHERE id = ?1",
        params![plan_id],
        |row| row.get(0),
    )
    .optional()
    .map_err(StoreError::from)
}

fn invalid(reason: &str) -> StoreError {
    StoreError::InvalidPlanStock(reason.to_owned())
}

/// The characters a plan takes stock from, or the empty set when none are listed.
fn read_characters(tx: &Transaction<'_>, plan_id: i64) -> Result<BTreeSet<CharacterId>> {
    let mut stmt =
        tx.prepare("SELECT character_id FROM plan_stock_characters WHERE plan_id = ?1")?;
    let rows = stmt
        .query_map(params![plan_id], |row| row.get::<_, i64>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows.into_iter().collect())
}

/// The places a plan takes stock from: all of them when no place is listed.
fn read_places(tx: &Transaction<'_>, plan_id: i64) -> Result<Places> {
    let mut stmt = tx.prepare("SELECT location_id FROM plan_stock_places WHERE plan_id = ?1")?;
    let rows = stmt
        .query_map(params![plan_id], |row| row.get::<_, i64>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if rows.is_empty() {
        return Ok(Places::All);
    }
    Ok(Places::Only(rows.into_iter().collect()))
}

/// Empties the stock scope tables of a plan.
fn clear_scope(tx: &Transaction<'_>, plan_id: i64) -> Result<()> {
    tx.execute(
        "DELETE FROM plan_stock_characters WHERE plan_id = ?1",
        params![plan_id],
    )?;
    tx.execute(
        "DELETE FROM plan_stock_places WHERE plan_id = ?1",
        params![plan_id],
    )?;
    Ok(())
}

/// Writes the scope rows of a plan over its old ones.
fn write_scope(
    tx: &Transaction<'_>,
    plan_id: i64,
    characters: &BTreeSet<CharacterId>,
    places: &Places,
) -> Result<()> {
    clear_scope(tx, plan_id)?;
    for character_id in characters {
        tx.execute(
            "INSERT INTO plan_stock_characters (plan_id, character_id) VALUES (?1, ?2)",
            params![plan_id, character_id],
        )?;
    }
    if let Places::Only(listed) = places {
        for location_id in listed {
            tx.execute(
                "INSERT INTO plan_stock_places (plan_id, location_id) VALUES (?1, ?2)",
                params![plan_id, location_id],
            )?;
        }
    }
    Ok(())
}

/// Copies the stock source and scope rows of a plan to its duplicate.
pub(crate) fn copy_scope(tx: &Transaction<'_>, plan_id: i64, new_id: i64) -> Result<()> {
    tx.execute(
        "UPDATE plans SET stock_source = (SELECT stock_source FROM plans WHERE id = ?1) \
         WHERE id = ?2",
        params![plan_id, new_id],
    )?;
    tx.execute(
        "INSERT INTO plan_stock_characters (plan_id, character_id) \
         SELECT ?2, character_id FROM plan_stock_characters WHERE plan_id = ?1",
        params![plan_id, new_id],
    )?;
    tx.execute(
        "INSERT INTO plan_stock_places (plan_id, location_id) \
         SELECT ?2, location_id FROM plan_stock_places WHERE plan_id = ?1",
        params![plan_id, new_id],
    )?;
    Ok(())
}

impl Store {
    /// The stock source of a plan: pasted, or the characters and places it reads. Reading a scope
    /// with no place rows gives all places.
    pub fn plan_stock(&mut self, id: i64) -> Result<PlanStock> {
        let tx = self.conn.transaction()?;
        let stock = match source_of(&tx, id)?.ok_or(StoreError::NotFound)? {
            0 => PlanStock::Pasted,
            1 => PlanStock::Characters(StockScope {
                characters: read_characters(&tx, id)?,
                places: read_places(&tx, id)?,
            }),
            code => return Err(bad_code(TABLE, id, "stock_source", code)),
        };
        tx.commit()?;
        Ok(stock)
    }

    /// Replaces the stock source of a plan and its scope rows: characters need at least one of
    /// their kind, chosen places at least one place. Pasted stock also turns Reserve off.
    /// Nothing is written when a check fails or the plan is unknown.
    pub fn set_plan_stock(&mut self, id: i64, stock: &PlanStock) -> Result<()> {
        let tx = self.conn.transaction()?;
        let exists = source_of(&tx, id)?;
        if exists.is_none() {
            return Err(StoreError::NotFound);
        }
        match stock {
            PlanStock::Pasted => {
                tx.execute(
                    "UPDATE plans SET stock_source = 0, reserve = 0 WHERE id = ?1",
                    params![id],
                )?;
                clear_scope(&tx, id)?;
            }
            PlanStock::Characters(scope) => {
                if scope.characters.is_empty() {
                    return Err(invalid("pick at least one character"));
                }
                if let Places::Only(places) = &scope.places
                    && places.is_empty()
                {
                    return Err(invalid("pick at least one place, or all places"));
                }
                tx.execute(
                    "UPDATE plans SET stock_source = 1 WHERE id = ?1",
                    params![id],
                )?;
                write_scope(&tx, id, &scope.characters, &scope.places)?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Turns the Reserve tick of a plan on or off. Turning it on needs stock from characters.
    pub fn set_reserve(&mut self, id: i64, on: bool) -> Result<()> {
        let tx = self.conn.transaction()?;
        let Some(source) = source_of(&tx, id)? else {
            return Err(StoreError::NotFound);
        };
        let from_characters = match source {
            0 => false,
            1 => true,
            code => return Err(bad_code(TABLE, id, "stock_source", code)),
        };
        if on && !from_characters {
            return Err(invalid("reserve needs stock from characters"));
        }
        tx.execute(
            "UPDATE plans SET reserve = ?2 WHERE id = ?1",
            params![id, on],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// The plans with Reserve on, in Plans list order.
    pub fn reserving_plans(&mut self) -> Result<Vec<i64>> {
        let tx = self.conn.transaction()?;
        let ids = {
            let mut stmt =
                tx.prepare("SELECT id FROM plans WHERE reserve = 1 ORDER BY position, id")?;
            stmt.query_map([], |row| row.get::<_, i64>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        tx.commit()?;
        Ok(ids)
    }

    /// Assigns a running job to a plan, replacing any plan it had. An unknown plan is refused.
    pub fn assign_job(&mut self, job_id: i64, plan_id: i64, now: i64) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT OR REPLACE INTO job_assignments (job_id, plan_id, assigned_at) \
             VALUES (?1, ?2, ?3)",
            params![job_id, plan_id, now],
        )
        .map_err(missing_reference)?;
        tx.commit()?;
        Ok(())
    }

    /// Removes the assignment of a job; a job without one is fine.
    pub fn unassign_job(&mut self, job_id: i64) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "DELETE FROM job_assignments WHERE job_id = ?1",
            params![job_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Every assignment: the running job, then the plan it is assigned to.
    pub fn job_assignments(&mut self) -> Result<BTreeMap<i64, i64>> {
        let tx = self.conn.transaction()?;
        let rows = {
            let mut stmt = tx.prepare("SELECT job_id, plan_id FROM job_assignments")?;
            stmt.query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        tx.commit()?;
        Ok(rows.into_iter().collect())
    }
}

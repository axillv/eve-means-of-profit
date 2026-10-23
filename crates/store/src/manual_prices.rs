//! Prices the player typed for the buy lines of a plan. They stay with the plan until they are
//! cleared or the plan is deleted.

use mop_core::industry::TypeId;
use mop_core::money::Money;
use rusqlite::params;

use crate::error::{Result, StoreError};
use crate::migrate::Store;
use crate::{from_stored, missing_reference};

const TABLE: &str = "plan_manual_prices";

/// A price typed for one item on a plan: the price per unit and when it was typed (Unix seconds).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ManualPrice {
    pub type_id: TypeId,
    pub per_unit: Money,
    pub set_at: i64,
}

impl Store {
    /// The prices typed for a plan, by item id.
    pub fn manual_prices(&mut self, plan: i64) -> Result<Vec<ManualPrice>> {
        let tx = self.conn.transaction()?;
        let raw = {
            let mut stmt = tx.prepare(
                "SELECT type_id, per_unit, set_at FROM plan_manual_prices \
                 WHERE plan = ?1 ORDER BY type_id",
            )?;
            let rows = stmt.query_map(params![plan], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        let prices = raw
            .into_iter()
            .map(|(type_id, per_unit, set_at)| {
                let type_id = from_stored(type_id, TABLE, plan, "type_id")?;
                if per_unit <= 0 {
                    return Err(StoreError::BadData(format!(
                        "{TABLE} row (plan {plan}, type {type_id}), column per_unit: \
                         {per_unit} is not a price"
                    )));
                }
                Ok(ManualPrice {
                    type_id,
                    per_unit: Money::from_ten_thousandths(per_unit),
                    set_at,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        tx.commit()?;
        Ok(prices)
    }

    /// Sets the price typed for an item on a plan, replacing an earlier one. A price that is not
    /// above zero is refused and nothing is written.
    pub fn set_manual_price(
        &mut self,
        plan: i64,
        type_id: TypeId,
        per_unit: Money,
        set_at: i64,
    ) -> Result<()> {
        if per_unit.ten_thousandths() <= 0 {
            return Err(StoreError::BadPrice);
        }
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO plan_manual_prices (plan, type_id, per_unit, set_at) \
             VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT (plan, type_id) DO UPDATE SET \
                per_unit = excluded.per_unit, set_at = excluded.set_at",
            params![plan, i64::from(type_id), per_unit.ten_thousandths(), set_at],
        )
        .map_err(missing_reference)?;
        tx.commit()?;
        Ok(())
    }

    /// Removes the price typed for an item on a plan. Nothing is removed when there is none.
    pub fn clear_manual_price(&mut self, plan: i64, type_id: TypeId) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "DELETE FROM plan_manual_prices WHERE plan = ?1 AND type_id = ?2",
            params![plan, i64::from(type_id)],
        )?;
        tx.commit()?;
        Ok(())
    }
}

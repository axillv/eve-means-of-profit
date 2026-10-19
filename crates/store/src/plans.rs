//! Plans: what to build, where, how it is priced and how it is shipped.

use std::collections::{BTreeMap, BTreeSet};

use profit_core::freight::Packing;
use profit_core::industry::TypeId;
use profit_market::book::PriceRule;
use rusqlite::{OptionalExtension, Row, Transaction, params};

use crate::error::{Result, StoreError};
use crate::migrate::Store;
use crate::{
    bad_code, clean_name, copy_scope, ensure_free, from_stored, id_names, is_taken,
    missing_reference, to_stored_u64,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanRecord {
    pub id: i64,
    pub name: String,
    pub product: TypeId,
    pub quantity: u64,
    pub build: BTreeSet<TypeId>,
    /// Only the ME values the user set; the Build page applies the defaults.
    pub me: BTreeMap<TypeId, u8>,
    pub manufacturing: Option<i64>,
    pub reactions: Option<i64>,
    pub price_rule: PriceRule,
    pub route: Option<i64>,
    /// `None` means the cheapest service.
    pub service: Option<i64>,
    pub packing: Packing,
    pub pasted_stock: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanSummary {
    pub id: i64,
    pub name: String,
    pub product: TypeId,
    pub quantity: u64,
    pub updated_at: i64,
}

const TABLE: &str = "plans";

pub(crate) fn price_rule_code(rule: PriceRule) -> i64 {
    match rule {
        PriceRule::WalkSell => 0,
        PriceRule::LowestSell => 1,
        PriceRule::HighestBuy => 2,
    }
}

pub(crate) fn price_rule_from_code(code: i64) -> Option<PriceRule> {
    match code {
        0 => Some(PriceRule::WalkSell),
        1 => Some(PriceRule::LowestSell),
        2 => Some(PriceRule::HighestBuy),
        _ => None,
    }
}

pub(crate) fn packing_code(packing: Packing) -> i64 {
    match packing {
        Packing::FillOnly => 0,
        Packing::TopUp => 1,
    }
}

fn packing_from_code(code: i64) -> Option<Packing> {
    match code {
        0 => Some(Packing::FillOnly),
        1 => Some(Packing::TopUp),
        _ => None,
    }
}

/// The names of the plans whose `condition` holds for `id`, in list order.
pub(crate) fn plans_using(tx: &Transaction<'_>, condition: &str, id: i64) -> Result<Vec<String>> {
    let sql = format!("SELECT id, name FROM plans WHERE {condition}");
    let mut rows = id_names(tx, &sql, params![id])?;
    rows.sort_by_cached_key(|(id, name)| (name.to_lowercase(), *id));
    Ok(rows.into_iter().map(|(_, name)| name).collect())
}

/// The stored columns of one plan, before they are turned back into Rust types.
struct RawPlan {
    id: i64,
    name: String,
    product: i64,
    quantity: i64,
    manufacturing: Option<i64>,
    reactions: Option<i64>,
    price_rule: i64,
    route: Option<i64>,
    service: Option<i64>,
    packing: i64,
    pasted_stock: String,
    created_at: i64,
    updated_at: i64,
}

fn raw_plan(row: &Row<'_>) -> rusqlite::Result<RawPlan> {
    Ok(RawPlan {
        id: row.get(0)?,
        name: row.get(1)?,
        product: row.get(2)?,
        quantity: row.get(3)?,
        manufacturing: row.get(4)?,
        reactions: row.get(5)?,
        price_rule: row.get(6)?,
        route: row.get(7)?,
        service: row.get(8)?,
        packing: row.get(9)?,
        pasted_stock: row.get(10)?,
        created_at: row.get(11)?,
        updated_at: row.get(12)?,
    })
}

const PLAN_COLUMNS: &str = "id, name, product, quantity, manufacturing, reactions, price_rule, \
    route, service, packing, pasted_stock, created_at, updated_at";

fn read_plan(tx: &Transaction<'_>, id: i64) -> Result<Option<PlanRecord>> {
    let raw = tx
        .query_row(
            &format!("SELECT {PLAN_COLUMNS} FROM plans WHERE id = ?1"),
            params![id],
            raw_plan,
        )
        .optional()?;
    let Some(raw) = raw else {
        return Ok(None);
    };
    let build = read_type_ids(tx, id)?;
    let me = read_me(tx, id)?;
    let price_rule = price_rule_from_code(raw.price_rule)
        .ok_or_else(|| bad_code(TABLE, id, "price_rule", raw.price_rule))?;
    let packing = packing_from_code(raw.packing)
        .ok_or_else(|| bad_code(TABLE, id, "packing", raw.packing))?;
    Ok(Some(PlanRecord {
        id: raw.id,
        name: raw.name,
        product: from_stored(raw.product, TABLE, id, "product")?,
        quantity: from_stored(raw.quantity, TABLE, id, "quantity")?,
        build,
        me,
        manufacturing: raw.manufacturing,
        reactions: raw.reactions,
        price_rule,
        route: raw.route,
        service: raw.service,
        packing,
        pasted_stock: raw.pasted_stock,
        created_at: raw.created_at,
        updated_at: raw.updated_at,
    }))
}

fn read_type_ids(tx: &Transaction<'_>, plan_id: i64) -> Result<BTreeSet<TypeId>> {
    let mut stmt = tx.prepare("SELECT type_id FROM plan_build WHERE plan_id = ?1")?;
    let raw = stmt
        .query_map(params![plan_id], |row| row.get::<_, i64>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    raw.into_iter()
        .map(|type_id| from_stored(type_id, "plan_build", plan_id, "type_id"))
        .collect()
}

fn read_me(tx: &Transaction<'_>, plan_id: i64) -> Result<BTreeMap<TypeId, u8>> {
    let mut stmt = tx.prepare("SELECT type_id, me FROM plan_me WHERE plan_id = ?1")?;
    let raw = stmt
        .query_map(params![plan_id], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    raw.into_iter()
        .map(|(type_id, me)| {
            Ok((
                from_stored(type_id, "plan_me", plan_id, "type_id")?,
                from_stored(me, "plan_me", plan_id, "me")?,
            ))
        })
        .collect()
}

/// Replaces the build set and the ME values of a plan.
fn write_sets(tx: &Transaction<'_>, id: i64, plan: &PlanRecord) -> Result<()> {
    tx.execute("DELETE FROM plan_build WHERE plan_id = ?1", params![id])?;
    tx.execute("DELETE FROM plan_me WHERE plan_id = ?1", params![id])?;
    for type_id in &plan.build {
        tx.execute(
            "INSERT INTO plan_build (plan_id, type_id) VALUES (?1, ?2)",
            params![id, i64::from(*type_id)],
        )?;
    }
    for (type_id, me) in &plan.me {
        tx.execute(
            "INSERT INTO plan_me (plan_id, type_id, me) VALUES (?1, ?2, ?3)",
            params![id, i64::from(*type_id), i64::from(*me)],
        )?;
    }
    Ok(())
}

/// Inserts a new plan row with both times set to `now`, then its sets. The plan goes last in the
/// list. Returns its id.
fn insert_plan(tx: &Transaction<'_>, plan: &PlanRecord, name: &str, now: i64) -> Result<i64> {
    let quantity = to_stored_u64(plan.quantity)?;
    tx.execute(
        "INSERT INTO plans (name, product, quantity, manufacturing, reactions, price_rule, route, \
            service, packing, pasted_stock, created_at, updated_at, position) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11, \
            (SELECT COALESCE(MAX(position), -1) + 1 FROM plans))",
        params![
            name,
            i64::from(plan.product),
            quantity,
            plan.manufacturing,
            plan.reactions,
            price_rule_code(plan.price_rule),
            plan.route,
            plan.service,
            packing_code(plan.packing),
            plan.pasted_stock,
            now
        ],
    )
    .map_err(missing_reference)?;
    let id = tx.last_insert_rowid();
    write_sets(tx, id, plan)?;
    Ok(id)
}

fn plan_names(tx: &Transaction<'_>) -> Result<Vec<(i64, String)>> {
    id_names(tx, "SELECT id, name FROM plans", [])
}

impl Store {
    /// Adds a plan. The id, `created_at` and `updated_at` of `plan` are ignored; `now` is used.
    pub fn create_plan(&mut self, plan: &PlanRecord, now: i64) -> Result<i64> {
        let tx = self.conn.transaction()?;
        let name = clean_name(&plan.name)?;
        ensure_free(&plan_names(&tx)?, &name, None)?;
        let id = insert_plan(&tx, plan, &name, now)?;
        tx.commit()?;
        Ok(id)
    }

    /// Saves a plan over the one with its id. `created_at` is kept; `updated_at` becomes `now`.
    pub fn save_plan(&mut self, plan: &PlanRecord, now: i64) -> Result<()> {
        let tx = self.conn.transaction()?;
        let name = clean_name(&plan.name)?;
        ensure_free(&plan_names(&tx)?, &name, Some(plan.id))?;
        let quantity = to_stored_u64(plan.quantity)?;
        let changed = tx
            .execute(
                "UPDATE plans SET name = ?2, product = ?3, quantity = ?4, manufacturing = ?5, \
                    reactions = ?6, price_rule = ?7, route = ?8, service = ?9, packing = ?10, \
                    pasted_stock = ?11, updated_at = ?12 \
                 WHERE id = ?1",
                params![
                    plan.id,
                    name,
                    i64::from(plan.product),
                    quantity,
                    plan.manufacturing,
                    plan.reactions,
                    price_rule_code(plan.price_rule),
                    plan.route,
                    plan.service,
                    packing_code(plan.packing),
                    plan.pasted_stock,
                    now
                ],
            )
            .map_err(missing_reference)?;
        if changed == 0 {
            return Err(StoreError::NotFound);
        }
        write_sets(&tx, plan.id, plan)?;
        tx.commit()?;
        Ok(())
    }

    pub fn plan(&mut self, id: i64) -> Result<Option<PlanRecord>> {
        let tx = self.conn.transaction()?;
        let plan = read_plan(&tx, id)?;
        tx.commit()?;
        Ok(plan)
    }

    /// All plans, in list order: by position, then id.
    pub fn plans(&mut self) -> Result<Vec<PlanSummary>> {
        let tx = self.conn.transaction()?;
        let raw = {
            let mut stmt = tx.prepare(
                "SELECT id, name, product, quantity, updated_at FROM plans ORDER BY position, id",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        let plans = raw
            .into_iter()
            .map(|(id, name, product, quantity, updated_at)| {
                Ok(PlanSummary {
                    id,
                    name,
                    product: from_stored(product, TABLE, id, "product")?,
                    quantity: from_stored(quantity, TABLE, id, "quantity")?,
                    updated_at,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        tx.commit()?;
        Ok(plans)
    }

    /// Puts the plans in the given order. The list must hold every plan exactly once; otherwise
    /// nothing is written.
    pub fn set_plan_order(&mut self, order: &[i64]) -> Result<()> {
        let tx = self.conn.transaction()?;
        let known: BTreeSet<i64> = plan_names(&tx)?.into_iter().map(|(id, _)| id).collect();
        let asked: BTreeSet<i64> = order.iter().copied().collect();
        if order.len() != known.len() || asked != known {
            return Err(StoreError::NotFound);
        }
        for (position, id) in (0_i64..).zip(order) {
            tx.execute(
                "UPDATE plans SET position = ?2 WHERE id = ?1",
                params![id, position],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn rename_plan(&mut self, id: i64, name: &str) -> Result<()> {
        let tx = self.conn.transaction()?;
        let name = clean_name(name)?;
        ensure_free(&plan_names(&tx)?, &name, Some(id))?;
        let changed = tx.execute(
            "UPDATE plans SET name = ?2 WHERE id = ?1",
            params![id, name],
        )?;
        if changed == 0 {
            return Err(StoreError::NotFound);
        }
        tx.commit()?;
        Ok(())
    }

    /// Copies a plan as `<name> (copy)`, or the first free `<name> (copy N)` after it, at the end
    /// of the list. The copy keeps the stock source and scope, has Reserve off and no assignments.
    pub fn duplicate_plan(&mut self, id: i64, now: i64) -> Result<i64> {
        let tx = self.conn.transaction()?;
        let Some(plan) = read_plan(&tx, id)? else {
            return Err(StoreError::NotFound);
        };
        let taken = plan_names(&tx)?;
        let mut copy = 1_u32;
        let name = loop {
            let candidate = if copy == 1 {
                format!("{} (copy)", plan.name)
            } else {
                format!("{} (copy {copy})", plan.name)
            };
            if !is_taken(&taken, &candidate, None) {
                break candidate;
            }
            copy += 1;
        };
        let new_id = insert_plan(&tx, &plan, &name, now)?;
        copy_scope(&tx, id, new_id)?;
        tx.commit()?;
        Ok(new_id)
    }

    pub fn delete_plan(&mut self, id: i64) -> Result<()> {
        let tx = self.conn.transaction()?;
        let changed = tx.execute("DELETE FROM plans WHERE id = ?1", params![id])?;
        if changed == 0 {
            return Err(StoreError::NotFound);
        }
        tx.commit()?;
        Ok(())
    }
}

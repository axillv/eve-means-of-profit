//! Plans: what to build, where, how it is priced and how it is shipped.

use std::collections::{BTreeMap, BTreeSet};

use mop_core::freight::Packing;
use mop_core::industry::TypeId;
use mop_market::book::PriceRule;
use rusqlite::{OptionalExtension, Row, Transaction, params};

use crate::error::{Result, StoreError};
use crate::invention::copy_invention;
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

/// How far a plan has got, in the order it advances: automatically from game data, or by hand.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum PlanStage {
    #[default]
    Planned,
    Buying,
    Building,
    Done,
    Sold,
}

impl PlanStage {
    /// The word the row shows for this stage.
    pub fn label(self) -> &'static str {
        match self {
            Self::Planned => "Planned",
            Self::Buying => "Buying",
            Self::Building => "Building",
            Self::Done => "Done",
            Self::Sold => "Sold",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanSummary {
    pub id: i64,
    pub name: String,
    pub product: TypeId,
    pub quantity: u64,
    pub updated_at: i64,
    /// The furthest stage the game data has shown.
    pub stage: PlanStage,
    /// A stage the user picked by hand, which the row shows instead.
    pub stage_override: Option<PlanStage>,
}

impl PlanSummary {
    /// The stage the row shows: the user's choice when there is one, else the automatic stage.
    pub fn effective_stage(&self) -> PlanStage {
        self.stage_override.unwrap_or(self.stage)
    }
}

/// How a plan sells its product: a sell order at the Jita lowest sell, or into Jita buy orders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SellMode {
    #[default]
    SellOrder,
    BuyOrders,
}

const TABLE: &str = "plans";

fn sell_mode_code(mode: SellMode) -> i64 {
    match mode {
        SellMode::SellOrder => 0,
        SellMode::BuyOrders => 1,
    }
}

fn sell_mode_from_code(code: i64) -> Option<SellMode> {
    match code {
        0 => Some(SellMode::SellOrder),
        1 => Some(SellMode::BuyOrders),
        _ => None,
    }
}

fn stage_code(stage: PlanStage) -> i64 {
    match stage {
        PlanStage::Planned => 0,
        PlanStage::Buying => 1,
        PlanStage::Building => 2,
        PlanStage::Done => 3,
        PlanStage::Sold => 4,
    }
}

fn stage_from_code(code: i64) -> Option<PlanStage> {
    match code {
        0 => Some(PlanStage::Planned),
        1 => Some(PlanStage::Buying),
        2 => Some(PlanStage::Building),
        3 => Some(PlanStage::Done),
        4 => Some(PlanStage::Sold),
        _ => None,
    }
}

pub(crate) fn price_rule_code(rule: PriceRule) -> i64 {
    match rule {
        PriceRule::WalkSell => 0,
        PriceRule::LowestSell => 1,
        PriceRule::HighestBuy => 2,
        PriceRule::SellFivePercent => 3,
        PriceRule::SellWeightedAverage => 4,
        PriceRule::SellMedian => 5,
        PriceRule::BuyFivePercent => 6,
        PriceRule::BuyWeightedAverage => 7,
        PriceRule::BuyMedian => 8,
    }
}

pub(crate) fn price_rule_from_code(code: i64) -> Option<PriceRule> {
    match code {
        0 => Some(PriceRule::WalkSell),
        1 => Some(PriceRule::LowestSell),
        2 => Some(PriceRule::HighestBuy),
        3 => Some(PriceRule::SellFivePercent),
        4 => Some(PriceRule::SellWeightedAverage),
        5 => Some(PriceRule::SellMedian),
        6 => Some(PriceRule::BuyFivePercent),
        7 => Some(PriceRule::BuyWeightedAverage),
        8 => Some(PriceRule::BuyMedian),
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
                "SELECT id, name, product, quantity, updated_at, stage, stage_override \
                 FROM plans ORDER BY position, id",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, Option<i64>>(6)?,
                ))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        let plans = raw
            .into_iter()
            .map(
                |(id, name, product, quantity, updated_at, stage, stage_override)| {
                    let stage = stage_from_code(stage)
                        .ok_or_else(|| bad_code(TABLE, id, "stage", stage))?;
                    let stage_override = stage_override
                        .map(|code| {
                            stage_from_code(code)
                                .ok_or_else(|| bad_code(TABLE, id, "stage_override", code))
                        })
                        .transpose()?;
                    Ok(PlanSummary {
                        id,
                        name,
                        product: from_stored(product, TABLE, id, "product")?,
                        quantity: from_stored(quantity, TABLE, id, "quantity")?,
                        updated_at,
                        stage,
                        stage_override,
                    })
                },
            )
            .collect::<Result<Vec<_>>>()?;
        tx.commit()?;
        Ok(plans)
    }

    /// Raises a plan's automatic stage to `to`. A stage below the stored one writes nothing, so the
    /// automatic stage never goes backwards. An unknown plan is `NotFound`.
    pub fn advance_plan_stage(&mut self, id: i64, to: PlanStage) -> Result<()> {
        let tx = self.conn.transaction()?;
        let stored: Option<i64> = tx
            .query_row(
                "SELECT stage FROM plans WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .optional()?;
        let Some(stored) = stored else {
            return Err(StoreError::NotFound);
        };
        let stage = stage_from_code(stored).ok_or_else(|| bad_code(TABLE, id, "stage", stored))?;
        if to > stage {
            tx.execute(
                "UPDATE plans SET stage = ?2 WHERE id = ?1",
                params![id, stage_code(to)],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Sets the stage the row shows, or clears it with `None` so the automatic stage shows again. An
    /// unknown plan is `NotFound`.
    pub fn set_plan_stage_override(&mut self, id: i64, stage: Option<PlanStage>) -> Result<()> {
        let tx = self.conn.transaction()?;
        let changed = tx.execute(
            "UPDATE plans SET stage_override = ?2 WHERE id = ?1",
            params![id, stage.map(stage_code)],
        )?;
        if changed == 0 {
            return Err(StoreError::NotFound);
        }
        tx.commit()?;
        Ok(())
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

    pub fn plan_sell_mode(&mut self, id: i64) -> Result<SellMode> {
        let tx = self.conn.transaction()?;
        let code: i64 = tx
            .query_row(
                "SELECT sell_mode FROM plans WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or(StoreError::NotFound)?;
        let mode =
            sell_mode_from_code(code).ok_or_else(|| bad_code(TABLE, id, "sell_mode", code))?;
        tx.commit()?;
        Ok(mode)
    }

    pub fn set_plan_sell_mode(&mut self, id: i64, mode: SellMode) -> Result<()> {
        let tx = self.conn.transaction()?;
        let changed = tx.execute(
            "UPDATE plans SET sell_mode = ?2 WHERE id = ?1",
            params![id, sell_mode_code(mode)],
        )?;
        if changed == 0 {
            return Err(StoreError::NotFound);
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
        copy_invention(&tx, id, new_id)?;
        tx.execute(
            "UPDATE plans SET sell_mode = (SELECT sell_mode FROM plans WHERE id = ?1) WHERE id = ?2",
            params![id, new_id],
        )?;
        tx.commit()?;
        Ok(new_id)
    }

    pub fn delete_plan(&mut self, id: i64) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "DELETE FROM plan_manual_prices WHERE plan = ?1",
            params![id],
        )?;
        let changed = tx.execute("DELETE FROM plans WHERE id = ?1", params![id])?;
        if changed == 0 {
            return Err(StoreError::NotFound);
        }
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PRODUCT: TypeId = 2456;

    fn open_temp() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("eve.db");
        let store = Store::open(&path, 1_000).expect("open").store;
        (dir, store)
    }

    fn record() -> PlanRecord {
        PlanRecord {
            id: 0,
            name: "Test plan".to_owned(),
            product: PRODUCT,
            quantity: 10,
            build: BTreeSet::new(),
            me: BTreeMap::new(),
            manufacturing: None,
            reactions: None,
            price_rule: PriceRule::default(),
            route: None,
            service: None,
            packing: Packing::default(),
            pasted_stock: String::new(),
            created_at: 0,
            updated_at: 0,
        }
    }

    fn summary(store: &mut Store, id: i64) -> PlanSummary {
        store
            .plans()
            .expect("read the plans")
            .into_iter()
            .find(|plan| plan.id == id)
            .expect("the plan")
    }

    #[test]
    fn a_new_plan_starts_planned_and_advances_only_forwards() {
        let (_dir, mut store) = open_temp();
        let id = store.create_plan(&record(), 1_000).expect("create");

        let plan = summary(&mut store, id);
        assert_eq!(plan.stage, PlanStage::Planned);
        assert_eq!(plan.stage_override, None);
        assert_eq!(plan.effective_stage(), PlanStage::Planned);

        store
            .advance_plan_stage(id, PlanStage::Building)
            .expect("advance to Building");
        assert_eq!(summary(&mut store, id).stage, PlanStage::Building);

        store
            .advance_plan_stage(id, PlanStage::Planned)
            .expect("advance to Planned writes nothing");
        assert_eq!(summary(&mut store, id).stage, PlanStage::Building);

        store
            .advance_plan_stage(id, PlanStage::Done)
            .expect("advance to Done");
        assert_eq!(summary(&mut store, id).stage, PlanStage::Done);
    }

    #[test]
    fn an_override_shows_over_the_automatic_stage_until_cleared() {
        let (_dir, mut store) = open_temp();
        let id = store.create_plan(&record(), 1_000).expect("create");
        store
            .advance_plan_stage(id, PlanStage::Done)
            .expect("advance to Done");

        store
            .set_plan_stage_override(id, Some(PlanStage::Sold))
            .expect("set the override");
        let plan = summary(&mut store, id);
        assert_eq!(plan.stage, PlanStage::Done);
        assert_eq!(plan.stage_override, Some(PlanStage::Sold));
        assert_eq!(plan.effective_stage(), PlanStage::Sold);

        store
            .set_plan_stage_override(id, None)
            .expect("clear the override");
        let plan = summary(&mut store, id);
        assert_eq!(plan.stage, PlanStage::Done);
        assert_eq!(plan.stage_override, None);
        assert_eq!(plan.effective_stage(), PlanStage::Done);
    }

    #[test]
    fn an_unknown_plan_is_not_found_and_changes_nothing() {
        let (_dir, mut store) = open_temp();
        let id = store.create_plan(&record(), 1_000).expect("create");
        store
            .advance_plan_stage(id, PlanStage::Done)
            .expect("advance to Done");
        store
            .set_plan_stage_override(id, Some(PlanStage::Sold))
            .expect("set the override");
        let before = summary(&mut store, id);

        assert!(matches!(
            store.advance_plan_stage(999, PlanStage::Sold),
            Err(StoreError::NotFound)
        ));
        assert!(matches!(
            store.set_plan_stage_override(999, Some(PlanStage::Planned)),
            Err(StoreError::NotFound)
        ));
        assert!(matches!(
            store.set_plan_stage_override(999, None),
            Err(StoreError::NotFound)
        ));
        assert_eq!(summary(&mut store, id), before);
    }

    #[test]
    fn a_copy_starts_planned_without_an_override() {
        let (_dir, mut store) = open_temp();
        let id = store.create_plan(&record(), 1_000).expect("create");
        store
            .advance_plan_stage(id, PlanStage::Sold)
            .expect("advance to Sold");
        store
            .set_plan_stage_override(id, Some(PlanStage::Building))
            .expect("set the override");

        let copy = store.duplicate_plan(id, 2_000).expect("duplicate");
        let copied = summary(&mut store, copy);
        assert_eq!(copied.stage, PlanStage::Planned);
        assert_eq!(copied.stage_override, None);
        assert_eq!(copied.effective_stage(), PlanStage::Planned);
    }
}

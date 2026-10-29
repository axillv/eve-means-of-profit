//! The list of assets, orders and jobs as each refresh saw them: what appeared, changed or is
//! gone, by EVE id. Each kind keeps the latest state per character and appends the differences.

use std::collections::BTreeMap;

use mop_core::industry::TypeId;
use mop_core::money::Money;
use rusqlite::{Transaction, params};

use crate::error::{Result, StoreError};
use crate::migrate::Store;
use crate::{bad_code, from_stored, to_stored_u64};

const ASSETS: &str = "asset_state";
const ORDERS: &str = "order_state";
const JOBS: &str = "job_state";
const CHANGES: &str = "change_list";

/// The kind of thing a change list holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Asset,
    Order,
    Job,
}

impl Kind {
    fn code(self) -> i64 {
        match self {
            Self::Asset => 0,
            Self::Order => 1,
            Self::Job => 2,
        }
    }
}

/// What happened to one thing since the previous refresh.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Appeared,
    Changed,
    Gone,
}

impl Event {
    fn code(self) -> i64 {
        match self {
            Self::Appeared => 0,
            Self::Changed => 1,
            Self::Gone => 2,
        }
    }

    fn from_code(code: i64) -> Option<Self> {
        match code {
            0 => Some(Self::Appeared),
            1 => Some(Self::Changed),
            2 => Some(Self::Gone),
            _ => None,
        }
    }
}

/// One appended change, as [`Store::changes`] returns it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeRow {
    pub kind: Kind,
    /// The asset item id, order id or job id the change is about.
    pub entity_id: i64,
    /// Unix seconds of the refresh that saw the change.
    pub at: i64,
    pub event: Event,
    pub type_id: TypeId,
    /// Units for an asset, units left for an order, runs for a job.
    pub quantity: u64,
    /// The location, or a job's facility, at the time of the change.
    pub place: i64,
    /// An order's price or a job's cost; `None` for an asset.
    pub price: Option<Money>,
    /// The asset flag, `"buy"`/`"sell"`, or the job status.
    pub state: Option<String>,
}

/// One asset as a refresh saw it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetRow {
    pub item_id: i64,
    pub type_id: TypeId,
    pub quantity: u64,
    pub location_id: i64,
    pub flag: Option<String>,
    pub singleton: bool,
}

/// One market order as a refresh saw it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderRow {
    pub order_id: i64,
    pub type_id: TypeId,
    pub location_id: i64,
    pub is_buy: bool,
    pub price: Money,
    pub volume_remain: u64,
}

/// One industry job as a refresh saw it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobRow {
    pub job_id: i64,
    pub type_id: TypeId,
    pub runs: u32,
    pub facility_id: i64,
    pub state: String,
    pub cost: Option<Money>,
}

/// The values a change reports, and the values two states are compared by.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Snapshot {
    type_id: TypeId,
    quantity: u64,
    place: i64,
    price: Option<i64>,
    state: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AssetState {
    type_id: TypeId,
    quantity: u64,
    place: i64,
    flag: Option<String>,
    singleton: bool,
}

impl AssetState {
    fn same(&self, other: &Self) -> bool {
        self.type_id == other.type_id
            && self.quantity == other.quantity
            && self.place == other.place
            && self.flag == other.flag
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            type_id: self.type_id,
            quantity: self.quantity,
            place: self.place,
            price: None,
            state: self.flag.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct OrderState {
    type_id: TypeId,
    place: i64,
    is_buy: bool,
    price: i64,
    volume_remain: u64,
}

impl OrderState {
    fn same(&self, other: &Self) -> bool {
        self.type_id == other.type_id
            && self.place == other.place
            && self.is_buy == other.is_buy
            && self.price == other.price
            && self.volume_remain == other.volume_remain
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            type_id: self.type_id,
            quantity: self.volume_remain,
            place: self.place,
            price: Some(self.price),
            state: Some(if self.is_buy { "buy" } else { "sell" }.to_owned()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct JobState {
    type_id: TypeId,
    runs: u32,
    place: i64,
    state: String,
    cost: Option<i64>,
}

impl JobState {
    fn same(&self, other: &Self) -> bool {
        self.type_id == other.type_id
            && self.runs == other.runs
            && self.place == other.place
            && self.state == other.state
            && self.cost == other.cost
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            type_id: self.type_id,
            quantity: u64::from(self.runs),
            place: self.place,
            price: self.cost,
            state: Some(self.state.clone()),
        }
    }
}

impl Store {
    /// Records the assets a character holds now, appending what appeared, changed or is gone.
    /// Every row is checked before the one transaction; a bad row writes nothing. Returns the
    /// number of changes appended.
    pub fn record_assets(
        &mut self,
        character_id: u64,
        at: i64,
        rows: &[AssetRow],
    ) -> Result<usize> {
        let character = to_stored_u64(character_id)?;
        ensure_at(at)?;
        let mut current: BTreeMap<i64, AssetState> = BTreeMap::new();
        for row in rows {
            ensure_id(row.item_id, "asset id")?;
            ensure_id(row.location_id, "asset place")?;
            let state = AssetState {
                type_id: row.type_id,
                quantity: row.quantity,
                place: row.location_id,
                flag: row.flag.clone(),
                singleton: row.singleton,
            };
            if current.insert(row.item_id, state).is_some() {
                return Err(repeated("asset", row.item_id));
            }
        }
        let tx = self.conn.transaction()?;
        let previous = asset_states(&tx, character)?;
        let changes = plan(&previous, &current, AssetState::same);
        append_changes(
            &tx,
            character,
            Kind::Asset,
            at,
            &changes,
            AssetState::snapshot,
        )?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO asset_state (character_id, item_id, type_id, quantity, location_id, \
                 flag, singleton) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
                 ON CONFLICT (character_id, item_id) DO UPDATE SET type_id = excluded.type_id, \
                 quantity = excluded.quantity, location_id = excluded.location_id, \
                 flag = excluded.flag, singleton = excluded.singleton",
            )?;
            for (&item_id, state) in &current {
                stmt.execute(params![
                    character,
                    item_id,
                    i64::from(state.type_id),
                    to_stored_u64(state.quantity)?,
                    state.place,
                    state.flag,
                    state.singleton,
                ])?;
            }
        }
        {
            let mut stmt =
                tx.prepare("DELETE FROM asset_state WHERE character_id = ?1 AND item_id = ?2")?;
            for (id, event, _) in &changes {
                if *event == Event::Gone {
                    stmt.execute(params![character, id])?;
                }
            }
        }
        tx.commit()?;
        Ok(changes.len())
    }

    /// Records the orders a character has now, appending what appeared, changed or is gone.
    /// Returns the number of changes appended.
    pub fn record_orders(
        &mut self,
        character_id: u64,
        at: i64,
        rows: &[OrderRow],
    ) -> Result<usize> {
        let character = to_stored_u64(character_id)?;
        ensure_at(at)?;
        let mut current: BTreeMap<i64, OrderState> = BTreeMap::new();
        for row in rows {
            ensure_id(row.order_id, "order id")?;
            ensure_id(row.location_id, "order place")?;
            ensure_price(row.price, "order price")?;
            let state = OrderState {
                type_id: row.type_id,
                place: row.location_id,
                is_buy: row.is_buy,
                price: row.price.ten_thousandths(),
                volume_remain: row.volume_remain,
            };
            if current.insert(row.order_id, state).is_some() {
                return Err(repeated("order", row.order_id));
            }
        }
        let tx = self.conn.transaction()?;
        let previous = order_states(&tx, character)?;
        let changes = plan(&previous, &current, OrderState::same);
        append_changes(
            &tx,
            character,
            Kind::Order,
            at,
            &changes,
            OrderState::snapshot,
        )?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO order_state (character_id, order_id, type_id, location_id, is_buy, \
                 price, volume_remain) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
                 ON CONFLICT (character_id, order_id) DO UPDATE SET type_id = excluded.type_id, \
                 location_id = excluded.location_id, is_buy = excluded.is_buy, \
                 price = excluded.price, volume_remain = excluded.volume_remain",
            )?;
            for (&order_id, state) in &current {
                stmt.execute(params![
                    character,
                    order_id,
                    i64::from(state.type_id),
                    state.place,
                    state.is_buy,
                    state.price,
                    to_stored_u64(state.volume_remain)?,
                ])?;
            }
        }
        {
            let mut stmt =
                tx.prepare("DELETE FROM order_state WHERE character_id = ?1 AND order_id = ?2")?;
            for (id, event, _) in &changes {
                if *event == Event::Gone {
                    stmt.execute(params![character, id])?;
                }
            }
        }
        tx.commit()?;
        Ok(changes.len())
    }

    /// Records the jobs a character has now, appending what appeared, changed or is gone.
    /// Returns the number of changes appended.
    pub fn record_jobs(&mut self, character_id: u64, at: i64, rows: &[JobRow]) -> Result<usize> {
        let character = to_stored_u64(character_id)?;
        ensure_at(at)?;
        let mut current: BTreeMap<i64, JobState> = BTreeMap::new();
        for row in rows {
            ensure_id(row.job_id, "job id")?;
            ensure_id(row.facility_id, "job place")?;
            if let Some(cost) = row.cost {
                ensure_price(cost, "job cost")?;
            }
            let state = JobState {
                type_id: row.type_id,
                runs: row.runs,
                place: row.facility_id,
                state: row.state.clone(),
                cost: row.cost.map(Money::ten_thousandths),
            };
            if current.insert(row.job_id, state).is_some() {
                return Err(repeated("job", row.job_id));
            }
        }
        let tx = self.conn.transaction()?;
        let previous = job_states(&tx, character)?;
        let changes = plan(&previous, &current, JobState::same);
        append_changes(&tx, character, Kind::Job, at, &changes, JobState::snapshot)?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO job_state (character_id, job_id, type_id, runs, facility_id, state, \
                 cost) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
                 ON CONFLICT (character_id, job_id) DO UPDATE SET type_id = excluded.type_id, \
                 runs = excluded.runs, facility_id = excluded.facility_id, \
                 state = excluded.state, cost = excluded.cost",
            )?;
            for (&job_id, state) in &current {
                stmt.execute(params![
                    character,
                    job_id,
                    i64::from(state.type_id),
                    i64::from(state.runs),
                    state.place,
                    state.state,
                    state.cost,
                ])?;
            }
        }
        {
            let mut stmt =
                tx.prepare("DELETE FROM job_state WHERE character_id = ?1 AND job_id = ?2")?;
            for (id, event, _) in &changes {
                if *event == Event::Gone {
                    stmt.execute(params![character, id])?;
                }
            }
        }
        tx.commit()?;
        Ok(changes.len())
    }

    /// The changes appended for one character and kind, oldest first.
    pub fn changes(&mut self, character_id: u64, kind: Kind) -> Result<Vec<ChangeRow>> {
        let character = to_stored_u64(character_id)?;
        let tx = self.conn.transaction()?;
        let mut changes = Vec::new();
        {
            let mut stmt = tx.prepare(
                "SELECT entity_id, at, event, type_id, quantity, place, price, state \
                 FROM change_list WHERE character_id = ?1 AND kind = ?2 ORDER BY seq",
            )?;
            let rows = stmt.query_map(params![character, kind.code()], |row| {
                Ok(StoredChange {
                    entity_id: row.get(0)?,
                    at: row.get(1)?,
                    event: row.get(2)?,
                    type_id: row.get(3)?,
                    quantity: row.get(4)?,
                    place: row.get(5)?,
                    price: row.get(6)?,
                    state: row.get(7)?,
                })
            })?;
            for row in rows {
                changes.push(row?.into_change(kind)?);
            }
        }
        tx.commit()?;
        Ok(changes)
    }
}

/// The changes the current rows have against the stored ones, in id order.
fn plan<S: Clone>(
    previous: &BTreeMap<i64, S>,
    current: &BTreeMap<i64, S>,
    same: impl Fn(&S, &S) -> bool,
) -> Vec<(i64, Event, S)> {
    let mut changes = Vec::new();
    for (&id, state) in current {
        match previous.get(&id) {
            None => changes.push((id, Event::Appeared, state.clone())),
            Some(last) if !same(last, state) => changes.push((id, Event::Changed, state.clone())),
            Some(_) => {}
        }
    }
    for (&id, last) in previous {
        if !current.contains_key(&id) {
            changes.push((id, Event::Gone, last.clone()));
        }
    }
    changes.sort_by_key(|(id, _, _)| *id);
    changes
}

/// Appends one row per change, in the order given.
fn append_changes<S>(
    tx: &Transaction<'_>,
    character: i64,
    kind: Kind,
    at: i64,
    changes: &[(i64, Event, S)],
    snapshot: impl Fn(&S) -> Snapshot,
) -> Result<()> {
    let mut stmt = tx.prepare(
        "INSERT INTO change_list (character_id, kind, entity_id, at, event, type_id, quantity, \
         place, price, state) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
    )?;
    for (id, event, state) in changes {
        let snapshot = snapshot(state);
        stmt.execute(params![
            character,
            kind.code(),
            id,
            at,
            event.code(),
            i64::from(snapshot.type_id),
            to_stored_u64(snapshot.quantity)?,
            snapshot.place,
            snapshot.price,
            snapshot.state,
        ])?;
    }
    Ok(())
}

fn ensure_at(at: i64) -> Result<()> {
    if at < 0 {
        return Err(StoreError::BadData(format!(
            "fetch time {at} is before zero"
        )));
    }
    Ok(())
}

fn ensure_id(id: i64, what: &str) -> Result<()> {
    if id < 1 {
        return Err(StoreError::BadData(format!("{what} {id} is below 1")));
    }
    Ok(())
}

fn ensure_price(price: Money, what: &str) -> Result<()> {
    if price.ten_thousandths() < 0 {
        return Err(StoreError::BadData(format!(
            "{what} {} is below zero",
            price.to_plain()
        )));
    }
    Ok(())
}

fn repeated(what: &str, id: i64) -> StoreError {
    StoreError::BadData(format!("{what} {id} is listed twice"))
}

fn stored_non_negative(value: i64, table: &str, row: i64, column: &'static str) -> Result<i64> {
    if value < 0 {
        return Err(StoreError::BadData(format!(
            "{table} row {row}, column {column}: {value} is out of range"
        )));
    }
    Ok(value)
}

fn stored_bool(value: i64, table: &str, row: i64, column: &'static str) -> Result<bool> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        other => Err(bad_code(table, row, column, other)),
    }
}

struct StoredAsset {
    item_id: i64,
    type_id: i64,
    quantity: i64,
    place: i64,
    flag: Option<String>,
    singleton: i64,
}

impl StoredAsset {
    fn into_state(self) -> Result<AssetState> {
        Ok(AssetState {
            type_id: from_stored(self.type_id, ASSETS, self.item_id, "type_id")?,
            quantity: from_stored(self.quantity, ASSETS, self.item_id, "quantity")?,
            place: stored_non_negative(self.place, ASSETS, self.item_id, "location_id")?,
            flag: self.flag,
            singleton: stored_bool(self.singleton, ASSETS, self.item_id, "singleton")?,
        })
    }
}

struct StoredOrder {
    order_id: i64,
    type_id: i64,
    place: i64,
    is_buy: i64,
    price: i64,
    volume_remain: i64,
}

impl StoredOrder {
    fn into_state(self) -> Result<OrderState> {
        Ok(OrderState {
            type_id: from_stored(self.type_id, ORDERS, self.order_id, "type_id")?,
            place: stored_non_negative(self.place, ORDERS, self.order_id, "location_id")?,
            is_buy: stored_bool(self.is_buy, ORDERS, self.order_id, "is_buy")?,
            price: stored_non_negative(self.price, ORDERS, self.order_id, "price")?,
            volume_remain: from_stored(self.volume_remain, ORDERS, self.order_id, "volume_remain")?,
        })
    }
}

struct StoredJob {
    job_id: i64,
    type_id: i64,
    runs: i64,
    place: i64,
    state: String,
    cost: Option<i64>,
}

impl StoredJob {
    fn into_state(self) -> Result<JobState> {
        Ok(JobState {
            type_id: from_stored(self.type_id, JOBS, self.job_id, "type_id")?,
            runs: from_stored(self.runs, JOBS, self.job_id, "runs")?,
            place: stored_non_negative(self.place, JOBS, self.job_id, "facility_id")?,
            state: self.state,
            cost: self
                .cost
                .map(|cost| stored_non_negative(cost, JOBS, self.job_id, "cost"))
                .transpose()?,
        })
    }
}

fn asset_states(tx: &Transaction<'_>, character: i64) -> Result<BTreeMap<i64, AssetState>> {
    let mut stmt = tx.prepare(
        "SELECT item_id, type_id, quantity, location_id, flag, singleton \
         FROM asset_state WHERE character_id = ?1",
    )?;
    let rows = stmt.query_map(params![character], |row| {
        Ok(StoredAsset {
            item_id: row.get(0)?,
            type_id: row.get(1)?,
            quantity: row.get(2)?,
            place: row.get(3)?,
            flag: row.get(4)?,
            singleton: row.get(5)?,
        })
    })?;
    let mut states = BTreeMap::new();
    for row in rows {
        let row = row?;
        states.insert(row.item_id, row.into_state()?);
    }
    Ok(states)
}

fn order_states(tx: &Transaction<'_>, character: i64) -> Result<BTreeMap<i64, OrderState>> {
    let mut stmt = tx.prepare(
        "SELECT order_id, type_id, location_id, is_buy, price, volume_remain \
         FROM order_state WHERE character_id = ?1",
    )?;
    let rows = stmt.query_map(params![character], |row| {
        Ok(StoredOrder {
            order_id: row.get(0)?,
            type_id: row.get(1)?,
            place: row.get(2)?,
            is_buy: row.get(3)?,
            price: row.get(4)?,
            volume_remain: row.get(5)?,
        })
    })?;
    let mut states = BTreeMap::new();
    for row in rows {
        let row = row?;
        states.insert(row.order_id, row.into_state()?);
    }
    Ok(states)
}

fn job_states(tx: &Transaction<'_>, character: i64) -> Result<BTreeMap<i64, JobState>> {
    let mut stmt = tx.prepare(
        "SELECT job_id, type_id, runs, facility_id, state, cost \
         FROM job_state WHERE character_id = ?1",
    )?;
    let rows = stmt.query_map(params![character], |row| {
        Ok(StoredJob {
            job_id: row.get(0)?,
            type_id: row.get(1)?,
            runs: row.get(2)?,
            place: row.get(3)?,
            state: row.get(4)?,
            cost: row.get(5)?,
        })
    })?;
    let mut states = BTreeMap::new();
    for row in rows {
        let row = row?;
        states.insert(row.job_id, row.into_state()?);
    }
    Ok(states)
}

struct StoredChange {
    entity_id: i64,
    at: i64,
    event: i64,
    type_id: i64,
    quantity: i64,
    place: i64,
    price: Option<i64>,
    state: Option<String>,
}

impl StoredChange {
    fn into_change(self, kind: Kind) -> Result<ChangeRow> {
        let entity_id = self.entity_id;
        ensure_id(entity_id, "change entity")?;
        let event = Event::from_code(self.event)
            .ok_or_else(|| bad_code(CHANGES, entity_id, "event", self.event))?;
        Ok(ChangeRow {
            kind,
            entity_id,
            at: stored_non_negative(self.at, CHANGES, entity_id, "at")?,
            event,
            type_id: from_stored(self.type_id, CHANGES, entity_id, "type_id")?,
            quantity: from_stored(self.quantity, CHANGES, entity_id, "quantity")?,
            place: stored_non_negative(self.place, CHANGES, entity_id, "place")?,
            price: self
                .price
                .map(|price| {
                    stored_non_negative(price, CHANGES, entity_id, "price")
                        .map(Money::from_ten_thousandths)
                })
                .transpose()?,
            state: self.state,
        })
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    const CHARACTER: u64 = 90_000_001;
    const OTHER: u64 = 90_000_002;
    const STATION: i64 = 60_003_760;

    fn open_temp() -> (TempDir, Store) {
        let dir = TempDir::new().expect("a temp folder");
        let store = Store::open(&dir.path().join("eve.db"), 1_000)
            .expect("the store opens")
            .store;
        (dir, store)
    }

    fn money(count: i64) -> Money {
        Money::from_ten_thousandths(count)
    }

    fn asset(
        item_id: i64,
        type_id: TypeId,
        quantity: u64,
        location_id: i64,
        flag: Option<&str>,
        singleton: bool,
    ) -> AssetRow {
        AssetRow {
            item_id,
            type_id,
            quantity,
            location_id,
            flag: flag.map(str::to_owned),
            singleton,
        }
    }

    fn order(order_id: i64, volume_remain: u64) -> OrderRow {
        OrderRow {
            order_id,
            type_id: 34,
            location_id: STATION,
            is_buy: false,
            price: money(57_500),
            volume_remain,
        }
    }

    fn job(job_id: i64, state: &str, runs: u32) -> JobRow {
        JobRow {
            job_id,
            type_id: 35,
            runs,
            facility_id: STATION,
            state: state.to_owned(),
            cost: Some(money(153_770_000)),
        }
    }

    fn change(
        kind: Kind,
        (entity_id, at, event): (i64, i64, Event),
        (type_id, quantity, place): (TypeId, u64, i64),
        price: Option<Money>,
        state: Option<&str>,
    ) -> ChangeRow {
        ChangeRow {
            kind,
            entity_id,
            at,
            event,
            type_id,
            quantity,
            place,
            price,
            state: state.map(str::to_owned),
        }
    }

    #[test]
    fn assets_appear_change_and_go_by_item_id() {
        let (_dir, mut store) = open_temp();
        let first = asset(500, 34, 10, STATION, Some("Hangar"), false);
        assert_eq!(
            store
                .record_assets(CHARACTER, 1_000, std::slice::from_ref(&first))
                .expect("first record"),
            1
        );
        assert_eq!(
            store.changes(CHARACTER, Kind::Asset).expect("changes"),
            vec![change(
                Kind::Asset,
                (500, 1_000, Event::Appeared),
                (34, 10, STATION),
                None,
                Some("Hangar")
            )]
        );

        let moved = asset(500, 34, 7, STATION + 1, Some("Cargo"), false);
        assert_eq!(
            store
                .record_assets(CHARACTER, 1_100, std::slice::from_ref(&moved))
                .expect("second record"),
            1
        );
        assert_eq!(
            store.changes(CHARACTER, Kind::Asset).expect("changes"),
            vec![
                change(
                    Kind::Asset,
                    (500, 1_000, Event::Appeared),
                    (34, 10, STATION),
                    None,
                    Some("Hangar")
                ),
                change(
                    Kind::Asset,
                    (500, 1_100, Event::Changed),
                    (34, 7, STATION + 1),
                    None,
                    Some("Cargo")
                ),
            ]
        );

        assert_eq!(
            store
                .record_assets(CHARACTER, 1_200, &[])
                .expect("empty record"),
            1
        );
        let changes = store.changes(CHARACTER, Kind::Asset).expect("changes");
        assert_eq!(changes.len(), 3);
        assert_eq!(
            changes.last(),
            Some(&change(
                Kind::Asset,
                (500, 1_200, Event::Gone),
                (34, 7, STATION + 1),
                None,
                Some("Cargo")
            ))
        );
    }

    #[test]
    fn an_unchanged_asset_appends_nothing() {
        let (_dir, mut store) = open_temp();
        let row = asset(500, 34, 10, STATION, Some("Hangar"), false);
        store
            .record_assets(CHARACTER, 1_000, std::slice::from_ref(&row))
            .expect("first record");
        assert_eq!(
            store
                .record_assets(CHARACTER, 1_100, std::slice::from_ref(&row))
                .expect("same record"),
            0
        );
        assert_eq!(
            store
                .changes(CHARACTER, Kind::Asset)
                .expect("changes")
                .len(),
            1
        );
    }

    #[test]
    fn orders_appear_change_and_go_keeping_the_price_and_side() {
        let (_dir, mut store) = open_temp();
        assert_eq!(
            store
                .record_orders(CHARACTER, 1_000, &[order(7001, 10)])
                .expect("first record"),
            1
        );
        assert_eq!(
            store.changes(CHARACTER, Kind::Order).expect("changes"),
            vec![change(
                Kind::Order,
                (7001, 1_000, Event::Appeared),
                (34, 10, STATION),
                Some(money(57_500)),
                Some("sell")
            )]
        );

        assert_eq!(
            store
                .record_orders(CHARACTER, 1_100, &[order(7001, 4)])
                .expect("second record"),
            1
        );
        let changed = store.changes(CHARACTER, Kind::Order).expect("changes");
        assert_eq!(
            changed.last(),
            Some(&change(
                Kind::Order,
                (7001, 1_100, Event::Changed),
                (34, 4, STATION),
                Some(money(57_500)),
                Some("sell")
            ))
        );

        assert_eq!(
            store
                .record_orders(CHARACTER, 1_200, &[])
                .expect("empty record"),
            1
        );
        let changes = store.changes(CHARACTER, Kind::Order).expect("changes");
        assert_eq!(
            changes.last(),
            Some(&change(
                Kind::Order,
                (7001, 1_200, Event::Gone),
                (34, 4, STATION),
                Some(money(57_500)),
                Some("sell")
            ))
        );
    }

    #[test]
    fn a_negative_order_price_writes_nothing() {
        let (_dir, mut store) = open_temp();
        store
            .record_orders(CHARACTER, 1_000, &[order(7001, 10)])
            .expect("first record");
        let mut bad = order(7001, 9);
        bad.price = money(-1);
        let refused = store.record_orders(CHARACTER, 1_100, &[bad]);
        assert!(
            matches!(refused, Err(StoreError::BadData(_))),
            "{refused:?}"
        );
        assert_eq!(
            store
                .changes(CHARACTER, Kind::Order)
                .expect("changes")
                .len(),
            1
        );
        // The stored state is still the first order, so recording it again changes nothing.
        assert_eq!(
            store
                .record_orders(CHARACTER, 1_200, &[order(7001, 10)])
                .expect("same record"),
            0
        );
    }

    #[test]
    fn jobs_appear_change_and_go_keeping_the_last_state() {
        let (_dir, mut store) = open_temp();
        assert_eq!(
            store
                .record_jobs(CHARACTER, 1_000, &[job(8001, "active", 10)])
                .expect("first record"),
            1
        );
        assert_eq!(
            store.changes(CHARACTER, Kind::Job).expect("changes"),
            vec![change(
                Kind::Job,
                (8001, 1_000, Event::Appeared),
                (35, 10, STATION),
                Some(money(153_770_000)),
                Some("active")
            )]
        );

        assert_eq!(
            store
                .record_jobs(CHARACTER, 1_100, &[job(8001, "ready", 10)])
                .expect("second record"),
            1
        );
        let changed = store.changes(CHARACTER, Kind::Job).expect("changes");
        assert_eq!(
            changed.last(),
            Some(&change(
                Kind::Job,
                (8001, 1_100, Event::Changed),
                (35, 10, STATION),
                Some(money(153_770_000)),
                Some("ready")
            ))
        );

        assert_eq!(
            store
                .record_jobs(CHARACTER, 1_200, &[])
                .expect("empty record"),
            1
        );
        let changes = store.changes(CHARACTER, Kind::Job).expect("changes");
        assert_eq!(
            changes.last(),
            Some(&change(
                Kind::Job,
                (8001, 1_200, Event::Gone),
                (35, 10, STATION),
                Some(money(153_770_000)),
                Some("ready")
            ))
        );
    }

    #[test]
    fn changes_are_appended_in_id_order() {
        let (_dir, mut store) = open_temp();
        let rows = [
            asset(600, 34, 1, STATION, None, false),
            asset(500, 34, 2, STATION, None, false),
        ];
        assert_eq!(
            store
                .record_assets(CHARACTER, 1_000, &rows)
                .expect("record"),
            2
        );
        let ids: Vec<i64> = store
            .changes(CHARACTER, Kind::Asset)
            .expect("changes")
            .into_iter()
            .map(|change| change.entity_id)
            .collect();
        assert_eq!(ids, vec![500, 600]);
    }

    #[test]
    fn characters_and_kinds_stay_apart() {
        let (_dir, mut store) = open_temp();
        store
            .record_assets(CHARACTER, 1_000, &[asset(500, 34, 1, STATION, None, false)])
            .expect("one character");
        store
            .record_assets(OTHER, 1_100, &[asset(500, 34, 9, STATION, None, false)])
            .expect("another character");
        assert_eq!(store.changes(OTHER, Kind::Asset).expect("other").len(), 1);
        assert_eq!(store.changes(CHARACTER, Kind::Asset).expect("one").len(), 1);
        assert!(
            store
                .changes(CHARACTER, Kind::Order)
                .expect("orders")
                .is_empty()
        );
        assert!(
            store
                .changes(CHARACTER, Kind::Job)
                .expect("jobs")
                .is_empty()
        );
    }

    #[test]
    fn bad_rows_are_refused_and_write_nothing() {
        let (_dir, mut store) = open_temp();
        let cases = [
            vec![asset(0, 34, 1, STATION, None, false)],
            vec![asset(500, 34, 1, 0, None, false)],
            vec![
                asset(500, 34, 1, STATION, None, false),
                asset(500, 34, 2, STATION, None, false),
            ],
        ];
        for rows in cases {
            let refused = store.record_assets(CHARACTER, 1_000, &rows);
            assert!(
                matches!(refused, Err(StoreError::BadData(_))),
                "{refused:?}"
            );
        }
        let refused = store.record_assets(CHARACTER, -1, &[]);
        assert!(
            matches!(refused, Err(StoreError::BadData(_))),
            "{refused:?}"
        );
        let too_big = asset(500, 34, u64::MAX, STATION, None, false);
        let refused = store.record_assets(CHARACTER, 1_000, &[too_big]);
        assert!(matches!(refused, Err(StoreError::TooLarge)), "{refused:?}");
        assert!(
            store
                .changes(CHARACTER, Kind::Asset)
                .expect("changes")
                .is_empty()
        );
    }

    #[test]
    fn a_stored_change_out_of_range_is_bad_data() {
        let (_dir, mut store) = open_temp();
        // A table without the checks stands for a file another build wrote.
        store
            .conn
            .execute_batch(
                "DROP TABLE change_list;
                 CREATE TABLE change_list (
                     seq INTEGER PRIMARY KEY,
                     character_id INTEGER NOT NULL,
                     kind INTEGER NOT NULL,
                     entity_id INTEGER NOT NULL,
                     at INTEGER NOT NULL,
                     event INTEGER NOT NULL,
                     type_id INTEGER NOT NULL,
                     quantity INTEGER NOT NULL,
                     place INTEGER NOT NULL,
                     price INTEGER,
                     state TEXT
                 );
                 INSERT INTO change_list
                     (character_id, kind, entity_id, at, event, type_id, quantity, place, price, state)
                     VALUES (90000001, 0, 500, 1000, 0, 5000000000, 10, 60003760, NULL, 'Hangar');",
            )
            .expect("recreate");
        let refused = store.changes(CHARACTER, Kind::Asset);
        assert!(
            matches!(&refused, Err(StoreError::BadData(text)) if text.contains("type_id")),
            "{refused:?}"
        );
    }
}

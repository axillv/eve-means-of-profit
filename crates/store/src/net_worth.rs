//! The net worth history: what each character held at each refresh, each day's prices and the
//! history imported from jEve. Kept for good, so the chart can be drawn again at any price basis.
//! Nothing in this module changes or deletes a stored record.

use std::collections::{BTreeMap, BTreeSet};

use mop_core::industry::TypeId;
use mop_core::money::Money;
use mop_core::net_worth::{CharacterKey, Holdings, ItemPrices, Part, PriceBasis, Record, Snapshot};
use rusqlite::{OptionalExtension, Transaction, params};

use crate::error::{Result, StoreError};
use crate::migrate::Store;
use crate::{bad_code, from_stored, to_stored_u64};

const RECORDS: &str = "net_worth_records";
const ITEMS: &str = "item_set_items";

const KIND_SEEN: i64 = 0;
const KIND_IMPORTED: i64 = 1;
const KIND_REMOVED: i64 = 2;

/// What an import of records did. Every record is counted once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImportReport {
    pub added: usize,
    /// The same character at the same time was imported before.
    pub duplicates: usize,
    /// At or after the character's first record seen by the app, which wins.
    pub after_app_data: usize,
}

/// How the chart values items and which parts it draws.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetWorthSettings {
    pub basis: PriceBasis,
    pub shown: BTreeSet<Part>,
}

/// The saved prices of some types over some days, as read by [`Store::price_history`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PriceHistory {
    rows: BTreeMap<TypeId, BTreeMap<i64, ItemPrices>>,
}

impl PriceHistory {
    /// The prices of `type_id` on the latest day at or before `day`; `None` when there is none.
    pub fn get(&self, type_id: TypeId, day: i64) -> Option<ItemPrices> {
        self.rows
            .get(&type_id)?
            .range(..=day)
            .next_back()
            .map(|(_, prices)| *prices)
    }
}

impl Store {
    /// Adds a record the app made: what a character held, or that it was removed. A set of items equal
    /// to the one the character's previous seen record used is reused, not written again.
    pub fn add_record(&mut self, record: &Record) -> Result<i64> {
        let tx = self.conn.transaction()?;
        let id = match &record.snapshot {
            Snapshot::Seen(holdings) => {
                let (previous_assets, previous_jobs) =
                    previous_seen_sets(&tx, &record.character)?.unwrap_or((None, None));
                let assets = set_for(&tx, previous_assets, &holdings.assets)?;
                let jobs = set_for(&tx, previous_jobs, &holdings.jobs_output)?;
                let money = (
                    stored_money(holdings.wallet),
                    stored_money(holdings.sell_orders),
                    stored_money(holdings.buy_escrow),
                );
                insert_record(
                    &tx,
                    &record.character,
                    record.at,
                    KIND_SEEN,
                    money,
                    (Some(assets), Some(jobs)),
                )?
            }
            Snapshot::Removed => insert_record(
                &tx,
                &record.character,
                record.at,
                KIND_REMOVED,
                (None, None, None),
                (None, None),
            )?,
            Snapshot::Imported(_) => {
                return Err(StoreError::BadData(
                    "imported records go through import_records".to_owned(),
                ));
            }
        };
        tx.commit()?;
        Ok(id)
    }

    /// Stores jEve's records. Each one is skipped when the same character at the same time was
    /// imported before, or when it is at or after that character's first record seen by the app.
    /// Any other kind of record refuses the whole list, and nothing is written.
    pub fn import_records(&mut self, records: &[Record]) -> Result<ImportReport> {
        let tx = self.conn.transaction()?;
        let mut report = ImportReport {
            added: 0,
            duplicates: 0,
            after_app_data: 0,
        };
        for record in records {
            let Snapshot::Imported(parts) = &record.snapshot else {
                return Err(StoreError::BadData(
                    "only imported records go through import_records".to_owned(),
                ));
            };
            let (character_id, character_name) = key_columns(&record.character)?;
            let imported_at: i64 = tx.query_row(
                "SELECT count(*) FROM net_worth_records \
                 WHERE character_id IS ?1 AND character_name IS ?2 AND kind = ?3 AND at = ?4",
                params![character_id, character_name, KIND_IMPORTED, record.at],
                |row| row.get(0),
            )?;
            if imported_at > 0 {
                report.duplicates += 1;
                continue;
            }
            let first_seen: Option<i64> = tx.query_row(
                "SELECT min(at) FROM net_worth_records \
                 WHERE character_id IS ?1 AND character_name IS ?2 AND kind = ?3",
                params![character_id, character_name, KIND_SEEN],
                |row| row.get(0),
            )?;
            if first_seen.is_some_and(|first| record.at >= first) {
                report.after_app_data += 1;
                continue;
            }
            let id = insert_record(
                &tx,
                &record.character,
                record.at,
                KIND_IMPORTED,
                (None, None, None),
                (None, None),
            )?;
            for (part, amount) in parts {
                tx.execute(
                    "INSERT INTO net_worth_imported (record_id, part, amount) VALUES (?1, ?2, ?3)",
                    params![id, part_code(*part), amount.ten_thousandths()],
                )?;
            }
            report.added += 1;
        }
        tx.commit()?;
        Ok(report)
    }

    /// The records from `from` to `to` (Unix seconds, both included), each character's latest record
    /// before `from` first, in time order.
    pub fn records(&mut self, from: i64, to: i64) -> Result<Vec<Record>> {
        let tx = self.conn.transaction()?;
        let sql = format!(
            "SELECT {RECORD_COLUMNS} FROM net_worth_records WHERE at BETWEEN ?1 AND ?2 \
             UNION ALL \
             SELECT {RECORD_COLUMNS} FROM net_worth_records AS carried \
             WHERE carried.at < ?1 AND carried.id = ( \
                SELECT latest.id FROM net_worth_records AS latest \
                WHERE latest.character_id IS carried.character_id \
                  AND latest.character_name IS carried.character_name AND latest.at < ?1 \
                ORDER BY latest.at DESC, latest.id DESC LIMIT 1) \
             ORDER BY at, id"
        );
        let raws = raw_rows(&tx, &sql, params![from, to])?;
        let records = raws
            .into_iter()
            .map(|raw| raw.into_record(&tx))
            .collect::<Result<Vec<_>>>()?;
        tx.commit()?;
        Ok(records)
    }

    /// Saves the prices of `day`. A price that is `None` keeps the stored value of that day.
    pub fn save_prices(&mut self, day: i64, prices: &[(TypeId, ItemPrices)]) -> Result<()> {
        let tx = self.conn.transaction()?;
        for (type_id, item) in prices {
            tx.execute(
                "INSERT INTO item_prices (day, type_id, lowest_sell, highest_buy, average) \
                 VALUES (?1, ?2, ?3, ?4, ?5) \
                 ON CONFLICT (day, type_id) DO UPDATE SET \
                    lowest_sell = COALESCE(excluded.lowest_sell, item_prices.lowest_sell), \
                    highest_buy = COALESCE(excluded.highest_buy, item_prices.highest_buy), \
                    average = COALESCE(excluded.average, item_prices.average)",
                params![
                    day,
                    i64::from(*type_id),
                    stored_money(item.lowest_sell),
                    stored_money(item.highest_buy),
                    stored_money(item.average),
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// The prices of `types` from `from_day` to `to_day`, plus each type's latest day before `from_day`.
    pub fn price_history(
        &mut self,
        types: &BTreeSet<TypeId>,
        from_day: i64,
        to_day: i64,
    ) -> Result<PriceHistory> {
        let tx = self.conn.transaction()?;
        let mut rows = BTreeMap::new();
        for type_id in types {
            let mut stmt = tx.prepare(
                "SELECT day, lowest_sell, highest_buy, average FROM item_prices \
                 WHERE type_id = ?1 AND day BETWEEN ?2 AND ?3 \
                 UNION ALL \
                 SELECT * FROM (SELECT day, lowest_sell, highest_buy, average FROM item_prices \
                    WHERE type_id = ?1 AND day < ?2 ORDER BY day DESC LIMIT 1)",
            )?;
            let found = stmt.query_map(params![i64::from(*type_id), from_day, to_day], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    ItemPrices {
                        lowest_sell: row
                            .get::<_, Option<i64>>(1)?
                            .map(Money::from_ten_thousandths),
                        highest_buy: row
                            .get::<_, Option<i64>>(2)?
                            .map(Money::from_ten_thousandths),
                        average: row
                            .get::<_, Option<i64>>(3)?
                            .map(Money::from_ten_thousandths),
                    },
                ))
            })?;
            let mut by_day = BTreeMap::new();
            for row in found {
                let (day, prices) = row?;
                by_day.insert(day, prices);
            }
            rows.insert(*type_id, by_day);
        }
        tx.commit()?;
        Ok(PriceHistory { rows })
    }

    /// The saved settings, or the defaults (midpoint, every part) when none were ever saved.
    pub fn net_worth_settings(&mut self) -> Result<NetWorthSettings> {
        let tx = self.conn.transaction()?;
        let basis: Option<i64> = tx
            .query_row(
                "SELECT basis FROM net_worth_settings WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .optional()?;
        let settings = match basis {
            None => NetWorthSettings {
                basis: PriceBasis::Midpoint,
                shown: Part::ALL.into_iter().collect(),
            },
            Some(code) => {
                let basis = basis_from_code(code)
                    .ok_or_else(|| bad_code("net_worth_settings", 1, "basis", code))?;
                let mut stmt = tx.prepare("SELECT id, part FROM net_worth_shown")?;
                let rows =
                    stmt.query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))?;
                let mut shown = BTreeSet::new();
                for row in rows {
                    let (id, code) = row?;
                    let part = part_from_code(code)
                        .ok_or_else(|| bad_code("net_worth_shown", id, "part", code))?;
                    shown.insert(part);
                }
                NetWorthSettings { basis, shown }
            }
        };
        tx.commit()?;
        Ok(settings)
    }

    pub fn save_net_worth_settings(&mut self, settings: &NetWorthSettings) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO net_worth_settings (id, basis) VALUES (1, ?1) \
             ON CONFLICT (id) DO UPDATE SET basis = ?1",
            params![basis_code(settings.basis)],
        )?;
        tx.execute("DELETE FROM net_worth_shown", [])?;
        for part in &settings.shown {
            tx.execute(
                "INSERT INTO net_worth_shown (part) VALUES (?1)",
                params![part_code(*part)],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
}

const RECORD_COLUMNS: &str = "id, character_id, character_name, at, kind, wallet, sell_orders, \
     buy_escrow, assets_set, jobs_set";

/// The stored columns of one record, before they are turned back into Rust types.
struct RawRecord {
    id: i64,
    character_id: Option<i64>,
    character_name: Option<String>,
    at: i64,
    kind: i64,
    wallet: Option<i64>,
    sell_orders: Option<i64>,
    buy_escrow: Option<i64>,
    assets_set: Option<i64>,
    jobs_set: Option<i64>,
}

fn raw_rows(
    tx: &Transaction<'_>,
    sql: &str,
    bound: impl rusqlite::Params,
) -> Result<Vec<RawRecord>> {
    let mut stmt = tx.prepare(sql)?;
    let rows = stmt.query_map(bound, |row| {
        Ok(RawRecord {
            id: row.get(0)?,
            character_id: row.get(1)?,
            character_name: row.get(2)?,
            at: row.get(3)?,
            kind: row.get(4)?,
            wallet: row.get(5)?,
            sell_orders: row.get(6)?,
            buy_escrow: row.get(7)?,
            assets_set: row.get(8)?,
            jobs_set: row.get(9)?,
        })
    })?;
    rows.collect::<rusqlite::Result<_>>()
        .map_err(StoreError::from)
}

impl RawRecord {
    fn into_record(self, tx: &Transaction<'_>) -> Result<Record> {
        let character = match (self.character_id, self.character_name) {
            (Some(id), None) => {
                CharacterKey::Id(from_stored(id, RECORDS, self.id, "character_id")?)
            }
            (None, Some(name)) => CharacterKey::Name(name),
            _ => {
                return Err(StoreError::BadData(format!(
                    "{RECORDS} row {}, column character_id: needs exactly one of an id or a name",
                    self.id
                )));
            }
        };
        let snapshot = match self.kind {
            KIND_SEEN => Snapshot::Seen(Holdings {
                wallet: self.wallet.map(Money::from_ten_thousandths),
                assets: items_of(tx, required_set(self.assets_set, self.id, "assets_set")?)?,
                sell_orders: self.sell_orders.map(Money::from_ten_thousandths),
                buy_escrow: self.buy_escrow.map(Money::from_ten_thousandths),
                jobs_output: items_of(tx, required_set(self.jobs_set, self.id, "jobs_set")?)?,
            }),
            KIND_IMPORTED => Snapshot::Imported(imported_parts(tx, self.id)?),
            KIND_REMOVED => Snapshot::Removed,
            code => return Err(bad_code(RECORDS, self.id, "kind", code)),
        };
        Ok(Record {
            character,
            at: self.at,
            snapshot,
        })
    }
}

/// The two columns that name a character: one is set, the other is NULL.
fn key_columns(key: &CharacterKey) -> Result<(Option<i64>, Option<&str>)> {
    match key {
        CharacterKey::Id(id) => Ok((Some(to_stored_u64(*id)?), None)),
        CharacterKey::Name(name) => Ok((None, Some(name.as_str()))),
    }
}

fn insert_record(
    tx: &Transaction<'_>,
    key: &CharacterKey,
    at: i64,
    kind: i64,
    money: (Option<i64>, Option<i64>, Option<i64>),
    sets: (Option<i64>, Option<i64>),
) -> Result<i64> {
    let (character_id, character_name) = key_columns(key)?;
    let (wallet, sell_orders, buy_escrow) = money;
    let (assets_set, jobs_set) = sets;
    tx.execute(
        "INSERT INTO net_worth_records (character_id, character_name, at, kind, wallet, \
         sell_orders, buy_escrow, assets_set, jobs_set) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            character_id,
            character_name,
            at,
            kind,
            wallet,
            sell_orders,
            buy_escrow,
            assets_set,
            jobs_set
        ],
    )?;
    Ok(tx.last_insert_rowid())
}

/// The assets and jobs sets of the character's latest seen record, if it has one.
fn previous_seen_sets(
    tx: &Transaction<'_>,
    key: &CharacterKey,
) -> Result<Option<(Option<i64>, Option<i64>)>> {
    let (character_id, character_name) = key_columns(key)?;
    Ok(tx
        .query_row(
            "SELECT assets_set, jobs_set FROM net_worth_records \
             WHERE character_id IS ?1 AND character_name IS ?2 AND kind = ?3 \
             ORDER BY id DESC LIMIT 1",
            params![character_id, character_name, KIND_SEEN],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?)
}

/// The set a new record points at: the previous set when it holds the same items, else a new one.
fn set_for(
    tx: &Transaction<'_>,
    previous: Option<i64>,
    items: &BTreeMap<TypeId, u64>,
) -> Result<i64> {
    if let Some(set) = previous {
        let same = items_of(tx, set)? == *items;
        if same {
            return Ok(set);
        }
    }
    write_items(tx, items)
}

fn write_items(tx: &Transaction<'_>, items: &BTreeMap<TypeId, u64>) -> Result<i64> {
    tx.execute("INSERT INTO item_sets DEFAULT VALUES", [])?;
    let set = tx.last_insert_rowid();
    let mut stmt =
        tx.prepare("INSERT INTO item_set_items (set_id, type_id, units) VALUES (?1, ?2, ?3)")?;
    for (type_id, units) in items {
        stmt.execute(params![set, i64::from(*type_id), to_stored_u64(*units)?])?;
    }
    Ok(set)
}

/// The (type, units) pairs of a set.
fn items_of(tx: &Transaction<'_>, set: i64) -> Result<BTreeMap<TypeId, u64>> {
    let mut stmt =
        tx.prepare("SELECT rowid, type_id, units FROM item_set_items WHERE set_id = ?1")?;
    let rows = stmt.query_map(params![set], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, i64>(2)?,
        ))
    })?;
    let mut items: BTreeMap<TypeId, u64> = BTreeMap::new();
    for row in rows {
        let (id, type_id, units) = row?;
        let type_id: TypeId = from_stored(type_id, ITEMS, id, "type_id")?;
        items.insert(type_id, from_stored(units, ITEMS, id, "units")?);
    }
    Ok(items)
}

fn required_set(set: Option<i64>, row: i64, column: &str) -> Result<i64> {
    set.ok_or_else(|| {
        StoreError::BadData(format!(
            "{RECORDS} row {row}, column {column}: a seen record has no set"
        ))
    })
}

fn imported_parts(tx: &Transaction<'_>, record: i64) -> Result<BTreeMap<Part, Money>> {
    let mut stmt =
        tx.prepare("SELECT part, amount FROM net_worth_imported WHERE record_id = ?1")?;
    let rows = stmt.query_map(params![record], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
    })?;
    let mut parts = BTreeMap::new();
    for row in rows {
        let (code, amount) = row?;
        let part = part_from_code(code)
            .ok_or_else(|| bad_code("net_worth_imported", record, "part", code))?;
        parts.insert(part, Money::from_ten_thousandths(amount));
    }
    Ok(parts)
}

fn stored_money(money: Option<Money>) -> Option<i64> {
    money.map(Money::ten_thousandths)
}

fn part_code(part: Part) -> i64 {
    match part {
        Part::Wallet => 0,
        Part::Assets => 1,
        Part::SellOrders => 2,
        Part::BuyEscrow => 3,
        Part::IndustryJobs => 4,
        Part::ContractCollateral => 5,
        Part::Contracts => 6,
        Part::Implants => 7,
    }
}

fn part_from_code(code: i64) -> Option<Part> {
    Part::ALL.into_iter().find(|part| part_code(*part) == code)
}

fn basis_code(basis: PriceBasis) -> i64 {
    match basis {
        PriceBasis::Midpoint => 0,
        PriceBasis::HighestBuy => 1,
        PriceBasis::LowestSell => 2,
        PriceBasis::Average => 3,
    }
}

fn basis_from_code(code: i64) -> Option<PriceBasis> {
    match code {
        0 => Some(PriceBasis::Midpoint),
        1 => Some(PriceBasis::HighestBuy),
        2 => Some(PriceBasis::LowestSell),
        3 => Some(PriceBasis::Average),
        _ => None,
    }
}

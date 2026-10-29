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

    /// The day of the prices [`PriceHistory::get`] returns for `type_id` on `day`; `None` when there is none.
    pub fn saved_day(&self, type_id: TypeId, day: i64) -> Option<i64> {
        self.rows
            .get(&type_id)?
            .range(..=day)
            .next_back()
            .map(|(saved, _)| *saved)
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

/// A group of the player's characters (an EVE account holds up to three of them), with its name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    pub id: i64,
    pub name: String,
    /// The character ids in the account, ascending.
    pub characters: Vec<u64>,
}

/// The most characters one account holds.
const ACCOUNT_SIZE: i64 = 3;

impl Store {
    /// The accounts by name, each with its characters.
    pub fn accounts(&self) -> Result<Vec<Account>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, name FROM accounts ORDER BY name")?;
        let mut accounts = stmt
            .query_map([], |row| {
                Ok(Account {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    characters: Vec::new(),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut stmt = self.conn.prepare(
            "SELECT character_id, account_id FROM account_characters ORDER BY character_id",
        )?;
        let rows = stmt
            .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut members: BTreeMap<i64, Vec<u64>> = BTreeMap::new();
        for (character, account) in rows {
            let character = from_stored(character, "account_characters", account, "character_id")?;
            members.entry(account).or_default().push(character);
        }
        for account in &mut accounts {
            account.characters = members.remove(&account.id).unwrap_or_default();
        }
        Ok(accounts)
    }

    /// Adds an account under the trimmed name. A used name is refused.
    pub fn add_account(&mut self, name: &str) -> Result<i64> {
        let name = account_name(name)?;
        let tx = self.conn.transaction()?;
        if name_taken(&tx, name, None)? {
            return Err(StoreError::NameTaken(name.to_string()));
        }
        tx.execute("INSERT INTO accounts (name) VALUES (?1)", params![name])?;
        let id = tx.last_insert_rowid();
        tx.commit()?;
        Ok(id)
    }

    /// Gives an account a new trimmed name. A name used by another account is refused.
    pub fn rename_account(&mut self, id: i64, name: &str) -> Result<()> {
        let name = account_name(name)?;
        let tx = self.conn.transaction()?;
        require_account(&tx, id)?;
        if name_taken(&tx, name, Some(id))? {
            return Err(StoreError::NameTaken(name.to_string()));
        }
        tx.execute(
            "UPDATE accounts SET name = ?2 WHERE id = ?1",
            params![id, name],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Puts the character in the account, out of any other, or in none when `account` is `None`.
    /// An account holds at most three characters; a fourth is refused and nothing changes.
    pub fn set_account(&mut self, character_id: u64, account: Option<i64>) -> Result<()> {
        let character = to_stored_u64(character_id)?;
        let tx = self.conn.transaction()?;
        match account {
            None => {
                tx.execute(
                    "DELETE FROM account_characters WHERE character_id = ?1",
                    params![character],
                )?;
            }
            Some(account) => {
                require_account(&tx, account)?;
                let held: i64 = tx.query_row(
                    "SELECT COUNT(*) FROM account_characters \
                     WHERE account_id = ?1 AND character_id <> ?2",
                    params![account, character],
                    |row| row.get(0),
                )?;
                if held >= ACCOUNT_SIZE {
                    return Err(StoreError::TooLarge);
                }
                tx.execute(
                    "INSERT INTO account_characters (character_id, account_id) VALUES (?1, ?2) \
                     ON CONFLICT (character_id) DO UPDATE SET account_id = ?2",
                    params![character, account],
                )?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Records the account's PLEX count from time `at`. A count already recorded at `at` is replaced.
    pub fn set_plex(&mut self, account: i64, count: u64, at: i64) -> Result<()> {
        let count = to_stored_u64(count)?;
        let tx = self.conn.transaction()?;
        require_account(&tx, account)?;
        tx.execute(
            "INSERT INTO plex_counts (account_id, at, count) VALUES (?1, ?2, ?3) \
             ON CONFLICT (account_id, at) DO UPDATE SET count = ?3",
            params![account, at, count],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// The account's PLEX counts by time.
    pub fn plex_counts(&self, account: i64) -> Result<Vec<(i64, u64)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT at, count FROM plex_counts WHERE account_id = ?1 ORDER BY at")?;
        let rows = stmt
            .query_map(params![account], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|(at, count)| Ok((at, from_stored(count, "plex_counts", at, "count")?)))
            .collect()
    }

    /// Per account, the count of its latest entry at or before `at`. Accounts with none are left out.
    pub fn plex_at(&self, at: i64) -> Result<BTreeMap<i64, u64>> {
        let mut stmt = self.conn.prepare(
            "SELECT p.account_id, p.count FROM plex_counts p \
             WHERE p.at = (SELECT MAX(q.at) FROM plex_counts q \
                           WHERE q.account_id = p.account_id AND q.at <= ?1)",
        )?;
        let rows = stmt
            .query_map(params![at], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut counts = BTreeMap::new();
        for (account, count) in rows {
            counts.insert(
                account,
                from_stored(count, "plex_counts", account, "count")?,
            );
        }
        Ok(counts)
    }
}

/// The name without its surrounding spaces, or `EmptyName` when nothing is left.
fn account_name(name: &str) -> Result<&str> {
    let name = name.trim();
    if name.is_empty() {
        return Err(StoreError::EmptyName);
    }
    Ok(name)
}

/// Whether another account than `except` already uses the name.
fn name_taken(tx: &Transaction<'_>, name: &str, except: Option<i64>) -> Result<bool> {
    let holder: Option<i64> = tx
        .query_row(
            "SELECT id FROM accounts WHERE name = ?1",
            params![name],
            |row| row.get(0),
        )
        .optional()?;
    Ok(matches!(holder, Some(id) if Some(id) != except))
}

/// `NotFound` unless the account exists.
fn require_account(tx: &Transaction<'_>, id: i64) -> Result<()> {
    let found: Option<i64> = tx
        .query_row(
            "SELECT id FROM accounts WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )
        .optional()?;
    found.map(|_| ()).ok_or(StoreError::NotFound)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn open_temp() -> (tempfile::TempDir, PathBuf, Store) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("eve.db");
        let store = Store::open(&path, 1_000).expect("open").store;
        (dir, path, store)
    }

    fn names(store: &Store) -> Vec<(i64, String, Vec<u64>)> {
        store
            .accounts()
            .expect("accounts")
            .into_iter()
            .map(|account| (account.id, account.name, account.characters))
            .collect()
    }

    #[test]
    fn add_trims_and_refuses_empty_and_taken_names() {
        let (_dir, _path, mut store) = open_temp();
        let id = store.add_account(" Main acct ").expect("add");
        assert_eq!(names(&store), vec![(id, "Main acct".to_string(), vec![])]);
        assert!(matches!(
            store.add_account("Main acct"),
            Err(StoreError::NameTaken(name)) if name == "Main acct"
        ));
        assert!(matches!(
            store.add_account("  "),
            Err(StoreError::EmptyName)
        ));
        assert!(matches!(
            store.rename_account(99, "x"),
            Err(StoreError::NotFound)
        ));
    }

    #[test]
    fn characters_move_between_accounts_and_survive_reopen() {
        let (_dir, path, mut store) = open_temp();
        let a = store.add_account("A").expect("add a");
        let b = store.add_account("B").expect("add b");
        for character in [101, 102, 103] {
            store.set_account(character, Some(a)).expect("set");
        }
        assert_eq!(names(&store)[0].2, vec![101, 102, 103]);
        assert!(matches!(
            store.set_account(104, Some(a)),
            Err(StoreError::TooLarge)
        ));
        assert_eq!(names(&store)[0].2, vec![101, 102, 103]);
        store.set_account(103, Some(b)).expect("move");
        store.set_account(102, None).expect("remove");
        drop(store);
        let store = Store::open(&path, 1_000).expect("reopen").store;
        let accounts = names(&store);
        assert_eq!(accounts[0], (a, "A".to_string(), vec![101]));
        assert_eq!(accounts[1], (b, "B".to_string(), vec![103]));
    }

    #[test]
    fn plex_history_answers_by_time() {
        let (_dir, _path, mut store) = open_temp();
        let a = store.add_account("A").expect("add");
        store.set_plex(a, 500, 1_000_000_000).expect("set");
        store.set_plex(a, 650, 1_000_604_800).expect("set");
        assert!(store.plex_at(999_999_999).expect("at").is_empty());
        assert_eq!(
            store.plex_at(1_000_000_000).expect("at"),
            BTreeMap::from([(a, 500)])
        );
        assert_eq!(
            store.plex_at(1_000_604_799).expect("at"),
            BTreeMap::from([(a, 500)])
        );
        assert_eq!(
            store.plex_at(2_000_000_000).expect("at"),
            BTreeMap::from([(a, 650)])
        );
        assert_eq!(
            store.plex_counts(a).expect("counts"),
            vec![(1_000_000_000, 500), (1_000_604_800, 650)]
        );
    }

    #[test]
    fn same_time_replaces_and_unknown_account_is_refused() {
        let (_dir, _path, mut store) = open_temp();
        let a = store.add_account("A").expect("add");
        store.set_plex(a, 500, 1_000_000_000).expect("set");
        store.set_plex(a, 700, 1_000_000_000).expect("replace");
        assert_eq!(
            store.plex_counts(a).expect("counts"),
            vec![(1_000_000_000, 700)]
        );
        assert!(matches!(
            store.set_plex(99, 1, 0),
            Err(StoreError::NotFound)
        ));
    }
}

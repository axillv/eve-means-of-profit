//! Each character's wallet journal and market transactions, kept by EVE's id. Saves only add:
//! a row already kept is never changed or removed, so the history stays as it was first seen.

use std::collections::BTreeSet;

use mop_core::industry::TypeId;
use mop_core::money::Money;
use rusqlite::{OptionalExtension, Transaction, params};

use crate::error::{Result, StoreError};
use crate::migrate::Store;
use crate::{bad_code, from_stored, to_stored_u64};

const JOURNAL: &str = "wallet_journal";
const TRANSACTIONS: &str = "wallet_transactions";

/// One line of a character's wallet journal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalRow {
    pub id: i64,
    pub at: i64,
    pub ref_type: String,
    pub ref_id: Option<i64>,
    pub first_party_id: Option<i64>,
    pub second_party_id: Option<i64>,
    pub amount: Option<Money>,
    pub balance: Option<Money>,
    pub reason: Option<String>,
    pub tax: Option<Money>,
    pub tax_receiver_id: Option<i64>,
    pub context_id: Option<i64>,
    pub context_id_type: Option<String>,
    pub description: String,
}

/// One market transaction of a character.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransactionRow {
    pub transaction_id: i64,
    pub at: i64,
    pub client_id: i64,
    pub is_buy: bool,
    pub is_personal: bool,
    pub journal_ref_id: i64,
    pub location_id: i64,
    pub quantity: u32,
    pub type_id: TypeId,
    pub unit_price: Money,
}

/// One character's wallet journal and the time it was last fetched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredJournal {
    pub character_id: u64,
    pub fetched_at: i64,
    pub rows: Vec<JournalRow>,
}

/// One character's market transactions and the time they were last fetched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredTransactions {
    pub character_id: u64,
    pub fetched_at: i64,
    pub rows: Vec<TransactionRow>,
}

impl Store {
    /// Saves the journal rows just fetched, adding only the ids not kept before, and records the
    /// fetch time. Every row is checked first: a row the store will not keep refuses the whole
    /// save and nothing is written, so the rows kept before stay.
    pub fn save_journal(
        &mut self,
        character_id: u64,
        fetched_at: i64,
        rows: &[JournalRow],
    ) -> Result<()> {
        let character = to_stored_u64(character_id)?;
        for row in rows {
            ensure_journal(row)?;
        }
        ensure_journal_ids_once(rows)?;
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO wallet_journal_fetches (character_id, fetched_at) VALUES (?1, ?2) \
             ON CONFLICT (character_id) DO UPDATE SET fetched_at = ?2",
            params![character, fetched_at],
        )?;
        {
            let mut stmt = tx.prepare(
                "INSERT OR IGNORE INTO wallet_journal (character_id, id, at, ref_type, ref_id, \
                 first_party_id, second_party_id, amount, balance, reason, tax, tax_receiver_id, \
                 context_id, context_id_type, description) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            )?;
            for row in rows {
                stmt.execute(params![
                    character,
                    row.id,
                    row.at,
                    row.ref_type,
                    row.ref_id,
                    row.first_party_id,
                    row.second_party_id,
                    row.amount.map(Money::ten_thousandths),
                    row.balance.map(Money::ten_thousandths),
                    row.reason,
                    row.tax.map(Money::ten_thousandths),
                    row.tax_receiver_id,
                    row.context_id,
                    row.context_id_type,
                    row.description,
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Saves the transactions just fetched, adding only the ids not kept before, and records the
    /// fetch time. Every row is checked first: a row the store will not keep refuses the whole
    /// save and nothing is written, so the rows kept before stay.
    pub fn save_transactions(
        &mut self,
        character_id: u64,
        fetched_at: i64,
        rows: &[TransactionRow],
    ) -> Result<()> {
        let character = to_stored_u64(character_id)?;
        for row in rows {
            ensure_transaction(row)?;
        }
        ensure_transaction_ids_once(rows)?;
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO wallet_transactions_fetches (character_id, fetched_at) VALUES (?1, ?2) \
             ON CONFLICT (character_id) DO UPDATE SET fetched_at = ?2",
            params![character, fetched_at],
        )?;
        {
            let mut stmt = tx.prepare(
                "INSERT OR IGNORE INTO wallet_transactions (character_id, transaction_id, at, \
                 client_id, is_buy, is_personal, journal_ref_id, location_id, quantity, type_id, \
                 unit_price) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            )?;
            for row in rows {
                stmt.execute(params![
                    character,
                    row.transaction_id,
                    row.at,
                    row.client_id,
                    flag_value(row.is_buy),
                    flag_value(row.is_personal),
                    row.journal_ref_id,
                    row.location_id,
                    i64::from(row.quantity),
                    i64::from(row.type_id),
                    row.unit_price.ten_thousandths(),
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// The character's wallet journal and the time it was last fetched; `None` when no fetch was
    /// saved. The rows come by id.
    pub fn journal(&mut self, character_id: u64) -> Result<Option<StoredJournal>> {
        let character = to_stored_u64(character_id)?;
        let tx = self.conn.transaction()?;
        let fetched_at: Option<i64> = tx
            .query_row(
                "SELECT fetched_at FROM wallet_journal_fetches WHERE character_id = ?1",
                params![character],
                |row| row.get(0),
            )
            .optional()?;
        let Some(fetched_at) = fetched_at else {
            return Ok(None);
        };
        let rows = journal_rows(&tx, character)?;
        tx.commit()?;
        Ok(Some(StoredJournal {
            character_id,
            fetched_at,
            rows,
        }))
    }

    /// The character's market transactions and the time they were last fetched; `None` when no
    /// fetch was saved. The rows come by transaction id.
    pub fn transactions(&mut self, character_id: u64) -> Result<Option<StoredTransactions>> {
        let character = to_stored_u64(character_id)?;
        let tx = self.conn.transaction()?;
        let fetched_at: Option<i64> = tx
            .query_row(
                "SELECT fetched_at FROM wallet_transactions_fetches WHERE character_id = ?1",
                params![character],
                |row| row.get(0),
            )
            .optional()?;
        let Some(fetched_at) = fetched_at else {
            return Ok(None);
        };
        let rows = transaction_rows(&tx, character)?;
        tx.commit()?;
        Ok(Some(StoredTransactions {
            character_id,
            fetched_at,
            rows,
        }))
    }
}

/// A row the journal table will keep, or `BadData` naming the row and the reason.
fn ensure_journal(row: &JournalRow) -> Result<()> {
    let bad = |reason: &str| StoreError::BadData(format!("wallet journal id {}: {reason}", row.id));
    if row.id < 0 {
        return Err(bad("the id is negative"));
    }
    if row.ref_type.is_empty() {
        return Err(bad("the reference type is empty"));
    }
    Ok(())
}

/// A row the transaction table will keep, or `BadData` naming the row and the reason.
fn ensure_transaction(row: &TransactionRow) -> Result<()> {
    let bad = |reason: &str| {
        StoreError::BadData(format!(
            "wallet transaction {}: {reason}",
            row.transaction_id
        ))
    };
    if row.transaction_id < 0 {
        return Err(bad("the id is negative"));
    }
    if row.quantity == 0 {
        return Err(bad("the quantity is zero"));
    }
    if row.unit_price < Money::ZERO {
        return Err(bad("the unit price is below zero"));
    }
    Ok(())
}

fn ensure_journal_ids_once(rows: &[JournalRow]) -> Result<()> {
    let mut seen = BTreeSet::new();
    for row in rows {
        if !seen.insert(row.id) {
            return Err(StoreError::BadData(format!(
                "wallet journal id {} is listed twice",
                row.id
            )));
        }
    }
    Ok(())
}

fn ensure_transaction_ids_once(rows: &[TransactionRow]) -> Result<()> {
    let mut seen = BTreeSet::new();
    for row in rows {
        if !seen.insert(row.transaction_id) {
            return Err(StoreError::BadData(format!(
                "wallet transaction {} is listed twice",
                row.transaction_id
            )));
        }
    }
    Ok(())
}

fn flag_value(flag: bool) -> i64 {
    if flag { 1 } else { 0 }
}

/// The character's stored journal rows, by id.
fn journal_rows(tx: &Transaction<'_>, character: i64) -> Result<Vec<JournalRow>> {
    let mut stmt = tx.prepare(
        "SELECT id, at, ref_type, ref_id, first_party_id, second_party_id, amount, balance, \
         reason, tax, tax_receiver_id, context_id, context_id_type, description \
         FROM wallet_journal WHERE character_id = ?1 ORDER BY id",
    )?;
    let rows = stmt.query_map(params![character], |row| {
        Ok(StoredJournalRow {
            id: row.get(0)?,
            at: row.get(1)?,
            ref_type: row.get(2)?,
            ref_id: row.get(3)?,
            first_party_id: row.get(4)?,
            second_party_id: row.get(5)?,
            amount: row.get(6)?,
            balance: row.get(7)?,
            reason: row.get(8)?,
            tax: row.get(9)?,
            tax_receiver_id: row.get(10)?,
            context_id: row.get(11)?,
            context_id_type: row.get(12)?,
            description: row.get(13)?,
        })
    })?;
    rows.map(|row| row?.into_row()).collect()
}

/// The character's stored transactions, by transaction id.
fn transaction_rows(tx: &Transaction<'_>, character: i64) -> Result<Vec<TransactionRow>> {
    let mut stmt = tx.prepare(
        "SELECT transaction_id, at, client_id, is_buy, is_personal, journal_ref_id, location_id, \
         quantity, type_id, unit_price FROM wallet_transactions WHERE character_id = ?1 \
         ORDER BY transaction_id",
    )?;
    let rows = stmt.query_map(params![character], |row| {
        Ok(StoredTransactionRow {
            transaction_id: row.get(0)?,
            at: row.get(1)?,
            client_id: row.get(2)?,
            is_buy: row.get(3)?,
            is_personal: row.get(4)?,
            journal_ref_id: row.get(5)?,
            location_id: row.get(6)?,
            quantity: row.get(7)?,
            type_id: row.get(8)?,
            unit_price: row.get(9)?,
        })
    })?;
    rows.map(|row| row?.into_row()).collect()
}

/// One journal row as the database holds it, before it is turned back into Rust types.
struct StoredJournalRow {
    id: i64,
    at: i64,
    ref_type: Option<String>,
    ref_id: Option<i64>,
    first_party_id: Option<i64>,
    second_party_id: Option<i64>,
    amount: Option<i64>,
    balance: Option<i64>,
    reason: Option<String>,
    tax: Option<i64>,
    tax_receiver_id: Option<i64>,
    context_id: Option<i64>,
    context_id_type: Option<String>,
    description: String,
}

impl StoredJournalRow {
    /// The row's journal entry. A value the table's checks should have refused is `BadData`
    /// naming the column it came from; nothing is clamped to fit.
    fn into_row(self) -> Result<JournalRow> {
        if self.id < 0 {
            return Err(stored_range(self.id, JOURNAL, self.id, "id"));
        }
        let Some(ref_type) = self.ref_type else {
            return Err(StoreError::BadData(format!(
                "{JOURNAL} row {}, column ref_type: missing",
                self.id
            )));
        };
        if ref_type.is_empty() {
            return Err(StoreError::BadData(format!(
                "{JOURNAL} row {}, column ref_type: empty",
                self.id
            )));
        }
        Ok(JournalRow {
            id: self.id,
            at: self.at,
            ref_type,
            ref_id: self.ref_id,
            first_party_id: self.first_party_id,
            second_party_id: self.second_party_id,
            amount: self.amount.map(Money::from_ten_thousandths),
            balance: self.balance.map(Money::from_ten_thousandths),
            reason: self.reason,
            tax: self.tax.map(Money::from_ten_thousandths),
            tax_receiver_id: self.tax_receiver_id,
            context_id: self.context_id,
            context_id_type: self.context_id_type,
            description: self.description,
        })
    }
}

/// One transaction row as the database holds it, before it is turned back into Rust types.
struct StoredTransactionRow {
    transaction_id: i64,
    at: i64,
    client_id: i64,
    is_buy: i64,
    is_personal: i64,
    journal_ref_id: i64,
    location_id: i64,
    quantity: i64,
    type_id: i64,
    unit_price: i64,
}

impl StoredTransactionRow {
    /// The row's transaction. A value the table's checks should have refused is `BadData` naming
    /// the column it came from; nothing is clamped to fit.
    fn into_row(self) -> Result<TransactionRow> {
        if self.transaction_id < 0 {
            return Err(stored_range(
                self.transaction_id,
                TRANSACTIONS,
                self.transaction_id,
                "transaction_id",
            ));
        }
        if self.unit_price < 0 {
            return Err(stored_range(
                self.unit_price,
                TRANSACTIONS,
                self.transaction_id,
                "unit_price",
            ));
        }
        Ok(TransactionRow {
            transaction_id: self.transaction_id,
            at: self.at,
            client_id: self.client_id,
            is_buy: stored_flag(self.is_buy, self.transaction_id, "is_buy")?,
            is_personal: stored_flag(self.is_personal, self.transaction_id, "is_personal")?,
            journal_ref_id: self.journal_ref_id,
            location_id: self.location_id,
            quantity: stored_quantity(self.quantity, self.transaction_id)?,
            type_id: from_stored(self.type_id, TRANSACTIONS, self.transaction_id, "type_id")?,
            unit_price: Money::from_ten_thousandths(self.unit_price),
        })
    }
}

fn stored_flag(value: i64, transaction: i64, column: &'static str) -> Result<bool> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        other => Err(bad_code(TRANSACTIONS, transaction, column, other)),
    }
}

fn stored_quantity(value: i64, transaction: i64) -> Result<u32> {
    u32::try_from(value)
        .ok()
        .filter(|quantity| *quantity >= 1)
        .ok_or_else(|| stored_range(value, TRANSACTIONS, transaction, "quantity"))
}

fn stored_range(value: i64, table: &str, row: i64, column: &'static str) -> StoreError {
    StoreError::BadData(format!(
        "{table} row {row}, column {column}: {value} is out of range"
    ))
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

    fn journal(id: i64) -> JournalRow {
        JournalRow {
            id,
            at: 1_793_231_998,
            ref_type: "player_trading".to_owned(),
            ref_id: Some(9_000_000_000),
            first_party_id: Some(1_000_000),
            second_party_id: Some(1_000_001),
            amount: Some(Money::from_ten_thousandths(-12_345_678_900)),
            balance: Some(Money::from_ten_thousandths(123_456_789_000)),
            reason: Some("fees".to_owned()),
            tax: Some(Money::from_ten_thousandths(12_500)),
            tax_receiver_id: Some(1_000_002),
            context_id: Some(60_003_760),
            context_id_type: Some("station_id".to_owned()),
            description: "sold".to_owned(),
        }
    }

    fn transaction(id: i64) -> TransactionRow {
        TransactionRow {
            transaction_id: id,
            at: 1_793_000_000,
            client_id: 1_000_001,
            is_buy: true,
            is_personal: false,
            journal_ref_id: 9_000_000_002,
            location_id: 60_003_760,
            quantity: 10,
            type_id: 34,
            unit_price: Money::from_ten_thousandths(57_500),
        }
    }

    #[test]
    fn a_journal_save_is_read_back_by_id() {
        let (_dir, _path, mut store) = open_temp();
        // Written out of order; the read comes by id.
        store
            .save_journal(
                CHARACTER,
                1_000,
                &[
                    journal(9_000_000_002),
                    journal(9_000_000_001),
                    journal(9_000_000_000),
                ],
            )
            .expect("save");
        let stored = store.journal(CHARACTER).expect("read").expect("some");
        assert_eq!(stored.character_id, CHARACTER);
        assert_eq!(stored.fetched_at, 1_000);
        let ids: Vec<i64> = stored.rows.iter().map(|row| row.id).collect();
        assert_eq!(ids, [9_000_000_000, 9_000_000_001, 9_000_000_002]);
        assert_eq!(stored.rows[0], journal(9_000_000_000));
        assert!(store.journal(999).expect("read").is_none());
    }

    #[test]
    fn a_journal_save_adds_only_new_ids() {
        let (_dir, _path, mut store) = open_temp();
        store
            .save_journal(
                CHARACTER,
                1_000,
                &[journal(9_000_000_001), journal(9_000_000_002)],
            )
            .expect("save");
        let mut changed = journal(9_000_000_001);
        changed.description = "changed".to_owned();
        store
            .save_journal(
                CHARACTER,
                2_000,
                &[changed, journal(9_000_000_002), journal(9_000_000_003)],
            )
            .expect("save again");
        let stored = store.journal(CHARACTER).expect("read").expect("some");
        assert_eq!(stored.fetched_at, 2_000);
        let ids: Vec<i64> = stored.rows.iter().map(|row| row.id).collect();
        assert_eq!(ids, [9_000_000_001, 9_000_000_002, 9_000_000_003]);
        assert_eq!(stored.rows[0].description, "sold");
        store
            .save_journal(CHARACTER, 3_000, &[])
            .expect("save empty");
        let stored = store.journal(CHARACTER).expect("read").expect("some");
        assert_eq!(stored.fetched_at, 3_000);
        assert_eq!(stored.rows.len(), 3);
    }

    #[test]
    fn a_journal_row_the_store_refuses_writes_nothing() {
        let (_dir, _path, mut store) = open_temp();
        store
            .save_journal(CHARACTER, 1_000, &[journal(9_000_000_001)])
            .expect("save");
        let mut empty_type = journal(9_000_000_002);
        empty_type.ref_type = String::new();
        let mut negative = journal(-1);
        negative.ref_type = "bounty_prizes".to_owned();
        let repeated = [journal(9_000_000_002), journal(9_000_000_002)];
        for rows in [vec![empty_type], vec![negative], repeated.to_vec()] {
            let refused = store.save_journal(CHARACTER, 2_000, &rows);
            assert!(
                matches!(refused, Err(StoreError::BadData(_))),
                "{refused:?}"
            );
            let stored = store.journal(CHARACTER).expect("read").expect("some");
            assert_eq!(stored.fetched_at, 1_000);
            assert_eq!(stored.rows, vec![journal(9_000_000_001)]);
        }
    }

    #[test]
    fn characters_are_kept_apart() {
        let (_dir, _path, mut store) = open_temp();
        store
            .save_journal(CHARACTER, 1_000, &[journal(9_000_000_001)])
            .expect("save");
        store
            .save_transactions(CHARACTER, 1_000, &[transaction(7_000_000_002)])
            .expect("save transactions");
        store
            .save_journal(OTHER, 1_500, &[journal(9_000_000_009)])
            .expect("save other");
        let other = store.journal(OTHER).expect("read").expect("some");
        assert_eq!(other.character_id, OTHER);
        assert_eq!(other.fetched_at, 1_500);
        assert_eq!(other.rows, vec![journal(9_000_000_009)]);
        assert!(store.transactions(OTHER).expect("read").is_none());
    }

    #[test]
    fn transactions_are_read_back_by_transaction_id() {
        let (_dir, _path, mut store) = open_temp();
        store
            .save_transactions(
                CHARACTER,
                1_000,
                &[transaction(7_000_000_002), transaction(7_000_000_001)],
            )
            .expect("save");
        let stored = store.transactions(CHARACTER).expect("read").expect("some");
        assert_eq!(stored.character_id, CHARACTER);
        assert_eq!(stored.fetched_at, 1_000);
        assert_eq!(
            stored.rows,
            vec![transaction(7_000_000_001), transaction(7_000_000_002)]
        );
        assert!(store.transactions(999).expect("read").is_none());
    }

    #[test]
    fn a_transaction_save_adds_only_new_ids() {
        let (_dir, _path, mut store) = open_temp();
        store
            .save_transactions(
                CHARACTER,
                1_000,
                &[transaction(7_000_000_002), transaction(7_000_000_001)],
            )
            .expect("save");
        let mut changed = transaction(7_000_000_002);
        changed.unit_price = Money::from_ten_thousandths(1);
        store
            .save_transactions(CHARACTER, 2_000, &[changed, transaction(7_000_000_003)])
            .expect("save again");
        let stored = store.transactions(CHARACTER).expect("read").expect("some");
        assert_eq!(stored.fetched_at, 2_000);
        assert_eq!(
            stored.rows,
            vec![
                transaction(7_000_000_001),
                transaction(7_000_000_002),
                transaction(7_000_000_003)
            ]
        );
        assert_eq!(
            stored.rows[1].unit_price,
            Money::from_ten_thousandths(57_500)
        );
        store
            .save_transactions(CHARACTER, 3_000, &[])
            .expect("save empty");
        let stored = store.transactions(CHARACTER).expect("read").expect("some");
        assert_eq!(stored.fetched_at, 3_000);
        assert_eq!(stored.rows.len(), 3);
    }

    #[test]
    fn a_transaction_row_the_store_refuses_writes_nothing() {
        let (_dir, _path, mut store) = open_temp();
        store
            .save_transactions(CHARACTER, 1_000, &[transaction(7_000_000_001)])
            .expect("save");
        let mut no_quantity = transaction(7_000_000_002);
        no_quantity.quantity = 0;
        let mut negative_price = transaction(7_000_000_002);
        negative_price.unit_price = Money::from_ten_thousandths(-1);
        let mut negative_id = transaction(-1);
        negative_id.unit_price = Money::ZERO;
        let repeated = [transaction(7_000_000_002), transaction(7_000_000_002)];
        for rows in [
            vec![no_quantity],
            vec![negative_price],
            vec![negative_id],
            repeated.to_vec(),
        ] {
            let refused = store.save_transactions(CHARACTER, 2_000, &rows);
            assert!(
                matches!(refused, Err(StoreError::BadData(_))),
                "{refused:?}"
            );
            let stored = store.transactions(CHARACTER).expect("read").expect("some");
            assert_eq!(stored.fetched_at, 1_000);
            assert_eq!(stored.rows, vec![transaction(7_000_000_001)]);
        }
    }

    #[test]
    fn a_stored_journal_value_outside_the_checks_is_bad_data() {
        let (_dir, _path, mut store) = open_temp();
        store
            .save_journal(CHARACTER, 1_000, &[journal(9_000_000_001)])
            .expect("save");
        // A table without the checks stands for a file another build wrote.
        store
            .conn
            .execute_batch(
                "DROP TABLE wallet_journal;
                 CREATE TABLE wallet_journal (
                     character_id INTEGER NOT NULL,
                     id INTEGER NOT NULL,
                     at INTEGER NOT NULL,
                     ref_type TEXT NOT NULL,
                     ref_id INTEGER,
                     first_party_id INTEGER,
                     second_party_id INTEGER,
                     amount INTEGER,
                     balance INTEGER,
                     reason TEXT,
                     tax INTEGER,
                     tax_receiver_id INTEGER,
                     context_id INTEGER,
                     context_id_type TEXT,
                     description TEXT NOT NULL,
                     PRIMARY KEY (character_id, id)
                 )",
            )
            .expect("recreate");
        let insert = |id: i64, ref_type: &str| {
            format!(
                "INSERT INTO wallet_journal (character_id, id, at, ref_type, description) \
                 VALUES ({CHARACTER}, {id}, 1, '{ref_type}', 'x')"
            )
        };
        for (sql, column) in [
            (insert(4001, ""), "ref_type"),
            (insert(-1, "bounty_prizes"), "id"),
        ] {
            store.conn.execute_batch(&sql).expect("insert");
            let refused = store.journal(CHARACTER);
            assert!(
                matches!(&refused, Err(StoreError::BadData(text)) if text.contains(column)),
                "{refused:?}"
            );
            store
                .conn
                .execute("DELETE FROM wallet_journal WHERE id < 0 OR id = 4001", [])
                .expect("delete");
        }
    }

    #[test]
    fn a_stored_transaction_value_outside_the_checks_is_bad_data() {
        let (_dir, _path, mut store) = open_temp();
        store
            .save_transactions(CHARACTER, 1_000, &[transaction(7_000_000_001)])
            .expect("save");
        store
            .conn
            .execute_batch(
                "DROP TABLE wallet_transactions;
                 CREATE TABLE wallet_transactions (
                     character_id INTEGER NOT NULL,
                     transaction_id INTEGER NOT NULL,
                     at INTEGER NOT NULL,
                     client_id INTEGER NOT NULL,
                     is_buy INTEGER NOT NULL,
                     is_personal INTEGER NOT NULL,
                     journal_ref_id INTEGER NOT NULL,
                     location_id INTEGER NOT NULL,
                     quantity INTEGER NOT NULL,
                     type_id INTEGER NOT NULL,
                     unit_price INTEGER NOT NULL,
                     PRIMARY KEY (character_id, transaction_id)
                 )",
            )
            .expect("recreate");
        let insert = |id: i64, is_buy: i64, is_personal: i64, quantity: i64, unit_price: i64| {
            format!(
                "INSERT INTO wallet_transactions (character_id, transaction_id, at, client_id, \
                 is_buy, is_personal, journal_ref_id, location_id, quantity, type_id, unit_price) \
                 VALUES ({CHARACTER}, {id}, 1, 1, {is_buy}, {is_personal}, 1, 1, {quantity}, 34, \
                 {unit_price})"
            )
        };
        for (sql, column) in [
            (insert(-1, 1, 0, 1, 0), "transaction_id"),
            (insert(4002, 2, 0, 1, 0), "is_buy"),
            (insert(4003, 0, 2, 2, 0), "is_personal"),
            (insert(4004, 1, 0, 0, 0), "quantity"),
            (insert(4005, 1, 0, 1, -1), "unit_price"),
        ] {
            store.conn.execute_batch(&sql).expect("insert");
            let refused = store.transactions(CHARACTER);
            assert!(
                matches!(&refused, Err(StoreError::BadData(text)) if text.contains(column)),
                "{refused:?}"
            );
            store
                .conn
                .execute(
                    "DELETE FROM wallet_transactions WHERE transaction_id < 0 OR transaction_id BETWEEN 4002 AND 4005",
                    [],
                )
                .expect("delete");
        }
    }
}

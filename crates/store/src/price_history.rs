//! The price history: one reading a day for each market the user chose, kept as long as the
//! market's retention allows, and the room the history takes.

use std::cmp::Reverse;
use std::collections::{BTreeMap, VecDeque};

use mop_core::industry::TypeId;
use mop_core::money::Money;
use mop_core::price_history::{ROW_BYTES, Retention, RetentionError, days_to_drop};
use mop_market::fuzzwork::{Side, SummaryRow};
use mop_market::history::{DayStat, HISTORY_DAYS, HistoryDay, summarise};
use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension, params};

use crate::error::{Result, StoreError};
use crate::migrate::Store;
use crate::{bad_code, from_stored, to_stored_u64};

const MARKETS: &str = "history_markets";
const READINGS: &str = "price_history";
const DAY_HISTORY: &str = "market_history";
const SETTINGS: &str = "history_settings";
const MARKET_COLUMNS: &str = "id, kind, place, daily_days, total_days, enabled";

/// What kind of place a market is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarketKind {
    Region,
    System,
    Station,
    Structure,
}

/// A market whose price readings are kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryMarket {
    pub id: i64,
    pub kind: MarketKind,
    /// The region, system, station or structure id.
    pub place: u64,
    pub retention: Retention,
    /// A disabled market records nothing, but its readings are kept.
    pub enabled: bool,
}

/// What a recording or a retention change did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordOutcome {
    pub rows_written: u64,
    pub days_dropped: u64,
}

/// What a retention change did: it writes no readings, so `rows_written` is always 0.
pub type ThinOutcome = RecordOutcome;

/// How much one market's history holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarketUsage {
    pub market: i64,
    pub days: u32,
    pub first_day: Option<i64>,
    pub last_day: Option<i64>,
    pub rows: u64,
    /// `rows` times `ROW_BYTES`. An estimate; see `database_bytes` for the file's real size.
    pub estimated_bytes: u64,
}

/// The price history settings: the size limit, if any, and whether prices refresh by themselves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistorySettings {
    pub limit_bytes: Option<u64>,
    pub auto_refresh: bool,
}

/// The stored columns of one market, before they are turned back into Rust types.
type RawMarket = (i64, i64, i64, i64, Option<i64>, i64);

/// The stored columns of one price reading: day, side, min, max, weighted average, median, five
/// percent, volume and orders.
type RawReading = (i64, i64, i64, i64, i64, i64, i64, i64, i64);

fn kind_code(kind: MarketKind) -> i64 {
    match kind {
        MarketKind::Region => 0,
        MarketKind::System => 1,
        MarketKind::Station => 2,
        MarketKind::Structure => 3,
    }
}

fn kind_from_code(code: i64) -> Option<MarketKind> {
    match code {
        0 => Some(MarketKind::Region),
        1 => Some(MarketKind::System),
        2 => Some(MarketKind::Station),
        3 => Some(MarketKind::Structure),
        _ => None,
    }
}

fn side_code(side: Side) -> i64 {
    match side {
        Side::Sell => 0,
        Side::Buy => 1,
    }
}

fn side_from_code(code: i64) -> Option<Side> {
    match code {
        0 => Some(Side::Sell),
        1 => Some(Side::Buy),
        _ => None,
    }
}

fn flag_from_stored(value: i64, table: &str, row: i64, column: &str) -> Result<bool> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(bad_code(table, row, column, value)),
    }
}

/// A retention rule read from the database. The daily period and the total limit are checked
/// the same way a new rule is, so a stored rule the app would refuse is bad data.
fn retention_from_stored(id: i64, daily: i64, total: Option<i64>) -> Result<Retention> {
    let daily_days: u32 = from_stored(daily, MARKETS, id, "daily_days")?;
    let total_days: Option<u32> = total
        .map(|value| from_stored(value, MARKETS, id, "total_days"))
        .transpose()?;
    Retention::new(daily_days, total_days).map_err(|err| {
        let column = match err {
            RetentionError::TotalBelowDaily => "total_days",
            RetentionError::ZeroDaily | RetentionError::Overflow => "daily_days",
        };
        StoreError::BadData(format!("{MARKETS} row {id}, column {column}: {err}"))
    })
}

/// A rule the store will keep: the same checks as a new rule, reported as a reason the user can read.
fn checked_retention(retention: Retention) -> Result<Retention> {
    Retention::new(retention.daily_days, retention.total_days)
        .map_err(|err| StoreError::InvalidRetention(err.to_string()))
}

fn raw_market(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawMarket> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
    ))
}

fn market_from_stored(raw: RawMarket) -> Result<HistoryMarket> {
    let (id, kind, place, daily, total, enabled) = raw;
    let kind = kind_from_code(kind).ok_or_else(|| bad_code(MARKETS, id, "kind", kind))?;
    Ok(HistoryMarket {
        id,
        kind,
        place: from_stored(place, MARKETS, id, "place")?,
        retention: retention_from_stored(id, daily, total)?,
        enabled: flag_from_stored(enabled, MARKETS, id, "enabled")?,
    })
}

/// The market with this id, or `NotFound`.
fn market_row(conn: &Connection, id: i64) -> Result<HistoryMarket> {
    let raw = conn
        .query_row(
            &format!("SELECT {MARKET_COLUMNS} FROM history_markets WHERE id = ?1"),
            params![id],
            raw_market,
        )
        .optional()?
        .ok_or(StoreError::NotFound)?;
    market_from_stored(raw)
}

fn reading_error(day: i64, column: &str, value: i64) -> StoreError {
    StoreError::BadData(format!(
        "{READINGS} day {day}, column {column}: {value} is not a valid value"
    ))
}

/// The stored columns of one day of ESI sales history, read as whatever SQLite holds: day,
/// average, highest, lowest, volume and orders.
type RawHistoryDay = (Value, Value, Value, Value, Value, Value);

fn raw_history_day(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawHistoryDay> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
    ))
}

fn history_error(type_id: TypeId, day: Option<i64>, column: &str, value: &str) -> StoreError {
    let row = match day {
        Some(day) => format!("{type_id}/{day}"),
        None => type_id.to_string(),
    };
    StoreError::BadData(format!(
        "{DAY_HISTORY} row {row}, column {column}: {value} is out of range"
    ))
}

fn history_integer(type_id: TypeId, day: Option<i64>, column: &str, value: Value) -> Result<i64> {
    match value {
        Value::Integer(value) => Ok(value),
        Value::Null => Err(history_error(type_id, day, column, "NULL")),
        Value::Real(value) => Err(history_error(type_id, day, column, &value.to_string())),
        Value::Text(value) => Err(history_error(type_id, day, column, &format!("{value:?}"))),
        Value::Blob(value) => Err(history_error(type_id, day, column, &format!("{value:?}"))),
    }
}

fn history_money(type_id: TypeId, day: i64, column: &str, value: Value) -> Result<Money> {
    let value = history_integer(type_id, Some(day), column, value)?;
    if value < 0 {
        return Err(history_error(
            type_id,
            Some(day),
            column,
            &value.to_string(),
        ));
    }
    Ok(Money::from_ten_thousandths(value))
}

fn history_u64(type_id: TypeId, day: i64, column: &str, value: Value) -> Result<u64> {
    let value = history_integer(type_id, Some(day), column, value)?;
    u64::try_from(value)
        .ok()
        .ok_or_else(|| history_error(type_id, Some(day), column, &value.to_string()))
}

fn history_day_from_stored(type_id: TypeId, raw: RawHistoryDay) -> Result<HistoryDay> {
    let (day, average, highest, lowest, volume, orders) = raw;
    let day = history_integer(type_id, None, "day", day)?;
    Ok(HistoryDay {
        day,
        average: history_money(type_id, day, "average", average)?,
        highest: history_money(type_id, day, "highest", highest)?,
        lowest: history_money(type_id, day, "lowest", lowest)?,
        volume: history_u64(type_id, day, "volume", volume)?,
        orders: history_u64(type_id, day, "orders", orders)?,
    })
}

fn write_history_day(conn: &Connection, type_id: TypeId, day: &HistoryDay) -> Result<()> {
    conn.execute(
        "INSERT INTO market_history (type_id, day, average, highest, lowest, volume, orders) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            i64::from(type_id),
            day.day,
            day.average.ten_thousandths(),
            day.highest.ten_thousandths(),
            day.lowest.ten_thousandths(),
            to_stored_u64(day.volume)?,
            to_stored_u64(day.orders)?,
        ],
    )?;
    Ok(())
}

/// The newest day the market has a reading for, if it has any.
fn newest_day(conn: &Connection, market: i64) -> Result<Option<i64>> {
    Ok(conn.query_row(
        "SELECT MAX(day) FROM price_history WHERE market = ?1",
        params![market],
        |row| row.get(0),
    )?)
}

fn held_days(conn: &Connection, market: i64) -> Result<Vec<i64>> {
    let mut stmt = conn.prepare("SELECT DISTINCT day FROM price_history WHERE market = ?1")?;
    let days = stmt
        .query_map(params![market], |row| row.get(0))?
        .collect::<rusqlite::Result<Vec<i64>>>()?;
    Ok(days)
}

/// Drops the readings the market's retention no longer keeps, counted back from `today`, and
/// returns how many days went.
fn thin(conn: &Connection, market: i64, today: i64, retention: &Retention) -> Result<u64> {
    let days = held_days(conn, market)?;
    let dropped = days_to_drop(&days, today, retention);
    for day in &dropped {
        conn.execute(
            "DELETE FROM price_history WHERE market = ?1 AND day = ?2",
            params![market, day],
        )?;
    }
    u64::try_from(dropped.len())
        .ok()
        .ok_or(StoreError::TooLarge)
}

fn write_reading(
    conn: &Connection,
    market: i64,
    day: i64,
    type_id: TypeId,
    side: Side,
    row: &SummaryRow,
) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO price_history \
         (market, type_id, side, day, min, max, weighted_average, median, five_percent, volume, orders) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            market,
            i64::from(type_id),
            side_code(side),
            day,
            row.min.ten_thousandths(),
            row.max.ten_thousandths(),
            row.weighted_average.ten_thousandths(),
            row.median.ten_thousandths(),
            row.five_percent.ten_thousandths(),
            to_stored_u64(row.volume)?,
            i64::from(row.orders),
        ],
    )?;
    Ok(())
}

/// One market's readings, oldest day first: the days with how many rows each holds.
struct Held {
    market: i64,
    rows: u64,
    days: VecDeque<(i64, u64)>,
}

fn load_held(conn: &Connection) -> Result<Vec<Held>> {
    let mut stmt = conn.prepare("SELECT id FROM history_markets ORDER BY id")?;
    let ids = stmt
        .query_map([], |row| row.get(0))?
        .collect::<rusqlite::Result<Vec<i64>>>()?;
    let mut held = Vec::new();
    for market in ids {
        let mut stmt = conn.prepare(
            "SELECT day, COUNT(*) FROM price_history WHERE market = ?1 GROUP BY day ORDER BY day",
        )?;
        let counts = stmt
            .query_map(params![market], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<Vec<(i64, i64)>>>()?;
        let mut rows: u64 = 0;
        let mut days = VecDeque::new();
        for (day, count) in counts {
            let count = u64::try_from(count).ok().ok_or(StoreError::TooLarge)?;
            rows = rows.checked_add(count).ok_or(StoreError::TooLarge)?;
            days.push_back((day, count));
        }
        held.push(Held { market, rows, days });
    }
    Ok(held)
}

impl Store {
    /// Every market in the price history, by id.
    pub fn history_markets(&self) -> Result<Vec<HistoryMarket>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {MARKET_COLUMNS} FROM history_markets ORDER BY id"
        ))?;
        let raws = stmt
            .query_map([], raw_market)?
            .collect::<rusqlite::Result<Vec<RawMarket>>>()?;
        raws.into_iter().map(market_from_stored).collect()
    }

    /// Adds a market with its retention. The market starts enabled. A market already added for
    /// the same kind and place is `Duplicate`.
    pub fn add_history_market(
        &mut self,
        kind: MarketKind,
        place: u64,
        retention: Retention,
    ) -> Result<i64> {
        let retention = checked_retention(retention)?;
        let place = to_stored_u64(place)?;
        let tx = self.conn.transaction()?;
        let taken: bool = tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM history_markets WHERE kind = ?1 AND place = ?2)",
            params![kind_code(kind), place],
            |row| row.get(0),
        )?;
        if taken {
            return Err(StoreError::Duplicate);
        }
        tx.execute(
            "INSERT INTO history_markets (kind, place, daily_days, total_days, enabled) \
             VALUES (?1, ?2, ?3, ?4, 1)",
            params![
                kind_code(kind),
                place,
                i64::from(retention.daily_days),
                retention.total_days.map(i64::from),
            ],
        )?;
        let id = tx.last_insert_rowid();
        tx.commit()?;
        Ok(id)
    }

    /// Saves a market's retention, then drops the readings it no longer keeps, counted back from
    /// the market's newest day.
    pub fn set_retention(&mut self, market: i64, retention: Retention) -> Result<ThinOutcome> {
        let retention = checked_retention(retention)?;
        let tx = self.conn.transaction()?;
        market_row(&tx, market)?;
        tx.execute(
            "UPDATE history_markets SET daily_days = ?2, total_days = ?3 WHERE id = ?1",
            params![
                market,
                i64::from(retention.daily_days),
                retention.total_days.map(i64::from),
            ],
        )?;
        let days_dropped = match newest_day(&tx, market)? {
            Some(today) => thin(&tx, market, today, &retention)?,
            None => 0,
        };
        tx.commit()?;
        Ok(RecordOutcome {
            rows_written: 0,
            days_dropped,
        })
    }

    /// Turns a market's recording on or off. A market that is off records nothing.
    pub fn set_history_enabled(&mut self, market: i64, enabled: bool) -> Result<()> {
        let changed = self.conn.execute(
            "UPDATE history_markets SET enabled = ?2 WHERE id = ?1",
            params![market, i64::from(enabled)],
        )?;
        if changed == 0 {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }

    /// Saves one day's readings of a market, then drops the readings its retention no longer
    /// keeps, counted back from `day`. A reading already saved for that day and type is replaced.
    /// A disabled market writes nothing.
    pub fn record(
        &mut self,
        market: i64,
        day: i64,
        rows: &[(TypeId, Side, SummaryRow)],
    ) -> Result<RecordOutcome> {
        let tx = self.conn.transaction()?;
        let found = market_row(&tx, market)?;
        if !found.enabled {
            return Ok(RecordOutcome {
                rows_written: 0,
                days_dropped: 0,
            });
        }
        let mut rows_written: u64 = 0;
        for (type_id, side, row) in rows {
            write_reading(&tx, market, day, *type_id, *side, row)?;
            rows_written = rows_written.checked_add(1).ok_or(StoreError::TooLarge)?;
        }
        let days_dropped = thin(&tx, market, day, &found.retention)?;
        tx.commit()?;
        Ok(RecordOutcome {
            rows_written,
            days_dropped,
        })
    }

    /// The readings of one type on one side, oldest day first.
    pub fn series(
        &self,
        market: i64,
        type_id: TypeId,
        side: Side,
    ) -> Result<Vec<(i64, SummaryRow)>> {
        market_row(&self.conn, market)?;
        let mut stmt = self.conn.prepare(
            "SELECT day, side, min, max, weighted_average, median, five_percent, volume, orders \
             FROM price_history WHERE market = ?1 AND type_id = ?2 ORDER BY day",
        )?;
        let raws = stmt
            .query_map(params![market, i64::from(type_id)], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<RawReading>>>()?;
        let mut readings = Vec::new();
        for (day, side_value, min, max, weighted, median, five, volume, orders) in raws {
            let row_side =
                side_from_code(side_value).ok_or_else(|| reading_error(day, "side", side_value))?;
            if row_side != side {
                continue;
            }
            let row = SummaryRow {
                min: Money::from_ten_thousandths(min),
                max: Money::from_ten_thousandths(max),
                weighted_average: Money::from_ten_thousandths(weighted),
                median: Money::from_ten_thousandths(median),
                five_percent: Money::from_ten_thousandths(five),
                volume: u64::try_from(volume)
                    .ok()
                    .ok_or_else(|| reading_error(day, "volume", volume))?,
                orders: u32::try_from(orders)
                    .ok()
                    .ok_or_else(|| reading_error(day, "orders", orders))?,
            };
            readings.push((day, row));
        }
        Ok(readings)
    }

    /// How much each market's history holds, in market order. A market with no readings is listed
    /// with zeros.
    pub fn history_usage(&self) -> Result<Vec<MarketUsage>> {
        let mut stmt = self.conn.prepare(
            "SELECT m.id, COUNT(DISTINCT p.day), MIN(p.day), MAX(p.day), COUNT(p.day) \
             FROM history_markets m LEFT JOIN price_history p ON p.market = m.id \
             GROUP BY m.id ORDER BY m.id",
        )?;
        let raws = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        raws.into_iter()
            .map(|(market, days, first_day, last_day, rows)| {
                let rows = u64::try_from(rows).ok().ok_or(StoreError::TooLarge)?;
                Ok(MarketUsage {
                    market,
                    days: u32::try_from(days).ok().ok_or(StoreError::TooLarge)?,
                    first_day,
                    last_day,
                    rows,
                    estimated_bytes: rows.checked_mul(ROW_BYTES).ok_or(StoreError::TooLarge)?,
                })
            })
            .collect()
    }

    /// The size of the database file, from its pages.
    pub fn database_bytes(&self) -> Result<u64> {
        let pages: i64 = self
            .conn
            .pragma_query_value(None, "page_count", |row| row.get(0))?;
        let page_size: i64 = self
            .conn
            .pragma_query_value(None, "page_size", |row| row.get(0))?;
        let bytes = pages.checked_mul(page_size).ok_or(StoreError::TooLarge)?;
        u64::try_from(bytes).ok().ok_or(StoreError::TooLarge)
    }

    /// The saved price history settings.
    pub fn history_settings(&self) -> Result<HistorySettings> {
        let (limit, auto_refresh): (Option<i64>, i64) = self.conn.query_row(
            "SELECT limit_bytes, auto_refresh FROM history_settings WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        Ok(HistorySettings {
            limit_bytes: limit
                .map(|value| from_stored(value, SETTINGS, 1, "limit_bytes"))
                .transpose()?,
            auto_refresh: flag_from_stored(auto_refresh, SETTINGS, 1, "auto_refresh")?,
        })
    }

    pub fn save_history_settings(&mut self, settings: &HistorySettings) -> Result<()> {
        let limit = settings.limit_bytes.map(to_stored_u64).transpose()?;
        let changed = self.conn.execute(
            "UPDATE history_settings SET limit_bytes = ?1, auto_refresh = ?2 WHERE id = 1",
            params![limit, i64::from(settings.auto_refresh)],
        )?;
        if changed == 0 {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }

    /// Drops the oldest day of the market with the most rows, one day at a time, until the
    /// history fits `limit_bytes`. A market's newest day is never dropped. Returns the days
    /// dropped; when every market is down to its newest day, that can be more than the limit.
    pub fn enforce_limit(&mut self, limit_bytes: u64) -> Result<u64> {
        let tx = self.conn.transaction()?;
        let mut held = load_held(&tx)?;
        let mut total_rows = held.iter().try_fold(0u64, |sum, market| {
            sum.checked_add(market.rows).ok_or(StoreError::TooLarge)
        })?;
        let mut dropped: u64 = 0;
        loop {
            let total_bytes = total_rows
                .checked_mul(ROW_BYTES)
                .ok_or(StoreError::TooLarge)?;
            if total_bytes <= limit_bytes {
                break;
            }
            // Ties go to the lower id, so the order is the same on every run.
            let Some(market) = held
                .iter_mut()
                .filter(|market| market.days.len() > 1)
                .max_by_key(|market| (market.rows, Reverse(market.market)))
            else {
                break;
            };
            let Some((day, rows)) = market.days.pop_front() else {
                break;
            };
            tx.execute(
                "DELETE FROM price_history WHERE market = ?1 AND day = ?2",
                params![market.market, day],
            )?;
            // `rows` is one of the day counts that make up `market.rows` and `total_rows`, so
            // subtracting it cannot underflow.
            market.rows = market.rows.saturating_sub(rows);
            total_rows = total_rows.saturating_sub(rows);
            dropped = dropped.checked_add(1).ok_or(StoreError::TooLarge)?;
        }
        tx.commit()?;
        Ok(dropped)
    }

    /// Stops the price history of a market and deletes its readings. The Forge may be removed too.
    pub fn forget_market(&mut self, market: i64) -> Result<()> {
        let tx = self.conn.transaction()?;
        market_row(&tx, market)?;
        tx.execute(
            "DELETE FROM price_history WHERE market = ?1",
            params![market],
        )?;
        tx.execute("DELETE FROM history_markets WHERE id = ?1", params![market])?;
        tx.commit()?;
        Ok(())
    }

    /// Saves an item's ESI sales history: the type's earlier rows go, then the newest 30 days are
    /// written, all in one transaction. An empty list deletes the type's rows and writes nothing.
    pub fn save_day_history(&mut self, type_id: TypeId, days: &[HistoryDay]) -> Result<u64> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "DELETE FROM market_history WHERE type_id = ?1",
            params![i64::from(type_id)],
        )?;
        let keep = usize::try_from(HISTORY_DAYS)
            .ok()
            .ok_or(StoreError::TooLarge)?;
        let mut by_day = BTreeMap::new();
        for day in days {
            by_day.insert(day.day, day);
        }
        let kept = by_day.values().rev().take(keep).collect::<Vec<_>>();
        let mut rows_written: u64 = 0;
        for day in kept.into_iter().rev() {
            write_history_day(&tx, type_id, day)?;
            rows_written = rows_written.checked_add(1).ok_or(StoreError::TooLarge)?;
        }
        tx.commit()?;
        Ok(rows_written)
    }

    /// One type's saved sales history, oldest day first.
    pub fn day_history(&self, type_id: TypeId) -> Result<Vec<HistoryDay>> {
        let mut stmt = self.conn.prepare(
            "SELECT day, average, highest, lowest, volume, orders FROM market_history \
             WHERE type_id = ?1 ORDER BY day",
        )?;
        let raws = stmt
            .query_map(params![i64::from(type_id)], raw_history_day)?
            .collect::<rusqlite::Result<Vec<RawHistoryDay>>>()?;
        raws.into_iter()
            .map(|raw| history_day_from_stored(type_id, raw))
            .collect()
    }

    /// The units sold a day and 30-day middle of one type, when it has a saved history.
    pub fn day_stat(&self, type_id: TypeId) -> Result<Option<DayStat>> {
        Ok(summarise(&self.day_history(type_id)?))
    }

    /// The units sold a day and 30-day middle of every type that has a saved history, by type.
    pub fn day_stats(&self) -> Result<BTreeMap<TypeId, DayStat>> {
        let mut stmt = self.conn.prepare(
            "SELECT type_id, day, average, highest, lowest, volume, orders FROM market_history \
             ORDER BY type_id, day",
        )?;
        let raws = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Value>(1)?,
                    row.get::<_, Value>(2)?,
                    row.get::<_, Value>(3)?,
                    row.get::<_, Value>(4)?,
                    row.get::<_, Value>(5)?,
                    row.get::<_, Value>(6)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut grouped: BTreeMap<TypeId, Vec<HistoryDay>> = BTreeMap::new();
        for (type_value, day, average, highest, lowest, volume, orders) in raws {
            let type_id: TypeId = from_stored(type_value, DAY_HISTORY, type_value, "type_id")?;
            let raw = (day, average, highest, lowest, volume, orders);
            let history_day = history_day_from_stored(type_id, raw)?;
            grouped.entry(type_id).or_default().push(history_day);
        }
        let mut stats = BTreeMap::new();
        for (type_id, days) in grouped {
            if let Some(stat) = summarise(&days) {
                stats.insert(type_id, stat);
            }
        }
        Ok(stats)
    }

    /// Saves the market prices of one net worth point. A copy already saved at the same time is
    /// replaced whole. A failed row writes nothing.
    pub fn save_point_prices(&mut self, prices: &PointPrices) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "DELETE FROM point_price_rows WHERE at = ?1",
            params![prices.at],
        )?;
        tx.execute("DELETE FROM point_prices WHERE at = ?1", params![prices.at])?;
        tx.execute(
            "INSERT INTO point_prices (at, published_at) VALUES (?1, ?2)",
            params![prices.at, prices.published_at],
        )?;
        for ((type_id, side), row) in &prices.rows {
            write_point_row(&tx, prices.at, *type_id, *side, row)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// The saved copies whose time is from `from` to `to`, both included, oldest first.
    pub fn point_prices(&self, from: i64, to: i64) -> Result<Vec<PointPrices>> {
        let mut heads = self.conn.prepare(
            "SELECT at, published_at FROM point_prices WHERE at BETWEEN ?1 AND ?2 ORDER BY at",
        )?;
        let heads = heads
            .query_map(params![from, to], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, Option<i64>>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<(i64, Option<i64>)>>>()?;
        heads
            .into_iter()
            .map(|(at, published_at)| {
                Ok(PointPrices {
                    at,
                    published_at,
                    rows: point_rows(&self.conn, at)?,
                })
            })
            .collect()
    }

    /// How many copies and how many price rows are saved, in that order.
    pub fn point_prices_size(&self) -> Result<(u64, u64)> {
        let copies: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM point_prices", [], |row| row.get(0))?;
        let rows: i64 =
            self.conn
                .query_row("SELECT COUNT(*) FROM point_price_rows", [], |row| {
                    row.get(0)
                })?;
        Ok((
            from_stored(copies, "point_prices", 0, "at")?,
            from_stored(rows, "point_price_rows", 0, "at")?,
        ))
    }
}

/// The market prices of every type and side at the moment a net worth point was recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PointPrices {
    /// When the point was recorded.
    pub at: i64,
    /// When the market published the prices, if it was known.
    pub published_at: Option<i64>,
    /// One row for each type and side that had a price.
    pub rows: BTreeMap<(TypeId, Side), SummaryRow>,
}

/// One saved price row: type, side code, then the five prices, volume and orders as stored.
type RawPointRow = (i64, i64, i64, i64, i64, i64, i64, i64, i64);

fn write_point_row(
    conn: &Connection,
    at: i64,
    type_id: TypeId,
    side: Side,
    row: &SummaryRow,
) -> Result<()> {
    conn.execute(
        "INSERT INTO point_price_rows \
         (at, type_id, side, min, max, weighted_average, median, five_percent, volume, orders) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            at,
            i64::from(type_id),
            side_code(side),
            row.min.ten_thousandths(),
            row.max.ten_thousandths(),
            row.weighted_average.ten_thousandths(),
            row.median.ten_thousandths(),
            row.five_percent.ten_thousandths(),
            to_stored_u64(row.volume)?,
            i64::from(row.orders),
        ],
    )?;
    Ok(())
}

/// The saved price rows of one copy, by type and side.
fn point_rows(conn: &Connection, at: i64) -> Result<BTreeMap<(TypeId, Side), SummaryRow>> {
    let mut stmt = conn.prepare(
        "SELECT type_id, side, min, max, weighted_average, median, five_percent, volume, orders \
         FROM point_price_rows WHERE at = ?1",
    )?;
    let raws = stmt
        .query_map(params![at], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
                row.get(7)?,
                row.get(8)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<RawPointRow>>>()?;
    let mut rows = BTreeMap::new();
    for (type_value, side_value, min, max, weighted, median, five, volume, orders) in raws {
        let type_id: TypeId = from_stored(type_value, "point_price_rows", at, "type_id")?;
        let side = side_from_code(side_value)
            .ok_or_else(|| bad_code("point_price_rows", at, "side", side_value))?;
        let row = SummaryRow {
            min: Money::from_ten_thousandths(min),
            max: Money::from_ten_thousandths(max),
            weighted_average: Money::from_ten_thousandths(weighted),
            median: Money::from_ten_thousandths(median),
            five_percent: Money::from_ten_thousandths(five),
            volume: from_stored(volume, "point_price_rows", at, "volume")?,
            orders: from_stored(orders, "point_price_rows", at, "orders")?,
        };
        rows.insert((type_id, side), row);
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::BTreeMap;
    use std::rc::Rc;

    use mop_market::error::MarketError;
    use mop_market::fuzzwork::{BytesHttp, BytesResponse};
    use mop_market::history::History;

    use super::*;

    const FORGE_PLACE: u64 = 10_000_002;
    const TRITANIUM: TypeId = 34;
    const PAU: TypeId = 11_688;

    fn isk(whole: i64) -> Money {
        Money::from_ten_thousandths(whole * 10_000)
    }

    fn row(prices: [i64; 5], volume: u64, orders: u32) -> SummaryRow {
        let [min, max, weighted_average, median, five_percent] = prices;
        SummaryRow {
            min: isk(min),
            max: isk(max),
            weighted_average: isk(weighted_average),
            median: isk(median),
            five_percent: isk(five_percent),
            volume,
            orders,
        }
    }

    fn pau_sell() -> SummaryRow {
        row([61_000, 75_000, 63_100, 66_000, 61_200], 1_200, 14)
    }

    fn pau_buy() -> SummaryRow {
        row([50_000, 55_500, 53_000, 51_000, 55_010], 800, 9)
    }

    fn pau_copy(at: i64, published_at: Option<i64>) -> PointPrices {
        PointPrices {
            at,
            published_at,
            rows: BTreeMap::from([
                ((PAU, Side::Sell), pau_sell()),
                ((PAU, Side::Buy), pau_buy()),
            ]),
        }
    }

    fn open_temp() -> (tempfile::TempDir, std::path::PathBuf, Store) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("eve.db");
        let store = Store::open(&path, 1_000).expect("open").store;
        (dir, path, store)
    }

    #[test]
    fn a_copy_comes_back_whole_after_reopening() {
        let (_dir, path, mut store) = open_temp();
        let copy = pau_copy(1_000_000_000, Some(999_999_000));
        store.save_point_prices(&copy).expect("save");
        drop(store);

        let reopened = Store::open(&path, 2_000).expect("reopen").store;

        assert_eq!(
            reopened
                .point_prices(999_999_999, 1_000_000_000)
                .expect("read"),
            vec![copy]
        );
        assert!(
            reopened
                .point_prices(0, 999_999_999)
                .expect("read")
                .is_empty()
        );
    }

    #[test]
    fn saving_again_at_the_same_time_replaces_the_copy() {
        let (_dir, _path, mut store) = open_temp();
        store
            .save_point_prices(&pau_copy(1_000_000_000, Some(999_999_000)))
            .expect("first");
        let tritanium = PointPrices {
            at: 1_000_000_000,
            published_at: None,
            rows: BTreeMap::from([((TRITANIUM, Side::Sell), pau_sell())]),
        };
        store.save_point_prices(&tritanium).expect("replace");
        store
            .save_point_prices(&pau_copy(1_000_086_400, None))
            .expect("second");

        let copies = store.point_prices(0, i64::MAX).expect("read");

        assert_eq!(copies.len(), 2);
        assert_eq!(copies.first(), Some(&tritanium));
        assert_eq!(store.point_prices_size().expect("size"), (2, 3));
    }

    #[test]
    fn a_negative_price_writes_nothing_and_keeps_the_earlier_copies() {
        let (_dir, _path, mut store) = open_temp();
        let good = pau_copy(1_000_000_000, Some(999_999_000));
        store.save_point_prices(&good).expect("first");
        let mut bad = pau_copy(1_000_086_400, None);
        bad.rows.insert(
            (PAU, Side::Sell),
            SummaryRow {
                five_percent: Money::from_ten_thousandths(-1),
                ..pau_sell()
            },
        );

        let result = store.save_point_prices(&bad);
        assert!(matches!(result, Err(StoreError::Sqlite(_))), "{result:?}");
        let mut replacing = bad.clone();
        replacing.at = good.at;
        assert!(store.save_point_prices(&replacing).is_err());

        assert_eq!(store.point_prices(0, i64::MAX).expect("read"), vec![good]);
        assert_eq!(store.point_prices_size().expect("size"), (1, 2));
    }

    #[test]
    fn forgetting_a_market_keeps_every_copy() {
        let (_dir, _path, mut store) = open_temp();
        store
            .save_point_prices(&pau_copy(1_000_000_000, None))
            .expect("save");
        let forge = store
            .history_markets()
            .expect("markets")
            .into_iter()
            .find(|market| market.place == FORGE_PLACE)
            .expect("the Forge");

        store.forget_market(forge.id).expect("forget");

        assert_eq!(store.point_prices(0, i64::MAX).expect("read").len(), 1);
        assert_eq!(store.point_prices_size().expect("size"), (1, 2));
    }

    fn history_day(day: i64, average: i64, volume: u64) -> HistoryDay {
        HistoryDay {
            day,
            average: Money::from_ten_thousandths(average),
            highest: Money::from_ten_thousandths(average),
            lowest: Money::from_ten_thousandths(average),
            volume,
            orders: 1,
        }
    }

    #[test]
    fn saving_day_history_keeps_the_newest_30_days() {
        let (_dir, _path, mut store) = open_temp();
        let days = (20_455..=20_485)
            .map(|day| history_day(day, 100_000, 100))
            .collect::<Vec<_>>();

        assert_eq!(store.save_day_history(34, &days).expect("save"), 30);
        let saved = store.day_history(34).expect("days");
        assert_eq!(saved.len(), 30);
        assert_eq!(saved.first().map(|day| day.day), Some(20_456));
        assert_eq!(saved.last().map(|day| day.day), Some(20_485));
        assert_eq!(
            store.day_stat(34).expect("stat"),
            Some(DayStat {
                last_day: 20_485,
                units_day: 100,
                middle: Money::from_ten_thousandths(100_000),
                days: 30,
            })
        );
    }

    #[test]
    fn saving_day_history_replaces_the_type_and_keeps_one_row_a_day() {
        let (_dir, _path, mut store) = open_temp();
        let first = (20_455..=20_485)
            .map(|day| history_day(day, 100_000, 100))
            .collect::<Vec<_>>();
        store.save_day_history(34, &first).expect("first");

        let changed = [
            history_day(20_485, 200_000, 30),
            history_day(20_486, 400_000, 60),
        ];
        assert_eq!(store.save_day_history(34, &changed).expect("replace"), 2);
        let saved = store.day_history(34).expect("days");
        assert_eq!(saved.len(), 2);
        assert_eq!(saved[0].day, 20_485);
        assert_eq!(saved[0].average, Money::from_ten_thousandths(200_000));
        assert_eq!(
            store.day_stat(34).expect("stat"),
            Some(DayStat {
                last_day: 20_486,
                units_day: 3,
                middle: Money::from_ten_thousandths(300_000),
                days: 2,
            })
        );
    }

    #[test]
    fn an_empty_day_history_deletes_the_type() {
        let (_dir, _path, mut store) = open_temp();
        store
            .save_day_history(34, &[history_day(20_484, 100_000, 30)])
            .expect("save");

        assert_eq!(store.save_day_history(34, &[]).expect("clear"), 0);
        assert!(store.day_history(34).expect("days").is_empty());
        assert_eq!(store.day_stat(34).expect("stat"), None);
    }

    #[test]
    fn day_stats_holds_every_type() {
        let (_dir, _path, mut store) = open_temp();
        store
            .save_day_history(34, &[history_day(20_484, 100_000, 30)])
            .expect("34");
        store
            .save_day_history(35, &[history_day(20_484, 50_000, 30)])
            .expect("35");

        let stats = store.day_stats().expect("stats");
        assert_eq!(
            stats.get(&34),
            Some(&DayStat {
                last_day: 20_484,
                units_day: 1,
                middle: Money::from_ten_thousandths(100_000),
                days: 1,
            })
        );
        assert_eq!(
            stats.get(&35),
            Some(&DayStat {
                last_day: 20_484,
                units_day: 1,
                middle: Money::from_ten_thousandths(50_000),
                days: 1,
            })
        );
    }

    #[test]
    fn a_stored_type_that_does_not_fit_is_bad_data() {
        let (_dir, _path, mut store) = open_temp();
        store
            .save_day_history(34, &[history_day(20_484, 100_000, 30)])
            .expect("save");
        store
            .conn
            .execute(
                "UPDATE market_history SET type_id = ?1",
                params![i64::from(u32::MAX) + 1],
            )
            .expect("raw update");

        let err = store.day_stats().expect_err("bad data");
        let StoreError::BadData(what) = err else {
            panic!("expected BadData, got {err:?}");
        };
        assert!(what.contains("market_history"), "{what}");
        assert!(what.contains("type_id"), "{what}");
    }

    #[test]
    fn a_stored_value_of_the_wrong_kind_is_bad_data_naming_the_column() {
        let (_dir, _path, mut store) = open_temp();
        store
            .save_day_history(34, &[history_day(20_484, 100_000, 30)])
            .expect("save");
        store
            .conn
            .execute("UPDATE market_history SET average = 'x'", [])
            .expect("raw update");

        let err = store.day_history(34).expect_err("bad data");
        let StoreError::BadData(what) = err else {
            panic!("expected BadData, got {err:?}");
        };
        assert!(what.contains("market_history"), "{what}");
        assert!(what.contains("average"), "{what}");
    }

    struct DayHistoryFake {
        bodies: BTreeMap<u32, String>,
        calls: Rc<RefCell<Vec<String>>>,
    }

    impl DayHistoryFake {
        fn new(bodies: BTreeMap<u32, String>) -> (Self, Rc<RefCell<Vec<String>>>) {
            let calls = Rc::new(RefCell::new(Vec::new()));
            (
                Self {
                    bodies,
                    calls: Rc::clone(&calls),
                },
                calls,
            )
        }
    }

    impl BytesHttp for DayHistoryFake {
        fn get_bytes(
            &self,
            url: &str,
            _if_none_match: Option<&str>,
        ) -> std::result::Result<BytesResponse, MarketError> {
            self.calls.borrow_mut().push(url.to_owned());
            let type_id = url
                .rsplit_once("type_id=")
                .and_then(|(_, text)| text.parse::<u32>().ok());
            let body = type_id
                .and_then(|type_id| self.bodies.get(&type_id))
                .cloned()
                .unwrap_or_default();
            Ok(BytesResponse {
                status: 200,
                etag: None,
                last_modified: None,
                retry_after: None,
                body: body.into_bytes(),
            })
        }
    }

    fn history_json(average: &str, volume: u64) -> String {
        format!(
            r#"[{{"average":{average},"date":"2026-01-01","highest":{average},"lowest":{average},"order_count":1,"volume":{volume}}}]"#
        )
    }

    #[test]
    fn the_refresh_loop_fetches_and_saves_every_type() {
        let mut bodies = BTreeMap::new();
        bodies.insert(34, history_json("10.0", 300));
        bodies.insert(35, history_json("5.0", 150));
        let (fake, calls) = DayHistoryFake::new(bodies);
        let history = History::forge(fake);

        let (_dir, _path, mut store) = open_temp();
        for type_id in [34, 35] {
            let days = history.fetch(type_id).expect("fetch");
            store.save_day_history(type_id, &days).expect("save");
        }
        assert_eq!(store.day_stats().expect("stats").len(), 2);

        // A second fetch of 34 still has the one day; no duplicate row.
        let days = history.fetch(34).expect("refetch");
        store.save_day_history(34, &days).expect("replace");
        assert_eq!(store.day_history(34).expect("days").len(), 1);
        assert_eq!(calls.borrow().len(), 3);
    }
}

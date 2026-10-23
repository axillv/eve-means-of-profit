//! The price history: one reading a day for each market the user chose, kept as long as the
//! market's retention allows, and the room the history takes.

use std::cmp::Reverse;
use std::collections::VecDeque;

use mop_core::industry::TypeId;
use mop_core::money::Money;
use mop_core::price_history::{ROW_BYTES, Retention, RetentionError, days_to_drop};
use mop_market::fuzzwork::{Side, SummaryRow};
use rusqlite::{Connection, OptionalExtension, params};

use crate::error::{Result, StoreError};
use crate::migrate::Store;
use crate::{bad_code, from_stored, to_stored_u64};

const MARKETS: &str = "history_markets";
const READINGS: &str = "price_history";
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
}

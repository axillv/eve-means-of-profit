//! Freight routes between two systems, and the services (rate cards) that carry along them.

use mop_core::freight::RateCard;
use mop_core::money::{Money, Rate};
use mop_core::volume::Volume;
use mop_sde::RouteKind;
use rusqlite::{OptionalExtension, Row, params};

use crate::error::{Result, StoreError};
use crate::migrate::Store;
use crate::plans::plans_using;
use crate::{bad_code, clean_name, ensure_free, from_stored, id_names, missing_reference};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteRow {
    pub id: i64,
    pub from_system: u32,
    pub to_system: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceRow {
    pub id: i64,
    pub route_id: i64,
    pub name: String,
    pub card: RateCard,
    pub route_kind: RouteKind,
    pub jumps_override: Option<u32>,
}

const SERVICE_COLUMNS: &str = "id, route_id, name, per_m3, per_jump, collateral_ppm, minimum, \
    max_m3, max_collateral, route_kind, jumps_override";

fn route_kind_code(kind: RouteKind) -> i64 {
    match kind {
        RouteKind::Shortest => 0,
        RouteKind::HighSecOnly => 1,
    }
}

fn route_kind_from_code(code: i64) -> Option<RouteKind> {
    match code {
        0 => Some(RouteKind::Shortest),
        1 => Some(RouteKind::HighSecOnly),
        _ => None,
    }
}

/// The stored values of one service row, before they are turned back into Rust types.
struct RawService {
    id: i64,
    route_id: i64,
    name: String,
    money: [Option<i64>; 4],
    collateral_ppm: Option<i64>,
    max_m3: Option<i64>,
    route_kind: i64,
    jumps_override: Option<i64>,
}

fn raw_service(row: &Row<'_>) -> rusqlite::Result<RawService> {
    Ok(RawService {
        id: row.get(0)?,
        route_id: row.get(1)?,
        name: row.get(2)?,
        money: [row.get(3)?, row.get(4)?, row.get(6)?, row.get(8)?],
        collateral_ppm: row.get(5)?,
        max_m3: row.get(7)?,
        route_kind: row.get(9)?,
        jumps_override: row.get(10)?,
    })
}

fn service_from_raw(raw: RawService) -> Result<ServiceRow> {
    const TABLE: &str = "services";
    let id = raw.id;
    let [per_m3, per_jump, minimum, max_collateral] = raw.money;
    let card = RateCard {
        per_m3: per_m3.map(Money::from_ten_thousandths),
        per_jump: per_jump.map(Money::from_ten_thousandths),
        collateral: raw
            .collateral_ppm
            .map(|ppm| from_stored(ppm, TABLE, id, "collateral_ppm").map(Rate::from_ppm))
            .transpose()?,
        minimum: minimum.map(Money::from_ten_thousandths),
        max_m3: raw.max_m3.map(Volume::from_ten_thousandths),
        max_collateral: max_collateral.map(Money::from_ten_thousandths),
    };
    let route_kind = route_kind_from_code(raw.route_kind)
        .ok_or_else(|| bad_code(TABLE, id, "route_kind", raw.route_kind))?;
    let jumps_override = raw
        .jumps_override
        .map(|jumps| from_stored(jumps, TABLE, id, "jumps_override"))
        .transpose()?;
    Ok(ServiceRow {
        id,
        route_id: raw.route_id,
        name: raw.name,
        card,
        route_kind,
        jumps_override,
    })
}

/// Stores a rate card's parts as they are kept in the table: money and volume as ten-thousandths,
/// the collateral share as ppm, and an absent part as NULL.
fn card_values(card: &RateCard) -> [Option<i64>; 6] {
    [
        card.per_m3.map(Money::ten_thousandths),
        card.per_jump.map(Money::ten_thousandths),
        card.collateral.map(|rate| i64::from(rate.ppm())),
        card.minimum.map(Money::ten_thousandths),
        card.max_m3.map(Volume::ten_thousandths),
        card.max_collateral.map(Money::ten_thousandths),
    ]
}

impl Store {
    pub fn add_route(&mut self, from: u32, to: u32) -> Result<i64> {
        let tx = self.conn.transaction()?;
        let exists: Option<i64> = tx
            .query_row(
                "SELECT id FROM routes WHERE from_system = ?1 AND to_system = ?2",
                params![i64::from(from), i64::from(to)],
                |row| row.get(0),
            )
            .optional()?;
        if exists.is_some() {
            return Err(StoreError::RouteExists);
        }
        tx.execute(
            "INSERT INTO routes (from_system, to_system) VALUES (?1, ?2)",
            params![i64::from(from), i64::from(to)],
        )?;
        let id = tx.last_insert_rowid();
        tx.commit()?;
        Ok(id)
    }

    /// All routes, by id.
    pub fn routes(&mut self) -> Result<Vec<RouteRow>> {
        let tx = self.conn.transaction()?;
        let raw = {
            let mut stmt =
                tx.prepare("SELECT id, from_system, to_system FROM routes ORDER BY id")?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        let routes = raw
            .into_iter()
            .map(|(id, from, to)| {
                Ok(RouteRow {
                    id,
                    from_system: from_stored(from, "routes", id, "from_system")?,
                    to_system: from_stored(to, "routes", id, "to_system")?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        tx.commit()?;
        Ok(routes)
    }

    /// Deletes a route and its services. Refused while a plan uses the route or one of its services.
    pub fn delete_route(&mut self, id: i64) -> Result<()> {
        let tx = self.conn.transaction()?;
        let users = plans_using(
            &tx,
            "route = ?1 OR service IN (SELECT id FROM services WHERE route_id = ?1)",
            id,
        )?;
        if !users.is_empty() {
            return Err(StoreError::InUse { plans: users });
        }
        let changed = tx.execute("DELETE FROM routes WHERE id = ?1", params![id])?;
        if changed == 0 {
            return Err(StoreError::NotFound);
        }
        tx.commit()?;
        Ok(())
    }

    pub fn add_service(
        &mut self,
        route_id: i64,
        name: &str,
        card: &RateCard,
        kind: RouteKind,
        jumps_override: Option<u32>,
    ) -> Result<i64> {
        let tx = self.conn.transaction()?;
        let name = clean_name(name)?;
        let taken = id_names(
            &tx,
            "SELECT id, name FROM services WHERE route_id = ?1",
            params![route_id],
        )?;
        ensure_free(&taken, &name, None)?;
        let [
            per_m3,
            per_jump,
            collateral,
            minimum,
            max_m3,
            max_collateral,
        ] = card_values(card);
        tx.execute(
            "INSERT INTO services (route_id, name, per_m3, per_jump, collateral_ppm, minimum, \
                max_m3, max_collateral, route_kind, jumps_override) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                route_id,
                name,
                per_m3,
                per_jump,
                collateral,
                minimum,
                max_m3,
                max_collateral,
                route_kind_code(kind),
                jumps_override.map(i64::from)
            ],
        )
        .map_err(missing_reference)?;
        let id = tx.last_insert_rowid();
        tx.commit()?;
        Ok(id)
    }

    pub fn update_service(
        &mut self,
        id: i64,
        name: &str,
        card: &RateCard,
        kind: RouteKind,
        jumps_override: Option<u32>,
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        let name = clean_name(name)?;
        let route_id: Option<i64> = tx
            .query_row(
                "SELECT route_id FROM services WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .optional()?;
        let Some(route_id) = route_id else {
            return Err(StoreError::NotFound);
        };
        let taken = id_names(
            &tx,
            "SELECT id, name FROM services WHERE route_id = ?1",
            params![route_id],
        )?;
        ensure_free(&taken, &name, Some(id))?;
        let [
            per_m3,
            per_jump,
            collateral,
            minimum,
            max_m3,
            max_collateral,
        ] = card_values(card);
        tx.execute(
            "UPDATE services SET name = ?2, per_m3 = ?3, per_jump = ?4, collateral_ppm = ?5, \
                minimum = ?6, max_m3 = ?7, max_collateral = ?8, route_kind = ?9, jumps_override = ?10 \
             WHERE id = ?1",
            params![
                id,
                name,
                per_m3,
                per_jump,
                collateral,
                minimum,
                max_m3,
                max_collateral,
                route_kind_code(kind),
                jumps_override.map(i64::from)
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Deletes a service. A plan that picked it goes back to "cheapest".
    pub fn delete_service(&mut self, id: i64) -> Result<()> {
        let tx = self.conn.transaction()?;
        let changed = tx.execute("DELETE FROM services WHERE id = ?1", params![id])?;
        if changed == 0 {
            return Err(StoreError::NotFound);
        }
        tx.commit()?;
        Ok(())
    }

    /// The services of one route, ordered by name ignoring case, then id.
    pub fn services(&mut self, route_id: i64) -> Result<Vec<ServiceRow>> {
        let tx = self.conn.transaction()?;
        let raw = {
            let mut stmt = tx.prepare(&format!(
                "SELECT {SERVICE_COLUMNS} FROM services WHERE route_id = ?1"
            ))?;
            let rows = stmt.query_map(params![route_id], raw_service)?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        let mut services = raw
            .into_iter()
            .map(service_from_raw)
            .collect::<Result<Vec<_>>>()?;
        services.sort_by_cached_key(|row| (row.name.to_lowercase(), row.id));
        tx.commit()?;
        Ok(services)
    }
}

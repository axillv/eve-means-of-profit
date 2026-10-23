//! Named facilities: where a job is built, with its structure, security and rigs.

use mop_core::industry::TypeId;
use mop_core::job_cost::NPC_STATION_TAX;
use mop_sde::{Facility, Security};
use rusqlite::{Transaction, params};

use crate::error::{Result, StoreError};
use crate::migrate::Store;
use crate::plans::plans_using;
use crate::{bad_code, clean_name, ensure_free, from_stored, id_names};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FacilityRow {
    pub id: i64,
    pub name: String,
    pub facility: Facility,
}

const TABLE: &str = "facilities";

fn security_code(security: Security) -> i64 {
    match security {
        Security::High => 0,
        Security::Low => 1,
        Security::Null => 2,
    }
}

fn security_from_code(code: i64) -> Option<Security> {
    match code {
        0 => Some(Security::High),
        1 => Some(Security::Low),
        2 => Some(Security::Null),
        _ => None,
    }
}

fn write_rigs(tx: &Transaction<'_>, id: i64, rigs: &[TypeId]) -> Result<()> {
    tx.execute(
        "DELETE FROM facility_rigs WHERE facility_id = ?1",
        params![id],
    )?;
    for (position, type_id) in (0_i64..).zip(rigs) {
        tx.execute(
            "INSERT INTO facility_rigs (facility_id, position, type_id) VALUES (?1, ?2, ?3)",
            params![id, position, i64::from(*type_id)],
        )?;
    }
    Ok(())
}

fn read_rigs(tx: &Transaction<'_>, id: i64) -> Result<Vec<TypeId>> {
    let mut stmt =
        tx.prepare("SELECT type_id FROM facility_rigs WHERE facility_id = ?1 ORDER BY position")?;
    let raw = stmt
        .query_map(params![id], |row| row.get::<_, i64>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    raw.into_iter()
        .map(|type_id| from_stored(type_id, TABLE, id, "type_id"))
        .collect()
}

impl Store {
    pub fn add_facility(&mut self, name: &str, facility: &Facility) -> Result<i64> {
        let tx = self.conn.transaction()?;
        let name = clean_name(name)?;
        let taken = id_names(&tx, "SELECT id, name FROM facilities", [])?;
        ensure_free(&taken, &name, None)?;
        // An NPC station has the NPC tax; a structure's tax is unknown until the user types it.
        let tax = facility
            .structure
            .is_none()
            .then(|| i64::from(NPC_STATION_TAX.ppm()));
        tx.execute(
            "INSERT INTO facilities (name, structure, security, tax_ppm) VALUES (?1, ?2, ?3, ?4)",
            params![
                name,
                facility.structure.map(i64::from),
                security_code(facility.security),
                tax
            ],
        )?;
        let id = tx.last_insert_rowid();
        write_rigs(&tx, id, &facility.rigs)?;
        tx.commit()?;
        Ok(id)
    }

    pub fn update_facility(&mut self, id: i64, name: &str, facility: &Facility) -> Result<()> {
        let tx = self.conn.transaction()?;
        let name = clean_name(name)?;
        let taken = id_names(&tx, "SELECT id, name FROM facilities", [])?;
        ensure_free(&taken, &name, Some(id))?;
        let changed = tx.execute(
            "UPDATE facilities SET name = ?2, structure = ?3, security = ?4 WHERE id = ?1",
            params![
                id,
                name,
                facility.structure.map(i64::from),
                security_code(facility.security)
            ],
        )?;
        if changed == 0 {
            return Err(StoreError::NotFound);
        }
        write_rigs(&tx, id, &facility.rigs)?;
        tx.commit()?;
        Ok(())
    }

    pub fn delete_facility(&mut self, id: i64) -> Result<()> {
        let tx = self.conn.transaction()?;
        let users = plans_using(&tx, "manufacturing = ?1 OR reactions = ?1", id)?;
        if !users.is_empty() {
            return Err(StoreError::InUse { plans: users });
        }
        let changed = tx.execute("DELETE FROM facilities WHERE id = ?1", params![id])?;
        if changed == 0 {
            return Err(StoreError::NotFound);
        }
        tx.commit()?;
        Ok(())
    }

    /// All facilities, ordered by name ignoring case, then id.
    pub fn facilities(&mut self) -> Result<Vec<FacilityRow>> {
        let tx = self.conn.transaction()?;
        let raw = {
            let mut stmt = tx.prepare("SELECT id, name, structure, security FROM facilities")?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        let mut out = Vec::with_capacity(raw.len());
        for (id, name, structure, security) in raw {
            let security = security_from_code(security)
                .ok_or_else(|| bad_code(TABLE, id, "security", security))?;
            let structure = structure
                .map(|value| from_stored(value, TABLE, id, "structure"))
                .transpose()?;
            let rigs = read_rigs(&tx, id)?;
            out.push(FacilityRow {
                id,
                name,
                facility: Facility {
                    structure,
                    security,
                    rigs,
                },
            });
        }
        out.sort_by_cached_key(|row| (row.name.to_lowercase(), row.id));
        tx.commit()?;
        Ok(out)
    }
}

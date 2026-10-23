//! What job costs and fees need from the user's own setup: each facility's solar system and tax,
//! and the fee skills, standings and broker fee the user typed.

use std::collections::BTreeMap;

use mop_core::fees::Standing;
use mop_core::money::Rate;
use rusqlite::{OptionalExtension, Transaction, params};

use crate::error::{Result, StoreError};
use crate::from_stored;
use crate::migrate::Store;

/// A facility tax or broker fee can be at most 100%.
const MAX_RATE_PPM: u32 = 1_000_000;
/// The highest Accounting and Broker Relations level in the game.
const MAX_SKILL_LEVEL: u8 = 5;

const FACILITIES: &str = "facilities";
const FEE_SETTINGS: &str = "fee_settings";

/// Where a facility is and what it charges. Either can be unknown: the system until the user
/// picks it, the tax of a structure until the user types it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FacilityCosts {
    pub system: Option<u32>,
    pub tax: Option<Rate>,
}

/// The user's fee skills and standings, and a broker fee typed by hand. With no override, the
/// broker fee is worked out from the standings and skills.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FeeSettings {
    pub accounting: u8,
    pub broker_relations: u8,
    pub faction_standing: Standing,
    pub corporation_standing: Standing,
    pub broker_override: Option<Rate>,
}

fn invalid(reason: &str) -> StoreError {
    StoreError::InvalidCosts(reason.to_owned())
}

fn rate_from_stored(value: i64, table: &str, row: i64, column: &str) -> Result<Rate> {
    let ppm: u32 = from_stored(value, table, row, column)?;
    Ok(Rate::from_ppm(ppm))
}

fn facility_costs_from(id: i64, system: Option<i64>, tax: Option<i64>) -> Result<FacilityCosts> {
    Ok(FacilityCosts {
        system: system
            .map(|value| from_stored(value, FACILITIES, id, "system"))
            .transpose()?,
        tax: tax
            .map(|value| rate_from_stored(value, FACILITIES, id, "tax_ppm"))
            .transpose()?,
    })
}

fn read_facility_costs(tx: &Transaction<'_>, id: i64) -> Result<Option<FacilityCosts>> {
    let raw = tx
        .query_row(
            "SELECT system, tax_ppm FROM facilities WHERE id = ?1",
            params![id],
            |row| Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, Option<i64>>(1)?)),
        )
        .optional()?;
    raw.map(|(system, tax)| facility_costs_from(id, system, tax))
        .transpose()
}

fn standing_from_stored(value: i64, column: &str) -> Result<Standing> {
    let hundredths: i16 = from_stored(value, FEE_SETTINGS, 1, column)?;
    Standing::from_hundredths(hundredths).ok().ok_or_else(|| {
        StoreError::BadData(format!(
            "{FEE_SETTINGS} row 1, column {column}: {value} is out of range"
        ))
    })
}

impl Store {
    pub fn facility_costs(&mut self, id: i64) -> Result<FacilityCosts> {
        let tx = self.conn.transaction()?;
        let costs = read_facility_costs(&tx, id)?.ok_or(StoreError::NotFound)?;
        tx.commit()?;
        Ok(costs)
    }

    /// Every facility's costs, by facility id.
    pub fn all_facility_costs(&mut self) -> Result<BTreeMap<i64, FacilityCosts>> {
        let tx = self.conn.transaction()?;
        let raw = {
            let mut stmt = tx.prepare("SELECT id, system, tax_ppm FROM facilities")?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                ))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        let mut out = BTreeMap::new();
        for (id, system, tax) in raw {
            out.insert(id, facility_costs_from(id, system, tax)?);
        }
        tx.commit()?;
        Ok(out)
    }

    /// Saves a facility's solar system and tax. Nothing is written when a value is refused.
    pub fn set_facility_costs(&mut self, id: i64, costs: &FacilityCosts) -> Result<()> {
        if costs.system == Some(0) {
            return Err(invalid("pick a solar system"));
        }
        let tax = costs.tax.map(Rate::ppm);
        if tax.is_some_and(|ppm| ppm > MAX_RATE_PPM) {
            return Err(invalid("tax must be from 0% to 100%"));
        }
        let tx = self.conn.transaction()?;
        let changed = tx.execute(
            "UPDATE facilities SET system = ?2, tax_ppm = ?3 WHERE id = ?1",
            params![id, costs.system.map(i64::from), tax.map(i64::from)],
        )?;
        if changed == 0 {
            return Err(StoreError::NotFound);
        }
        tx.commit()?;
        Ok(())
    }

    pub fn fee_settings(&mut self) -> Result<FeeSettings> {
        let tx = self.conn.transaction()?;
        let raw = tx
            .query_row(
                "SELECT accounting, broker_relations, faction_standing, corporation_standing, \
                    broker_override_ppm FROM fee_settings WHERE id = 1",
                [],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, Option<i64>>(4)?,
                    ))
                },
            )
            .optional()?;
        let settings = match raw {
            None => FeeSettings::default(),
            Some((accounting, broker_relations, faction, corporation, broker_override)) => {
                FeeSettings {
                    accounting: from_stored(accounting, FEE_SETTINGS, 1, "accounting")?,
                    broker_relations: from_stored(
                        broker_relations,
                        FEE_SETTINGS,
                        1,
                        "broker_relations",
                    )?,
                    faction_standing: standing_from_stored(faction, "faction_standing")?,
                    corporation_standing: standing_from_stored(
                        corporation,
                        "corporation_standing",
                    )?,
                    broker_override: broker_override
                        .map(|value| {
                            rate_from_stored(value, FEE_SETTINGS, 1, "broker_override_ppm")
                        })
                        .transpose()?,
                }
            }
        };
        tx.commit()?;
        Ok(settings)
    }

    /// Saves the fee settings. Nothing is written when a value is refused.
    pub fn save_fee_settings(&mut self, settings: &FeeSettings) -> Result<()> {
        if settings.accounting > MAX_SKILL_LEVEL || settings.broker_relations > MAX_SKILL_LEVEL {
            return Err(invalid("skill levels are 0 to 5"));
        }
        if settings
            .broker_override
            .is_some_and(|rate| rate.ppm() > MAX_RATE_PPM)
        {
            return Err(invalid("broker fee must be from 0% to 100%"));
        }
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO fee_settings (id, accounting, broker_relations, faction_standing, \
                corporation_standing, broker_override_ppm) VALUES (1, ?1, ?2, ?3, ?4, ?5) \
             ON CONFLICT (id) DO UPDATE SET accounting = excluded.accounting, \
                broker_relations = excluded.broker_relations, \
                faction_standing = excluded.faction_standing, \
                corporation_standing = excluded.corporation_standing, \
                broker_override_ppm = excluded.broker_override_ppm",
            params![
                i64::from(settings.accounting),
                i64::from(settings.broker_relations),
                i64::from(settings.faction_standing.hundredths()),
                i64::from(settings.corporation_standing.hundredths()),
                settings.broker_override.map(|rate| i64::from(rate.ppm()))
            ],
        )?;
        tx.commit()?;
        Ok(())
    }
}

//! The invention activities of the blueprints: what an attempt makes, uses up and needs.

use profit_core::industry::TypeId;
use profit_core::money::Rate;
use rusqlite::{Row, params};

use crate::error::SdeError;
use crate::query::Sde;

/// The invention activity of one blueprint, as the game data gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventionActivity {
    pub blueprint: TypeId,
    /// Every product, in the order the game data lists them.
    pub products: Vec<InventionProduct>,
    /// The materials an attempt uses up, as type id and quantity, in file order.
    pub materials: Vec<(TypeId, u64)>,
    /// The skills an attempt needs, in file order.
    pub skills: Vec<SkillRequirement>,
}

/// One product of an invention attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventionProduct {
    pub type_id: TypeId,
    /// The chance that an attempt succeeds, before any decryptor.
    pub probability: Rate,
    /// The runs a successful attempt adds to the copy.
    pub quantity: u32,
}

/// A skill an invention attempt needs, at a level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillRequirement {
    pub type_id: TypeId,
    /// The skill's English name.
    pub name: String,
    pub level: u8,
}

impl Sde {
    /// The invention activity of this blueprint, or `None` when it has none.
    pub fn invention(&self, blueprint: TypeId) -> Result<Option<InventionActivity>, SdeError> {
        let mut statement = self.conn.prepare(
            "SELECT product_id, probability_ppm, quantity FROM invention_products
             WHERE blueprint_id = ?1 ORDER BY position",
        )?;
        let products = statement
            .query_map(params![blueprint], |row| {
                Ok(InventionProduct {
                    type_id: row.get(0)?,
                    probability: Rate::from_ppm(row.get(1)?),
                    quantity: row.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if products.is_empty() {
            return Ok(None);
        }
        let mut statement = self.conn.prepare(
            "SELECT type_id, quantity FROM invention_materials
             WHERE blueprint_id = ?1 ORDER BY position",
        )?;
        let materials = statement
            .query_map(params![blueprint], |row| {
                let stored: i64 = row.get(1)?;
                // The import writes only positive quantities, so a negative one is a corrupt database.
                let Ok(quantity) = u64::try_from(stored) else {
                    return Err(rusqlite::Error::IntegralValueOutOfRange(1, stored));
                };
                Ok((row.get(0)?, quantity))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        // The names come from the types table, which holds every skill the import accepted.
        let mut statement = self.conn.prepare(
            "SELECT s.type_id, t.name, s.level FROM invention_skills s
             JOIN types t ON t.type_id = s.type_id
             WHERE s.blueprint_id = ?1 ORDER BY s.position",
        )?;
        let skills = statement
            .query_map(params![blueprint], skill_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(Some(InventionActivity {
            blueprint,
            products,
            materials,
            skills,
        }))
    }
}

fn skill_from_row(row: &Row<'_>) -> rusqlite::Result<SkillRequirement> {
    Ok(SkillRequirement {
        type_id: row.get(0)?,
        name: row.get(1)?,
        level: row.get(2)?,
    })
}

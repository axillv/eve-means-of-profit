//! The invention activities of the blueprints: what an attempt makes, uses up and needs.

use mop_core::industry::TypeId;
use mop_core::money::Rate;
use rusqlite::{OptionalExtension, Row, params};

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

/// A skill that some invention attempt needs, listed once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventionSkill {
    pub type_id: TypeId,
    /// The skill's English name.
    pub name: String,
    /// True for the encryption-methods skills, which pick the decryptor's effect.
    pub encryption: bool,
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

    /// The invention that makes the blueprint of `product`: its activity, and the product line that
    /// is that blueprint. `None` when the product has no recipe or no invention makes its blueprint.
    /// When several blueprints invent it, the one with the smallest id is used.
    pub fn invention_for(
        &self,
        product: TypeId,
    ) -> Result<Option<(InventionActivity, InventionProduct)>, SdeError> {
        let blueprint: Option<TypeId> = self
            .conn
            .query_row(
                "SELECT blueprint_id FROM recipes WHERE product_id = ?1",
                params![product],
                |row| row.get(0),
            )
            .optional()?;
        let Some(blueprint) = blueprint else {
            return Ok(None);
        };
        let source: Option<TypeId> = self
            .conn
            .query_row(
                "SELECT blueprint_id FROM invention_products WHERE product_id = ?1
                 ORDER BY blueprint_id LIMIT 1",
                params![blueprint],
                |row| row.get(0),
            )
            .optional()?;
        let Some(source) = source else {
            return Ok(None);
        };
        let Some(activity) = self.invention(source)? else {
            return Ok(None);
        };
        let line = activity
            .products
            .iter()
            .find(|line| line.type_id == blueprint)
            .cloned();
        Ok(line.map(|line| (activity, line)))
    }

    /// Every skill that some invention attempt needs, once each, sorted by name ignoring case, then id.
    pub fn invention_skills(&self) -> Result<Vec<InventionSkill>, SdeError> {
        let mut statement = self.conn.prepare(
            "SELECT DISTINCT s.type_id, t.name FROM invention_skills s
             JOIN types t ON t.type_id = s.type_id",
        )?;
        let mut skills = statement
            .query_map([], |row| {
                let name: String = row.get(1)?;
                let encryption = is_encryption(&name);
                Ok(InventionSkill {
                    type_id: row.get(0)?,
                    name,
                    encryption,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        skills.sort_by(|a, b| {
            a.name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then(a.type_id.cmp(&b.type_id))
        });
        Ok(skills)
    }
}

/// The two science skills of an activity and its encryption skill, as (science 1, science 2,
/// encryption), in the activity's skill order. `None` unless the activity has exactly three skills,
/// one of them encryption.
pub fn split_skills(activity: &InventionActivity) -> Option<(TypeId, TypeId, TypeId)> {
    let [first, second, third] = activity.skills.as_slice() else {
        return None;
    };
    match (
        is_encryption(&first.name),
        is_encryption(&second.name),
        is_encryption(&third.name),
    ) {
        (false, false, true) => Some((first.type_id, second.type_id, third.type_id)),
        (false, true, false) => Some((first.type_id, third.type_id, second.type_id)),
        (true, false, false) => Some((second.type_id, third.type_id, first.type_id)),
        _ => None,
    }
}

fn is_encryption(name: &str) -> bool {
    name.ends_with(" Encryption Methods")
}

fn skill_from_row(row: &Row<'_>) -> rusqlite::Result<SkillRequirement> {
    Ok(SkillRequirement {
        type_id: row.get(0)?,
        name: row.get(1)?,
        level: row.get(2)?,
    })
}

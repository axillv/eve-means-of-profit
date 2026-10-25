//! The invention choices of the user. Per plan and T2 part: whether the part is invented and with
//! which decryptor. App-wide: the invention skill levels and how many attempts to buy.

use std::collections::BTreeMap;

use mop_core::industry::TypeId;
use mop_core::invention::AttemptRule;
use mop_core::money::Rate;
use rusqlite::{Transaction, params};

use crate::error::{Result, StoreError};
use crate::from_stored;
use crate::migrate::Store;
use crate::settings::settings_with_prefix;

/// Whether one T2 part is invented, and the decryptor it uses when it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InventionChoice {
    pub invent: bool,
    pub decryptor: Option<TypeId>,
}

impl Default for InventionChoice {
    /// Invented, with no decryptor: the choice that is not stored.
    fn default() -> Self {
        Self {
            invent: true,
            decryptor: None,
        }
    }
}

const SKILL_PREFIX: &str = "invention.skill.";
const KEY_ATTEMPTS: &str = "invention.attempts";
const MAX_SKILL: u8 = 5;

fn skill_key(type_id: TypeId) -> String {
    format!("{SKILL_PREFIX}{type_id}")
}

fn bad_setting(key: &str, value: &str) -> StoreError {
    StoreError::BadData(format!(
        "setting {key} has the value {value:?}, which is not valid"
    ))
}

/// Reads one stored choice row, refusing a flag or id that no choice has.
fn choice_from_row(
    plan_id: i64,
    type_id: i64,
    invent: i64,
    decryptor: Option<i64>,
) -> Result<(TypeId, InventionChoice)> {
    let part = from_stored::<TypeId>(type_id, "plan_invention", plan_id, "type_id")?;
    let invent = match invent {
        0 => false,
        1 => true,
        other => {
            return Err(StoreError::BadData(format!(
                "plan_invention row {plan_id}, column invent: unknown code {other}"
            )));
        }
    };
    let decryptor = decryptor
        .map(|id| from_stored::<TypeId>(id, "plan_invention", plan_id, "decryptor"))
        .transpose()?;
    Ok((part, InventionChoice { invent, decryptor }))
}

/// Copies the invention rows of a plan to its duplicate.
pub(crate) fn copy_invention(tx: &Transaction<'_>, plan_id: i64, new_id: i64) -> Result<()> {
    tx.execute(
        "INSERT INTO plan_invention (plan_id, type_id, invent, decryptor) \
         SELECT ?2, type_id, invent, decryptor FROM plan_invention WHERE plan_id = ?1",
        params![plan_id, new_id],
    )?;
    Ok(())
}

/// The attempts rule for a stored value, or `None` when the value is not one of them.
fn rule_from_text(text: &str) -> Option<AttemptRule> {
    let confidence = |ppm| Some(AttemptRule::Confidence(Rate::from_ppm(ppm)));
    match text {
        "expected" => Some(AttemptRule::Expected),
        "90" => confidence(900_000),
        "95" => confidence(950_000),
        "99" => confidence(990_000),
        _ => None,
    }
}

impl Store {
    /// The stored invention choices of a plan. A part with no row is invented with no decryptor.
    pub fn plan_invention(&mut self, plan_id: i64) -> Result<BTreeMap<TypeId, InventionChoice>> {
        let mut stmt = self
            .conn
            .prepare("SELECT type_id, invent, decryptor FROM plan_invention WHERE plan_id = ?1")?;
        let rows = stmt.query_map(params![plan_id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Option<i64>>(2)?,
            ))
        })?;
        let mut choices = BTreeMap::new();
        for row in rows {
            let (type_id, invent, decryptor) = row?;
            let (part, choice) = choice_from_row(plan_id, type_id, invent, decryptor)?;
            choices.insert(part, choice);
        }
        Ok(choices)
    }

    /// Saves the choice for one T2 part of a plan. The default choice removes the stored row.
    /// Nothing is written when the plan is unknown.
    pub fn set_plan_invention(
        &mut self,
        plan_id: i64,
        type_id: TypeId,
        choice: InventionChoice,
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        let plans: i64 = tx.query_row(
            "SELECT count(*) FROM plans WHERE id = ?1",
            params![plan_id],
            |row| row.get(0),
        )?;
        if plans == 0 {
            return Err(StoreError::NotFound);
        }
        if choice == InventionChoice::default() {
            tx.execute(
                "DELETE FROM plan_invention WHERE plan_id = ?1 AND type_id = ?2",
                params![plan_id, type_id],
            )?;
        } else {
            tx.execute(
                "INSERT INTO plan_invention (plan_id, type_id, invent, decryptor) \
                 VALUES (?1, ?2, ?3, ?4) \
                 ON CONFLICT (plan_id, type_id) DO UPDATE SET \
                 invent = excluded.invent, decryptor = excluded.decryptor",
                params![plan_id, type_id, choice.invent, choice.decryptor],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// The invention skill levels that were set, by T2 product id. A level of 0 that was typed
    /// is listed; a skill that was never set is not.
    pub fn invention_skills(&self) -> Result<BTreeMap<TypeId, u8>> {
        let rows = settings_with_prefix(&self.conn, SKILL_PREFIX)?;
        let mut skills = BTreeMap::new();
        for (key, value) in rows {
            let id_text = key.strip_prefix(SKILL_PREFIX).unwrap_or_default();
            let digits = !id_text.is_empty() && id_text.bytes().all(|b| b.is_ascii_digit());
            let type_id: TypeId = match digits.then(|| id_text.parse().ok()).flatten() {
                Some(type_id) => type_id,
                None => return Err(bad_setting(&key, id_text)),
            };
            let single_digit = value.len() == 1 && value.bytes().all(|b| b.is_ascii_digit());
            let level = match value.parse::<u8>() {
                Ok(level) if single_digit && level <= MAX_SKILL => level,
                _ => return Err(bad_setting(&key, &value)),
            };
            skills.insert(type_id, level);
        }
        Ok(skills)
    }

    /// Saves the invention skill level of a T2 product. Levels above 5 are refused.
    pub fn set_invention_skill(&mut self, type_id: TypeId, level: u8) -> Result<()> {
        if level > MAX_SKILL {
            return Err(StoreError::InvalidCosts(
                "skill levels are 0 to 5".to_owned(),
            ));
        }
        self.set_setting(&skill_key(type_id), &level.to_string())
    }

    /// Removes the saved skill level of a T2 product, so it counts as 0 again.
    pub fn clear_invention_skill(&mut self, type_id: TypeId) -> Result<()> {
        self.clear_setting(&skill_key(type_id))
    }

    /// How many attempts to buy. Absent means `Expected`.
    pub fn attempt_rule(&self) -> Result<AttemptRule> {
        match self.setting(KEY_ATTEMPTS)? {
            None => Ok(AttemptRule::Expected),
            Some(text) => rule_from_text(&text).ok_or_else(|| bad_setting(KEY_ATTEMPTS, &text)),
        }
    }

    /// Saves how many attempts to buy. A confidence other than 90%, 95% or 99% is refused.
    pub fn set_attempt_rule(&mut self, rule: AttemptRule) -> Result<()> {
        let text = match rule {
            AttemptRule::Expected => "expected",
            AttemptRule::Confidence(level) => match level.ppm() {
                900_000 => "90",
                950_000 => "95",
                990_000 => "99",
                _ => {
                    return Err(StoreError::InvalidCosts(
                        "attempts are expected, 90%, 95% or 99% sure".to_owned(),
                    ));
                }
            },
        };
        self.set_setting(KEY_ATTEMPTS, text)
    }
}

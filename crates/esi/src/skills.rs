//! A character's trained skill levels and NPC standings from ESI. Read on a click, never saved to disk.

use std::collections::BTreeMap;

use mop_core::industry::TypeId;
use serde_json::Value;

use crate::client::{Answer, Client, bad_data};
use crate::error::EsiError;
use crate::token::AccessToken;

/// The highest level a skill can have in EVE.
const MAX_LEVEL: u64 = 5;
/// The lowest and highest standing EVE gives.
const MAX_STANDING: f64 = 10.0;
/// The largest standing times 100, as a whole number.
const HUNDREDTHS_LIMIT: i16 = 1_000;

/// The skills a character can use now. `levels` holds `active_skill_level`, so alpha clones see their own limits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CharacterSkills {
    pub levels: BTreeMap<TypeId, u8>,
    pub total_sp: u64,
}

/// Who gave a standing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StandingFrom {
    Agent,
    NpcCorp,
    Faction,
}

/// One standing of the character. `hundredths` is the standing times 100, rounded half away from zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NpcStanding {
    pub from_id: u32,
    pub from: StandingFrom,
    pub hundredths: i16,
}

const SKILLS_PATH: &str = "skills";
const STANDINGS_PATH: &str = "standings";

/// The character's trained skill levels, read from ESI on each call.
pub fn read_skills(
    client: &mut Client<'_>,
    now: i64,
    token: &AccessToken,
    character: u32,
) -> Result<CharacterSkills, EsiError> {
    let path = format!("/characters/{character}/{SKILLS_PATH}");
    let group = format!("{character}/{SKILLS_PATH}");
    let answer = client.get_json(now, token, &path, &group)?;
    parse_skills(&answer, &path)
}

/// The character's standings with agents, NPC corporations and factions, in ESI's order.
pub fn read_standings(
    client: &mut Client<'_>,
    now: i64,
    token: &AccessToken,
    character: u32,
) -> Result<Vec<NpcStanding>, EsiError> {
    let path = format!("/characters/{character}/{STANDINGS_PATH}");
    let group = format!("{character}/{STANDINGS_PATH}");
    let answer = client.get_json(now, token, &path, &group)?;
    parse_standings(&answer, &path)
}

fn parse_skills(answer: &Answer, route: &str) -> Result<CharacterSkills, EsiError> {
    let body = &answer.body;
    let rows = field(body, "skills", route)?;
    let rows = array(rows, route)?;
    let mut levels = BTreeMap::new();
    for row in rows {
        let type_id = narrow(unsigned(row, "skill_id", route)?, "skill_id", route)?;
        let level = unsigned(row, "active_skill_level", route)?;
        if level > MAX_LEVEL {
            return Err(bad_data(route, "active_skill_level", &level.to_string()));
        }
        if levels
            .insert(type_id, narrow(level, "active_skill_level", route)?)
            .is_some()
        {
            return Err(bad_data(
                route,
                "skill_id",
                &format!("{type_id:?} repeated"),
            ));
        }
    }
    let total_sp = unsigned(body, "total_sp", route)?;
    Ok(CharacterSkills { levels, total_sp })
}

fn parse_standings(answer: &Answer, route: &str) -> Result<Vec<NpcStanding>, EsiError> {
    let rows = array(&answer.body, route)?;
    rows.iter().map(|row| standing(row, route)).collect()
}

fn standing(row: &Value, route: &str) -> Result<NpcStanding, EsiError> {
    let from_id = unsigned(row, "from_id", route)?;
    let from_id = narrow(from_id, "from_id", route)?;
    let from = match text(row, "from_type", route)? {
        "agent" => StandingFrom::Agent,
        "npc_corp" => StandingFrom::NpcCorp,
        "faction" => StandingFrom::Faction,
        other => return Err(bad_data(route, "from_type", other)),
    };
    let value = number(row, "standing", route)?;
    if !(-MAX_STANDING..=MAX_STANDING).contains(&value) {
        return Err(bad_data(route, "standing", &value.to_string()));
    }
    // `round` rounds half away from zero; the range check above keeps the result within `HUNDREDTHS_LIMIT`.
    let rounded = (value * 100.0).round();
    let hundredths: i16 = format!("{rounded:.0}")
        .parse()
        .ok()
        .filter(|candidate: &i16| candidate.abs() <= HUNDREDTHS_LIMIT)
        .ok_or_else(|| bad_data(route, "standing", &value.to_string()))?;
    Ok(NpcStanding {
        from_id,
        from,
        hundredths,
    })
}

/// A whole number from EVE that must fit the narrower type, else `BadData` naming the field.
fn narrow<T: TryFrom<u64>>(value: u64, key: &'static str, route: &str) -> Result<T, EsiError> {
    T::try_from(value)
        .ok()
        .ok_or_else(|| bad_data(route, key, &value.to_string()))
}

fn array<'a>(value: &'a Value, route: &str) -> Result<&'a Vec<Value>, EsiError> {
    value
        .as_array()
        .ok_or_else(|| bad_data(route, "body", &value.to_string()))
}

/// The field `key` of an object. A missing field is `BadData`.
fn field<'a>(object: &'a Value, key: &'static str, route: &str) -> Result<&'a Value, EsiError> {
    object
        .get(key)
        .ok_or_else(|| bad_data(route, key, "missing"))
}

fn unsigned(object: &Value, key: &'static str, route: &str) -> Result<u64, EsiError> {
    let value = field(object, key, route)?;
    value
        .as_u64()
        .ok_or_else(|| bad_data(route, key, &value.to_string()))
}

fn number(object: &Value, key: &'static str, route: &str) -> Result<f64, EsiError> {
    let value = field(object, key, route)?;
    value
        .as_f64()
        .ok_or_else(|| bad_data(route, key, &value.to_string()))
}

fn text<'a>(object: &'a Value, key: &'static str, route: &str) -> Result<&'a str, EsiError> {
    let value = field(object, key, route)?;
    value
        .as_str()
        .ok_or_else(|| bad_data(route, key, &value.to_string()))
}

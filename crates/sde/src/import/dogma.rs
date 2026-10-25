use std::collections::{BTreeMap, BTreeSet};

use mop_core::industry::{Activity, TypeId};
use mop_core::invention::Decryptor;
use serde::Deserialize;
use serde_json::Number;

use crate::error::SdeError;

use super::{
    ATTR_CHANCE, ATTR_DECRYPTOR_ME, ATTR_DECRYPTOR_RUNS, ATTR_DECRYPTOR_TE, ATTR_HIGH_SECURITY,
    ATTR_JOB_COST, ATTR_LOW_SECURITY, ATTR_NULL_SECURITY, ATTR_RIG_MATERIAL_MANUFACTURING,
    ATTR_RIG_MATERIAL_REACTION, ATTR_RIG_SIZE, ATTR_STRUCTURE_MATERIAL, DECRYPTOR_GROUP_NAME,
    GroupRow, MODIFIER_SOURCES_FILE, RIG_CATEGORY, RIG_FIT_ATTRIBUTES, STRUCTURE_GROUPS, Source,
    TARGET_FILTERS_FILE, TYPE_DOGMA_FILE, TypeRow, parse_file, required,
};

/// The structures, rigs and target filters the facility queries read.
pub(super) struct Facilities {
    pub(super) structures: Vec<StructureRow>,
    pub(super) rigs: Vec<RigRow>,
    pub(super) rig_fits: Vec<(TypeId, u32)>,
    pub(super) filters: Vec<FilterRow>,
}

pub(super) struct StructureRow {
    pub(super) type_id: TypeId,
    pub(super) rig_size: u8,
    /// Material bonus for manufacturing, in hundredths. `None` when the structure has none.
    pub(super) manufacturing_bonus: Option<u32>,
}

pub(super) struct RigRow {
    pub(super) type_id: TypeId,
    pub(super) activity: Activity,
    /// Material bonus in tenths of a percent, negative.
    pub(super) bonus_tenths: i32,
    pub(super) rig_size: u8,
    /// One entry per material list entry; `None` covers every product.
    pub(super) filter_ids: BTreeSet<Option<TypeId>>,
    /// Security multipliers in tenths. Reactions have no high security multiplier.
    pub(super) high_tenths: Option<u32>,
    pub(super) low_tenths: u32,
    pub(super) null_tenths: u32,
}

pub(super) struct FilterRow {
    pub(super) id: TypeId,
    pub(super) category_ids: Vec<u32>,
    pub(super) group_ids: Vec<u32>,
}

/// The attributes of one type, as the text the download gives each value.
pub(super) struct Dogma {
    pub(super) line: usize,
    pub(super) values: BTreeMap<u32, String>,
}

impl Dogma {
    /// Attribute `id` as an exact integer at `decimals` decimal places. Absent is `None`. Text that
    /// is not exact at that scale, or does not fit `T`, is `BadNumber`.
    fn optional_number<T: TryFrom<i64>>(
        &self,
        id: u32,
        decimals: u32,
    ) -> Result<Option<T>, SdeError> {
        let Some(text) = self.values.get(&id) else {
            return Ok(None);
        };
        let bad = || SdeError::BadNumber {
            file: TYPE_DOGMA_FILE.to_owned(),
            line: self.line,
            field: format!("attribute {id}"),
            text: text.clone(),
        };
        let value = exact_scaled(text, decimals).ok_or_else(bad)?;
        let number = T::try_from(value).ok().ok_or_else(bad)?;
        Ok(Some(number))
    }

    /// The text the download gives attribute `id`. Empty when there is none.
    fn text(&self, id: u32) -> String {
        self.values.get(&id).cloned().unwrap_or_default()
    }

    fn number<T: TryFrom<i64>>(&self, id: u32, decimals: u32) -> Result<T, SdeError> {
        self.optional_number(id, decimals)?
            .ok_or_else(|| SdeError::MissingField {
                file: TYPE_DOGMA_FILE.to_owned(),
                line: self.line,
                field: format!("attribute {id}"),
            })
    }
}

/// The number in `text` with `decimals` decimal places, read from its digits. `None` when it has
/// more decimal places than that, unless the extra ones are zeros, or does not fit an `i64`.
fn exact_scaled(text: &str, decimals: u32) -> Option<i64> {
    let (negative, unsigned) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let (whole, fraction) = unsigned.split_once('.').unwrap_or((unsigned, ""));
    let digits = |part: &str| part.bytes().all(|byte| byte.is_ascii_digit());
    if whole.is_empty() || !digits(whole) || !digits(fraction) {
        return None;
    }
    let width = usize::try_from(decimals).ok()?;
    let (kept, dropped) = if fraction.len() > width {
        fraction.split_at(width)
    } else {
        (fraction, "")
    };
    if dropped.bytes().any(|byte| byte != b'0') {
        return None;
    }
    let magnitude: i64 = format!("{whole}{kept:0<width$}").parse().ok()?;
    if negative {
        magnitude.checked_neg()
    } else {
        Some(magnitude)
    }
}

#[derive(Deserialize)]
struct DogmaRecord {
    #[serde(rename = "_key")]
    key: Option<TypeId>,
    #[serde(rename = "dogmaAttributes", default)]
    attributes: Vec<DogmaAttribute>,
}

#[derive(Deserialize)]
struct DogmaAttribute {
    #[serde(rename = "attributeID")]
    id: u32,
    value: Number,
}

#[derive(Deserialize)]
struct ModifierSourceRecord {
    #[serde(rename = "_key")]
    key: Option<TypeId>,
    #[serde(default)]
    manufacturing: Option<MaterialSources>,
    #[serde(default)]
    reaction: Option<MaterialSources>,
}

#[derive(Deserialize)]
struct MaterialSources {
    #[serde(default)]
    material: Vec<MaterialSource>,
}

#[derive(Deserialize)]
struct MaterialSource {
    #[serde(rename = "dogmaAttributeID")]
    attribute_id: Option<u32>,
    #[serde(rename = "filterID")]
    filter_id: Option<TypeId>,
}

#[derive(Deserialize)]
struct TargetFilterRecord {
    #[serde(rename = "_key")]
    key: Option<TypeId>,
    #[serde(rename = "categoryIDs", default)]
    category_ids: Vec<u32>,
    #[serde(rename = "groupIDs", default)]
    group_ids: Vec<u32>,
}

impl Facilities {
    /// Reads the published structures and rigs of the download, and the target filters. A rig is a
    /// published type in the rig category with a material bonus; a structure is one of the
    /// structure groups.
    pub(super) fn read(
        source: &mut Source,
        types: &[TypeRow],
        groups: &[GroupRow],
    ) -> Result<Self, SdeError> {
        let category_of: BTreeMap<u32, u32> = groups
            .iter()
            .map(|group| (group.id, group.category_id))
            .collect();
        let modifiers = read_modifier_sources(source)?;
        // A rig-category type without a material entry is not a rig: its attributes are not read.
        let wanted: BTreeMap<TypeId, u32> = types
            .iter()
            .filter(|row| {
                row.published
                    && (STRUCTURE_GROUPS.contains(&row.group_id)
                        || (category_of.get(&row.group_id) == Some(&RIG_CATEGORY)
                            && modifiers
                                .get(&row.id)
                                .is_some_and(|record| rig_material(record).is_some())))
            })
            .map(|row| (row.id, row.group_id))
            .collect();

        let dogma = read_dogma(source, &wanted)?;
        let mut structures = Vec::new();
        let mut rigs = Vec::new();
        let mut rig_fits = Vec::new();
        for (&id, &group_id) in &wanted {
            let attributes = dogma.get(&id).ok_or_else(|| SdeError::NoDogma {
                file: TYPE_DOGMA_FILE.to_owned(),
                type_id: id,
            })?;
            let record = modifiers.get(&id);
            if STRUCTURE_GROUPS.contains(&group_id) {
                structures.push(structure_row(id, attributes, record)?);
            } else if let Some(rig) = rig_row(id, attributes, record)? {
                rig_fits.extend(fit_groups(id, attributes)?);
                rigs.push(rig);
            }
        }
        let filters = read_filters(source)?;
        Ok(Self {
            structures,
            rigs,
            rig_fits,
            filters,
        })
    }
}

/// The attributes of the wanted types, by type id.
fn read_dogma(
    source: &mut Source,
    wanted: &BTreeMap<TypeId, u32>,
) -> Result<BTreeMap<TypeId, Dogma>, SdeError> {
    let mut dogma = BTreeMap::new();
    for (line, record) in parse_file::<DogmaRecord>(source, TYPE_DOGMA_FILE)? {
        let id = required(record.key, TYPE_DOGMA_FILE, line, "_key")?;
        if wanted.contains_key(&id) {
            let values = record
                .attributes
                .into_iter()
                .map(|attribute| (attribute.id, attribute.value.to_string()))
                .collect();
            dogma.insert(id, Dogma { line, values });
        }
    }
    Ok(dogma)
}

/// The decryptors, and the job cost bonus of every type that has one, as type id and hundredths.
type Attributes = (Vec<Decryptor>, Vec<(TypeId, u32)>);

/// The decryptors, and the job cost bonus of every type that has one, from the attribute file.
pub(super) fn read_attributes(
    source: &mut Source,
    types: &[TypeRow],
    groups: &[GroupRow],
) -> Result<Attributes, SdeError> {
    let decryptor_groups: BTreeSet<u32> = groups
        .iter()
        .filter(|group| group.name.as_deref() == Some(DECRYPTOR_GROUP_NAME))
        .map(|group| group.id)
        .collect();
    let decryptor_ids: BTreeSet<TypeId> = types
        .iter()
        .filter(|row| row.published && decryptor_groups.contains(&row.group_id))
        .map(|row| row.id)
        .collect();
    let mut decryptor_dogma = BTreeMap::new();
    let mut job_costs = Vec::new();
    for (line, record) in parse_file::<DogmaRecord>(source, TYPE_DOGMA_FILE)? {
        let id = required(record.key, TYPE_DOGMA_FILE, line, "_key")?;
        if let Some(bonus) = record
            .attributes
            .iter()
            .find(|attribute| attribute.id == ATTR_JOB_COST)
        {
            job_costs.push((id, job_cost_hundredths(line, &bonus.value.to_string())?));
        }
        if decryptor_ids.contains(&id) {
            let values = record
                .attributes
                .iter()
                .map(|attribute| (attribute.id, attribute.value.to_string()))
                .collect();
            decryptor_dogma.insert(id, Dogma { line, values });
        }
    }
    let mut decryptors = Vec::new();
    for id in decryptor_ids {
        let dogma = decryptor_dogma
            .get(&id)
            .ok_or(SdeError::IncompleteDecryptor {
                type_id: id,
                attribute: ATTR_CHANCE,
            })?;
        decryptors.push(decryptor_from_dogma(id, dogma)?);
    }
    Ok((decryptors, job_costs))
}

/// A structure's job cost bonus in hundredths: `0.97` is 97. Between 0 and 1, at most 2 decimals.
fn job_cost_hundredths(line: usize, text: &str) -> Result<u32, SdeError> {
    let bad = || bad_number(line, ATTR_JOB_COST, text);
    let value = exact_scaled(text, 2).ok_or_else(bad)?;
    u32::try_from(value)
        .ok()
        .filter(|&hundredths| hundredths <= 100)
        .ok_or_else(bad)
}

/// One decryptor from its four attributes. The chance is in hundredths, at most 10.00.
fn decryptor_from_dogma(type_id: TypeId, dogma: &Dogma) -> Result<Decryptor, SdeError> {
    let missing = |attribute| SdeError::IncompleteDecryptor { type_id, attribute };
    let chance: u32 = dogma
        .optional_number(ATTR_CHANCE, 2)?
        .ok_or_else(|| missing(ATTR_CHANCE))?;
    if chance > 1000 {
        return Err(bad_number(
            dogma.line,
            ATTR_CHANCE,
            &dogma.text(ATTR_CHANCE),
        ));
    }
    Ok(Decryptor {
        type_id,
        chance_hundredths: chance,
        me: dogma
            .optional_number(ATTR_DECRYPTOR_ME, 0)?
            .ok_or_else(|| missing(ATTR_DECRYPTOR_ME))?,
        te: dogma
            .optional_number(ATTR_DECRYPTOR_TE, 0)?
            .ok_or_else(|| missing(ATTR_DECRYPTOR_TE))?,
        runs: dogma
            .optional_number(ATTR_DECRYPTOR_RUNS, 0)?
            .ok_or_else(|| missing(ATTR_DECRYPTOR_RUNS))?,
    })
}

/// The error for attribute `attribute` of the type at `line` of the attribute file.
fn bad_number(line: usize, attribute: u32, text: &str) -> SdeError {
    SdeError::BadNumber {
        file: TYPE_DOGMA_FILE.to_owned(),
        line,
        field: format!("attribute {attribute}"),
        text: text.to_owned(),
    }
}

fn read_modifier_sources(
    source: &mut Source,
) -> Result<BTreeMap<TypeId, ModifierSourceRecord>, SdeError> {
    let mut modifiers = BTreeMap::new();
    for (line, record) in parse_file::<ModifierSourceRecord>(source, MODIFIER_SOURCES_FILE)? {
        let id = required(record.key, MODIFIER_SOURCES_FILE, line, "_key")?;
        modifiers.insert(id, record);
    }
    Ok(modifiers)
}

fn read_filters(source: &mut Source) -> Result<Vec<FilterRow>, SdeError> {
    let mut filters = Vec::new();
    for (line, record) in parse_file::<TargetFilterRecord>(source, TARGET_FILTERS_FILE)? {
        filters.push(FilterRow {
            id: required(record.key, TARGET_FILTERS_FILE, line, "_key")?,
            category_ids: record.category_ids,
            group_ids: record.group_ids,
        });
    }
    Ok(filters)
}

/// A structure: its size, and its manufacturing material bonus when its modifier source has one.
fn structure_row(
    id: TypeId,
    attributes: &Dogma,
    record: Option<&ModifierSourceRecord>,
) -> Result<StructureRow, SdeError> {
    let has_material = record
        .and_then(|record| record.manufacturing.as_ref())
        .is_some_and(|sources| {
            sources
                .material
                .iter()
                .any(|entry| entry.attribute_id == Some(ATTR_STRUCTURE_MATERIAL))
        });
    Ok(StructureRow {
        type_id: id,
        rig_size: attributes.number(ATTR_RIG_SIZE, 0)?,
        manufacturing_bonus: if has_material {
            Some(attributes.number(ATTR_STRUCTURE_MATERIAL, 2)?)
        } else {
            None
        },
    })
}

/// A rig: a type with a material bonus. `None` for a type that is not a rig, and for a rig whose
/// bonus or security values the download does not give: such a rig is left out, not an error.
fn rig_row(
    id: TypeId,
    attributes: &Dogma,
    record: Option<&ModifierSourceRecord>,
) -> Result<Option<RigRow>, SdeError> {
    let Some((activity, entries)) = record.and_then(rig_material) else {
        return Ok(None);
    };
    let bonus_attribute = match activity {
        Activity::Manufacturing => ATTR_RIG_MATERIAL_MANUFACTURING,
        Activity::Reaction => ATTR_RIG_MATERIAL_REACTION,
    };
    let high_tenths: Option<u32> = attributes.optional_number(ATTR_HIGH_SECURITY, 1)?;
    let high_tenths = match (activity, high_tenths) {
        (Activity::Manufacturing, None) => return Ok(None),
        (_, high) => high,
    };
    let bonus_tenths = attributes.optional_number::<i32>(bonus_attribute, 1)?;
    let low_tenths = attributes.optional_number::<u32>(ATTR_LOW_SECURITY, 1)?;
    let null_tenths = attributes.optional_number::<u32>(ATTR_NULL_SECURITY, 1)?;
    let (Some(bonus_tenths), Some(low_tenths), Some(null_tenths)) =
        (bonus_tenths, low_tenths, null_tenths)
    else {
        return Ok(None);
    };
    Ok(Some(RigRow {
        type_id: id,
        activity,
        bonus_tenths,
        rig_size: attributes.number(ATTR_RIG_SIZE, 0)?,
        filter_ids: entries.iter().map(|entry| entry.filter_id).collect(),
        high_tenths,
        low_tenths,
        null_tenths,
    }))
}

/// The activity and material entries of a rig. Manufacturing first; a rig with a bonus for both
/// would be read as manufacturing.
fn rig_material(record: &ModifierSourceRecord) -> Option<(Activity, &[MaterialSource])> {
    if let Some(entries) = record.manufacturing.as_ref().and_then(non_empty_material) {
        return Some((Activity::Manufacturing, entries));
    }
    let entries = record.reaction.as_ref().and_then(non_empty_material)?;
    Some((Activity::Reaction, entries))
}

fn non_empty_material(sources: &MaterialSources) -> Option<&[MaterialSource]> {
    Some(sources.material.as_slice()).filter(|entries| !entries.is_empty())
}

/// The group ids a rig fits, from its fitting attributes.
fn fit_groups(id: TypeId, attributes: &Dogma) -> Result<Vec<(TypeId, u32)>, SdeError> {
    let mut groups = Vec::new();
    for attribute in RIG_FIT_ATTRIBUTES {
        if let Some(group) = attributes.optional_number::<u32>(attribute, 0)? {
            groups.push((id, group));
        }
    }
    Ok(groups)
}

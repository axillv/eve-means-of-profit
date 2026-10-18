//! Reads the download into memory and checks it. Then writes a fresh database in one transaction.

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use profit_core::industry::{Activity, Recipe, TypeId};
use profit_core::volume::Volume;
use rusqlite::{Connection, Transaction, params};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Number;

use crate::error::SdeError;
use crate::schema::{CREATE_TABLES, SCHEMA_VERSION, activity_name};

const SDE_FILE: &str = "_sde.jsonl";
const CATEGORIES_FILE: &str = "categories.jsonl";
const GROUPS_FILE: &str = "groups.jsonl";
const TYPES_FILE: &str = "types.jsonl";
const BLUEPRINTS_FILE: &str = "blueprints.jsonl";
const TYPE_DOGMA_FILE: &str = "typeDogma.jsonl";
const MODIFIER_SOURCES_FILE: &str = "industryModifierSources.jsonl";
const TARGET_FILTERS_FILE: &str = "industryTargetFilters.jsonl";

/// Engineering complexes and refineries: the groups of the structures the app offers.
const STRUCTURE_GROUPS: [u32; 2] = [1404, 1406];
const RIG_CATEGORY: u32 = 66;

/// Dogma attributes, as in the download. Values are read as exact decimals at the stated places.
const ATTR_RIG_SIZE: u32 = 1547;
const ATTR_STRUCTURE_MATERIAL: u32 = 2600;
const ATTR_RIG_MATERIAL_MANUFACTURING: u32 = 2594;
const ATTR_RIG_MATERIAL_REACTION: u32 = 2714;
const ATTR_HIGH_SECURITY: u32 = 2355;
const ATTR_LOW_SECURITY: u32 = 2356;
const ATTR_NULL_SECURITY: u32 = 2357;
/// The groups a rig fits, one per attribute.
const RIG_FIT_ATTRIBUTES: [u32; 20] = [
    1298, 1299, 1300, 1301, 1872, 1879, 1880, 1881, 2065, 2396, 2476, 2477, 2478, 2479, 2480, 2481,
    2482, 2483, 2484, 2485,
];

/// What an import read and wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportReport {
    pub build_number: u64,
    pub release_date: String,
    pub types: usize,
    pub groups: usize,
    pub categories: usize,
    pub recipes: usize,
    pub material_rows: usize,
    pub structures: usize,
    pub rigs: usize,
    /// Recipes made by unpublished blueprints. They are not imported.
    pub skipped_unpublished: usize,
    /// Recipes that lost to a blueprint with a lower id for the same product.
    pub duplicates: usize,
}

/// Reads the download at `source` and writes a fresh database at `db`.
///
/// `source` is a directory of `.jsonl` files, or a zip with them at its root. Nothing is written
/// until the whole download has been read and checked. On an error, `db` is left as it was.
pub fn import(source: &Path, db: &Path) -> Result<ImportReport, SdeError> {
    let pending = pending_path(db);
    remove_if_exists(&pending)?;
    let download = Download::read(&mut Source::open(source)?)?;
    let report = download.report();
    match write_pending(&pending, &download).and_then(|()| rename_over(&pending, db)) {
        Ok(()) => Ok(report),
        Err(error) => Err(match try_remove(&pending) {
            Ok(()) => error,
            Err(cleanup) => cleanup_failed(&pending, &cleanup, &error),
        }),
    }
}

/// Where the files come from: a directory, or a zip with the files at its root.
enum Source {
    Directory(PathBuf),
    Zip {
        path: PathBuf,
        archive: zip::ZipArchive<File>,
    },
}

impl Source {
    fn open(path: &Path) -> Result<Self, SdeError> {
        if path.is_dir() {
            return Ok(Self::Directory(path.to_path_buf()));
        }
        let file = File::open(path).map_err(|source| SdeError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let archive = zip::ZipArchive::new(file).map_err(SdeError::Zip)?;
        Ok(Self::Zip {
            path: path.to_path_buf(),
            archive,
        })
    }

    /// The whole text of one file.
    fn read(&mut self, name: &str) -> Result<String, SdeError> {
        match self {
            Self::Directory(dir) => {
                let path = dir.join(name);
                fs::read_to_string(&path).map_err(|source| {
                    if source.kind() == ErrorKind::NotFound {
                        SdeError::MissingFile(name.to_owned())
                    } else {
                        SdeError::Io { path, source }
                    }
                })
            }
            Self::Zip { path, archive } => {
                let mut entry = archive.by_name(name).map_err(|error| {
                    if matches!(error, zip::result::ZipError::FileNotFound) {
                        SdeError::MissingFile(name.to_owned())
                    } else {
                        SdeError::Zip(error)
                    }
                })?;
                let mut text = String::new();
                entry
                    .read_to_string(&mut text)
                    .map_err(|source| SdeError::Io {
                        path: path.clone(),
                        source,
                    })?;
                Ok(text)
            }
        }
    }
}

/// The whole download, read and checked. Nothing here refers to the database.
struct Download {
    build_number: u64,
    release_date: String,
    categories: Vec<CategoryRow>,
    groups: Vec<GroupRow>,
    types: Vec<TypeRow>,
    recipes: Vec<RecipeRow>,
    skipped_unpublished: usize,
    duplicates: usize,
    facilities: Facilities,
}

impl Download {
    fn read(source: &mut Source) -> Result<Self, SdeError> {
        let (build_number, release_date) = read_build(source)?;
        let categories = read_rows(source, CATEGORIES_FILE, CategoryRow::from_record)?;
        let groups = read_rows(source, GROUPS_FILE, GroupRow::from_record)?;
        let types = read_types(source, &groups)?;
        let recipes = Recipes::from_blueprints(&types, parse_file(source, BLUEPRINTS_FILE)?)?;
        let facilities = Facilities::read(source, &types, &groups)?;
        Ok(Self {
            build_number,
            release_date,
            categories,
            groups,
            types,
            skipped_unpublished: recipes.skipped_unpublished,
            duplicates: recipes.duplicates,
            recipes: recipes.by_product.into_values().collect(),
            facilities,
        })
    }

    fn report(&self) -> ImportReport {
        ImportReport {
            build_number: self.build_number,
            release_date: self.release_date.clone(),
            types: self.types.len(),
            groups: self.groups.len(),
            categories: self.categories.len(),
            recipes: self.recipes.len(),
            material_rows: self
                .recipes
                .iter()
                .map(|row| row.recipe.materials.len())
                .sum(),
            structures: self.facilities.structures.len(),
            rigs: self.facilities.rigs.len(),
            skipped_unpublished: self.skipped_unpublished,
            duplicates: self.duplicates,
        }
    }
}

struct CategoryRow {
    id: u32,
    published: bool,
}

struct GroupRow {
    id: u32,
    category_id: u32,
    published: bool,
}

struct TypeRow {
    id: TypeId,
    name: String,
    group_id: u32,
    published: bool,
    volume: Option<Volume>,
    packaged_volume: Option<Volume>,
    portion_size: u64,
}

struct RecipeRow {
    product_id: TypeId,
    blueprint_id: TypeId,
    recipe: Recipe,
}

#[derive(Deserialize)]
struct BuildRecord {
    #[serde(rename = "buildNumber")]
    build_number: Option<u64>,
    #[serde(rename = "releaseDate")]
    release_date: Option<String>,
}

#[derive(Deserialize, Clone, Copy)]
struct CategoryRecord {
    #[serde(rename = "_key")]
    key: Option<u32>,
    published: Option<bool>,
}

#[derive(Deserialize, Clone, Copy)]
struct GroupRecord {
    #[serde(rename = "_key")]
    key: Option<u32>,
    #[serde(rename = "categoryID")]
    category_id: Option<u32>,
    published: Option<bool>,
}

#[derive(Deserialize)]
struct TypeRecord {
    #[serde(rename = "_key")]
    key: Option<TypeId>,
    name: Option<LocalizedName>,
    #[serde(rename = "groupID")]
    group_id: Option<u32>,
    published: Option<bool>,
    volume: Option<Number>,
    #[serde(rename = "packagedVolume")]
    packaged_volume: Option<Number>,
    #[serde(rename = "portionSize")]
    portion_size: Option<u64>,
}

#[derive(Deserialize)]
struct LocalizedName {
    en: Option<String>,
}

#[derive(Deserialize)]
struct BlueprintRecord {
    #[serde(rename = "_key")]
    key: Option<TypeId>,
    #[serde(rename = "blueprintTypeID")]
    type_id: Option<TypeId>,
    #[serde(rename = "maxProductionLimit")]
    max_production_limit: Option<u64>,
    #[serde(default)]
    activities: Activities,
}

#[derive(Deserialize, Default)]
struct Activities {
    manufacturing: Option<ActivityRecord>,
    reaction: Option<ActivityRecord>,
    invention: Option<ActivityRecord>,
}

#[derive(Deserialize)]
struct ActivityRecord {
    /// Absent for something that is built from nothing.
    #[serde(default)]
    materials: Vec<Quantity>,
    #[serde(default)]
    products: Vec<Quantity>,
}

#[derive(Deserialize)]
struct Quantity {
    #[serde(rename = "typeID")]
    type_id: TypeId,
    quantity: u64,
}

impl CategoryRow {
    fn from_record(record: CategoryRecord, line: usize) -> Result<Self, SdeError> {
        Ok(Self {
            id: required(record.key, CATEGORIES_FILE, line, "_key")?,
            published: required(record.published, CATEGORIES_FILE, line, "published")?,
        })
    }
}

impl GroupRow {
    fn from_record(record: GroupRecord, line: usize) -> Result<Self, SdeError> {
        Ok(Self {
            id: required(record.key, GROUPS_FILE, line, "_key")?,
            category_id: required(record.category_id, GROUPS_FILE, line, "categoryID")?,
            published: required(record.published, GROUPS_FILE, line, "published")?,
        })
    }
}

impl TypeRow {
    fn from_record(record: TypeRecord, line: usize) -> Result<Self, SdeError> {
        let name =
            record
                .name
                .and_then(|name| name.en)
                .ok_or_else(|| SdeError::MissingEnglishName {
                    file: TYPES_FILE.to_owned(),
                    line,
                })?;
        Ok(Self {
            id: required(record.key, TYPES_FILE, line, "_key")?,
            name,
            group_id: required(record.group_id, TYPES_FILE, line, "groupID")?,
            published: required(record.published, TYPES_FILE, line, "published")?,
            volume: optional_volume(record.volume, TYPES_FILE, line, "volume")?,
            packaged_volume: optional_volume(
                record.packaged_volume,
                TYPES_FILE,
                line,
                "packagedVolume",
            )?,
            portion_size: required(record.portion_size, TYPES_FILE, line, "portionSize")?,
        })
    }
}

/// Recipes by product, as the blueprints give them.
#[derive(Default)]
struct Recipes {
    by_product: BTreeMap<TypeId, RecipeRow>,
    skipped_unpublished: usize,
    duplicates: usize,
}

impl Recipes {
    /// One recipe per product, from the published blueprints. When several make the same product,
    /// the lowest blueprint id wins.
    fn from_blueprints(
        types: &[TypeRow],
        blueprints: Vec<(usize, BlueprintRecord)>,
    ) -> Result<Self, SdeError> {
        let published: BTreeMap<TypeId, bool> =
            types.iter().map(|row| (row.id, row.published)).collect();
        let invented: BTreeSet<TypeId> = blueprints
            .iter()
            .filter_map(|(_, blueprint)| blueprint.activities.invention.as_ref())
            .flat_map(|activity| activity.products.iter().map(|product| product.type_id))
            .collect();
        let mut recipes = Self::default();
        for (line, blueprint) in blueprints {
            let blueprint_id = required(blueprint.key, BLUEPRINTS_FILE, line, "_key")?;
            let type_id = required(blueprint.type_id, BLUEPRINTS_FILE, line, "blueprintTypeID")?;
            let is_published = published
                .get(&type_id)
                .copied()
                .ok_or(SdeError::UnknownType {
                    file: BLUEPRINTS_FILE.to_owned(),
                    line,
                    type_id,
                })?;
            let made_by_invention = invented.contains(&type_id);
            let activities = [
                (Activity::Manufacturing, blueprint.activities.manufacturing),
                (Activity::Reaction, blueprint.activities.reaction),
            ];
            for (activity, record) in activities {
                // An activity that makes nothing has no recipe, so it is not counted as skipped.
                let Some(record) = record else {
                    continue;
                };
                let Some(product) = record.products.first() else {
                    continue;
                };
                if !is_published {
                    recipes.skipped_unpublished += 1;
                    continue;
                }
                let recipe = recipe_from_activity(
                    activity,
                    &record,
                    product.quantity,
                    line,
                    blueprint.max_production_limit,
                    made_by_invention,
                )?;
                recipes.insert(RecipeRow {
                    product_id: product.type_id,
                    blueprint_id,
                    recipe,
                });
            }
        }
        Ok(recipes)
    }

    fn insert(&mut self, row: RecipeRow) {
        match self.by_product.entry(row.product_id) {
            Entry::Vacant(slot) => {
                slot.insert(row);
            }
            Entry::Occupied(mut slot) => {
                self.duplicates += 1;
                if row.blueprint_id < slot.get().blueprint_id {
                    slot.insert(row);
                }
            }
        }
    }
}

/// The recipe of one manufacturing or reaction activity whose first product yields
/// `output_per_run`.
fn recipe_from_activity(
    activity: Activity,
    record: &ActivityRecord,
    output_per_run: u64,
    line: usize,
    max_runs: Option<u64>,
    made_by_invention: bool,
) -> Result<Recipe, SdeError> {
    Ok(Recipe {
        activity,
        output_per_run,
        max_runs_per_job: required(max_runs, BLUEPRINTS_FILE, line, "maxProductionLimit")?,
        materials: record
            .materials
            .iter()
            .map(|material| (material.type_id, material.quantity))
            .collect(),
        made_by_invention,
    })
}

/// The structures, rigs and target filters the facility queries read.
struct Facilities {
    structures: Vec<StructureRow>,
    rigs: Vec<RigRow>,
    rig_fits: Vec<(TypeId, u32)>,
    filters: Vec<FilterRow>,
}

struct StructureRow {
    type_id: TypeId,
    rig_size: u8,
    /// Material bonus for manufacturing, in hundredths. `None` when the structure has none.
    manufacturing_bonus: Option<u32>,
}

struct RigRow {
    type_id: TypeId,
    activity: Activity,
    /// Material bonus in tenths of a percent, negative.
    bonus_tenths: i32,
    rig_size: u8,
    /// `None` when the rig covers every product.
    filter_id: Option<TypeId>,
    /// Security multipliers in tenths. Reactions have no high security multiplier.
    high_tenths: Option<u32>,
    low_tenths: u32,
    null_tenths: u32,
}

struct FilterRow {
    id: TypeId,
    category_ids: Vec<u32>,
    group_ids: Vec<u32>,
}

/// The attributes of one type, as the text the download gives each value.
struct Dogma {
    line: usize,
    values: BTreeMap<u32, String>,
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
    fn read(source: &mut Source, types: &[TypeRow], groups: &[GroupRow]) -> Result<Self, SdeError> {
        let category_of: BTreeMap<u32, u32> = groups
            .iter()
            .map(|group| (group.id, group.category_id))
            .collect();
        let wanted: BTreeMap<TypeId, u32> = types
            .iter()
            .filter(|row| {
                row.published
                    && (STRUCTURE_GROUPS.contains(&row.group_id)
                        || category_of.get(&row.group_id) == Some(&RIG_CATEGORY))
            })
            .map(|row| (row.id, row.group_id))
            .collect();

        let dogma = read_dogma(source, &wanted)?;
        let modifiers = read_modifier_sources(source)?;
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
        .is_some_and(|sources| !sources.material.is_empty());
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

/// A rig: a type with a material bonus. `None` for a type that is not a rig.
fn rig_row(
    id: TypeId,
    attributes: &Dogma,
    record: Option<&ModifierSourceRecord>,
) -> Result<Option<RigRow>, SdeError> {
    let Some((activity, entry)) = record.and_then(rig_material) else {
        return Ok(None);
    };
    let (bonus_attribute, high_tenths) = match activity {
        Activity::Manufacturing => (
            ATTR_RIG_MATERIAL_MANUFACTURING,
            Some(attributes.number(ATTR_HIGH_SECURITY, 1)?),
        ),
        Activity::Reaction => (
            ATTR_RIG_MATERIAL_REACTION,
            attributes.optional_number(ATTR_HIGH_SECURITY, 1)?,
        ),
    };
    Ok(Some(RigRow {
        type_id: id,
        activity,
        bonus_tenths: attributes.number(bonus_attribute, 1)?,
        rig_size: attributes.number(ATTR_RIG_SIZE, 0)?,
        filter_id: entry.filter_id,
        high_tenths,
        low_tenths: attributes.number(ATTR_LOW_SECURITY, 1)?,
        null_tenths: attributes.number(ATTR_NULL_SECURITY, 1)?,
    }))
}

/// The activity and first material entry of a rig. Manufacturing first; a rig with a bonus for
/// both would be read as manufacturing.
fn rig_material(record: &ModifierSourceRecord) -> Option<(Activity, &MaterialSource)> {
    if let Some(entry) = record.manufacturing.as_ref().and_then(first_material) {
        return Some((Activity::Manufacturing, entry));
    }
    let entry = record.reaction.as_ref().and_then(first_material)?;
    Some((Activity::Reaction, entry))
}

fn first_material(sources: &MaterialSources) -> Option<&MaterialSource> {
    sources.material.first()
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

/// Reads the build number and release date from the first line of `_sde.jsonl`.
fn read_build(source: &mut Source) -> Result<(u64, String), SdeError> {
    let (line, record) = parse_file::<BuildRecord>(source, SDE_FILE)?
        .into_iter()
        .next()
        .ok_or(SdeError::MissingField {
            file: SDE_FILE.to_owned(),
            line: 1,
            field: "buildNumber".to_owned(),
        })?;
    Ok((
        required(record.build_number, SDE_FILE, line, "buildNumber")?,
        required(record.release_date, SDE_FILE, line, "releaseDate")?,
    ))
}

/// Reads the types. Each one must belong to a group the download lists.
fn read_types(source: &mut Source, groups: &[GroupRow]) -> Result<Vec<TypeRow>, SdeError> {
    let group_ids: BTreeSet<u32> = groups.iter().map(|group| group.id).collect();
    parse_file::<TypeRecord>(source, TYPES_FILE)?
        .into_iter()
        .map(|(line, record)| {
            let row = TypeRow::from_record(record, line)?;
            if group_ids.contains(&row.group_id) {
                Ok(row)
            } else {
                Err(SdeError::UnknownGroup {
                    file: TYPES_FILE.to_owned(),
                    line,
                    group_id: row.group_id,
                })
            }
        })
        .collect()
}

fn read_rows<R: DeserializeOwned, T>(
    source: &mut Source,
    file: &str,
    convert: fn(R, usize) -> Result<T, SdeError>,
) -> Result<Vec<T>, SdeError> {
    parse_file(source, file)?
        .into_iter()
        .map(|(line, record)| convert(record, line))
        .collect()
}

fn parse_file<R: DeserializeOwned>(
    source: &mut Source,
    file: &str,
) -> Result<Vec<(usize, R)>, SdeError> {
    let text = source.read(file)?;
    parse_lines(file, &text)
}

/// Every non-blank line of a `.jsonl` file, with its line number. Lines count from 1.
fn parse_lines<R: DeserializeOwned>(file: &str, text: &str) -> Result<Vec<(usize, R)>, SdeError> {
    let mut records = Vec::new();
    for (index, raw) in text.lines().enumerate() {
        if raw.trim().is_empty() {
            continue;
        }
        let line = index + 1;
        let record = serde_json::from_str(raw).map_err(|error| SdeError::Json {
            file: file.to_owned(),
            line,
            message: error.to_string(),
        })?;
        records.push((line, record));
    }
    Ok(records)
}

fn required<T>(value: Option<T>, file: &str, line: usize, field: &str) -> Result<T, SdeError> {
    value.ok_or_else(|| SdeError::MissingField {
        file: file.to_owned(),
        line,
        field: field.to_owned(),
    })
}

fn optional_volume(
    number: Option<Number>,
    file: &str,
    line: usize,
    field: &str,
) -> Result<Option<Volume>, SdeError> {
    number
        .map(|number| {
            // The literal text of the number, never a float: serde_json keeps it as written.
            let text = number.to_string();
            match Volume::parse_decimal(&text) {
                Ok(volume) => Ok(volume),
                Err(_) => Err(SdeError::BadNumber {
                    file: file.to_owned(),
                    line,
                    field: field.to_owned(),
                    text,
                }),
            }
        })
        .transpose()
}

/// SQLite integers are signed 64-bit. A value that does not fit is refused.
fn sql_int(value: u64) -> Result<i64, SdeError> {
    match i64::try_from(value) {
        Ok(number) => Ok(number),
        Err(_) => Err(SdeError::TooLarge { value }),
    }
}

/// The failure to remove the half-written file, together with the error that stopped the import.
/// The file is also removed at the start of the next import.
fn cleanup_failed(path: &Path, cleanup: &std::io::Error, error: &SdeError) -> SdeError {
    SdeError::Io {
        path: path.to_path_buf(),
        source: std::io::Error::new(
            cleanup.kind(),
            format!("{cleanup} (the import had already failed: {error})"),
        ),
    }
}

fn imported_at() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX)
        })
}

fn pending_path(db: &Path) -> PathBuf {
    let mut name = db.as_os_str().to_owned();
    name.push(".new");
    PathBuf::from(name)
}

fn remove_if_exists(path: &Path) -> Result<(), SdeError> {
    try_remove(path).map_err(|source| SdeError::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// Removes a file; one that is not there is fine.
fn try_remove(path: &Path) -> std::io::Result<()> {
    match fs::remove_file(path) {
        Err(source) if source.kind() != ErrorKind::NotFound => Err(source),
        Ok(()) | Err(_) => Ok(()),
    }
}

fn rename_over(from: &Path, to: &Path) -> Result<(), SdeError> {
    fs::rename(from, to).map_err(|source| SdeError::Io {
        path: from.to_path_buf(),
        source,
    })
}

/// Writes the whole database to `path` in one transaction, then closes it.
fn write_pending(path: &Path, download: &Download) -> Result<(), SdeError> {
    let mut conn = Connection::open(path)?;
    let tx = conn.transaction()?;
    tx.execute_batch(CREATE_TABLES)?;
    write_rows(&tx, download)?;
    tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    tx.commit()?;
    Ok(())
}

fn write_rows(tx: &Transaction<'_>, download: &Download) -> Result<(), SdeError> {
    tx.execute(
        "INSERT INTO meta (id, schema_version, build_number, release_date, imported_at)
         VALUES (1, ?1, ?2, ?3, ?4)",
        params![
            SCHEMA_VERSION,
            sql_int(download.build_number)?,
            download.release_date,
            imported_at()
        ],
    )?;
    let mut categories =
        tx.prepare("INSERT INTO categories (category_id, published) VALUES (?1, ?2)")?;
    for row in &download.categories {
        categories.execute(params![row.id, row.published])?;
    }
    let mut groups =
        tx.prepare("INSERT INTO groups (group_id, category_id, published) VALUES (?1, ?2, ?3)")?;
    for row in &download.groups {
        groups.execute(params![row.id, row.category_id, row.published])?;
    }
    let mut types = tx.prepare(
        "INSERT INTO types (type_id, name, name_lower, group_id, published, volume,
                            packaged_volume, portion_size)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
    )?;
    for row in &download.types {
        types.execute(params![
            row.id,
            row.name,
            row.name.to_lowercase(),
            row.group_id,
            row.published,
            row.volume.map(Volume::ten_thousandths),
            row.packaged_volume.map(Volume::ten_thousandths),
            sql_int(row.portion_size)?
        ])?;
    }
    let mut recipes = tx.prepare(
        "INSERT INTO recipes (product_id, blueprint_id, activity, output_per_run,
                              max_runs_per_job, made_by_invention)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )?;
    let mut materials = tx.prepare(
        "INSERT INTO recipe_materials (product_id, position, type_id, quantity)
         VALUES (?1, ?2, ?3, ?4)",
    )?;
    for row in &download.recipes {
        recipes.execute(params![
            row.product_id,
            row.blueprint_id,
            activity_name(row.recipe.activity),
            sql_int(row.recipe.output_per_run)?,
            sql_int(row.recipe.max_runs_per_job)?,
            row.recipe.made_by_invention
        ])?;
        for (position, &(type_id, quantity)) in (0_i64..).zip(&row.recipe.materials) {
            materials.execute(params![
                row.product_id,
                position,
                type_id,
                sql_int(quantity)?
            ])?;
        }
    }
    write_facilities(tx, &download.facilities)?;
    Ok(())
}

fn write_facilities(tx: &Transaction<'_>, facilities: &Facilities) -> Result<(), SdeError> {
    let mut structures = tx.prepare(
        "INSERT INTO structures (type_id, rig_size, manufacturing_bonus_hundredths)
         VALUES (?1, ?2, ?3)",
    )?;
    for row in &facilities.structures {
        structures.execute(params![row.type_id, row.rig_size, row.manufacturing_bonus])?;
    }
    let mut rigs = tx.prepare(
        "INSERT INTO rigs (type_id, activity, bonus_tenths, rig_size, filter_id, high_tenths,
                           low_tenths, null_tenths)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
    )?;
    for row in &facilities.rigs {
        rigs.execute(params![
            row.type_id,
            activity_name(row.activity),
            row.bonus_tenths,
            row.rig_size,
            row.filter_id,
            row.high_tenths,
            row.low_tenths,
            row.null_tenths
        ])?;
    }
    let mut fits = tx.prepare("INSERT INTO rig_fits (rig_id, group_id) VALUES (?1, ?2)")?;
    for &(rig_id, group_id) in &facilities.rig_fits {
        fits.execute(params![rig_id, group_id])?;
    }
    let mut filter_categories =
        tx.prepare("INSERT INTO filter_categories (filter_id, category_id) VALUES (?1, ?2)")?;
    let mut filter_groups =
        tx.prepare("INSERT INTO filter_groups (filter_id, group_id) VALUES (?1, ?2)")?;
    for filter in &facilities.filters {
        for &category_id in &filter.category_ids {
            filter_categories.execute(params![filter.id, category_id])?;
        }
        for &group_id in &filter.group_ids {
            filter_groups.execute(params![filter.id, group_id])?;
        }
    }
    Ok(())
}

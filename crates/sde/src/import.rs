//! Reads the download into memory and checks it. Then writes a fresh database in one transaction.

mod dogma;
mod write;

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::str::FromStr;

use mop_core::industry::{Activity, Recipe, TypeId};
use mop_core::invention::Decryptor;
use mop_core::money::Rate;
use mop_core::volume::Volume;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Number;

use crate::error::SdeError;
use crate::facility::{ENGINEERING_COMPLEX_GROUP, REFINERY_GROUP};
use crate::invention::{InventionActivity, InventionProduct, SkillRequirement};
use crate::stations::{self, Station};

use dogma::{Facilities, read_attributes};
use write::{
    cleanup_failed, pending_path, remove_if_exists, rename_over, try_remove, write_pending,
};

const SDE_FILE: &str = "_sde.jsonl";
const CATEGORIES_FILE: &str = "categories.jsonl";
const GROUPS_FILE: &str = "groups.jsonl";
const TYPES_FILE: &str = "types.jsonl";
const BLUEPRINTS_FILE: &str = "blueprints.jsonl";
const TYPE_DOGMA_FILE: &str = "typeDogma.jsonl";
const MODIFIER_SOURCES_FILE: &str = "industryModifierSources.jsonl";
const TARGET_FILTERS_FILE: &str = "industryTargetFilters.jsonl";
const MAP_SYSTEMS_FILE: &str = "mapSolarSystems.jsonl";
const MAP_STARGATES_FILE: &str = "mapStargates.jsonl";
const MAP_REGIONS_FILE: &str = "mapRegions.jsonl";

/// Engineering complexes and refineries: the groups of the structures the app offers.
const STRUCTURE_GROUPS: [u32; 2] = [ENGINEERING_COMPLEX_GROUP, REFINERY_GROUP];
const RIG_CATEGORY: u32 = 66;
/// The group of the eight decryptors the app reads. Other decryptor groups are not read.
const DECRYPTOR_GROUP_NAME: &str = "Generic Decryptor";

/// Dogma attributes, as in the download. Values are read as exact decimals at the stated places.
const ATTR_CHANCE: u32 = 1112;
const ATTR_DECRYPTOR_ME: u32 = 1113;
const ATTR_DECRYPTOR_TE: u32 = 1114;
const ATTR_DECRYPTOR_RUNS: u32 = 1124;
const ATTR_JOB_COST: u32 = 2601;
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
    pub systems: usize,
    pub stargates: usize,
    pub regions: usize,
    pub stations: usize,
    pub decryptors: usize,
    pub structure_job_costs: usize,
    /// Invention activities with at least one product. Activities without one are not counted.
    pub invention_activities: usize,
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
pub(crate) enum Source {
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
                fs::read_to_string(&path).map_err(|source| file_error(name, path, source))
            }
            Self::Zip { path, archive } => {
                let mut entry = archive
                    .by_name(name)
                    .map_err(|error| zip_error(name, error))?;
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

    /// Calls `visit` with each non-blank line of a `.jsonl` file, parsed as `R`. The file is read one
    /// line at a time, so it is never held whole. Lines count from 1.
    pub(crate) fn each_record<R: DeserializeOwned>(
        &mut self,
        name: &str,
        mut visit: impl FnMut(usize, R) -> Result<(), SdeError>,
    ) -> Result<(), SdeError> {
        match self {
            Self::Directory(dir) => {
                let path = dir.join(name);
                let file =
                    File::open(&path).map_err(|source| file_error(name, path.clone(), source))?;
                each_line(BufReader::new(file), name, &path, &mut visit)
            }
            Self::Zip { path, archive } => {
                let entry = archive
                    .by_name(name)
                    .map_err(|error| zip_error(name, error))?;
                each_line(BufReader::new(entry), name, path, &mut visit)
            }
        }
    }
}

/// The error for a file of a directory that cannot be read. A missing file is `MissingFile`.
fn file_error(name: &str, path: PathBuf, source: std::io::Error) -> SdeError {
    if source.kind() == ErrorKind::NotFound {
        SdeError::MissingFile(name.to_owned())
    } else {
        SdeError::Io { path, source }
    }
}

/// The error for a file of a zip that cannot be read. A missing file is `MissingFile`.
fn zip_error(name: &str, error: zip::result::ZipError) -> SdeError {
    if matches!(error, zip::result::ZipError::FileNotFound) {
        SdeError::MissingFile(name.to_owned())
    } else {
        SdeError::Zip(error)
    }
}

/// Each non-blank line of `reader`, parsed as `R`, with its line number. Lines count from 1.
fn each_line<R: DeserializeOwned>(
    reader: impl BufRead,
    file: &str,
    path: &Path,
    visit: &mut impl FnMut(usize, R) -> Result<(), SdeError>,
) -> Result<(), SdeError> {
    for (index, text) in reader.lines().enumerate() {
        let text = text.map_err(|source| SdeError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        if text.trim().is_empty() {
            continue;
        }
        let line = index + 1;
        let record = serde_json::from_str(&text).map_err(|error| SdeError::Json {
            file: file.to_owned(),
            line,
            message: error.to_string(),
        })?;
        visit(line, record)?;
    }
    Ok(())
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
    map: MapData,
    stations: Vec<Station>,
    decryptors: Vec<Decryptor>,
    /// The job cost bonus of each type that has one: its type id and multiplier in hundredths.
    job_costs: Vec<(TypeId, u32)>,
    inventions: Vec<InventionActivity>,
}

impl Download {
    fn read(source: &mut Source) -> Result<Self, SdeError> {
        let (build_number, release_date) = read_build(source)?;
        let categories = read_rows(source, CATEGORIES_FILE, CategoryRow::from_record)?;
        let groups = read_rows(source, GROUPS_FILE, GroupRow::from_record)?;
        let types = read_types(source, &groups)?;
        let (decryptors, job_costs) = read_attributes(source, &types, &groups)?;
        let blueprints: Vec<(usize, BlueprintRecord)> = parse_file(source, BLUEPRINTS_FILE)?;
        let inventions = read_inventions(&types, &blueprints)?;
        let recipes = Recipes::from_blueprints(&types, blueprints)?;
        let facilities = Facilities::read(source, &types, &groups)?;
        let map = MapData::read(source)?;
        let system_names: BTreeMap<u32, String> = map
            .systems
            .iter()
            .map(|row| (row.id, row.name.clone()))
            .collect();
        let stations = stations::read(source, &system_names)?;
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
            map,
            stations,
            decryptors,
            job_costs,
            inventions,
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
            systems: self.map.systems.len(),
            stargates: self.map.gates.len(),
            regions: self.map.regions.len(),
            stations: self.stations.len(),
            decryptors: self.decryptors.len(),
            structure_job_costs: self.job_costs.len(),
            invention_activities: self.inventions.len(),
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
    /// The English name, if the group has one.
    name: Option<String>,
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

#[derive(Deserialize)]
struct GroupRecord {
    #[serde(rename = "_key")]
    key: Option<u32>,
    #[serde(rename = "categoryID")]
    category_id: Option<u32>,
    published: Option<bool>,
    name: Option<LocalizedName>,
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
pub(crate) struct LocalizedName {
    pub(crate) en: Option<String>,
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
    invention: Option<InventionRecord>,
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

/// An invention activity. Its numbers are kept as written, so a bad one is named by its field.
#[derive(Deserialize)]
struct InventionRecord {
    #[serde(default)]
    materials: Vec<InventionMaterialRecord>,
    #[serde(default)]
    products: Vec<InventionProductRecord>,
    #[serde(default)]
    skills: Vec<SkillRecord>,
}

#[derive(Deserialize)]
struct InventionMaterialRecord {
    #[serde(rename = "typeID")]
    type_id: TypeId,
    quantity: Number,
}

#[derive(Deserialize)]
struct InventionProductRecord {
    #[serde(rename = "typeID")]
    type_id: TypeId,
    /// Absent for some unpublished blueprints.
    probability: Option<Number>,
    quantity: Number,
}

#[derive(Deserialize)]
struct SkillRecord {
    #[serde(rename = "typeID")]
    type_id: TypeId,
    level: Number,
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
            name: record.name.and_then(|name| name.en),
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

/// The invention activities of the blueprints, in file order. An activity with no products is not
/// kept. A skill must be in `types.jsonl`, because its name is kept with the activity.
fn read_inventions(
    types: &[TypeRow],
    blueprints: &[(usize, BlueprintRecord)],
) -> Result<Vec<InventionActivity>, SdeError> {
    let names: BTreeMap<TypeId, &str> = types
        .iter()
        .map(|row| (row.id, row.name.as_str()))
        .collect();
    let mut activities = Vec::new();
    for (line, blueprint) in blueprints {
        let line = *line;
        let Some(record) = &blueprint.activities.invention else {
            continue;
        };
        if record.products.is_empty() {
            continue;
        }
        // An activity with a product that has no chance cannot be costed, so it is not kept.
        let Some(chances) = record
            .products
            .iter()
            .map(|product| product.probability.as_ref())
            .collect::<Option<Vec<_>>>()
        else {
            continue;
        };
        let mut products = Vec::with_capacity(record.products.len());
        for (product, chance) in record.products.iter().zip(chances) {
            products.push(InventionProduct {
                type_id: product.type_id,
                probability: probability(chance, line)?,
                quantity: whole::<u32>(&product.quantity, |value| *value >= 1)
                    .ok_or_else(|| bad_whole(line, "quantity", &product.quantity))?,
            });
        }
        let mut materials = Vec::with_capacity(record.materials.len());
        for material in &record.materials {
            let quantity = whole::<u64>(&material.quantity, |value| *value >= 1)
                .ok_or_else(|| bad_whole(line, "quantity", &material.quantity))?;
            materials.push((material.type_id, quantity));
        }
        let mut skills = Vec::with_capacity(record.skills.len());
        for skill in &record.skills {
            let name = names
                .get(&skill.type_id)
                .ok_or_else(|| bad_reference(BLUEPRINTS_FILE, line, "typeID", skill.type_id))?;
            let level = whole::<u8>(&skill.level, |value| *value <= 5)
                .ok_or_else(|| bad_whole(line, "level", &skill.level))?;
            skills.push(SkillRequirement {
                type_id: skill.type_id,
                name: (*name).to_owned(),
                level,
            });
        }
        activities.push(InventionActivity {
            blueprint: required(blueprint.key, BLUEPRINTS_FILE, line, "_key")?,
            products,
            materials,
            skills,
        });
    }
    Ok(activities)
}

/// A whole number from its literal text, when `valid` accepts it. Anything else is `None`.
fn whole<T: FromStr>(number: &Number, valid: impl FnOnce(&T) -> bool) -> Option<T> {
    number.to_string().parse::<T>().ok().filter(valid)
}

/// A probability in parts per million, from its literal text: `0.34` is 340,000. Zero, more than
/// six decimals and values above 1 are refused.
fn probability(number: &Number, line: usize) -> Result<Rate, SdeError> {
    Rate::parse_fraction(&number.to_string())
        .ok()
        .filter(|rate| rate.ppm() >= 1)
        .ok_or_else(|| bad_whole(line, "probability", number))
}

fn bad_whole(line: usize, field: &str, number: &Number) -> SdeError {
    SdeError::BadNumber {
        file: BLUEPRINTS_FILE.to_owned(),
        line,
        field: field.to_owned(),
        text: number.to_string(),
    }
}

/// The regions, solar systems and stargates of the download, checked against each other.
struct MapData {
    regions: Vec<RegionRow>,
    systems: Vec<SystemRow>,
    gates: Vec<GateRow>,
}

struct RegionRow {
    id: u32,
    name: String,
}

struct SystemRow {
    id: u32,
    name: String,
    region_id: u32,
    /// Security status in millionths, as the text of the download gives it.
    security_millionths: i32,
}

struct GateRow {
    id: u32,
    from_system: u32,
    to_system: u32,
}

#[derive(Deserialize)]
struct RegionRecord {
    #[serde(rename = "_key")]
    key: Option<u32>,
    name: Option<LocalizedName>,
}

#[derive(Deserialize)]
struct SystemRecord {
    #[serde(rename = "_key")]
    key: Option<u32>,
    name: Option<LocalizedName>,
    #[serde(rename = "regionID")]
    region_id: Option<u32>,
    #[serde(rename = "securityStatus")]
    security_status: Option<Number>,
}

#[derive(Deserialize)]
struct StargateRecord {
    #[serde(rename = "_key")]
    key: Option<u32>,
    #[serde(rename = "solarSystemID")]
    system_id: Option<u32>,
    destination: Option<StargateEnd>,
}

#[derive(Deserialize)]
struct StargateEnd {
    #[serde(rename = "solarSystemID")]
    system_id: Option<u32>,
}

impl MapData {
    /// Reads the regions, then the systems, then the stargates. A system must be in a listed region,
    /// and both ends of a stargate must be listed systems.
    fn read(source: &mut Source) -> Result<Self, SdeError> {
        let regions = read_rows(source, MAP_REGIONS_FILE, RegionRow::from_record)?;
        let region_ids: BTreeSet<u32> = regions.iter().map(|row| row.id).collect();
        let systems = parse_file::<SystemRecord>(source, MAP_SYSTEMS_FILE)?
            .into_iter()
            .map(|(line, record)| SystemRow::from_record(record, line, &region_ids))
            .collect::<Result<Vec<_>, _>>()?;
        let system_ids: BTreeSet<u32> = systems.iter().map(|row| row.id).collect();
        let gates = parse_file::<StargateRecord>(source, MAP_STARGATES_FILE)?
            .into_iter()
            .map(|(line, record)| GateRow::from_record(record, line, &system_ids))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            regions,
            systems,
            gates,
        })
    }
}

impl RegionRow {
    fn from_record(record: RegionRecord, line: usize) -> Result<Self, SdeError> {
        Ok(Self {
            id: required(record.key, MAP_REGIONS_FILE, line, "_key")?,
            name: english_name(record.name, MAP_REGIONS_FILE, line)?,
        })
    }
}

impl SystemRow {
    fn from_record(
        record: SystemRecord,
        line: usize,
        region_ids: &BTreeSet<u32>,
    ) -> Result<Self, SdeError> {
        let file = MAP_SYSTEMS_FILE;
        let id = required(record.key, file, line, "_key")?;
        let name = english_name(record.name, file, line)?;
        let region_id = required(record.region_id, file, line, "regionID")?;
        if !region_ids.contains(&region_id) {
            return Err(bad_reference(file, line, "regionID", region_id));
        }
        let text = required(record.security_status, file, line, "securityStatus")?.to_string();
        let security_millionths =
            security_millionths(&text).ok_or_else(|| SdeError::BadNumber {
                file: file.to_owned(),
                line,
                field: "securityStatus".to_owned(),
                text,
            })?;
        Ok(Self {
            id,
            name,
            region_id,
            security_millionths,
        })
    }
}

impl GateRow {
    fn from_record(
        record: StargateRecord,
        line: usize,
        system_ids: &BTreeSet<u32>,
    ) -> Result<Self, SdeError> {
        let file = MAP_STARGATES_FILE;
        let row = Self {
            id: required(record.key, file, line, "_key")?,
            from_system: required(record.system_id, file, line, "solarSystemID")?,
            to_system: required(
                record.destination.and_then(|end| end.system_id),
                file,
                line,
                "destination.solarSystemID",
            )?,
        };
        for (field, system) in [
            ("solarSystemID", row.from_system),
            ("destination.solarSystemID", row.to_system),
        ] {
            if !system_ids.contains(&system) {
                return Err(bad_reference(file, line, field, system));
            }
        }
        Ok(row)
    }
}

pub(crate) fn english_name(
    name: Option<LocalizedName>,
    file: &str,
    line: usize,
) -> Result<String, SdeError> {
    name.and_then(|name| name.en)
        .ok_or_else(|| SdeError::MissingEnglishName {
            file: file.to_owned(),
            line,
        })
}

pub(crate) fn bad_reference(file: &str, line: usize, field: &str, id: u32) -> SdeError {
    SdeError::BadReference {
        file: file.to_owned(),
        line,
        field: field.to_owned(),
        id,
    }
}

/// A security status as millionths, from its text: `0.945913` is 945,913 and `-9e-05` is -90.
/// `None` when a non-zero digit lies past the sixth decimal place, when the value is outside
/// -1,000,000..=1,000,000, or when the text is not a number. Trailing zeros are fine.
fn security_millionths(text: &str) -> Option<i32> {
    let (mantissa, exponent) = match text.split_once(['e', 'E']) {
        Some((mantissa, exponent)) => (mantissa, exponent.parse::<i32>().ok()?),
        None => (text, 0),
    };
    let (negative, unsigned) = match mantissa.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, mantissa),
    };
    let (whole, fraction) = unsigned.split_once('.').unwrap_or((unsigned, ""));
    let digits = |part: &str| part.bytes().all(|byte| byte.is_ascii_digit());
    if whole.is_empty() || !digits(whole) || !digits(fraction) {
        return None;
    }
    // Trailing zeros of the fraction add no value, so they are dropped before the digits are counted.
    let fraction = fraction.trim_end_matches('0');
    // The value is all the digits times 10^(exponent - fraction length); times 10^6 gives millionths.
    let scale = exponent
        .checked_sub(i32::try_from(fraction.len()).ok()?)?
        .checked_add(6)?;
    let all_digits: i128 = format!("{whole}{fraction}").parse().ok()?;
    let power = 10_i128.checked_pow(scale.unsigned_abs())?;
    let millionths = if scale >= 0 {
        all_digits.checked_mul(power)?
    } else if all_digits % power == 0 {
        all_digits / power
    } else {
        return None;
    };
    let signed = if negative {
        millionths.checked_neg()?
    } else {
        millionths
    };
    if !(-1_000_000..=1_000_000).contains(&signed) {
        return None;
    }
    i32::try_from(signed).ok()
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

pub(crate) fn parse_file<R: DeserializeOwned>(
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

pub(crate) fn required<T>(
    value: Option<T>,
    file: &str,
    line: usize,
    field: &str,
) -> Result<T, SdeError> {
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

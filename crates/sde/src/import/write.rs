use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use mop_core::volume::Volume;
use rusqlite::{Connection, Transaction, params};

use super::{Download, Facilities, MapData};
use crate::error::SdeError;
use crate::invention::InventionActivity;
use crate::schema::{CREATE_TABLES, SCHEMA_VERSION, activity_name};
use crate::stations::Station;

/// SQLite integers are signed 64-bit. A value that does not fit is refused.
fn sql_int(value: u64) -> Result<i64, SdeError> {
    match i64::try_from(value) {
        Ok(number) => Ok(number),
        Err(_) => Err(SdeError::TooLarge { value }),
    }
}

/// The failure to remove the half-written file, together with the error that stopped the import.
/// The file is also removed at the start of the next import.
pub(super) fn cleanup_failed(path: &Path, cleanup: &std::io::Error, error: &SdeError) -> SdeError {
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

pub(super) fn pending_path(db: &Path) -> PathBuf {
    let mut name = db.as_os_str().to_owned();
    name.push(".new");
    PathBuf::from(name)
}

pub(super) fn remove_if_exists(path: &Path) -> Result<(), SdeError> {
    try_remove(path).map_err(|source| SdeError::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// Removes a file; one that is not there is fine.
pub(super) fn try_remove(path: &Path) -> std::io::Result<()> {
    match fs::remove_file(path) {
        Err(source) if source.kind() != ErrorKind::NotFound => Err(source),
        Ok(()) | Err(_) => Ok(()),
    }
}

pub(super) fn rename_over(from: &Path, to: &Path) -> Result<(), SdeError> {
    fs::rename(from, to).map_err(|source| SdeError::Io {
        path: from.to_path_buf(),
        source,
    })
}

/// Writes the whole database to `path` in one transaction, then closes it.
pub(super) fn write_pending(path: &Path, download: &Download) -> Result<(), SdeError> {
    let mut conn = Connection::open(path)?;
    let tx = conn.transaction()?;
    tx.execute_batch(CREATE_TABLES)?;
    write_rows(&tx, download)?;
    tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    tx.commit()?;
    Ok(())
}

/// The categories and groups, each with its name lowercased for the case-ignoring lookups.
fn write_categories_and_groups(tx: &Transaction<'_>, download: &Download) -> Result<(), SdeError> {
    let mut categories = tx.prepare(
        "INSERT INTO categories (category_id, published, name, name_lower) VALUES (?1, ?2, ?3, ?4)",
    )?;
    for row in &download.categories {
        categories.execute(params![
            row.id,
            row.published,
            row.name,
            row.name.as_ref().map(|name| name.to_lowercase())
        ])?;
    }
    let mut groups = tx.prepare(
        "INSERT INTO groups (group_id, category_id, published, name, name_lower)
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )?;
    for row in &download.groups {
        groups.execute(params![
            row.id,
            row.category_id,
            row.published,
            row.name,
            row.name.as_ref().map(|name| name.to_lowercase())
        ])?;
    }
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
    write_categories_and_groups(tx, download)?;
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
    write_inventions(tx, &download.inventions)?;
    write_facilities(tx, &download.facilities)?;
    write_attributes(tx, download)?;
    write_map(tx, &download.map)?;
    write_stations(tx, &download.stations)?;
    Ok(())
}

/// The invention activities: their products, materials and skills, each in file order.
fn write_inventions(
    tx: &Transaction<'_>,
    inventions: &[InventionActivity],
) -> Result<(), SdeError> {
    let mut invention_products = tx.prepare(
        "INSERT INTO invention_products (blueprint_id, position, product_id, probability_ppm,
                                         quantity)
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )?;
    let mut invention_materials = tx.prepare(
        "INSERT INTO invention_materials (blueprint_id, position, type_id, quantity)
         VALUES (?1, ?2, ?3, ?4)",
    )?;
    let mut invention_skills = tx.prepare(
        "INSERT INTO invention_skills (blueprint_id, position, type_id, level)
         VALUES (?1, ?2, ?3, ?4)",
    )?;
    for activity in inventions {
        for (position, product) in (0_i64..).zip(&activity.products) {
            invention_products.execute(params![
                activity.blueprint,
                position,
                product.type_id,
                product.probability.ppm(),
                product.quantity
            ])?;
        }
        for (position, &(type_id, quantity)) in (0_i64..).zip(&activity.materials) {
            invention_materials.execute(params![
                activity.blueprint,
                position,
                type_id,
                sql_int(quantity)?
            ])?;
        }
        for (position, skill) in (0_i64..).zip(&activity.skills) {
            invention_skills.execute(params![
                activity.blueprint,
                position,
                skill.type_id,
                skill.level
            ])?;
        }
    }
    Ok(())
}

fn write_attributes(tx: &Transaction<'_>, download: &Download) -> Result<(), SdeError> {
    let mut decryptors = tx.prepare(
        "INSERT INTO decryptors (type_id, chance_hundredths, me, te, runs)
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )?;
    for row in &download.decryptors {
        decryptors.execute(params![
            row.type_id,
            row.chance_hundredths,
            row.me,
            row.te,
            row.runs
        ])?;
    }
    let mut job_costs = tx.prepare(
        "INSERT INTO structure_job_costs (type_id, multiplier_hundredths) VALUES (?1, ?2)",
    )?;
    for &(type_id, multiplier) in &download.job_costs {
        job_costs.execute(params![type_id, multiplier])?;
    }
    Ok(())
}

fn write_stations(tx: &Transaction<'_>, stations: &[Station]) -> Result<(), SdeError> {
    let mut rows = tx.prepare("INSERT INTO stations (id, name, system_id) VALUES (?1, ?2, ?3)")?;
    for station in stations {
        rows.execute(params![station.id, station.name, station.system_id])?;
    }
    Ok(())
}

fn write_map(tx: &Transaction<'_>, map: &MapData) -> Result<(), SdeError> {
    let mut regions = tx.prepare("INSERT INTO regions (region_id, name) VALUES (?1, ?2)")?;
    for row in &map.regions {
        regions.execute(params![row.id, row.name])?;
    }
    let mut systems = tx.prepare(
        "INSERT INTO systems (system_id, name, name_lower, region_id, security_millionths)
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )?;
    for row in &map.systems {
        systems.execute(params![
            row.id,
            row.name,
            row.name.to_lowercase(),
            row.region_id,
            row.security_millionths
        ])?;
    }
    let mut gates =
        tx.prepare("INSERT INTO stargates (gate_id, from_system, to_system) VALUES (?1, ?2, ?3)")?;
    for row in &map.gates {
        gates.execute(params![row.id, row.from_system, row.to_system])?;
    }
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
        "INSERT INTO rigs (type_id, activity, bonus_tenths, rig_size, high_tenths,
                           low_tenths, null_tenths)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )?;
    let mut rig_filters =
        tx.prepare("INSERT INTO rig_filters (rig_id, filter_id) VALUES (?1, ?2)")?;
    for row in &facilities.rigs {
        rigs.execute(params![
            row.type_id,
            activity_name(row.activity),
            row.bonus_tenths,
            row.rig_size,
            row.high_tenths,
            row.low_tenths,
            row.null_tenths
        ])?;
        for filter_id in &row.filter_ids {
            rig_filters.execute(params![row.type_id, filter_id])?;
        }
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

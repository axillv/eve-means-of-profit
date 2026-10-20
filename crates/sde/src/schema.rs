//! The database layout. Written in one transaction by the import, read back by the queries.
//!
//! No migrations: a database with another version is imported again.

use profit_core::industry::Activity;

/// Written to `PRAGMA user_version`. A database with any other value is not read.
pub const SCHEMA_VERSION: u32 = 5;

/// The text stored for an activity in `recipes.activity`.
pub(crate) fn activity_name(activity: Activity) -> &'static str {
    match activity {
        Activity::Manufacturing => "manufacturing",
        Activity::Reaction => "reaction",
    }
}

pub(crate) fn activity_from_name(name: &str) -> Option<Activity> {
    match name {
        "manufacturing" => Some(Activity::Manufacturing),
        "reaction" => Some(Activity::Reaction),
        _ => None,
    }
}

/// The tables. The import creates them; the queries read them.
pub(crate) const CREATE_TABLES: &str = "
    CREATE TABLE meta (
        id INTEGER PRIMARY KEY CHECK (id = 1),
        schema_version INTEGER NOT NULL,
        build_number INTEGER NOT NULL,
        release_date TEXT NOT NULL,
        imported_at INTEGER NOT NULL
    );
    CREATE TABLE categories (
        category_id INTEGER PRIMARY KEY,
        published INTEGER NOT NULL
    );
    CREATE TABLE groups (
        group_id INTEGER PRIMARY KEY,
        category_id INTEGER NOT NULL,
        published INTEGER NOT NULL
    );
    CREATE TABLE types (
        type_id INTEGER PRIMARY KEY,
        name TEXT NOT NULL,
        name_lower TEXT NOT NULL,
        group_id INTEGER NOT NULL,
        published INTEGER NOT NULL,
        volume INTEGER,
        packaged_volume INTEGER,
        portion_size INTEGER NOT NULL
    );
    CREATE INDEX types_name_lower ON types (name_lower);
    CREATE TABLE recipes (
        product_id INTEGER PRIMARY KEY,
        blueprint_id INTEGER NOT NULL,
        activity TEXT NOT NULL,
        output_per_run INTEGER NOT NULL,
        max_runs_per_job INTEGER NOT NULL,
        made_by_invention INTEGER NOT NULL
    );
    CREATE TABLE recipe_materials (
        product_id INTEGER NOT NULL REFERENCES recipes (product_id),
        position INTEGER NOT NULL,
        type_id INTEGER NOT NULL,
        quantity INTEGER NOT NULL,
        PRIMARY KEY (product_id, position)
    );
    CREATE TABLE structures (
        type_id INTEGER PRIMARY KEY,
        rig_size INTEGER NOT NULL,
        manufacturing_bonus_hundredths INTEGER
    );
    CREATE TABLE rigs (
        type_id INTEGER PRIMARY KEY,
        activity TEXT NOT NULL,
        bonus_tenths INTEGER NOT NULL,
        rig_size INTEGER NOT NULL,
        high_tenths INTEGER,
        low_tenths INTEGER NOT NULL,
        null_tenths INTEGER NOT NULL
    );
    CREATE TABLE structure_job_costs (
        type_id INTEGER PRIMARY KEY,
        multiplier_hundredths INTEGER NOT NULL CHECK (multiplier_hundredths BETWEEN 0 AND 100)
    );
    CREATE TABLE decryptors (
        type_id INTEGER PRIMARY KEY,
        chance_hundredths INTEGER NOT NULL CHECK (chance_hundredths >= 0),
        me INTEGER NOT NULL,
        te INTEGER NOT NULL,
        runs INTEGER NOT NULL
    );
    CREATE TABLE rig_filters (
        rig_id INTEGER NOT NULL REFERENCES rigs (type_id),
        filter_id INTEGER
    );
    CREATE TABLE rig_fits (
        rig_id INTEGER NOT NULL REFERENCES rigs (type_id),
        group_id INTEGER NOT NULL,
        PRIMARY KEY (rig_id, group_id)
    );
    CREATE TABLE filter_categories (
        filter_id INTEGER NOT NULL,
        category_id INTEGER NOT NULL,
        PRIMARY KEY (filter_id, category_id)
    );
    CREATE TABLE filter_groups (
        filter_id INTEGER NOT NULL,
        group_id INTEGER NOT NULL,
        PRIMARY KEY (filter_id, group_id)
    );
    CREATE TABLE regions (
        region_id INTEGER PRIMARY KEY,
        name TEXT NOT NULL
    );
    CREATE TABLE systems (
        system_id INTEGER PRIMARY KEY,
        name TEXT NOT NULL,
        name_lower TEXT NOT NULL,
        region_id INTEGER NOT NULL REFERENCES regions (region_id),
        security_millionths INTEGER NOT NULL
    );
    CREATE INDEX systems_name_lower ON systems (name_lower);
    CREATE TABLE stargates (
        gate_id INTEGER PRIMARY KEY,
        from_system INTEGER NOT NULL REFERENCES systems (system_id),
        to_system INTEGER NOT NULL REFERENCES systems (system_id)
    );
    CREATE INDEX stargates_from_system ON stargates (from_system);
    CREATE TABLE stations (
        id INTEGER PRIMARY KEY,
        name TEXT NOT NULL,
        system_id INTEGER NOT NULL
    );
";

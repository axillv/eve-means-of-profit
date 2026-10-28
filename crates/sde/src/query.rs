//! Queries against an imported database: items, names, search and recipes.

use std::collections::BTreeMap;
use std::path::Path;

use mop_core::industry::{Recipe, RecipeBook, TypeId};
use mop_core::volume::Volume;
use rusqlite::types::Type;
use rusqlite::{Connection, OpenFlags, OptionalExtension, Row, params};

use crate::error::SdeError;
use crate::schema::{SCHEMA_VERSION, activity_from_name};

/// An open, read-only view of an imported database.
pub struct Sde {
    pub(crate) conn: Connection,
}

/// The build the database was imported from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdeBuild {
    pub number: u64,
    pub release_date: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub type_id: TypeId,
    pub name: String,
    pub group_id: u32,
    pub category_id: u32,
    pub published: bool,
    pub volume: Option<Volume>,
    pub packaged_volume: Option<Volume>,
    pub portion_size: u64,
}

impl Item {
    /// The volume the item takes up when hauled: its packaged volume if it has one, else its volume.
    pub fn haul_volume(&self) -> Option<Volume> {
        self.packaged_volume.or(self.volume)
    }
}

impl Sde {
    /// Opens the database read-only. A missing, empty or other-version database is `NotImported`.
    pub fn open(db: &Path) -> Result<Self, SdeError> {
        if !db.is_file() {
            return Err(SdeError::NotImported);
        }
        let conn = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let version: u32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version != SCHEMA_VERSION {
            return Err(SdeError::NotImported);
        }
        Ok(Self { conn })
    }

    pub fn build(&self) -> Result<SdeBuild, SdeError> {
        let build =
            self.conn
                .query_row("SELECT build_number, release_date FROM meta", [], |row| {
                    Ok(SdeBuild {
                        number: unsigned(0, row.get(0)?)?,
                        release_date: row.get(1)?,
                    })
                })?;
        Ok(build)
    }

    pub fn item(&self, id: TypeId) -> Result<Option<Item>, SdeError> {
        let item = self
            .conn
            .query_row(
                "SELECT t.type_id, t.name, t.group_id, g.category_id, t.published,
                        t.volume, t.packaged_volume, t.portion_size
                 FROM types t JOIN groups g ON g.group_id = t.group_id
                 WHERE t.type_id = ?1",
                params![id],
                item_from_row,
            )
            .optional()?;
        Ok(item)
    }

    /// The type with exactly this name, ignoring case and surrounding spaces. A published type wins
    /// over an unpublished one with the same name; then the lowest id.
    pub fn find_exact(&self, name: &str) -> Result<Option<TypeId>, SdeError> {
        let key = name.trim().to_lowercase();
        if key.is_empty() {
            return Ok(None);
        }
        let id = self
            .conn
            .query_row(
                "SELECT type_id FROM types WHERE name_lower = ?1
                 ORDER BY published DESC, type_id LIMIT 1",
                params![key],
                |row| row.get(0),
            )
            .optional()?;
        Ok(id)
    }

    /// The English name of the item group `id`, if the game data has one.
    pub fn group_name(&self, id: u32) -> Result<Option<String>, SdeError> {
        self.name_of("groups", "group_id", id)
    }

    /// The English name of the item category `id`, if the game data has one.
    pub fn category_name(&self, id: u32) -> Result<Option<String>, SdeError> {
        self.name_of("categories", "category_id", id)
    }

    /// The item group with exactly this name, ignoring case and surrounding spaces. A published
    /// group wins over an unpublished one with the same name; then the lowest id.
    pub fn find_group(&self, name: &str) -> Result<Option<u32>, SdeError> {
        self.find_id_by_name("groups", "group_id", name)
    }

    /// The item category with exactly this name, ignoring case and surrounding spaces. A published
    /// category wins over an unpublished one with the same name; then the lowest id.
    pub fn find_category(&self, name: &str) -> Result<Option<u32>, SdeError> {
        self.find_id_by_name("categories", "category_id", name)
    }

    /// The name stored for the row of `table` whose `key` column is `id`.
    fn name_of(&self, table: &str, key: &str, id: u32) -> Result<Option<String>, SdeError> {
        let name = self
            .conn
            .query_row(
                &format!("SELECT name FROM {table} WHERE {key} = ?1"),
                params![id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?;
        Ok(name.flatten())
    }

    /// The id of the row of `table` whose name is `name`, ignoring case and surrounding spaces.
    fn find_id_by_name(&self, table: &str, key: &str, name: &str) -> Result<Option<u32>, SdeError> {
        let wanted = name.trim().to_lowercase();
        if wanted.is_empty() {
            return Ok(None);
        }
        let id = self
            .conn
            .query_row(
                &format!(
                    "SELECT {key} FROM {table} WHERE name_lower = ?1
                     ORDER BY published DESC, {key} LIMIT 1"
                ),
                params![wanted],
                |row| row.get(0),
            )
            .optional()?;
        Ok(id)
    }

    /// Published types whose name contains `query`, best match first, at most `limit` of them.
    ///
    /// Rank: the whole name, then a name that starts with the query, then a later word that does
    /// (words split at spaces and `-`), then any other match. Ties: the shorter name, then the name,
    /// then the id.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<TypeId>, SdeError> {
        self.ranked_ids(
            "SELECT type_id, name, name_lower FROM types
             WHERE published = 1 AND instr(name_lower, ?1) > 0",
            query,
            limit,
        )
    }

    /// The ids that `sql` selects for `query`, best match first, at most `limit` of them. `sql`
    /// selects `(id, name, name_lower)` rows that contain the query as `?1`, already trimmed and
    /// lower case. The rank and the ties are the same for every search.
    pub(crate) fn ranked_ids(
        &self,
        sql: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<TypeId>, SdeError> {
        let needle = query.trim().to_lowercase();
        if needle.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let mut statement = self.conn.prepare(sql)?;
        let mut ranked = statement
            .query_map(params![needle], |row| {
                Ok((
                    row.get::<_, TypeId>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .map(|row| {
                row.map(|(id, name, lower)| (rank(&lower, &needle), name.chars().count(), name, id))
            })
            .collect::<rusqlite::Result<Vec<_>>>()?;
        ranked.sort();
        Ok(ranked
            .into_iter()
            .take(limit)
            .map(|(_, _, _, id)| id)
            .collect())
    }

    /// Every imported recipe, with its materials in the order the game lists them.
    pub fn recipe_book(&self) -> Result<RecipeBook, SdeError> {
        let mut materials: BTreeMap<TypeId, Vec<(TypeId, u64)>> = BTreeMap::new();
        let mut rows = self.conn.prepare(
            "SELECT product_id, type_id, quantity FROM recipe_materials
             ORDER BY product_id, position",
        )?;
        for row in rows.query_map([], |row| {
            Ok((
                row.get::<_, TypeId>(0)?,
                row.get::<_, TypeId>(1)?,
                unsigned(2, row.get(2)?)?,
            ))
        })? {
            let (product, type_id, quantity) = row?;
            materials
                .entry(product)
                .or_default()
                .push((type_id, quantity));
        }

        let mut recipes = self.conn.prepare(
            "SELECT product_id, activity, output_per_run, max_runs_per_job, made_by_invention
             FROM recipes",
        )?;
        let recipes = recipes
            .query_map([], |row| {
                let activity: String = row.get(1)?;
                let activity = activity_from_name(&activity)
                    .ok_or_else(|| rusqlite::Error::InvalidColumnType(1, activity, Type::Text))?;
                Ok((
                    row.get::<_, TypeId>(0)?,
                    Recipe {
                        activity,
                        output_per_run: unsigned(2, row.get(2)?)?,
                        max_runs_per_job: unsigned(3, row.get(3)?)?,
                        materials: Vec::new(),
                        made_by_invention: row.get(4)?,
                    },
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let mut book = RecipeBook::new();
        for (product, mut recipe) in recipes {
            recipe.materials = materials.remove(&product).unwrap_or_default();
            book.insert(product, recipe);
        }
        Ok(book)
    }

    /// Every imported recipe as `(product, blueprint)`, by product, one row each.
    pub fn product_blueprints(&self) -> Result<Vec<(TypeId, TypeId)>, SdeError> {
        let mut statement = self
            .conn
            .prepare("SELECT product_id, blueprint_id FROM recipes ORDER BY product_id")?;
        let pairs = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(pairs)
    }
}

fn item_from_row(row: &Row<'_>) -> rusqlite::Result<Item> {
    Ok(Item {
        type_id: row.get(0)?,
        name: row.get(1)?,
        group_id: row.get(2)?,
        category_id: row.get(3)?,
        published: row.get(4)?,
        volume: volume_column(row, 5)?,
        packaged_volume: volume_column(row, 6)?,
        portion_size: unsigned(7, row.get(7)?)?,
    })
}

fn volume_column(row: &Row<'_>, column: usize) -> rusqlite::Result<Option<Volume>> {
    Ok(row
        .get::<_, Option<i64>>(column)?
        .map(Volume::from_ten_thousandths))
}

/// A stored integer as an unsigned number. A negative value is a corrupt database.
fn unsigned(column: usize, value: i64) -> rusqlite::Result<u64> {
    match u64::try_from(value) {
        Ok(number) => Ok(number),
        Err(_) => Err(rusqlite::Error::IntegralValueOutOfRange(column, value)),
    }
}

/// 0 for the whole name, 1 for a name that starts with the query, 2 for a later word that starts
/// with it, 3 for any other match.
fn rank(lower: &str, needle: &str) -> u8 {
    if lower == needle {
        0
    } else if lower.starts_with(needle) {
        1
    } else if later_word_starts_with(lower, needle) {
        2
    } else {
        3
    }
}

fn later_word_starts_with(lower: &str, needle: &str) -> bool {
    lower.char_indices().any(|(index, c)| {
        (c == ' ' || c == '-')
            && lower
                .get(index + c.len_utf8()..)
                .is_some_and(|rest| rest.starts_with(needle))
    })
}

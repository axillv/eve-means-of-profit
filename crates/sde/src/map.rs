//! The map: solar systems, the stargates between them, and the jumps a route takes.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;

use rusqlite::{OptionalExtension, params};

use crate::error::SdeError;
use crate::query::Sde;

/// High sec starts at a security status of 0.45, in millionths. This is the community rule; it is
/// not checked against the game's own rounding of the displayed value.
const HIGH_SEC_MILLIONTHS: i32 = 450_000;

/// A solar system, with the name of its region.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SolarSystem {
    pub id: u32,
    pub name: String,
    pub region_id: u32,
    pub region_name: String,
    pub security_millionths: i32,
}

impl SolarSystem {
    pub fn is_high_sec(&self) -> bool {
        is_high_sec(self.security_millionths)
    }
}

/// Which gates a route may use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteKind {
    /// Any system on the way.
    Shortest,
    /// High sec systems only, both ends included.
    HighSecOnly,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MapError {
    /// A system id that the map does not know.
    UnknownSystem(u32),
}

impl fmt::Display for MapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownSystem(id) => write!(f, "system {id} is not in the map"),
        }
    }
}

impl std::error::Error for MapError {}

/// The systems and the stargates between them, in memory. Load it once with [`Sde::star_map`].
pub struct StarMap {
    security: BTreeMap<u32, i32>,
    gates: BTreeMap<u32, Vec<u32>>,
}

impl StarMap {
    /// The fewest gate jumps from one system to another. `None` when no route exists, or when a
    /// high sec route is asked for and either end is not high sec.
    pub fn jumps(&self, from: u32, to: u32, kind: RouteKind) -> Result<Option<u32>, MapError> {
        let from_security = self.security_of(from)?;
        let to_security = self.security_of(to)?;
        let high_only = kind == RouteKind::HighSecOnly;
        if high_only && !(is_high_sec(from_security) && is_high_sec(to_security)) {
            return Ok(None);
        }
        Ok(self.fewest_jumps(from, to, high_only))
    }

    fn security_of(&self, id: u32) -> Result<i32, MapError> {
        self.security
            .get(&id)
            .copied()
            .ok_or(MapError::UnknownSystem(id))
    }

    /// Breadth-first search over the gates. With `high_only`, only high sec systems are entered.
    fn fewest_jumps(&self, from: u32, to: u32, high_only: bool) -> Option<u32> {
        let mut seen = BTreeSet::from([from]);
        let mut queue = VecDeque::from([(from, 0_u32)]);
        while let Some((system, jumps)) = queue.pop_front() {
            if system == to {
                return Some(jumps);
            }
            // A route never visits more systems than the map has, so this cannot overflow.
            let next_jumps = jumps.checked_add(1)?;
            for &next in self.gates.get(&system).into_iter().flatten() {
                if high_only && !self.is_high_sec_system(next) {
                    continue;
                }
                if seen.insert(next) {
                    queue.push_back((next, next_jumps));
                }
            }
        }
        None
    }

    /// Gates only join listed systems (the import checks it), so an unknown id is not high sec.
    fn is_high_sec_system(&self, id: u32) -> bool {
        self.security
            .get(&id)
            .is_some_and(|&security| is_high_sec(security))
    }
}

fn is_high_sec(security_millionths: i32) -> bool {
    security_millionths >= HIGH_SEC_MILLIONTHS
}

impl Sde {
    /// The system with this id, or `None` when there is none.
    pub fn system(&self, id: u32) -> Result<Option<SolarSystem>, SdeError> {
        let system = self
            .conn
            .query_row(
                "SELECT s.system_id, s.name, s.region_id, r.name, s.security_millionths
                 FROM systems s JOIN regions r ON r.region_id = s.region_id
                 WHERE s.system_id = ?1",
                params![id],
                |row| {
                    Ok(SolarSystem {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        region_id: row.get(2)?,
                        region_name: row.get(3)?,
                        security_millionths: row.get(4)?,
                    })
                },
            )
            .optional()?;
        Ok(system)
    }

    /// The system with exactly this name, ignoring case and surrounding spaces.
    pub fn find_system(&self, name: &str) -> Result<Option<u32>, SdeError> {
        let key = name.trim().to_lowercase();
        if key.is_empty() {
            return Ok(None);
        }
        let id = self
            .conn
            .query_row(
                "SELECT system_id FROM systems WHERE name_lower = ?1
                 ORDER BY system_id LIMIT 1",
                params![key],
                |row| row.get(0),
            )
            .optional()?;
        Ok(id)
    }

    /// Systems whose name contains `query`, best match first, at most `limit` of them. Ranked as
    /// [`Sde::search`] ranks types.
    pub fn search_systems(&self, query: &str, limit: usize) -> Result<Vec<u32>, SdeError> {
        self.ranked_ids(
            "SELECT system_id, name, name_lower FROM systems
             WHERE instr(name_lower, ?1) > 0",
            query,
            limit,
        )
    }

    /// Loads the security of every system and every gate into memory.
    pub fn star_map(&self) -> Result<StarMap, SdeError> {
        let mut security = BTreeMap::new();
        let mut systems = self
            .conn
            .prepare("SELECT system_id, security_millionths FROM systems")?;
        for row in
            systems.query_map([], |row| Ok((row.get::<_, u32>(0)?, row.get::<_, i32>(1)?)))?
        {
            let (id, millionths) = row?;
            security.insert(id, millionths);
        }
        let mut gates: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
        let mut stargates = self
            .conn
            .prepare("SELECT from_system, to_system FROM stargates ORDER BY gate_id")?;
        for row in
            stargates.query_map([], |row| Ok((row.get::<_, u32>(0)?, row.get::<_, u32>(1)?)))?
        {
            let (from, to) = row?;
            gates.entry(from).or_default().push(to);
        }
        Ok(StarMap { security, gates })
    }
}

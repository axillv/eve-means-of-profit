//! The names of the NPC stations, from the game data. A name is the orbit, then the owner
//! corporation, then the operation when the station uses it: `Jita IV - Moon 4 - Caldari Navy
//! Assembly Plant`.

use std::collections::{BTreeMap, BTreeSet};

use rusqlite::{OptionalExtension, params};
use serde::Deserialize;

use crate::error::SdeError;
use crate::import::{LocalizedName, Source, bad_reference, english_name, parse_file, required};
use crate::query::Sde;

const STATIONS_FILE: &str = "npcStations.jsonl";
const CORPORATIONS_FILE: &str = "npcCorporations.jsonl";
const OPERATIONS_FILE: &str = "stationOperations.jsonl";
const MOONS_FILE: &str = "mapMoons.jsonl";
const PLANETS_FILE: &str = "mapPlanets.jsonl";
const STARS_FILE: &str = "mapStars.jsonl";

/// The Roman numerals a planet's number can be written with: 1 to 50.
const NUMERALS: [(u32, &str); 7] = [
    (50, "L"),
    (40, "XL"),
    (10, "X"),
    (9, "IX"),
    (5, "V"),
    (4, "IV"),
    (1, "I"),
];

/// An NPC station, with its English name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Station {
    pub id: i64,
    pub name: String,
    pub system_id: u32,
}

#[derive(Deserialize)]
struct StationRecord {
    #[serde(rename = "_key")]
    key: Option<i64>,
    #[serde(rename = "solarSystemID")]
    system_id: Option<u32>,
    #[serde(rename = "orbitID")]
    orbit_id: Option<u32>,
    #[serde(rename = "ownerID")]
    owner_id: Option<u32>,
    #[serde(rename = "operationID")]
    operation_id: Option<u32>,
    #[serde(rename = "useOperationName")]
    use_operation_name: Option<bool>,
}

#[derive(serde::Deserialize)]
struct CorporationRecord {
    #[serde(rename = "_key")]
    key: Option<u32>,
    name: Option<LocalizedName>,
}

#[derive(serde::Deserialize)]
struct OperationRecord {
    #[serde(rename = "_key")]
    key: Option<u32>,
    #[serde(rename = "operationName")]
    name: Option<LocalizedName>,
}

#[derive(serde::Deserialize)]
struct MoonRecord {
    #[serde(rename = "_key")]
    key: Option<u32>,
    #[serde(rename = "orbitID")]
    planet_id: Option<u32>,
    #[serde(rename = "orbitIndex")]
    index: Option<u32>,
    #[serde(rename = "uniqueName")]
    unique_name: Option<LocalizedName>,
}

#[derive(serde::Deserialize)]
struct PlanetRecord {
    #[serde(rename = "_key")]
    key: Option<u32>,
    #[serde(rename = "celestialIndex")]
    index: Option<u32>,
    #[serde(rename = "solarSystemID")]
    system_id: Option<u32>,
    #[serde(rename = "uniqueName")]
    unique_name: Option<LocalizedName>,
}

#[derive(serde::Deserialize)]
struct StarRecord {
    #[serde(rename = "_key")]
    key: Option<u32>,
    #[serde(rename = "solarSystemID")]
    system_id: Option<u32>,
}

/// A station record with its required fields checked.
struct Entry {
    line: usize,
    id: i64,
    system_id: u32,
    orbit_id: u32,
    owner_id: u32,
    operation_id: u32,
    use_operation_name: bool,
}

impl Entry {
    fn from_record(record: &StationRecord, line: usize) -> Result<Self, SdeError> {
        let file = STATIONS_FILE;
        Ok(Self {
            line,
            id: required(record.key, file, line, "_key")?,
            system_id: required(record.system_id, file, line, "solarSystemID")?,
            orbit_id: required(record.orbit_id, file, line, "orbitID")?,
            owner_id: required(record.owner_id, file, line, "ownerID")?,
            operation_id: required(record.operation_id, file, line, "operationID")?,
            use_operation_name: required(
                record.use_operation_name,
                file,
                line,
                "useOperationName",
            )?,
        })
    }
}

/// A moon that a station orbits. Its planet is the orbit it goes around.
struct Moon {
    planet_id: u32,
    index: Option<u32>,
    unique_name: Option<String>,
    line: usize,
}

/// A planet that a station orbits, or that a moon a station orbits goes around.
struct Planet {
    system_id: u32,
    index: Option<u32>,
    unique_name: Option<String>,
    line: usize,
}

/// A star that a station orbits. Its name is the name of its system.
struct Star {
    system_id: u32,
    line: usize,
}

/// The orbits the stations use, with the names of the solar systems.
struct Orbits<'a> {
    moons: BTreeMap<u32, Moon>,
    planets: BTreeMap<u32, Planet>,
    stars: BTreeMap<u32, Star>,
    system_names: &'a BTreeMap<u32, String>,
}

/// The stations of the download, named. Only the orbits that a station uses are read into memory.
pub(crate) fn read(
    source: &mut Source,
    system_names: &BTreeMap<u32, String>,
) -> Result<Vec<Station>, SdeError> {
    let corporations = corporation_names(source)?;
    let operations = operation_names(source)?;
    let entries = parse_file::<StationRecord>(source, STATIONS_FILE)?
        .into_iter()
        .map(|(line, record)| Entry::from_record(&record, line))
        .collect::<Result<Vec<_>, _>>()?;
    let orbit_ids: BTreeSet<u32> = entries.iter().map(|entry| entry.orbit_id).collect();
    let moons = moons(source, &orbit_ids)?;
    let planet_ids: BTreeSet<u32> = orbit_ids
        .iter()
        .copied()
        .chain(moons.values().map(|moon| moon.planet_id))
        .collect();
    let orbits = Orbits {
        planets: planets(source, &planet_ids, system_names)?,
        stars: stars(source, &orbit_ids, system_names)?,
        moons,
        system_names,
    };

    let mut stations = Vec::with_capacity(entries.len());
    for entry in &entries {
        system_name(system_names, entry.system_id, STATIONS_FILE, entry.line)?;
        let corporation = corporations
            .get(&entry.owner_id)
            .ok_or_else(|| bad_reference(STATIONS_FILE, entry.line, "ownerID", entry.owner_id))?;
        let operation = operations.get(&entry.operation_id).ok_or_else(|| {
            bad_reference(STATIONS_FILE, entry.line, "operationID", entry.operation_id)
        })?;
        let orbit = orbits.name(entry.orbit_id, entry.line)?;
        let mut name = format!("{orbit} - {corporation}");
        if entry.use_operation_name {
            name.push(' ');
            name.push_str(operation);
        }
        stations.push(Station {
            id: entry.id,
            name,
            system_id: entry.system_id,
        });
    }
    Ok(stations)
}

fn corporation_names(source: &mut Source) -> Result<BTreeMap<u32, String>, SdeError> {
    let mut names = BTreeMap::new();
    for (line, record) in parse_file::<CorporationRecord>(source, CORPORATIONS_FILE)? {
        let id = required(record.key, CORPORATIONS_FILE, line, "_key")?;
        names.insert(id, english_name(record.name, CORPORATIONS_FILE, line)?);
    }
    Ok(names)
}

fn operation_names(source: &mut Source) -> Result<BTreeMap<u32, String>, SdeError> {
    let mut names = BTreeMap::new();
    for (line, record) in parse_file::<OperationRecord>(source, OPERATIONS_FILE)? {
        let id = required(record.key, OPERATIONS_FILE, line, "_key")?;
        names.insert(id, english_name(record.name, OPERATIONS_FILE, line)?);
    }
    Ok(names)
}

/// The moons that a station orbits. The file is read line by line; a moon that no station orbits
/// is dropped as it is read.
fn moons(source: &mut Source, orbit_ids: &BTreeSet<u32>) -> Result<BTreeMap<u32, Moon>, SdeError> {
    let mut moons = BTreeMap::new();
    source.each_record(MOONS_FILE, |line, record: MoonRecord| {
        let id = required(record.key, MOONS_FILE, line, "_key")?;
        if orbit_ids.contains(&id) {
            moons.insert(
                id,
                Moon {
                    planet_id: required(record.planet_id, MOONS_FILE, line, "orbitID")?,
                    index: record.index,
                    unique_name: record
                        .unique_name
                        .map(|name| english_name(Some(name), MOONS_FILE, line))
                        .transpose()?,
                    line,
                },
            );
        }
        Ok(())
    })?;
    Ok(moons)
}

/// The planets in `wanted`. Each must be in a solar system the download lists.
fn planets(
    source: &mut Source,
    wanted: &BTreeSet<u32>,
    system_names: &BTreeMap<u32, String>,
) -> Result<BTreeMap<u32, Planet>, SdeError> {
    let mut planets = BTreeMap::new();
    source.each_record(PLANETS_FILE, |line, record: PlanetRecord| {
        let id = required(record.key, PLANETS_FILE, line, "_key")?;
        if wanted.contains(&id) {
            let system_id = required(record.system_id, PLANETS_FILE, line, "solarSystemID")?;
            system_name(system_names, system_id, PLANETS_FILE, line)?;
            planets.insert(
                id,
                Planet {
                    system_id,
                    index: record.index,
                    unique_name: record
                        .unique_name
                        .map(|name| english_name(Some(name), PLANETS_FILE, line))
                        .transpose()?,
                    line,
                },
            );
        }
        Ok(())
    })?;
    Ok(planets)
}

/// The stars in `wanted`. Each must be in a solar system the download lists.
fn stars(
    source: &mut Source,
    wanted: &BTreeSet<u32>,
    system_names: &BTreeMap<u32, String>,
) -> Result<BTreeMap<u32, Star>, SdeError> {
    let mut stars = BTreeMap::new();
    source.each_record(STARS_FILE, |line, record: StarRecord| {
        let id = required(record.key, STARS_FILE, line, "_key")?;
        if wanted.contains(&id) {
            let system_id = required(record.system_id, STARS_FILE, line, "solarSystemID")?;
            system_name(system_names, system_id, STARS_FILE, line)?;
            stars.insert(id, Star { system_id, line });
        }
        Ok(())
    })?;
    Ok(stars)
}

impl Orbits<'_> {
    /// The name of an orbit: a moon's, a planet's or a star's. An id that is none of the three is a
    /// bad reference in the station's line.
    fn name(&self, id: u32, line: usize) -> Result<String, SdeError> {
        if let Some(moon) = self.moons.get(&id) {
            return self.moon_name(moon);
        }
        if let Some(planet) = self.planets.get(&id) {
            return self.planet_name(planet);
        }
        match self.stars.get(&id) {
            Some(star) => {
                let system = system_name(self.system_names, star.system_id, STARS_FILE, star.line)?;
                Ok(system.to_owned())
            }
            None => Err(bad_reference(STATIONS_FILE, line, "orbitID", id)),
        }
    }

    /// A moon's unique name, else `<planet name> - Moon <orbit index>`.
    fn moon_name(&self, moon: &Moon) -> Result<String, SdeError> {
        if let Some(name) = &moon.unique_name {
            return Ok(name.clone());
        }
        let planet = self
            .planets
            .get(&moon.planet_id)
            .ok_or_else(|| bad_reference(MOONS_FILE, moon.line, "orbitID", moon.planet_id))?;
        let index = required(moon.index, MOONS_FILE, moon.line, "orbitIndex")?;
        Ok(format!("{} - Moon {index}", self.planet_name(planet)?))
    }

    /// A planet's unique name, else `<system name> <Roman numeral of its number>`.
    fn planet_name(&self, planet: &Planet) -> Result<String, SdeError> {
        if let Some(name) = &planet.unique_name {
            return Ok(name.clone());
        }
        let system = system_name(
            self.system_names,
            planet.system_id,
            PLANETS_FILE,
            planet.line,
        )?;
        let index = required(planet.index, PLANETS_FILE, planet.line, "celestialIndex")?;
        let numeral = roman(index).ok_or_else(|| SdeError::BadNumber {
            file: PLANETS_FILE.to_owned(),
            line: planet.line,
            field: "celestialIndex".to_owned(),
            text: index.to_string(),
        })?;
        Ok(format!("{system} {numeral}"))
    }
}

/// The name of a solar system, which must be in the download.
fn system_name<'a>(
    names: &'a BTreeMap<u32, String>,
    id: u32,
    file: &str,
    line: usize,
) -> Result<&'a str, SdeError> {
    names
        .get(&id)
        .map(String::as_str)
        .ok_or_else(|| bad_reference(file, line, "solarSystemID", id))
}

/// The Roman numeral of a planet's number, for 1 to 50. `None` for any other number.
fn roman(number: u32) -> Option<String> {
    if !(1..=50).contains(&number) {
        return None;
    }
    let mut rest = number;
    let mut text = String::new();
    for (value, numeral) in NUMERALS {
        while rest >= value {
            text.push_str(numeral);
            rest -= value;
        }
    }
    Some(text)
}

impl Sde {
    /// The station with this id, or `None` when the download has no such station.
    pub fn station(&self, id: i64) -> Result<Option<Station>, SdeError> {
        let station = self
            .conn
            .query_row(
                "SELECT id, name, system_id FROM stations WHERE id = ?1",
                params![id],
                |row| {
                    Ok(Station {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        system_id: row.get(2)?,
                    })
                },
            )
            .optional()?;
        Ok(station)
    }

    /// The English names of the stations with these ids. An id that is not a station is left out.
    pub fn station_names(&self, ids: &[i64]) -> Result<BTreeMap<i64, String>, SdeError> {
        let mut statement = self
            .conn
            .prepare("SELECT name FROM stations WHERE id = ?1")?;
        let mut names = BTreeMap::new();
        for &id in ids {
            let name: Option<String> = statement
                .query_row(params![id], |row| row.get(0))
                .optional()?;
            if let Some(name) = name {
                names.insert(id, name);
            }
        }
        Ok(names)
    }

    /// The stations whose name contains the query, best match first, at most `limit` of them, as
    /// `(id, name)`. The query is trimmed and ignores case; a blank query finds nothing. The rank
    /// and the ties are the same as for the system search.
    pub fn search_stations(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<(i64, String)>, SdeError> {
        let needle = query.trim().to_lowercase();
        if needle.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let mut statement = self
            .conn
            .prepare("SELECT id, name FROM stations WHERE instr(lower(name), ?1) > 0")?;
        let mut ranked = statement
            .query_map(params![needle], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })?
            .map(|row| {
                row.map(|(id, name)| {
                    let rank = station_rank(&name.to_lowercase(), &needle);
                    (rank, name.chars().count(), name, id)
                })
            })
            .collect::<rusqlite::Result<Vec<_>>>()?;
        ranked.sort();
        Ok(ranked
            .into_iter()
            .take(limit)
            .map(|(_, _, name, id)| (id, name))
            .collect())
    }
}

/// 0 for the whole name, 1 for a name that starts with the query, 2 for a later word (split at
/// ' ' or '-') that starts with it, 3 for any other match.
fn station_rank(lower: &str, needle: &str) -> u8 {
    if lower == needle {
        0
    } else if lower.starts_with(needle) {
        1
    } else if lower
        .split([' ', '-'])
        .skip(1)
        .any(|word| word.starts_with(needle))
    {
        2
    } else {
        3
    }
}

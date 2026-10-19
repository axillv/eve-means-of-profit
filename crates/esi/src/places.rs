//! Names for player structures (citadels, engineering complexes) from ESI, and the label every
//! place shows. A structure is named by the first character that may dock there. The name the
//! player typed wins over EVE's name. Stations and systems are named from the game data, never
//! through ESI.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use profit_core::stock::CharacterId;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::auth::Auth;
use crate::client::{Answer, Client, bad_data};
use crate::error::EsiError;
use crate::http::Http;
use crate::secrets::write_atomic;

const VERSION: u32 = 1;
/// The first id of a player structure. Below it are NPC stations and solar systems.
pub const FIRST_STRUCTURE_ID: i64 = 1_000_000_000_000;
const STRUCTURES_PATH: &str = "/universe/structures";
/// A name EVE gave is asked again after a week.
const NAMED_AGE: i64 = 604_800;
/// A refusal is asked again after a day.
const REFUSED_AGE: i64 = 86_400;

/// One structure's last check: the name EVE gave, its system, and whether every character was refused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StructureEntry {
    pub name: Option<String>,
    pub system_id: Option<u32>,
    pub refused: bool,
    pub checked_at: i64,
}

/// The structures the app has checked, by structure id.
pub type StructureNames = BTreeMap<i64, StructureEntry>;

/// What one naming run did: how many structures were asked, and the ones whose check failed, with the reason.
#[derive(Debug)]
pub struct NamingReport {
    pub asked: usize,
    pub failed: Vec<(i64, EsiError)>,
}

/// The outcome of asking about one structure.
enum Check {
    Entry(StructureEntry),
    Failed(EsiError),
}

/// Names the structures in `ids` that are due for a check, and saves `dir/places.json` when an entry changed.
/// `ErrorLimited` stops the run at once; the entries already changed in it are still saved.
pub fn name_structures(
    dir: &Path,
    names: &mut StructureNames,
    ids: &[i64],
    characters: &[CharacterId],
    auth: &mut Auth,
    http: &dyn Http,
    now: i64,
) -> Result<NamingReport, EsiError> {
    let mut report = NamingReport {
        asked: 0,
        failed: Vec::new(),
    };
    // A character id that cannot be a character is no character at all.
    let usable: Vec<u64> = characters
        .iter()
        .filter_map(|id| u64::try_from(*id).ok())
        .collect();
    if usable.is_empty() {
        return Ok(report);
    }
    let mut client = Client::new(http);
    let mut seen = BTreeSet::new();
    let mut changed = false;
    let mut stopped = None;
    for &id in ids {
        if id < FIRST_STRUCTURE_ID || !seen.insert(id) {
            continue;
        }
        match is_due(names.get(&id), now) {
            Ok(true) => {}
            Ok(false) => continue,
            Err(error) => {
                report.failed.push((id, error));
                continue;
            }
        }
        report.asked += 1;
        match check(&mut client, auth, &usable, id, names.get(&id), now) {
            Ok(Check::Entry(entry)) => {
                names.insert(id, entry);
                changed = true;
            }
            Ok(Check::Failed(error)) => report.failed.push((id, error)),
            Err(error) => {
                stopped = Some(error);
                break;
            }
        }
    }
    let saved = if changed { save(dir, names) } else { Ok(()) };
    // A stop is what the caller must act on, so it is returned even when the save also failed.
    match stopped {
        Some(error) => Err(error),
        None => saved.map(|()| report),
    }
}

/// Whether a structure is asked again at `now`. An entry that holds no name and was not refused is always asked again.
fn is_due(entry: Option<&StructureEntry>, now: i64) -> Result<bool, EsiError> {
    let Some(entry) = entry else {
        return Ok(true);
    };
    let age = if entry.refused {
        REFUSED_AGE
    } else if entry.name.is_some() {
        NAMED_AGE
    } else {
        return Ok(true);
    };
    let due_at = entry
        .checked_at
        .checked_add(age)
        .ok_or_else(|| bad_data(STRUCTURES_PATH, "checked_at", &entry.checked_at.to_string()))?;
    Ok(now >= due_at)
}

/// Asks the characters in order until one gives the name. Every character refused gives a refused entry that keeps the old name. Any other failure is reported, and the entry stays as it was. `Err` only for `ErrorLimited`.
fn check(
    client: &mut Client<'_>,
    auth: &mut Auth,
    characters: &[u64],
    id: i64,
    previous: Option<&StructureEntry>,
    now: i64,
) -> Result<Check, EsiError> {
    let path = format!("{STRUCTURES_PATH}/{id}");
    // The first failure that is not a refusal. No failure at all means every character was refused.
    let mut failure: Option<EsiError> = None;
    for &character in characters {
        let token = match auth.access_token(character, now) {
            Ok(token) => token,
            Err(error) => {
                failure.get_or_insert(error);
                continue;
            }
        };
        let group = format!("{character}/structures");
        match client.get_json(now, &token, &path, &group) {
            Ok(answer) => match named(&answer, &path) {
                Ok((name, system_id)) => {
                    return Ok(Check::Entry(StructureEntry {
                        name: Some(name),
                        system_id,
                        refused: false,
                        checked_at: now,
                    }));
                }
                Err(error) => {
                    failure.get_or_insert(error);
                }
            },
            Err(error @ EsiError::ErrorLimited { .. }) => return Err(error),
            Err(EsiError::Forbidden { .. } | EsiError::NotFound { .. }) => {}
            Err(error) => {
                failure.get_or_insert(error);
            }
        }
    }
    Ok(match failure {
        None => Check::Entry(StructureEntry {
            name: previous.and_then(|held| held.name.clone()),
            system_id: previous.and_then(|held| held.system_id),
            refused: true,
            checked_at: now,
        }),
        Some(error) => Check::Failed(error),
    })
}

/// The name and system from a 200 answer. A missing or empty name is unreadable; a missing system is `None`.
fn named(answer: &Answer, route: &str) -> Result<(String, Option<u32>), EsiError> {
    let name = match answer.body.get("name") {
        Some(Value::String(name)) if !name.is_empty() => name.clone(),
        other => {
            return Err(bad_data(
                route,
                "name",
                &other.map(Value::to_string).unwrap_or_default(),
            ));
        }
    };
    let system_id = match answer.body.get("solar_system_id") {
        None | Some(Value::Null) => None,
        Some(value) => Some(
            value
                .as_u64()
                .and_then(|id| u32::try_from(id).ok())
                .ok_or_else(|| bad_data(route, "solar_system_id", &value.to_string()))?,
        ),
    };
    Ok((name, system_id))
}

/// The label a place shows. A structure: the typed name, then EVE's name, then `Structure <id> (no access)` when refused, else `Structure <id>`. A station or system: the game data's name, else `Location <id>`.
pub fn place_label(
    id: i64,
    typed: Option<&str>,
    game_data: Option<&str>,
    structures: &StructureNames,
) -> String {
    if id < FIRST_STRUCTURE_ID {
        return game_data.map_or_else(|| format!("Location {id}"), str::to_owned);
    }
    if let Some(typed) = typed.map(str::trim).filter(|typed| !typed.is_empty()) {
        return typed.to_owned();
    }
    match structures.get(&id) {
        Some(StructureEntry {
            name: Some(name), ..
        }) => name.clone(),
        Some(StructureEntry { refused: true, .. }) => format!("Structure {id} (no access)"),
        _ => format!("Structure {id}"),
    }
}

#[derive(Serialize)]
struct FileOut<'a> {
    version: u32,
    structures: &'a StructureNames,
}

#[derive(Deserialize)]
struct Header {
    version: u32,
}

#[derive(Deserialize)]
struct FileIn {
    structures: StructureNames,
}

fn places_path(dir: &Path) -> PathBuf {
    dir.join("places.json")
}

/// Reads the saved structure names. A missing file is an empty map; a file that cannot be read, or has another version, is `BadFile` and is left as it is.
pub fn load_places(dir: &Path) -> Result<StructureNames, EsiError> {
    let path = places_path(dir);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(StructureNames::new()),
        Err(error) => return Err(EsiError::bad_file(&path, error.to_string())),
    };
    let header: Header = serde_json::from_slice(&bytes)
        .map_err(|error| EsiError::bad_file(&path, error.to_string()))?;
    if header.version != VERSION {
        return Err(EsiError::bad_file(
            &path,
            format!("unknown version {}", header.version),
        ));
    }
    let file: FileIn = serde_json::from_slice(&bytes)
        .map_err(|error| EsiError::bad_file(&path, error.to_string()))?;
    Ok(file.structures)
}

fn save(dir: &Path, names: &StructureNames) -> Result<(), EsiError> {
    let path = places_path(dir);
    let file = FileOut {
        version: VERSION,
        structures: names,
    };
    let bytes =
        serde_json::to_vec(&file).map_err(|error| EsiError::bad_file(&path, error.to_string()))?;
    write_atomic(&path, &bytes, true)
}

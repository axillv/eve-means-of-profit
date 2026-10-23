//! What every page reads: the login characters and their last data, jEve's profile, the structure
//! names, and the typed names. The worker reads the files at start; the pages only read what it
//! holds. Nothing here fetches from EVE.

pub mod eve;

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use mop_core::stock::{CharacterAssets, LocationId};
use mop_esi::character_data::CharacterData;
use mop_esi::characters::Character;
use mop_esi::error::EsiError;
use mop_esi::places::{self, FIRST_STRUCTURE_ID, StructureNames};
use mop_esi::saved;
use mop_jeve::{JeveData, JeveError, ReadOptions, read_profile};
use mop_sde::Sde;

pub use eve::{EveAccess, LiveEve, NoEve, PendingLogin};

/// The name jEveAssets gives the profile it has active.
pub(crate) const JEVE_PROFILE: &str = "#Default.db";

/// Where a character's assets in the stock come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The app's own login data.
    Login,
    /// jEveAssets' profile.
    Jeve,
}

/// The saved data the worker read: the login characters with their last data, and the structure names.
#[derive(Debug)]
pub struct Saved {
    pub characters: Vec<Character>,
    pub data: Vec<CharacterData>,
    pub structures: StructureNames,
}

/// What the worker read at start. Each part fails on its own.
#[derive(Debug)]
pub struct Loaded {
    pub saved: Result<Saved, EsiError>,
    pub jeve: Result<Option<JeveData>, JeveError>,
}

/// Reads the saved data and jEve's profile. Runs on the worker; fetches nothing.
pub fn load(eve: &dyn EveAccess, data_dir: &Path) -> Loaded {
    let saved = load_saved(eve, &data_dir.join("esi"));
    let jeve = match eve.jeve_folder() {
        Some(folder) => load_jeve(&folder.join(JEVE_PROFILE)),
        None => Ok(None),
    };
    Loaded { saved, jeve }
}

fn load_saved(eve: &dyn EveAccess, esi: &Path) -> Result<Saved, EsiError> {
    let characters = eve.characters();
    let mut data = Vec::new();
    for character in &characters {
        if let Some(held) = saved::load_character(esi, character.id)? {
            data.push(held);
        }
    }
    let structures = places::load_places(esi)?;
    Ok(Saved {
        characters,
        data,
        structures,
    })
}

/// jEve's profile, or `None` when the folder holds no `#Default.db`.
fn load_jeve(path: &Path) -> Result<Option<JeveData>, JeveError> {
    if !path.is_file() {
        return Ok(None);
    }
    read_profile(path, &ReadOptions::default()).map(Some)
}

/// The session: what the pages read. Held on the UI thread; the login behind it runs on the worker.
pub struct Session {
    eve: Arc<Mutex<Box<dyn EveAccess>>>,
    loaded: bool,
    login: Vec<Character>,
    data: BTreeMap<u64, CharacterData>,
    revisions: BTreeMap<u64, u64>,
    jeve: Option<JeveData>,
    structures: StructureNames,
    typed: BTreeMap<i64, String>,
}

impl Session {
    pub fn new(eve: Box<dyn EveAccess>) -> Self {
        Self {
            eve: Arc::new(Mutex::new(eve)),
            loaded: false,
            login: Vec::new(),
            data: BTreeMap::new(),
            revisions: BTreeMap::new(),
            jeve: None,
            structures: StructureNames::new(),
            typed: BTreeMap::new(),
        }
    }

    /// The login, for the worker's calls. Only the worker locks it.
    pub fn eve(&self) -> Arc<Mutex<Box<dyn EveAccess>>> {
        Arc::clone(&self.eve)
    }

    /// Whether the start-up read has arrived. The stub panels wait for it.
    pub fn loaded(&self) -> bool {
        self.loaded
    }

    /// Marks the start-up read as arrived, whether or not it held anything.
    pub fn mark_loaded(&mut self) {
        self.loaded = true;
    }

    /// The login characters, in the order they were added.
    pub fn characters(&self) -> &[Character] {
        &self.login
    }

    /// One login character's last data, when it has been saved.
    pub fn character_data(&self, character_id: u64) -> Option<&CharacterData> {
        self.data.get(&character_id)
    }

    /// jEve's profile, when one was read.
    pub fn jeve(&self) -> Option<&JeveData> {
        self.jeve.as_ref()
    }

    /// Stores the saved data read at start. The login characters are the ones the login holds.
    pub fn set_saved(&mut self, saved: Saved) {
        self.login = saved.characters;
        self.data = saved
            .data
            .into_iter()
            .map(|held| (held.character_id, held))
            .collect();
        self.structures = saved.structures;
    }

    /// Drops everything read for the login and jEve, after a start-up read failed.
    pub fn clear(&mut self) {
        self.login.clear();
        self.data.clear();
        self.jeve = None;
    }

    pub fn set_jeve(&mut self, jeve: Option<JeveData>) {
        self.jeve = jeve;
    }

    /// Adds a login character, or replaces the one with the same id. Its data and the other
    /// characters' data stay as they are.
    pub fn put_character(&mut self, character: Character) {
        match self.login.iter_mut().find(|held| held.id == character.id) {
            Some(held) => *held = character,
            None => self.login.push(character),
        }
    }

    /// Drops one login character and its data. The other characters, the structure names and
    /// jEve's profile stay.
    pub fn remove_character(&mut self, character_id: u64) {
        self.login.retain(|held| held.id != character_id);
        self.data.remove(&character_id);
    }

    pub fn set_structures(&mut self, structures: StructureNames) {
        self.structures = structures;
    }

    /// The structure names EVE gave, with each structure's last check.
    pub fn structures(&self) -> &StructureNames {
        &self.structures
    }

    pub fn set_typed_names(&mut self, typed: BTreeMap<i64, String>) {
        self.typed = typed;
    }

    /// How many times this character's data has been replaced. Starts at 0.
    pub fn revision(&self, character_id: u64) -> u64 {
        self.revisions.get(&character_id).copied().unwrap_or(0)
    }

    /// Stores a character's data and counts one more change for it.
    pub fn replace(&mut self, data: CharacterData) {
        let id = data.character_id;
        let count = self.revisions.entry(id).or_insert(0);
        // Cannot saturate: the count goes up by one per refresh click, never near u64::MAX.
        *count = count.saturating_add(1);
        self.data.insert(id, data);
    }

    /// The oldest fetch time among the login characters' data, or `None` when none holds any.
    pub fn oldest_fetch(&self) -> Option<i64> {
        self.login
            .iter()
            .filter_map(|character| self.data.get(&character.id))
            .filter_map(oldest_of)
            .min()
    }

    /// The assets the stock counts, one entry per character, sorted by character id. A login
    /// character's assets come first; a jEve character is used only when the login holds none for it.
    pub fn stock_characters(&self) -> Vec<CharacterAssets> {
        self.stock()
            .into_values()
            .map(|(_, assets)| assets)
            .collect()
    }

    /// Where a character's assets in the stock come from, or `None` when the stock has none for it.
    pub fn source(&self, character_id: u64) -> Option<Source> {
        self.stock().get(&character_id).map(|(source, _)| *source)
    }

    fn stock(&self) -> BTreeMap<u64, (Source, CharacterAssets)> {
        let mut stock = BTreeMap::new();
        for character in &self.login {
            let assets = self
                .data
                .get(&character.id)
                .and_then(|held| held.assets.as_ref());
            if let Some(owned) = assets {
                stock.insert(character.id, (Source::Login, owned.data.assets.clone()));
            }
        }
        if let Some(jeve) = &self.jeve {
            for held in &jeve.characters {
                let Ok(id) = u64::try_from(held.character_id) else {
                    continue;
                };
                stock
                    .entry(id)
                    .or_insert_with(|| (Source::Jeve, held.assets.clone()));
            }
        }
        stock
    }

    /// The name a place shows: the typed name, EVE's name for a structure, or the game data's name
    /// for a station or a system. `sde` is `None` before the game data is in.
    pub fn place_label(&self, id: LocationId, sde: Option<&Sde>) -> String {
        let typed = self.typed.get(&id).map(String::as_str);
        if id >= FIRST_STRUCTURE_ID {
            return places::place_label(id, typed, None, &self.structures);
        }
        let game_data = sde.and_then(|sde| game_name(sde, id));
        places::place_label(id, None, game_data.as_deref(), &self.structures)
    }
}

/// The game data's name for a station or a system id, or `None` when there is none.
fn game_name(sde: &Sde, id: LocationId) -> Option<String> {
    if (30_000_000..=32_999_999).contains(&id) {
        let system = u32::try_from(id).ok()?;
        return sde.system(system).ok().flatten().map(|system| system.name);
    }
    if (60_000_000..=63_999_999).contains(&id) {
        return sde
            .station_names(&[id])
            .ok()
            .and_then(|names| names.get(&id).cloned());
    }
    None
}

/// The oldest fetch time among one character's kinds of data.
fn oldest_of(held: &CharacterData) -> Option<i64> {
    let assets = held.assets.as_ref().map(|fetched| fetched.fetched_at);
    let orders = held.orders.as_ref().map(|fetched| fetched.fetched_at);
    let jobs = held.jobs.as_ref().map(|fetched| fetched.fetched_at);
    let wallet = held.wallet.as_ref().map(|fetched| fetched.fetched_at);
    [assets, orders, jobs, wallet].into_iter().flatten().min()
}

/// Locks the login. A lock poisoned by a panicked job is still used.
pub fn lock_eve(eve: &Mutex<Box<dyn EveAccess>>) -> std::sync::MutexGuard<'_, Box<dyn EveAccess>> {
    eve.lock().unwrap_or_else(PoisonError::into_inner)
}

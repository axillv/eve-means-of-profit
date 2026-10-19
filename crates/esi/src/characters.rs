//! The list of added characters and whether each one's login still works. Holds no secrets.

use std::collections::BTreeSet;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::EsiError;
use crate::secrets::write_atomic;

const FILE_NAME: &str = "characters.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Login {
    Ok,
    Expired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Character {
    pub id: u64,
    pub name: String,
    pub scopes: BTreeSet<String>,
    pub added_at: i64,
    pub login: Login,
}

#[derive(Serialize, Deserialize)]
struct CharacterFile {
    version: u32,
    characters: Vec<Character>,
}

/// `<dir>/characters.json`, in the order the characters were added.
pub struct CharacterList {
    path: PathBuf,
    rows: Vec<Character>,
}

impl CharacterList {
    pub fn open(dir: &Path) -> Result<Self, EsiError> {
        let path = dir.join(FILE_NAME);
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                return Ok(Self {
                    path,
                    rows: Vec::new(),
                });
            }
            Err(error) => return Err(EsiError::bad_file(&path, error.to_string())),
        };
        let file: CharacterFile = serde_json::from_slice(&bytes)
            .map_err(|error| EsiError::bad_file(&path, error.to_string()))?;
        if file.version != 1 {
            return Err(EsiError::bad_file(
                &path,
                format!("unknown version {}", file.version),
            ));
        }
        Ok(Self {
            path,
            rows: file.characters,
        })
    }

    pub fn all(&self) -> &[Character] {
        &self.rows
    }

    pub fn get(&self, id: u64) -> Option<&Character> {
        self.rows.iter().find(|row| row.id == id)
    }

    /// Adds the character, or replaces the name, scopes and login of the row that has its id (keeping its place and `added_at`).
    pub fn upsert(&mut self, incoming: Character) -> Result<Character, EsiError> {
        let mut rows = self.rows.clone();
        let stored = match rows.iter_mut().find(|row| row.id == incoming.id) {
            Some(row) => {
                row.name = incoming.name;
                row.scopes = incoming.scopes;
                row.login = incoming.login;
                row.clone()
            }
            None => {
                rows.push(incoming.clone());
                incoming
            }
        };
        self.commit(rows)?;
        Ok(stored)
    }

    pub fn set_login(&mut self, id: u64, login: Login) -> Result<(), EsiError> {
        let mut rows = self.rows.clone();
        let row = rows
            .iter_mut()
            .find(|row| row.id == id)
            .ok_or(EsiError::UnknownCharacter(id))?;
        row.login = login;
        self.commit(rows)
    }

    pub fn remove(&mut self, id: u64) -> Result<(), EsiError> {
        let mut rows = self.rows.clone();
        rows.retain(|row| row.id != id);
        self.commit(rows)
    }

    /// The file is written before the list changes, so a failed write leaves the list as it was.
    fn commit(&mut self, rows: Vec<Character>) -> Result<(), EsiError> {
        let bytes = serde_json::to_vec(&CharacterFile {
            version: 1,
            characters: rows.clone(),
        })
        .map_err(|error| EsiError::bad_file(&self.path, error.to_string()))?;
        write_atomic(&self.path, &bytes, false)?;
        self.rows = rows;
        Ok(())
    }
}

//! Finds jEveAssets' profile files on disk.

use std::path::{Path, PathBuf};

use crate::JeveError;

/// The folder where jEveAssets keeps its profiles under `home`.
pub fn profiles_dir_in(home: &Path, macos: bool) -> PathBuf {
    if macos {
        home.join("Library/Preferences/JEveAssets/profiles")
    } else {
        home.join(".jeveassets/profiles")
    }
}

/// The profile folder of the user running this app, when the home folder is known.
pub fn profiles_dir() -> Option<PathBuf> {
    std::env::home_dir().map(|home| profiles_dir_in(&home, cfg!(target_os = "macos")))
}

/// One profile file. jEveAssets marks the active profile with a leading `#`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileFile {
    pub name: String,
    pub active: bool,
    pub path: PathBuf,
}

/// The profiles in `dir`, the active one first, then by name. A missing folder has none.
pub fn list_profiles(dir: &Path) -> Result<Vec<ProfileFile>, JeveError> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(JeveError::Io {
                path: dir.to_path_buf(),
                source,
            });
        }
    };
    let mut profiles = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| JeveError::Io {
            path: dir.to_path_buf(),
            source,
        })?;
        let file_name = entry.file_name();
        let Some(file_name) = file_name.to_str() else {
            continue;
        };
        let Some(stem) = file_name.strip_suffix(".db") else {
            continue;
        };
        let file_type = entry.file_type().map_err(|source| JeveError::Io {
            path: entry.path(),
            source,
        })?;
        if !file_type.is_file() {
            continue;
        }
        profiles.push(ProfileFile {
            name: stem.strip_prefix('#').unwrap_or(stem).to_owned(),
            active: stem.starts_with('#'),
            path: entry.path(),
        });
    }
    profiles.sort_by(|a, b| b.active.cmp(&a.active).then_with(|| a.name.cmp(&b.name)));
    Ok(profiles)
}

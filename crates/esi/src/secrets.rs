//! Where the long-lived login keys are kept: the system keychain, or a plain file when there is no keychain.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use keyring::Entry;
use serde::{Deserialize, Serialize};

use crate::error::EsiError;
use crate::token::RefreshToken;

const SERVICE: &str = "eve-means-of-profit";
const PROBE_USER: &str = "probe";
const FILE_NAME: &str = "tokens.json";

pub trait SecretStore: Send {
    fn get(&self, character_id: u64) -> Result<Option<RefreshToken>, EsiError>;
    fn set(&self, character_id: u64, token: &RefreshToken) -> Result<(), EsiError>;
    fn delete(&self, character_id: u64) -> Result<(), EsiError>;
}

/// The store the login uses. `fallback` is set when the keychain is missing; the reason is the text the keychain gave.
pub struct Secrets {
    pub store: Box<dyn SecretStore>,
    pub fallback: Option<String>,
}

/// Probes the keychain once. When it works the keychain is used (and a token left in the file by an earlier fallback is still found); when not, the file is used.
pub fn open_secrets(dir: &Path) -> Secrets {
    match probe() {
        Ok(()) => Secrets {
            store: Box::new(KeychainFirst {
                keychain: Box::new(KeychainStore),
                file: Box::new(FileStore::new(dir)),
            }),
            fallback: None,
        },
        Err(reason) => Secrets {
            store: Box::new(FileStore::new(dir)),
            fallback: Some(reason),
        },
    }
}

fn probe() -> Result<(), String> {
    let entry = Entry::new(SERVICE, PROBE_USER).map_err(|error| error.to_string())?;
    entry
        .set_password("probe")
        .map_err(|error| error.to_string())?;
    let read = entry.get_password().map_err(|error| error.to_string());
    // The probe entry is removed whether or not it could be read back.
    entry
        .delete_credential()
        .map_err(|error| error.to_string())?;
    if read? == "probe" {
        Ok(())
    } else {
        Err("the keychain returned a different value".to_owned())
    }
}

/// The text the owner sees once per session when the keychain is missing.
pub fn fallback_warning(dir: impl AsRef<Path>, reason: &str) -> String {
    format!(
        "No system keychain found ({reason}). Logins are kept in a plain file: {}. Anyone who can read this file can read these characters' data.",
        dir.as_ref().join(FILE_NAME).display()
    )
}

/// The system keychain, one entry per character.
pub struct KeychainStore;

fn entry(character_id: u64) -> Result<Entry, EsiError> {
    Entry::new(SERVICE, &format!("character-{character_id}"))
        .map_err(|error| keychain_error(&error))
}

fn keychain_error(error: &keyring::Error) -> EsiError {
    EsiError::Keychain(error.to_string())
}

impl SecretStore for KeychainStore {
    fn get(&self, character_id: u64) -> Result<Option<RefreshToken>, EsiError> {
        match entry(character_id)?.get_password() {
            Ok(text) => Ok(Some(RefreshToken::new(text))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(keychain_error(&error)),
        }
    }

    fn set(&self, character_id: u64, token: &RefreshToken) -> Result<(), EsiError> {
        entry(character_id)?
            .set_password(token.as_str())
            .map_err(|error| keychain_error(&error))
    }

    fn delete(&self, character_id: u64) -> Result<(), EsiError> {
        match entry(character_id)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(keychain_error(&error)),
        }
    }
}

/// Reads and writes the keychain first; the file is read only for characters the keychain does not hold. A keychain write also removes the character from the file.
pub struct KeychainFirst {
    pub keychain: Box<dyn SecretStore>,
    pub file: Box<dyn SecretStore>,
}

impl SecretStore for KeychainFirst {
    fn get(&self, character_id: u64) -> Result<Option<RefreshToken>, EsiError> {
        match self.keychain.get(character_id)? {
            Some(token) => {
                // A copy left in the plain file by a failed cleanup is removed now; still best effort.
                drop(self.file.delete(character_id));
                Ok(Some(token))
            }
            None => self.file.get(character_id),
        }
    }

    fn set(&self, character_id: u64, token: &RefreshToken) -> Result<(), EsiError> {
        self.keychain.set(character_id, token)?;
        // The keychain holds the new token and is read first, so a leftover copy in the file is harmless and its removal is best effort.
        drop(self.file.delete(character_id));
        Ok(())
    }

    fn delete(&self, character_id: u64) -> Result<(), EsiError> {
        self.keychain.delete(character_id)?;
        // A damaged plain file must not stop the removal of a character the keychain already forgot.
        drop(self.file.delete(character_id));
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
struct TokenFile {
    version: u32,
    tokens: BTreeMap<String, String>,
}

/// `<dir>/tokens.json`, the plain-file fallback. The file is removed when it holds no token.
pub struct FileStore {
    path: PathBuf,
}

impl FileStore {
    pub fn new(dir: &Path) -> Self {
        Self {
            path: dir.join(FILE_NAME),
        }
    }

    fn load(&self) -> Result<BTreeMap<String, String>, EsiError> {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(BTreeMap::new()),
            Err(error) => return Err(EsiError::bad_file(&self.path, error.to_string())),
        };
        let file: TokenFile = serde_json::from_slice(&bytes)
            .map_err(|error| EsiError::bad_file(&self.path, error.to_string()))?;
        if file.version != 1 {
            return Err(EsiError::bad_file(
                &self.path,
                format!("unknown version {}", file.version),
            ));
        }
        Ok(file.tokens)
    }

    fn save(&self, tokens: BTreeMap<String, String>) -> Result<(), EsiError> {
        if tokens.is_empty() {
            return match fs::remove_file(&self.path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
                Err(source) => Err(EsiError::Io {
                    path: self.path.clone(),
                    source,
                }),
            };
        }
        let bytes = serde_json::to_vec(&TokenFile { version: 1, tokens })
            .map_err(|error| EsiError::bad_file(&self.path, error.to_string()))?;
        write_atomic(&self.path, &bytes, true)
    }
}

impl SecretStore for FileStore {
    fn get(&self, character_id: u64) -> Result<Option<RefreshToken>, EsiError> {
        Ok(self
            .load()?
            .get(&character_id.to_string())
            .map(|text| RefreshToken::new(text.as_str())))
    }

    fn set(&self, character_id: u64, token: &RefreshToken) -> Result<(), EsiError> {
        let mut tokens = self.load()?;
        tokens.insert(character_id.to_string(), token.as_str().to_owned());
        self.save(tokens)
    }

    fn delete(&self, character_id: u64) -> Result<(), EsiError> {
        let mut tokens = self.load()?;
        if tokens.remove(&character_id.to_string()).is_some() {
            self.save(tokens)?;
        }
        Ok(())
    }
}

/// Writes to a temporary file and renames it over `path`, so a crash never leaves half a file. `private` makes the file readable only by its owner on Unix.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8], private: bool) -> Result<(), EsiError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| EsiError::Io {
            path: parent.to_owned(),
            source,
        })?;
    }
    let temp = path.with_extension("json.tmp");
    let io_error = |source| EsiError::Io {
        path: temp.clone(),
        source,
    };
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    if private {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temp).map_err(io_error)?;
    #[cfg(unix)]
    if private {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(io_error)?;
    }
    file.write_all(bytes).map_err(io_error)?;
    file.sync_all().map_err(io_error)?;
    drop(file);
    fs::rename(&temp, path).map_err(|source| EsiError::Io {
        path: path.to_owned(),
        source,
    })
}

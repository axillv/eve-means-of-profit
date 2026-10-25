//! A character's data kept between runs, so the app opens with the last data it had. Money is
//! written as whole ten-thousandths, never as a float. EVE can give all of it again, so a format
//! change bumps the version and old files are fetched again.

use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use mop_core::money::Money;
use mop_core::stock::{Asset, CharacterAssets, ItemId};
use serde::{Deserialize, Serialize};

use crate::character_data::{CharacterData, Fetched, Job, Order, OwnedAssets};
use crate::error::EsiError;
use crate::secrets::write_atomic;

const VERSION: u32 = 1;

/// `<dir>/data-<character>.json`.
fn file_path(dir: &Path, character_id: u64) -> PathBuf {
    dir.join(format!("data-{character_id}.json"))
}

/// Money as its ten-thousandths, a whole number.
pub(crate) mod money {
    use mop_core::money::Money;
    use serde::{Deserialize, Deserializer, Serializer};

    pub(crate) fn serialize<S: Serializer>(
        money: &Money,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_i64(money.ten_thousandths())
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Money, D::Error> {
        i64::deserialize(deserializer).map(Money::from_ten_thousandths)
    }
}

/// An optional money amount: the ten-thousandths, or null.
pub(crate) mod money_opt {
    use mop_core::money::Money;
    use serde::{Deserialize, Deserializer, Serializer};

    pub(crate) fn serialize<S: Serializer>(
        money: &Option<Money>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match money {
            Some(money) => serializer.serialize_some(&money.ten_thousandths()),
            None => serializer.serialize_none(),
        }
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Money>, D::Error> {
        Option::<i64>::deserialize(deserializer).map(|found| found.map(Money::from_ten_thousandths))
    }
}

#[derive(Serialize, Deserialize)]
struct Header {
    version: u32,
}

/// The whole file, version 1.
#[derive(Serialize, Deserialize)]
struct File {
    version: u32,
    character_id: u64,
    assets: Option<Fetched<SavedAssets>>,
    orders: Option<Fetched<Vec<Order>>>,
    jobs: Option<Fetched<Vec<Job>>>,
    wallet: Option<Fetched<i64>>,
}

#[derive(Serialize, Deserialize)]
struct SavedAssets {
    updated: Option<i64>,
    rows: Vec<SavedAsset>,
    names: BTreeMap<ItemId, String>,
}

#[derive(Serialize, Deserialize)]
struct SavedAsset {
    item_id: ItemId,
    type_id: u32,
    location_id: ItemId,
    flag: Option<String>,
    quantity: u64,
    singleton: bool,
}

/// Writes the character's data, replacing the file. A kind not fetched yet is written as absent.
pub fn save_character(dir: &Path, data: &CharacterData) -> Result<(), EsiError> {
    let path = file_path(dir, data.character_id);
    let file = File {
        version: VERSION,
        character_id: data.character_id,
        assets: data.assets.as_ref().map(|held| {
            with_data(
                held,
                SavedAssets {
                    updated: held.data.assets.updated,
                    rows: held
                        .data
                        .assets
                        .assets
                        .iter()
                        .map(SavedAsset::from)
                        .collect(),
                    names: held.data.names.clone(),
                },
            )
        }),
        orders: data.orders.clone(),
        jobs: data.jobs.clone(),
        wallet: data
            .wallet
            .as_ref()
            .map(|held| with_data(held, held.data.ten_thousandths())),
    };
    let bytes =
        serde_json::to_vec(&file).map_err(|error| EsiError::bad_file(&path, error.to_string()))?;
    write_atomic(&path, &bytes, true)
}

/// Reads the character's saved data. `None` when there is no file; a file that cannot be read, or has another version, is `BadFile` and is left as it is.
pub fn load_character(dir: &Path, character_id: u64) -> Result<Option<CharacterData>, EsiError> {
    let path = file_path(dir, character_id);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
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
    let file: File = serde_json::from_slice(&bytes)
        .map_err(|error| EsiError::bad_file(&path, error.to_string()))?;
    if file.character_id != character_id {
        return Err(EsiError::bad_file(&path, "holds another character"));
    }
    into_data(file, &path).map(Some)
}

/// Deletes the character's saved data. A missing file is not an error.
pub fn forget(dir: &Path, character_id: u64) -> Result<(), EsiError> {
    let path = file_path(dir, character_id);
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(source) => Err(EsiError::Io { path, source }),
    }
}

/// Keeps the fetch time and EVE's times of `held` and puts `data` in their place.
pub(crate) fn with_data<T, U>(held: &Fetched<T>, data: U) -> Fetched<U> {
    Fetched {
        data,
        fetched_at: held.fetched_at,
        last_modified: held.last_modified,
        expires: held.expires,
    }
}

fn into_data(file: File, path: &Path) -> Result<CharacterData, EsiError> {
    let character_id = file.character_id;
    let assets = file
        .assets
        .map(|held| {
            let owned_character = i64::try_from(character_id)
                .ok()
                .ok_or_else(|| EsiError::bad_file(path, "character id out of range"))?;
            let Fetched {
                data: saved,
                fetched_at,
                last_modified,
                expires,
            } = held;
            Ok(Fetched {
                data: OwnedAssets {
                    assets: CharacterAssets {
                        character_id: owned_character,
                        updated: saved.updated,
                        assets: saved.rows.into_iter().map(Asset::from).collect(),
                    },
                    names: saved.names,
                },
                fetched_at,
                last_modified,
                expires,
            })
        })
        .transpose()?;
    let wallet = file
        .wallet
        .map(|held| with_data(&held, Money::from_ten_thousandths(held.data)));
    Ok(CharacterData {
        character_id,
        assets,
        orders: file.orders,
        jobs: file.jobs,
        wallet,
    })
}

impl From<&Asset> for SavedAsset {
    fn from(asset: &Asset) -> Self {
        Self {
            item_id: asset.item_id,
            type_id: asset.type_id,
            location_id: asset.location_id,
            flag: asset.flag.clone(),
            quantity: asset.quantity,
            singleton: asset.singleton,
        }
    }
}

impl From<SavedAsset> for Asset {
    fn from(saved: SavedAsset) -> Self {
        Self {
            item_id: saved.item_id,
            type_id: saved.type_id,
            location_id: saved.location_id,
            flag: saved.flag,
            quantity: saved.quantity,
            singleton: saved.singleton,
        }
    }
}

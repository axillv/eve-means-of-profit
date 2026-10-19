//! Reads jEveAssets' Tracker file: the net worth history it keeps per owner. The file is opened
//! read-only and never changed. The values are jEve's own, priced when jEve wrote them.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use profit_core::money::Money;
use profit_core::net_worth::{CharacterKey, Part, Record, Snapshot};
use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::Value;

use crate::JeveError;

/// jEve dates each point in Unix milliseconds; the records use seconds.
const MILLIS_PER_SECOND: i64 = 1_000;

/// The Tracker file inside a jEveAssets folder.
pub fn tracker_path(jeve_dir: &Path) -> PathBuf {
    jeve_dir.join("data").join("tracker.json")
}

/// The history in the file at `path`: per owner in file order, then per point in file order.
/// `None` when there is no file. One bad point fails the whole read.
pub fn read_tracker(path: &Path) -> Result<Option<Vec<Record>>, JeveError> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(JeveError::Io {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    let owners: Owners =
        serde_json::from_slice(&bytes).map_err(|error| JeveError::TrackerNotJson {
            reason: error.to_string(),
        })?;

    let mut records = Vec::new();
    for (owner, value) in owners.0 {
        let Value::Array(points) = value else {
            return Err(JeveError::TrackerNotJson {
                reason: format!("the entry for {owner} is not a list of points"),
            });
        };
        let character = character_key(&owner);
        for (index, value) in points.iter().enumerate() {
            let entry = Entry {
                owner: &owner,
                index,
                value,
            };
            records.push(record(&entry, character.clone())?);
        }
    }
    Ok(Some(records))
}

/// A digits-only owner key is a character id that fits `u64`; any other key is a name.
fn character_key(owner: &str) -> CharacterKey {
    let digits = !owner.is_empty() && owner.bytes().all(|byte| byte.is_ascii_digit());
    match owner.parse::<u64>() {
        Ok(id) if digits => CharacterKey::Id(id),
        _ => CharacterKey::Name(owner.to_owned()),
    }
}

/// The owners in file order. A map would sort them.
struct Owners(Vec<(String, Value)>);

impl<'de> Deserialize<'de> for Owners {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct OwnersVisitor;

        impl<'de> Visitor<'de> for OwnersVisitor {
            type Value = Owners;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an object of owners")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Owners, A::Error> {
                let mut owners = Vec::new();
                while let Some(owner) = map.next_entry::<String, Value>()? {
                    owners.push(owner);
                }
                Ok(Owners(owners))
            }
        }

        deserializer.deserialize_map(OwnersVisitor)
    }
}

/// One point, with the owner and position it came from, for the error message.
struct Entry<'a> {
    owner: &'a str,
    index: usize,
    value: &'a Value,
}

impl Entry<'_> {
    fn bad(&self, field: &'static str) -> JeveError {
        JeveError::TrackerBadData {
            owner: self.owner.to_owned(),
            index: self.index,
            field,
        }
    }

    /// A number in jEve's JSON, read from its text so no float is ever made.
    fn money(&self, value: Option<&Value>, field: &'static str) -> Result<Money, JeveError> {
        match value {
            Some(Value::Number(number)) => {
                Money::parse_decimal_rounded(&number.to_string()).map_err(|_error| self.bad(field))
            }
            _ => Err(self.bad(field)),
        }
    }

    fn required(&self, field: &'static str) -> Result<Money, JeveError> {
        self.money(self.value.get(field), field)
    }

    /// The sum of the `value` of each item of an array, failing when it does not fit.
    fn sum(&self, items: &[Value], field: &'static str) -> Result<Money, JeveError> {
        items.iter().try_fold(Money::ZERO, |total, item| {
            let value = self.money(item.get("value"), field)?;
            total.checked_add(value).ok_or_else(|| self.bad(field))
        })
    }
}

fn record(entry: &Entry<'_>, character: CharacterKey) -> Result<Record, JeveError> {
    let date = entry
        .value
        .get("date")
        .and_then(Value::as_i64)
        .filter(|date| *date >= 0)
        .ok_or_else(|| entry.bad("date"))?;
    // Not negative, so the division rounds down to the second.
    let at = date / MILLIS_PER_SECOND;

    // jEve's reader requires these fields; escrowstocover is checked but not used (ISK still to
    // add to buy orders is not owned).
    let assets_total = entry.required("assets")?;
    let escrows = entry.required("escrows")?;
    entry.required("escrowstocover")?;
    let sellorders = entry.required("sellorders")?;
    let walletbalance = entry.required("walletbalance")?;
    let manufacturing = entry.required("manufacturing")?;
    let contractcollateral = entry.required("contractcollateral")?;
    let contractvalue = entry.required("contractvalue")?;

    // jEve takes the wallet from the divisions when they are listed, and the assets from the
    // asset list when it is one; the totals are the fallback.
    let wallet = match entry.value.get("balance") {
        Some(Value::Array(divisions)) => entry.sum(divisions, "balance")?,
        _ => walletbalance,
    };
    let assets = match entry.value.get("asset") {
        Some(Value::Array(items)) => entry.sum(items, "asset")?,
        _ => assets_total,
    };
    let implants = match entry.value.get("implants") {
        Some(value) => entry.money(Some(value), "implants")?,
        None => Money::ZERO,
    };

    let values = BTreeMap::from([
        (Part::Wallet, wallet),
        (Part::Assets, assets),
        (Part::SellOrders, sellorders),
        (Part::BuyEscrow, escrows),
        (Part::IndustryJobs, manufacturing),
        (Part::ContractCollateral, contractcollateral),
        (Part::Contracts, contractvalue),
        (Part::Implants, implants),
    ]);
    Ok(Record {
        character,
        at,
        snapshot: Snapshot::Imported(values),
    })
}

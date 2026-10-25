//! Public industry data from ESI: every solar system's cost indices, and every item's adjusted and
//! average price. These calls need no login. Each kind keeps its fetch time and the time EVE allows
//! the next one, and the data is saved so job costs and net worth work offline with the last data.

use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use mop_core::industry::{Activity, TypeId};
use mop_core::money::{Money, Rate};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::character_data::{Fetched, Refreshed, fetched};
use crate::client::{Client, bad_data};
use crate::error::EsiError;
use crate::http::Http;
use crate::saved::with_data;
use crate::secrets::write_atomic;

const VERSION: u32 = 1;
const SYSTEMS_PATH: &str = "/industry/systems";
const PRICES_PATH: &str = "/markets/prices";
/// The age used when EVE gives no expiry time: one hour for both kinds.
const DEFAULT_AGE: i64 = 3_600;

/// The cost indices of one solar system. An activity the system does not list is `None`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CostIndices {
    pub manufacturing: Option<Rate>,
    pub reaction: Option<Rate>,
    pub invention: Option<Rate>,
    pub copying: Option<Rate>,
    pub research_me: Option<Rate>,
    pub research_te: Option<Rate>,
}

/// One item's prices. Either is `None` when EVE gives no such price for the item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MarketPrice {
    pub adjusted: Option<Money>,
    pub average: Option<Money>,
}

/// The two kinds of public data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndustryKind {
    Systems,
    Prices,
}

/// The public industry data the app holds. A kind never fetched, or failed, is `None`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IndustryData {
    pub systems: Option<Fetched<BTreeMap<u32, CostIndices>>>,
    pub prices: Option<Fetched<BTreeMap<TypeId, MarketPrice>>>,
}

/// Fetches one kind of public data, unless the data held is still fresh. A failure leaves the kind as it was.
pub fn refresh_industry(
    data: &mut IndustryData,
    kind: IndustryKind,
    http: &dyn Http,
    now: i64,
) -> Result<Refreshed, EsiError> {
    if let Some(until) = next_refresh_held(data, kind)?.filter(|until| now < *until) {
        return Ok(Refreshed::NotYet { until });
    }
    let mut client = Client::new(http);
    match kind {
        IndustryKind::Systems => {
            let answer = client.get_public_json(now, SYSTEMS_PATH, "public/systems")?;
            let systems = systems_of(&answer.body)?;
            data.systems = Some(fetched(systems, &answer, SYSTEMS_PATH, now)?);
        }
        IndustryKind::Prices => {
            let answer = client.get_public_json(now, PRICES_PATH, "public/prices")?;
            let prices = prices_of(&answer.body)?;
            data.prices = Some(fetched(prices, &answer, PRICES_PATH, now)?);
        }
    }
    Ok(Refreshed::Fetched)
}

impl IndustryData {
    /// The cost index of a solar system for a manufacturing or reaction job; `None` when the system or the activity is not listed.
    pub fn cost_index(&self, system: u32, activity: Activity) -> Option<Rate> {
        let indices = self.systems.as_ref()?.data.get(&system)?;
        match activity {
            Activity::Manufacturing => indices.manufacturing,
            Activity::Reaction => indices.reaction,
        }
    }

    /// The adjusted price of every item that has one.
    pub fn adjusted_prices(&self) -> BTreeMap<TypeId, Money> {
        self.prices
            .as_ref()
            .map(|held| {
                held.data
                    .iter()
                    .filter_map(|(type_id, price)| price.adjusted.map(|money| (*type_id, money)))
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// Writes the public data to `<dir>/industry.json`, replacing the file. A kind not fetched yet is written as absent.
pub fn save_industry(dir: &Path, data: &IndustryData) -> Result<(), EsiError> {
    let path = file_path(dir);
    let file = File {
        version: VERSION,
        systems: data.systems.as_ref().map(|held| {
            let saved = held
                .data
                .iter()
                .map(|(system, indices)| SavedSystem::new(*system, indices))
                .collect();
            with_data(held, saved)
        }),
        prices: data.prices.as_ref().map(|held| {
            let saved = held
                .data
                .iter()
                .map(|(type_id, price)| SavedPrice {
                    type_id: *type_id,
                    adjusted: price.adjusted.map(Money::ten_thousandths),
                    average: price.average.map(Money::ten_thousandths),
                })
                .collect();
            with_data(held, saved)
        }),
    };
    let bytes =
        serde_json::to_vec(&file).map_err(|error| EsiError::bad_file(&path, error.to_string()))?;
    write_atomic(&path, &bytes, false)
}

/// Reads the public data saved in `<dir>/industry.json`. No file gives empty data; a file that cannot be read, or has another version, is `BadFile` and is left as it is.
pub fn load_industry(dir: &Path) -> Result<IndustryData, EsiError> {
    let path = file_path(dir);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(IndustryData::default()),
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
    Ok(into_data(file))
}

fn next_refresh_held(data: &IndustryData, kind: IndustryKind) -> Result<Option<i64>, EsiError> {
    match kind {
        IndustryKind::Systems => data
            .systems
            .as_ref()
            .map(|held| held.next_refresh(DEFAULT_AGE))
            .transpose(),
        IndustryKind::Prices => data
            .prices
            .as_ref()
            .map(|held| held.next_refresh(DEFAULT_AGE))
            .transpose(),
    }
}

fn systems_of(body: &Value) -> Result<BTreeMap<u32, CostIndices>, EsiError> {
    let mut systems = BTreeMap::new();
    for row in rows(body, SYSTEMS_PATH)? {
        let system = id_field(row, "solar_system_id", SYSTEMS_PATH)?;
        if systems.contains_key(&system) {
            return Err(bad_data(
                SYSTEMS_PATH,
                "solar_system_id",
                &system.to_string(),
            ));
        }
        systems.insert(system, indices_of(row)?);
    }
    Ok(systems)
}

fn indices_of(row: &Value) -> Result<CostIndices, EsiError> {
    let entries = row
        .get("cost_indices")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            bad_data(
                SYSTEMS_PATH,
                "cost_indices",
                &text_of(row.get("cost_indices")),
            )
        })?;
    let mut indices = CostIndices::default();
    for entry in entries {
        let activity = entry
            .get("activity")
            .and_then(Value::as_str)
            .ok_or_else(|| bad_data(SYSTEMS_PATH, "activity", &text_of(entry.get("activity"))))?;
        let Some(slot) = slot_of(&mut indices, activity) else {
            continue;
        };
        if slot.is_some() {
            return Err(bad_data(SYSTEMS_PATH, "activity", activity));
        }
        let text = number_text(entry.get("cost_index"), SYSTEMS_PATH, "cost_index")?;
        let rate = Rate::parse_fraction(&text)
            .ok()
            .ok_or_else(|| bad_data(SYSTEMS_PATH, "cost_index", &text))?;
        *slot = Some(rate);
    }
    Ok(indices)
}

/// The field of the cost indices that an activity name fills; `None` for an activity the app does not use.
fn slot_of<'i>(indices: &'i mut CostIndices, activity: &str) -> Option<&'i mut Option<Rate>> {
    match activity {
        "manufacturing" => Some(&mut indices.manufacturing),
        "reaction" => Some(&mut indices.reaction),
        "invention" => Some(&mut indices.invention),
        "copying" => Some(&mut indices.copying),
        "researching_material_efficiency" => Some(&mut indices.research_me),
        "researching_time_efficiency" => Some(&mut indices.research_te),
        _ => None,
    }
}

fn prices_of(body: &Value) -> Result<BTreeMap<TypeId, MarketPrice>, EsiError> {
    let mut prices = BTreeMap::new();
    for row in rows(body, PRICES_PATH)? {
        let type_id = id_field(row, "type_id", PRICES_PATH)?;
        if prices.contains_key(&type_id) {
            return Err(bad_data(PRICES_PATH, "type_id", &type_id.to_string()));
        }
        let price = MarketPrice {
            adjusted: money_field(row, "adjusted_price")?,
            average: money_field(row, "average_price")?,
        };
        prices.insert(type_id, price);
    }
    Ok(prices)
}

/// A price field read to 0.0001 ISK. An absent or null field is `None`; a negative price is refused.
fn money_field(row: &Value, field: &'static str) -> Result<Option<Money>, EsiError> {
    let value = match row.get(field) {
        None | Some(Value::Null) => return Ok(None),
        Some(value) => value,
    };
    let text = number_text(Some(value), PRICES_PATH, field)?;
    let money = Money::parse_decimal_rounded(&text)
        .ok()
        .ok_or_else(|| bad_data(PRICES_PATH, field, &text))?;
    if money < Money::ZERO {
        return Err(bad_data(PRICES_PATH, field, &text));
    }
    Ok(Some(money))
}

fn rows<'v>(body: &'v Value, route: &str) -> Result<&'v [Value], EsiError> {
    body.as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| bad_data(route, "body", "not a list"))
}

fn id_field(row: &Value, field: &'static str, route: &str) -> Result<u32, EsiError> {
    row.get(field)
        .and_then(Value::as_u64)
        .and_then(|id| u32::try_from(id).ok())
        .ok_or_else(|| bad_data(route, field, &text_of(row.get(field))))
}

/// A JSON number as EVE wrote it, digits kept exactly; any other value is refused.
fn number_text(
    value: Option<&Value>,
    route: &str,
    field: &'static str,
) -> Result<String, EsiError> {
    match value {
        Some(Value::Number(number)) => Ok(number.to_string()),
        _ => Err(bad_data(route, field, &text_of(value))),
    }
}

fn text_of(value: Option<&Value>) -> String {
    value.map_or_else(String::new, Value::to_string)
}

/// `<dir>/industry.json`.
fn file_path(dir: &Path) -> PathBuf {
    dir.join("industry.json")
}

#[derive(Serialize, Deserialize)]
struct Header {
    version: u32,
}

/// The whole file, version 1. Rates are parts per million; money is ten-thousandths.
#[derive(Serialize, Deserialize)]
struct File {
    version: u32,
    systems: Option<Fetched<Vec<SavedSystem>>>,
    prices: Option<Fetched<Vec<SavedPrice>>>,
}

#[derive(Serialize, Deserialize)]
struct SavedSystem {
    solar_system_id: u32,
    manufacturing: Option<u32>,
    reaction: Option<u32>,
    invention: Option<u32>,
    copying: Option<u32>,
    research_me: Option<u32>,
    research_te: Option<u32>,
}

#[derive(Serialize, Deserialize)]
struct SavedPrice {
    type_id: TypeId,
    adjusted: Option<i64>,
    average: Option<i64>,
}

impl SavedSystem {
    fn new(solar_system_id: u32, indices: &CostIndices) -> Self {
        Self {
            solar_system_id,
            manufacturing: indices.manufacturing.map(Rate::ppm),
            reaction: indices.reaction.map(Rate::ppm),
            invention: indices.invention.map(Rate::ppm),
            copying: indices.copying.map(Rate::ppm),
            research_me: indices.research_me.map(Rate::ppm),
            research_te: indices.research_te.map(Rate::ppm),
        }
    }

    fn indices(&self) -> CostIndices {
        CostIndices {
            manufacturing: self.manufacturing.map(Rate::from_ppm),
            reaction: self.reaction.map(Rate::from_ppm),
            invention: self.invention.map(Rate::from_ppm),
            copying: self.copying.map(Rate::from_ppm),
            research_me: self.research_me.map(Rate::from_ppm),
            research_te: self.research_te.map(Rate::from_ppm),
        }
    }
}

fn into_data(file: File) -> IndustryData {
    IndustryData {
        systems: file.systems.map(|held| {
            let systems = held
                .data
                .iter()
                .map(|saved| (saved.solar_system_id, saved.indices()))
                .collect();
            with_data(&held, systems)
        }),
        prices: file.prices.map(|held| {
            let prices = held
                .data
                .iter()
                .map(|saved| {
                    let price = MarketPrice {
                        adjusted: saved.adjusted.map(Money::from_ten_thousandths),
                        average: saved.average.map(Money::from_ten_thousandths),
                    };
                    (saved.type_id, price)
                })
                .collect();
            with_data(&held, prices)
        }),
    }
}

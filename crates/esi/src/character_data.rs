//! One character's assets, market orders, industry jobs and wallet from ESI. Each kind keeps the
//! time it was fetched and the time EVE allows the next fetch, and a kind is only fetched when its
//! time has come.

use std::collections::BTreeMap;

use mop_core::industry::TypeId;
use mop_core::money::Money;
use mop_core::stock::{Asset, CharacterAssets, ItemId};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::auth::Auth;
use crate::client::{Answer, Client, bad_data};
use crate::error::EsiError;

const DAY_SECONDS: i64 = 86_400;
/// ESI takes at most this many item ids in one names request.
const NAME_BATCH: usize = 1_000;
/// A list claiming more pages than this is refused, so a bad header cannot start thousands of requests.
const MAX_PAGES: u32 = 1_000;

/// One kind of data as it was fetched: the data, the fetch time, and EVE's own times when it gave them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fetched<T> {
    pub data: T,
    pub fetched_at: i64,
    pub last_modified: Option<i64>,
    pub expires: Option<i64>,
}

impl<T> Fetched<T> {
    /// When this data may be fetched again: EVE's expiry time, or `default_age` after the fetch when EVE gave none.
    pub fn next_refresh(&self, default_age: i64) -> Result<i64, EsiError> {
        match self.expires {
            Some(expires) => Ok(expires),
            None => self
                .fetched_at
                .checked_add(default_age)
                .ok_or_else(|| bad_data("", "fetched_at", &self.fetched_at.to_string())),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Assets,
    Orders,
    Jobs,
    Wallet,
}

impl Kind {
    pub const ALL: [Kind; 4] = [Kind::Assets, Kind::Orders, Kind::Jobs, Kind::Wallet];

    /// The age used when EVE gives no expiry time: assets 1 hour, orders 20 minutes, jobs 5 minutes, wallet 2 minutes.
    fn default_age(self) -> i64 {
        match self {
            Kind::Assets => 3_600,
            Kind::Orders => 1_200,
            Kind::Jobs => 300,
            Kind::Wallet => 120,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Kind::Assets => "assets",
            Kind::Orders => "orders",
            Kind::Jobs => "jobs",
            Kind::Wallet => "wallet",
        }
    }
}

/// The character's assets, and the names players gave to the singleton ones (ships, containers).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedAssets {
    pub assets: CharacterAssets,
    pub names: BTreeMap<ItemId, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Order {
    pub order_id: i64,
    pub type_id: TypeId,
    pub location_id: i64,
    pub region_id: u32,
    pub is_buy: bool,
    #[serde(with = "crate::saved::money")]
    pub price: Money,
    pub volume_remain: u64,
    pub volume_total: u64,
    pub issued: i64,
    pub expires: i64,
    pub range: String,
    #[serde(default, with = "crate::saved::money_opt")]
    pub escrow: Option<Money>,
    pub min_volume: Option<u64>,
    pub is_corporation: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobStatus {
    Active,
    Cancelled,
    Delivered,
    Paused,
    Ready,
    Reverted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Job {
    pub job_id: i64,
    pub activity_id: u32,
    pub blueprint_type_id: TypeId,
    pub product_type_id: Option<TypeId>,
    pub runs: u32,
    pub status: JobStatus,
    pub start: i64,
    pub end: i64,
    pub facility_id: i64,
    pub station_id: i64,
    pub output_location_id: i64,
    pub installer_id: i64,
    #[serde(default, with = "crate::saved::money_opt")]
    pub cost: Option<Money>,
    pub licensed_runs: Option<u32>,
}

/// Everything the app holds for one character. A kind that was never fetched, or failed, is `None`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CharacterData {
    pub character_id: u64,
    pub assets: Option<Fetched<OwnedAssets>>,
    pub orders: Option<Fetched<Vec<Order>>>,
    pub jobs: Option<Fetched<Vec<Job>>>,
    pub wallet: Option<Fetched<Money>>,
}

impl CharacterData {
    pub fn new(character_id: u64) -> Self {
        Self {
            character_id,
            assets: None,
            orders: None,
            jobs: None,
            wallet: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refreshed {
    Fetched,
    /// Fetched recently enough: nothing was sent; the kind may be fetched again at `until`.
    NotYet {
        until: i64,
    },
}

/// Fetches one kind of data for one character, unless the data held is still fresh. A failure leaves the kind as it was.
pub fn refresh(
    data: &mut CharacterData,
    kind: Kind,
    auth: &mut Auth,
    client: &mut Client<'_>,
    now: i64,
) -> Result<Refreshed, EsiError> {
    if let Some(until) = next_refresh_held(data, kind)?.filter(|until| now < *until) {
        return Ok(Refreshed::NotYet { until });
    }
    let character = data.character_id;
    match kind {
        Kind::Assets => data.assets = Some(fetch_assets(auth, client, character, now)?),
        Kind::Orders => data.orders = Some(fetch_orders(auth, client, character, now)?),
        Kind::Jobs => data.jobs = Some(fetch_jobs(auth, client, character, now)?),
        Kind::Wallet => data.wallet = Some(fetch_wallet(auth, client, character, now)?),
    }
    Ok(Refreshed::Fetched)
}

/// Refreshes every kind in order and gives one result per kind. After an `ErrorLimited` answer the
/// kinds after it are not sent; each of them gets the same `ErrorLimited`.
pub fn refresh_all(
    data: &mut CharacterData,
    auth: &mut Auth,
    client: &mut Client<'_>,
    now: i64,
) -> Vec<(Kind, Result<Refreshed, EsiError>)> {
    let mut results = Vec::with_capacity(Kind::ALL.len());
    let mut limit: Option<u64> = None;
    for kind in Kind::ALL {
        if let Some(reset_seconds) = limit {
            results.push((kind, Err(EsiError::ErrorLimited { reset_seconds })));
            continue;
        }
        let result = refresh(data, kind, auth, client, now);
        if let Err(EsiError::ErrorLimited { reset_seconds }) = &result {
            limit = Some(*reset_seconds);
        }
        results.push((kind, result));
    }
    results
}

fn next_refresh_held(data: &CharacterData, kind: Kind) -> Result<Option<i64>, EsiError> {
    let age = kind.default_age();
    match kind {
        Kind::Assets => data
            .assets
            .as_ref()
            .map(|held| held.next_refresh(age))
            .transpose(),
        Kind::Orders => data
            .orders
            .as_ref()
            .map(|held| held.next_refresh(age))
            .transpose(),
        Kind::Jobs => data
            .jobs
            .as_ref()
            .map(|held| held.next_refresh(age))
            .transpose(),
        Kind::Wallet => data
            .wallet
            .as_ref()
            .map(|held| held.next_refresh(age))
            .transpose(),
    }
}

fn group(character: u64, kind: Kind) -> String {
    format!("{character}/{}", kind.name())
}

/// Wraps `data` with the fetch time and expiry taken from the answer's headers.
pub(crate) fn fetched<T>(
    data: T,
    answer: &Answer,
    route: &str,
    now: i64,
) -> Result<Fetched<T>, EsiError> {
    Ok(Fetched {
        data,
        fetched_at: now,
        last_modified: answer.http_date("last-modified", route)?,
        expires: answer.http_date("expires", route)?,
    })
}

fn fetch_assets(
    auth: &mut Auth,
    client: &mut Client<'_>,
    character: u64,
    now: i64,
) -> Result<Fetched<OwnedAssets>, EsiError> {
    let route = format!("/characters/{character}/assets");
    let group = group(character, Kind::Assets);
    let token = auth.access_token(character, now)?;
    let mut run = 1;
    let (page_one, rows) = loop {
        let page_one = client.get_json(now, &token, &format!("{route}?page=1"), &group)?;
        let pages = page_count(&page_one, &route)?;
        let last_modified = page_one.http_date("last-modified", &route)?;
        let mut rows = rows_of(&page_one, &route)?.to_vec();
        let mut changed = false;
        for page in 2..=pages {
            let answer = client.get_json(now, &token, &format!("{route}?page={page}"), &group)?;
            if answer.http_date("last-modified", &route)? != last_modified {
                changed = true;
                break;
            }
            rows.extend_from_slice(rows_of(&answer, &route)?);
        }
        if !changed {
            break (page_one, rows);
        }
        if run == 2 {
            return Err(EsiError::Inconsistent { route });
        }
        run += 1;
    };
    let assets = rows
        .iter()
        .map(|row| asset(row, &route))
        .collect::<Result<Vec<_>, _>>()?;
    let singletons: Vec<ItemId> = assets
        .iter()
        .filter(|asset| asset.singleton)
        .map(|asset| asset.item_id)
        .collect();
    let names = fetch_names(auth, client, character, &singletons, now)?;
    let owned = OwnedAssets {
        assets: CharacterAssets {
            character_id: i64::try_from(character)
                .ok()
                .ok_or_else(|| bad_data(&route, "character_id", &character.to_string()))?,
            updated: Some(page_one.http_date("last-modified", &route)?.unwrap_or(now)),
            assets,
        },
        names,
    };
    fetched(owned, &page_one, &route, now)
}

/// The names of the given items, in batches. A batch EVE answers with an error stays unnamed; the assets are still fetched.
fn fetch_names(
    auth: &mut Auth,
    client: &mut Client<'_>,
    character: u64,
    items: &[ItemId],
    now: i64,
) -> Result<BTreeMap<ItemId, String>, EsiError> {
    let route = format!("/characters/{character}/assets/names");
    let group = group(character, Kind::Assets);
    let mut names = BTreeMap::new();
    for batch in items.chunks(NAME_BATCH) {
        let token = auth.access_token(character, now)?;
        let found = client
            .post_json(now, &token, &route, &group, &json!(batch))
            .and_then(|answer| name_pairs(&answer.body, &route));
        if let Ok(found) = found {
            names.extend(found);
        }
    }
    Ok(names)
}

fn name_pairs(body: &Value, route: &str) -> Result<Vec<(ItemId, String)>, EsiError> {
    let mut pairs = Vec::new();
    for row in array(body, route)? {
        let Some(name) = row
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
        else {
            continue;
        };
        pairs.push((signed(row, "item_id", route)?, name.to_owned()));
    }
    Ok(pairs)
}

fn fetch_orders(
    auth: &mut Auth,
    client: &mut Client<'_>,
    character: u64,
    now: i64,
) -> Result<Fetched<Vec<Order>>, EsiError> {
    let route = format!("/characters/{character}/orders");
    let token = auth.access_token(character, now)?;
    let answer = client.get_json(now, &token, &route, &group(character, Kind::Orders))?;
    let orders = rows_of(&answer, &route)?
        .iter()
        .map(|row| order(row, &route))
        .collect::<Result<Vec<_>, _>>()?;
    fetched(orders, &answer, &route, now)
}

fn fetch_jobs(
    auth: &mut Auth,
    client: &mut Client<'_>,
    character: u64,
    now: i64,
) -> Result<Fetched<Vec<Job>>, EsiError> {
    let route = format!("/characters/{character}/industry/jobs");
    let token = auth.access_token(character, now)?;
    let path = format!("{route}?include_completed=true");
    let answer = client.get_json(now, &token, &path, &group(character, Kind::Jobs))?;
    let jobs = rows_of(&answer, &route)?
        .iter()
        .map(|row| job(row, &route))
        .collect::<Result<Vec<_>, _>>()?;
    fetched(jobs, &answer, &route, now)
}

fn fetch_wallet(
    auth: &mut Auth,
    client: &mut Client<'_>,
    character: u64,
    now: i64,
) -> Result<Fetched<Money>, EsiError> {
    let route = format!("/characters/{character}/wallet");
    let token = auth.access_token(character, now)?;
    let answer = client.get_json(now, &token, &route, &group(character, Kind::Wallet))?;
    let balance = money_of(&answer.body, "wallet", &route)?;
    fetched(balance, &answer, &route, now)
}

/// The number of pages an answer says it has; a missing header means one page.
fn page_count(answer: &Answer, route: &str) -> Result<u32, EsiError> {
    let Some(text) = answer.header("x-pages") else {
        return Ok(1);
    };
    match text.trim().parse::<u32>() {
        Ok(pages) if (1..=MAX_PAGES).contains(&pages) => Ok(pages),
        _ => Err(bad_data(route, "x-pages", text)),
    }
}

fn rows_of<'a>(answer: &'a Answer, route: &str) -> Result<&'a [Value], EsiError> {
    array(&answer.body, route).map(Vec::as_slice)
}

/// The body as a JSON array. Any other body is `BadData`.
pub(crate) fn array<'a>(body: &'a Value, route: &str) -> Result<&'a Vec<Value>, EsiError> {
    body.as_array()
        .ok_or_else(|| bad_data(route, "body", &body.to_string()))
}

fn asset(row: &Value, route: &str) -> Result<Asset, EsiError> {
    Ok(Asset {
        item_id: signed(row, "item_id", route)?,
        type_id: unsigned(row, "type_id", route)?,
        location_id: signed(row, "location_id", route)?,
        flag: Some(text(row, "location_flag", route)?.to_owned()),
        quantity: unsigned(row, "quantity", route)?,
        singleton: flag(row, "is_singleton", route)?,
    })
}

fn order(row: &Value, route: &str) -> Result<Order, EsiError> {
    let issued = time(row, "issued", route)?;
    let duration: u32 = unsigned(row, "duration", route)?;
    let expires = i64::from(duration)
        .checked_mul(DAY_SECONDS)
        .and_then(|span| issued.checked_add(span))
        .ok_or_else(|| bad_data(route, "duration", &duration.to_string()))?;
    Ok(Order {
        order_id: signed(row, "order_id", route)?,
        type_id: unsigned(row, "type_id", route)?,
        location_id: signed(row, "location_id", route)?,
        region_id: unsigned(row, "region_id", route)?,
        is_buy: flag_or(row, "is_buy_order", false, route)?,
        price: money(row, "price", route)?,
        volume_remain: unsigned(row, "volume_remain", route)?,
        volume_total: unsigned(row, "volume_total", route)?,
        issued,
        expires,
        range: text(row, "range", route)?.to_owned(),
        escrow: optional(row, "escrow")
            .map(|value| money_of(value, "escrow", route))
            .transpose()?,
        min_volume: optional_unsigned(row, "min_volume", route)?,
        is_corporation: flag(row, "is_corporation", route)?,
    })
}

fn job(row: &Value, route: &str) -> Result<Job, EsiError> {
    Ok(Job {
        job_id: signed(row, "job_id", route)?,
        activity_id: unsigned(row, "activity_id", route)?,
        blueprint_type_id: unsigned(row, "blueprint_type_id", route)?,
        product_type_id: optional_unsigned(row, "product_type_id", route)?,
        runs: unsigned(row, "runs", route)?,
        status: job_status(row, route)?,
        start: time(row, "start_date", route)?,
        end: time(row, "end_date", route)?,
        facility_id: signed(row, "facility_id", route)?,
        station_id: signed(row, "station_id", route)?,
        output_location_id: signed(row, "output_location_id", route)?,
        installer_id: signed(row, "installer_id", route)?,
        cost: optional(row, "cost")
            .map(|value| money_of(value, "cost", route))
            .transpose()?,
        licensed_runs: optional_unsigned(row, "licensed_runs", route)?,
    })
}

fn job_status(row: &Value, route: &str) -> Result<JobStatus, EsiError> {
    match text(row, "status", route)? {
        "active" => Ok(JobStatus::Active),
        "cancelled" => Ok(JobStatus::Cancelled),
        "delivered" => Ok(JobStatus::Delivered),
        "paused" => Ok(JobStatus::Paused),
        "ready" => Ok(JobStatus::Ready),
        "reverted" => Ok(JobStatus::Reverted),
        other => Err(bad_data(route, "status", other)),
    }
}

/// The field `key` of an object. A missing field is `BadData`.
pub(crate) fn field<'a>(
    object: &'a Value,
    key: &'static str,
    route: &str,
) -> Result<&'a Value, EsiError> {
    object
        .get(key)
        .ok_or_else(|| bad_data(route, key, "missing"))
}

/// The field `key` when it is present and not null.
fn optional<'a>(object: &'a Value, key: &str) -> Option<&'a Value> {
    object.get(key).filter(|value| !value.is_null())
}

pub(crate) fn signed<T: TryFrom<i64>>(
    object: &Value,
    key: &'static str,
    route: &str,
) -> Result<T, EsiError> {
    signed_of(field(object, key, route)?, key, route)
}

fn signed_of<T: TryFrom<i64>>(
    value: &Value,
    key: &'static str,
    route: &str,
) -> Result<T, EsiError> {
    value
        .as_i64()
        .and_then(|number| T::try_from(number).ok())
        .ok_or_else(|| bad_data(route, key, &shown(value)))
}

pub(crate) fn unsigned<T: TryFrom<u64>>(
    object: &Value,
    key: &'static str,
    route: &str,
) -> Result<T, EsiError> {
    unsigned_of(field(object, key, route)?, key, route)
}

fn unsigned_of<T: TryFrom<u64>>(
    value: &Value,
    key: &'static str,
    route: &str,
) -> Result<T, EsiError> {
    value
        .as_u64()
        .and_then(|number| T::try_from(number).ok())
        .ok_or_else(|| bad_data(route, key, &shown(value)))
}

fn optional_unsigned<T: TryFrom<u64>>(
    object: &Value,
    key: &'static str,
    route: &str,
) -> Result<Option<T>, EsiError> {
    optional(object, key)
        .map(|value| unsigned_of(value, key, route))
        .transpose()
}

pub(crate) fn text<'a>(
    object: &'a Value,
    key: &'static str,
    route: &str,
) -> Result<&'a str, EsiError> {
    let value = field(object, key, route)?;
    value
        .as_str()
        .ok_or_else(|| bad_data(route, key, &shown(value)))
}

fn flag(object: &Value, key: &'static str, route: &str) -> Result<bool, EsiError> {
    let value = field(object, key, route)?;
    value
        .as_bool()
        .ok_or_else(|| bad_data(route, key, &shown(value)))
}

fn flag_or(
    object: &Value,
    key: &'static str,
    default: bool,
    route: &str,
) -> Result<bool, EsiError> {
    optional(object, key).map_or(Ok(default), |value| {
        value
            .as_bool()
            .ok_or_else(|| bad_data(route, key, &shown(value)))
    })
}

fn money(object: &Value, key: &'static str, route: &str) -> Result<Money, EsiError> {
    money_of(field(object, key, route)?, key, route)
}

/// Money as EVE writes it, read exactly: the number's own text is parsed, never a float.
fn money_of(value: &Value, key: &'static str, route: &str) -> Result<Money, EsiError> {
    match value {
        Value::Number(number) => Money::parse_decimal(&number.to_string())
            .ok()
            .ok_or_else(|| bad_data(route, key, &number.to_string())),
        Value::Null | Value::Bool(_) | Value::String(_) | Value::Array(_) | Value::Object(_) => {
            Err(bad_data(route, key, &shown(value)))
        }
    }
}

/// An RFC 3339 UTC time (`2026-10-19T09:30:00Z`) as Unix seconds.
fn time(object: &Value, key: &'static str, route: &str) -> Result<i64, EsiError> {
    let value = text(object, key, route)?;
    utc_seconds(value).ok_or_else(|| bad_data(route, key, value))
}

pub(crate) fn utc_seconds(text: &str) -> Option<i64> {
    let bytes = text.as_bytes();
    if bytes.len() != 20 || bytes.get(19) != Some(&b'Z') {
        return None;
    }
    let digits = |range: std::ops::Range<usize>| -> Option<i64> {
        let part = text.get(range)?;
        if !part.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        part.parse().ok()
    };
    let separators_ok = bytes.get(4) == Some(&b'-')
        && bytes.get(7) == Some(&b'-')
        && bytes.get(10) == Some(&b'T')
        && bytes.get(13) == Some(&b':')
        && bytes.get(16) == Some(&b':');
    if !separators_ok {
        return None;
    }
    let (year, month, day) = (digits(0..4)?, digits(5..7)?, digits(8..10)?);
    let (hour, minute, second) = (digits(11..13)?, digits(14..16)?, digits(17..19)?);
    if !(1..=12).contains(&month)
        || !(1..=days_in_month(year, month)).contains(&day)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return None;
    }
    days_from_civil(year, month, day)?
        .checked_mul(DAY_SECONDS)?
        .checked_add(hour * 3_600 + minute * 60 + second)
}

fn days_in_month(year: i64, month: i64) -> i64 {
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days from 1970-01-01 to the given civil date (proleptic Gregorian calendar).
fn days_from_civil(year: i64, month: i64, day: i64) -> Option<i64> {
    let year = if month <= 2 {
        year.checked_sub(1)?
    } else {
        year
    };
    let era = year.div_euclid(400);
    let year_of_era = year.checked_sub(era.checked_mul(400)?)?;
    let shifted_month = (month + 9) % 12;
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era
        .checked_mul(365)?
        .checked_add(year_of_era / 4)?
        .checked_sub(year_of_era / 100)?
        .checked_add(day_of_year)?;
    era.checked_mul(146_097)?
        .checked_add(day_of_era)?
        .checked_sub(719_468)
}

/// A value as text for an error message: a string without its quotes, anything else as JSON.
fn shown(value: &Value) -> String {
    if let Value::String(text) = value {
        text.clone()
    } else {
        value.to_string()
    }
}

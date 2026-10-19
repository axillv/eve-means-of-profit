//! Reads the stockpiles in jEve's stockpile XML: the `stockpiles` element of jEve's export or of its
//! own settings file. The file is only read. Every stockpile comes in; what the app cannot hold is
//! left out, and each stockpile lists what it lost or changed.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io::Read;
use std::path::Path;

use profit_core::industry::TypeId;
use profit_core::stock::CharacterId;
use profit_core::stockpile::{
    Characters, Filter, Include, Place, Source, Stockpile, Target, include_loop,
};
use roxmltree::Node;

use crate::JeveError;

/// The largest file that is read. A bigger one is refused before it is parsed.
const MAX_FILE_BYTES: u64 = 50 * 1024 * 1024;

/// Exponents are capped here. Beyond it a number is either zero or far past what the app holds,
/// so the cap cannot change the outcome.
const EXPONENT_CAP: i128 = 1_000_000_000_000;

/// The most digits before the point that still fit a `u128` ceiling with room to spare.
const MAX_DIGITS_BEFORE_POINT: i128 = 20;

/// Attributes that hold one filter on a stockpile in files from older jEve versions.
const OLD_FILTER_ATTRIBUTES: [&str; 8] = [
    "characterid",
    "locationid",
    "container",
    "flagid",
    "inventory",
    "sellorders",
    "buyorders",
    "jobs",
];

/// What a stockpile file gave: the stockpiles in file order, and what could not be held.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StockpileImport {
    pub stockpiles: Vec<ImportedStockpile>,
    pub left_out: Vec<LeftOut>,
}

/// One stockpile. `includes` names the included stockpile by its final name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedStockpile {
    pub name: String,
    pub group: Option<String>,
    pub multiplier: u32,
    pub targets: Vec<Target>,
    pub includes: Vec<(String, u32)>,
    pub filters: Vec<Filter>,
}

/// One thing that was left out of, or changed in, the stockpile named `stockpile` (its final name).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeftOut {
    pub stockpile: String,
    pub what: String,
}

/// Reads the stockpile file at `path`. `existing` are the names the app already has; a name taken
/// by one of them, or by an earlier stockpile of the file, gets the next free number.
pub fn read_stockpiles_file(
    path: &Path,
    existing: &[String],
    type_name: &dyn Fn(TypeId) -> Option<String>,
) -> Result<StockpileImport, JeveError> {
    let io = |source: std::io::Error| JeveError::Io {
        path: path.to_path_buf(),
        source,
    };
    let file = std::fs::File::open(path).map_err(io)?;
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(io)?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_FILE_BYTES {
        return Err(bad("the file is larger than 50 MB"));
    }
    let text = std::str::from_utf8(&bytes)
        .ok()
        .ok_or_else(|| bad("not an XML file"))?;
    read_stockpiles(text, existing, type_name)
}

/// Reads the stockpiles in jEve's stockpile XML `xml`. See `read_stockpiles_file` for `existing`.
pub fn read_stockpiles(
    xml: &str,
    existing: &[String],
    type_name: &dyn Fn(TypeId) -> Option<String>,
) -> Result<StockpileImport, JeveError> {
    let document = roxmltree::Document::parse(xml)
        .ok()
        .ok_or_else(|| bad("not an XML file"))?;
    let root = document.root_element();
    if root.tag_name().name() != "settings" {
        return Err(bad("not a jEve settings or stockpile file"));
    }
    let list = root
        .children()
        .find(|node| node.has_tag_name("stockpiles"))
        .ok_or_else(|| bad("the file holds no stockpiles"))?;

    let mut taken: BTreeSet<String> = existing.iter().map(|name| name.to_lowercase()).collect();
    let mut all: BTreeMap<i64, Stockpile> = BTreeMap::new();
    let mut entries: BTreeMap<i64, Entry> = BTreeMap::new();
    for node in list
        .children()
        .filter(|child| child.has_tag_name("stockpile"))
    {
        let id = i64::try_from(all.len())
            .ok()
            .ok_or_else(|| bad("the file holds too many stockpiles"))?;
        let (stockpile, entry) = read_stockpile(node, id, &mut taken, type_name)?;
        all.insert(id, stockpile);
        entries.insert(id, entry);
    }

    // Subpiles are resolved only now, when every name in the file is final.
    let ids: Vec<i64> = all.keys().copied().collect();
    for id in ids {
        let pending = entries
            .get_mut(&id)
            .map(|entry| std::mem::take(&mut entry.pending))
            .unwrap_or_default();
        for include in pending {
            resolve(id, &include, &mut all, &mut entries);
        }
    }

    let names: BTreeMap<i64, String> = all
        .iter()
        .map(|(id, stockpile)| (*id, stockpile.name.clone()))
        .collect();
    let mut stockpiles = Vec::new();
    let mut left_out = Vec::new();
    for (id, stockpile) in all {
        let Some(entry) = entries.remove(&id) else {
            continue;
        };
        for what in entry.notes.into_lines() {
            left_out.push(LeftOut {
                stockpile: stockpile.name.clone(),
                what,
            });
        }
        let includes = stockpile
            .includes
            .iter()
            .filter_map(|include| {
                names
                    .get(&include.stockpile_id)
                    .map(|name| (name.clone(), include.count))
            })
            .collect();
        stockpiles.push(ImportedStockpile {
            name: stockpile.name,
            group: stockpile.group,
            multiplier: stockpile.multiplier,
            targets: stockpile.targets,
            includes,
            filters: stockpile.filters,
        });
    }
    Ok(StockpileImport {
        stockpiles,
        left_out,
    })
}

/// What the import keeps for a stockpile while it reads the file.
struct Entry {
    /// The name as the file writes it; subpiles find their stockpile by this.
    written: String,
    notes: Notes,
    /// The subpiles, resolved once every stockpile is read.
    pending: Vec<RawInclude>,
}

/// The notes for one stockpile, kept in the order the owner reads them.
#[derive(Default)]
struct Notes {
    name: Vec<String>,
    multiplier: Vec<String>,
    rows: Vec<String>,
    includes: Vec<String>,
    filters: Vec<String>,
    tail: Vec<String>,
}

impl Notes {
    fn into_lines(self) -> Vec<String> {
        [
            self.name,
            self.multiplier,
            self.rows,
            self.includes,
            self.filters,
            self.tail,
        ]
        .concat()
    }
}

/// A subpile as the file writes it, before its name is resolved.
struct RawInclude {
    name: String,
    text: String,
    minimum: Decimal,
}

fn read_stockpile(
    node: Node,
    id: i64,
    taken: &mut BTreeSet<String>,
    type_name: &dyn Fn(TypeId) -> Option<String>,
) -> Result<(Stockpile, Entry), JeveError> {
    let written = node
        .attribute("name")
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .ok_or_else(|| bad("a stockpile has no name"))?;
    let mut notes = Notes::default();
    let name = claim_name(written, taken, &mut notes.name);
    let group = node
        .attribute("stockpilegroup")
        .map(str::trim)
        .filter(|group| !group.is_empty())
        .map(str::to_owned);
    let multiplier = read_multiplier(node, written, &mut notes.multiplier)?;

    let mut targets = Vec::new();
    let mut pending = Vec::new();
    for child in node.children() {
        if child.has_tag_name("item") {
            read_row(child, written, type_name, &mut targets, &mut notes.rows)?;
        } else if child.has_tag_name("subpile") {
            pending.push(read_subpile(child, written)?);
        }
    }

    // An old single filter, written as attributes of the stockpile, comes first.
    let mut filter_nodes = Vec::new();
    if OLD_FILTER_ATTRIBUTES
        .iter()
        .any(|attribute| node.has_attribute(*attribute))
    {
        filter_nodes.push((node, true));
    }
    filter_nodes.extend(
        node.children()
            .filter(|child| child.has_tag_name("stockpilefilter"))
            .map(|child| (child, false)),
    );
    let found = !filter_nodes.is_empty();
    let mut filters = Vec::new();
    for (number, (filter_node, old)) in (1..).zip(filter_nodes) {
        let report = read_filter(filter_node, number, written, old)?;
        notes.filters.extend(report.notes);
        filters.extend(report.filter);
    }
    if !found {
        // jEve counts everything when a stockpile has no filters.
        filters.push(default_filter());
    } else if filters.is_empty() {
        notes
            .tail
            .push("no filter left: counts nothing until you add one".to_owned());
    }

    let stockpile = Stockpile {
        id,
        name,
        group,
        multiplier,
        targets,
        includes: Vec::new(),
        filters,
    };
    let entry = Entry {
        written: written.to_owned(),
        notes,
        pending,
    };
    Ok((stockpile, entry))
}

/// The name a stockpile is kept under: as written, or the first free `<name> (n)` after it.
fn claim_name(written: &str, taken: &mut BTreeSet<String>, notes: &mut Vec<String>) -> String {
    let mut name = written.to_owned();
    for number in 2.. {
        if !taken.contains(&name.to_lowercase()) {
            break;
        }
        name = format!("{written} ({number})");
    }
    if name != written {
        notes.push(format!("named \"{name}\": \"{written}\" is taken"));
    }
    taken.insert(name.to_lowercase());
    name
}

fn read_multiplier(node: Node, written: &str, notes: &mut Vec<String>) -> Result<u32, JeveError> {
    let Some(text) = node.attribute("multiplier") else {
        return Ok(1);
    };
    let value = number(text, written)?;
    if value.is_zero() {
        notes.push("multiplier 0 set to 1".to_owned());
        return Ok(1);
    }
    match value.ceiling_as::<u32>() {
        Some((units, rounded)) => {
            if rounded {
                notes.push(format!("multiplier {text} rounded up to {units}"));
            }
            Ok(units)
        }
        None => {
            notes.push(format!("multiplier {text} is too big; set to 1"));
            Ok(1)
        }
    }
}

/// One `item` row. Rows the app cannot hold are left out with a note; a repeated type adds up.
fn read_row(
    row: Node,
    written: &str,
    type_name: &dyn Fn(TypeId) -> Option<String>,
    targets: &mut Vec<Target>,
    notes: &mut Vec<String>,
) -> Result<(), JeveError> {
    let type_text = row
        .attribute("typeid")
        .ok_or_else(|| at(written, "a row has no typeid"))?;
    let type_id = match integer(type_text) {
        Some(id) => id,
        // Too big for any type id: the game data cannot have it.
        None if is_big_integer(type_text) => {
            notes.push(format!(
                "type {type_text} is not in the game data; row left out"
            ));
            return Ok(());
        }
        None => return Err(at(written, format!("\"{type_text}\" is not a number"))),
    };
    // jEve's total row, typeid 0, is not a target; it is skipped before anything else is read.
    if type_id == 0 {
        return Ok(());
    }
    let minimum_text = row
        .attribute("minimum")
        .ok_or_else(|| at(written, "a row has no minimum"))?;
    let minimum = number(minimum_text, written)?;
    let runs = flag(row, "runs", "a row", written)?;
    let ignore_multiplier = flag(row, "ignoremultiplier", "a row", written)?;

    let label = type_label(type_id.unsigned_abs(), type_name);
    if type_id.is_negative() {
        notes.push(format!("{label} (copy): blueprint copy row left out"));
        return Ok(());
    }
    if runs {
        notes.push(format!("{label}: blueprint runs row left out"));
        return Ok(());
    }
    if row.has_attribute("producttypeid") {
        notes.push(format!("{label}: blueprint materials row left out"));
        return Ok(());
    }
    let Some((id, name)) = u32::try_from(type_id)
        .ok()
        .and_then(|id| type_name(id).map(|name| (id, name)))
    else {
        notes.push(format!(
            "type {type_id} is not in the game data; row left out"
        ));
        return Ok(());
    };

    if minimum.is_zero() {
        notes.push(format!("{name}: target 0 left out"));
        return Ok(());
    }
    let Some((units, rounded)) = minimum.ceiling_as::<u64>() else {
        notes.push(format!(
            "{name}: {minimum_text} units is out of range; row left out"
        ));
        return Ok(());
    };
    if rounded {
        notes.push(format!(
            "{name}: {minimum_text} units rounded up to {units}"
        ));
    }
    match targets.iter_mut().find(|target| target.type_id == id) {
        Some(existing) if existing.ignore_multiplier == ignore_multiplier => {
            match existing.units.checked_add(units) {
                Some(total) => {
                    existing.units = total;
                    notes.push(format!("{name} listed twice; units added"));
                }
                None => notes.push(format!("{name} listed twice; second row left out")),
            }
        }
        Some(_) => notes.push(format!("{name} listed twice; second row left out")),
        None => targets.push(Target {
            type_id: id,
            units,
            ignore_multiplier,
        }),
    }
    Ok(())
}

fn read_subpile(node: Node, written: &str) -> Result<RawInclude, JeveError> {
    let name = node
        .attribute("name")
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .ok_or_else(|| at(written, "a subpile has no name"))?;
    let text = node
        .attribute("minimum")
        .ok_or_else(|| at(written, "a subpile has no minimum"))?;
    Ok(RawInclude {
        name: name.to_owned(),
        text: text.to_owned(),
        minimum: number(text, written)?,
    })
}

/// Resolves one subpile of stockpile `id`: the stockpile it names, its count, and no loop.
fn resolve(
    id: i64,
    include: &RawInclude,
    all: &mut BTreeMap<i64, Stockpile>,
    entries: &mut BTreeMap<i64, Entry>,
) {
    let wanted = include.name.to_lowercase();
    let target = entries
        .iter()
        .find(|(_, entry)| entry.written.to_lowercase() == wanted)
        .map(|(target, _)| *target);
    let Some(entry) = entries.get_mut(&id) else {
        return;
    };
    let name = &include.name;
    let Some(target) = target else {
        entry.notes.includes.push(format!(
            "includes \"{name}\", which is not in the file; left out"
        ));
        return;
    };
    if include.minimum.is_zero() {
        entry
            .notes
            .includes
            .push(format!("includes \"{name}\" 0 times; left out"));
        return;
    }
    let count = match include.minimum.ceiling_as::<u32>() {
        Some((count, true)) => {
            entry.notes.includes.push(format!(
                "includes \"{name}\" {} times; rounded up to {count}",
                include.text
            ));
            count
        }
        Some((count, false)) => count,
        None => {
            entry.notes.includes.push(format!(
                "includes \"{name}\" {} times is too big; set to 1",
                include.text
            ));
            1
        }
    };
    let Some(owner) = all.get_mut(&id) else {
        return;
    };
    owner.includes.push(Include {
        stockpile_id: target,
        count,
    });
    if include_loop(id, all).is_some() {
        if let Some(owner) = all.get_mut(&id) {
            owner.includes.pop();
        }
        entry.notes.includes.push(format!(
            "includes \"{name}\", which would make a loop; left out"
        ));
    }
}

/// One filter. `old` reads the attributes of a stockpile from an older file, not a filter element.
fn read_filter(node: Node, number: usize, written: &str, old: bool) -> Result<Report, JeveError> {
    let label = format!("filter {number}");
    let place_text = node
        .attribute("locationid")
        .ok_or_else(|| at(written, "a filter has no locationid"))?;
    let place_id = match integer(place_text) {
        Some(id) => id,
        // Too big for any location id: the app does not understand it.
        None if is_big_integer(place_text) => {
            return Ok(dropped(format!(
                "{label}: place {place_text} is not understood; filter left out"
            )));
        }
        None => return Err(at(written, format!("\"{place_text}\" is not a number"))),
    };
    let inventory = required_flag(node, "inventory", &label, written)?;
    let sell_orders = required_flag(node, "sellorders", &label, written)?;
    let buy_orders = required_flag(node, "buyorders", &label, written)?;
    let jobs = required_flag(node, "jobs", &label, written)?;
    let exclude = flag(node, "exclude", &label, written)?;
    let contracts = any_flag(
        node,
        &[
            "sellingcontracts",
            "soldcontracts",
            "buyingcontracts",
            "boughtcontracts",
        ],
        &label,
        written,
    )?;
    let transactions = any_flag(
        node,
        &["buytransactions", "selltransactions"],
        &label,
        written,
    )?;

    if (20_000_000..=20_999_999).contains(&place_id) {
        return Ok(dropped(format!(
            "{label}: constellation {place_id} is not supported; filter left out"
        )));
    }
    let Some(place) = place_of(place_id) else {
        return Ok(dropped(format!(
            "{label}: place {place_id} is not understood; filter left out"
        )));
    };
    if exclude {
        return Ok(dropped(format!(
            "{label}: exclude filter left out (counts more than in jEve)"
        )));
    }

    let mut notes = left_out_notes(node, old, &label, written, contracts, transactions)?;
    let sources = sources_of([
        (inventory, Source::Assets),
        (sell_orders, Source::SellOrders),
        (buy_orders, Source::BuyOrders),
        (jobs, Source::Jobs),
    ]);
    if sources.is_empty() {
        notes.push(format!(
            "{label}: no source the app counts; filter left out"
        ));
        return Ok(Report {
            filter: None,
            notes,
        });
    }
    let owners = owners(node, old, written)?;
    let characters = if owners.is_empty() {
        Characters::All
    } else {
        Characters::Only(owners)
    };
    Ok(Report {
        filter: Some(Filter {
            characters,
            place,
            sources,
        }),
        notes,
    })
}

/// The notes for what a filter counts that the app cannot hold, in the order the owner reads them.
fn left_out_notes(
    node: Node,
    old: bool,
    label: &str,
    written: &str,
    contracts: bool,
    transactions: bool,
) -> Result<Vec<String>, JeveError> {
    let singleton = flag(node, "singleton", label, written)?;
    let (containers, flags) = if old {
        (
            node.has_attribute("container"),
            node.has_attribute("flagid"),
        )
    } else {
        (
            node.children().any(|child| child.has_tag_name("container")),
            node.children().any(|child| child.has_tag_name("flag")),
        )
    };
    let job_days = node.has_attribute("jobsdaysless") || node.has_attribute("jobsdaysmore");
    let lost = [
        (containers, "containers left out"),
        (flags, "hangar flags left out"),
        (contracts, "contracts left out"),
        (transactions, "transactions left out"),
        (singleton, "singleton choice left out"),
        (job_days, "job days left out"),
    ];
    Ok(lost
        .into_iter()
        .filter(|(lost, _)| *lost)
        .map(|(_, what)| format!("{label}: {what}"))
        .collect())
}

/// The sources a filter counts, from its four source switches.
fn sources_of(switches: [(bool, Source); 4]) -> BTreeSet<Source> {
    switches
        .into_iter()
        .filter(|(on, _)| *on)
        .map(|(_, source)| source)
        .collect()
}

/// The characters a filter lists: its owners, or the old single character when it is above 0.
/// Empty means every character.
fn owners(node: Node, old: bool, written: &str) -> Result<BTreeSet<CharacterId>, JeveError> {
    let mut owners = BTreeSet::new();
    if old {
        if let Some(text) = node.attribute("characterid") {
            let id =
                integer(text).ok_or_else(|| at(written, format!("\"{text}\" is not a number")))?;
            if id > 0 {
                owners.insert(id);
            }
        }
    } else {
        for owner in node.children().filter(|child| child.has_tag_name("owner")) {
            let text = owner
                .attribute("ownerid")
                .ok_or_else(|| at(written, "an owner has no ownerid"))?;
            let id =
                integer(text).ok_or_else(|| at(written, format!("\"{text}\" is not a number")))?;
            owners.insert(id);
        }
    }
    Ok(owners)
}

/// A filter the app leaves out, with the one note that says why.
fn dropped(note: String) -> Report {
    Report {
        filter: None,
        notes: vec![note],
    }
}

/// What one filter gave: the filter if it is kept, and its notes.
struct Report {
    filter: Option<Filter>,
    notes: Vec<String>,
}

/// The place a jEve location id means, or `None` when the app does not understand it.
fn place_of(id: i64) -> Option<Place> {
    match id {
        0 => Some(Place::Everywhere),
        10_000_000..=10_999_999 => u32::try_from(id).ok().map(Place::Region),
        30_000_000..=32_999_999 => u32::try_from(id).ok().map(Place::System),
        60_000_000..=63_999_999 | 1_000_000_000_000..=i64::MAX => Some(Place::Location(id)),
        _ => None,
    }
}

fn default_filter() -> Filter {
    Filter {
        characters: Characters::All,
        place: Place::Everywhere,
        sources: [
            Source::Assets,
            Source::SellOrders,
            Source::BuyOrders,
            Source::Jobs,
        ]
        .into_iter()
        .collect(),
    }
}

/// A yes/no attribute: absent means no. Anything but `true` or `false` fails the import.
fn flag(node: Node, attribute: &str, label: &str, written: &str) -> Result<bool, JeveError> {
    match node.attribute(attribute) {
        None | Some("false") => Ok(false),
        Some("true") => Ok(true),
        Some(text) => Err(at(
            written,
            format!("{label} {attribute} is \"{text}\", not true or false"),
        )),
    }
}

/// A yes/no attribute that a filter must carry.
fn required_flag(
    node: Node,
    attribute: &str,
    label: &str,
    written: &str,
) -> Result<bool, JeveError> {
    if !node.has_attribute(attribute) {
        return Err(at(written, format!("a filter has no {attribute}")));
    }
    flag(node, attribute, label, written)
}

/// True when any of the yes/no attributes is `true`. Every attribute is checked, so a bad value
/// fails the import whichever one it is.
fn any_flag(
    node: Node,
    attributes: &[&str],
    label: &str,
    written: &str,
) -> Result<bool, JeveError> {
    let mut any = false;
    for attribute in attributes {
        any |= flag(node, attribute, label, written)?;
    }
    Ok(any)
}

/// How a type is named in a note: its name, or its number when the game data does not have it.
fn type_label(id: u64, type_name: &dyn Fn(TypeId) -> Option<String>) -> String {
    u32::try_from(id)
        .ok()
        .and_then(type_name)
        .unwrap_or_else(|| format!("type {id}"))
}

/// A whole number as jEve writes ids: an optional minus sign, then digits.
fn integer(text: &str) -> Option<i64> {
    let digits = text.strip_prefix('-').unwrap_or(text);
    if !digits_only(digits) {
        return None;
    }
    text.parse().ok()
}

/// Digits with an optional minus sign: a whole number, even one too big for an id.
fn is_big_integer(text: &str) -> bool {
    digits_only(text.strip_prefix('-').unwrap_or(text))
}

fn digits_only(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit())
}

fn number(text: &str, written: &str) -> Result<Decimal, JeveError> {
    Decimal::parse(text).ok_or_else(|| at(written, format!("\"{text}\" is not a number")))
}

/// A failure that names the stockpile it is in.
fn at(stockpile: &str, reason: impl fmt::Display) -> JeveError {
    bad(format!("stockpile \"{stockpile}\": {reason}"))
}

fn bad(reason: impl Into<String>) -> JeveError {
    JeveError::StockpilesBad(reason.into())
}

/// A number as written: digits, an optional fraction and an optional exponent. Kept exact, never
/// as a float. The value is `digits × 10^exponent`, and `digits` has no leading zeros.
#[derive(Debug, PartialEq, Eq)]
struct Decimal {
    digits: String,
    exponent: i128,
}

impl Decimal {
    fn parse(text: &str) -> Option<Decimal> {
        let (mantissa, exponent) = match text.split_once(['e', 'E']) {
            Some((mantissa, exponent)) => (mantissa, parse_exponent(exponent)?),
            None => (text, 0),
        };
        let (whole, fraction) = match mantissa.split_once('.') {
            Some((whole, fraction)) => (whole, Some(fraction)),
            None => (mantissa, None),
        };
        if !digits_only(whole) || fraction.is_some_and(|fraction| !digits_only(fraction)) {
            return None;
        }
        let fraction_len = fraction.map_or(0, str::len);
        let digits = format!("{whole}{}", fraction.unwrap_or(""));
        let exponent = exponent.checked_sub(i128::try_from(fraction_len).ok()?)?;
        Some(Decimal {
            digits: digits.trim_start_matches('0').to_owned(),
            exponent,
        })
    }

    fn is_zero(&self) -> bool {
        self.digits.is_empty()
    }

    /// The value rounded up to a whole number, and whether rounding was needed. `None` when the
    /// whole number does not fit in `T`.
    fn ceiling_as<T: TryFrom<u128>>(&self) -> Option<(T, bool)> {
        let (ceiling, rounded) = self.ceiling()?;
        Some((T::try_from(ceiling).ok()?, rounded))
    }

    fn ceiling(&self) -> Option<(u128, bool)> {
        if self.is_zero() {
            return Some((0, false));
        }
        let before = i128::try_from(self.digits.len())
            .ok()?
            .checked_add(self.exponent)?;
        if before > MAX_DIGITS_BEFORE_POINT {
            return None;
        }
        let split = usize::try_from(before.max(0)).ok()?.min(self.digits.len());
        let head = self.digits.get(..split)?;
        let tail = self.digits.get(split..)?;
        // Zeros the exponent adds after the digits written before the point.
        let zeros = u32::try_from((before - i128::try_from(split).ok()?).max(0)).ok()?;
        let whole = if head.is_empty() {
            0
        } else {
            head.parse::<u128>()
                .ok()?
                .checked_mul(10_u128.checked_pow(zeros)?)?
        };
        let rounded = tail.bytes().any(|digit| digit != b'0');
        Some((whole.checked_add(u128::from(rounded))?, rounded))
    }
}

fn parse_exponent(text: &str) -> Option<i128> {
    let (negative, digits) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    if !digits_only(digits) {
        return None;
    }
    let magnitude = digits
        .parse::<i128>()
        .unwrap_or(EXPONENT_CAP)
        .min(EXPONENT_CAP);
    Some(if negative { -magnitude } else { magnitude })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ceiling(text: &str) -> Option<(u64, bool)> {
        Decimal::parse(text)?.ceiling_as::<u64>()
    }

    #[test]
    fn decimals_are_read_exactly() {
        assert_eq!(ceiling("10.0"), Some((10, false)));
        assert_eq!(ceiling("2.5"), Some((3, true)));
        assert_eq!(ceiling("1.0E7"), Some((10_000_000, false)));
        assert_eq!(ceiling("1.55E1"), Some((16, true)));
        assert_eq!(ceiling("0.5"), Some((1, true)));
        assert_eq!(ceiling("0.00001"), Some((1, true)));
        assert_eq!(ceiling("18446744073709551615"), Some((u64::MAX, false)));
        assert_eq!(ceiling("18446744073709551616"), None);
        assert_eq!(ceiling("1.0E20"), None);
        assert_eq!(ceiling("1.0E999999999999999999"), None);
        assert_eq!(ceiling("1.0E-999999999999999999"), Some((1, true)));
        assert!(Decimal::parse("0.0").is_some_and(|value| value.is_zero()));
        assert!(Decimal::parse("0E9").is_some_and(|value| value.is_zero()));
    }

    #[test]
    fn numbers_that_are_not_decimals_are_refused() {
        for text in [
            "", "1,5", "-1", "+1", "1.", ".5", "1E", "1E+5", "1.5.5", "abc",
        ] {
            assert_eq!(Decimal::parse(text), None, "{text}");
        }
    }

    #[test]
    fn names_and_ids_are_checked_as_text() {
        assert_eq!(integer("-2457"), Some(-2457));
        assert_eq!(integer("+1"), None);
        assert_eq!(integer(""), None);
        assert_eq!(
            claim_name("Kit", &mut BTreeSet::new(), &mut Vec::new()),
            "Kit"
        );
    }
}

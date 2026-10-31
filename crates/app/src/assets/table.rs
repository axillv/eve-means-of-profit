//! The Assets panel of the Assets page: every stack in the picked place, with its units, volume
//! and value. [`build`] is pure; [`show`] draws the panel.

use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use egui::text::LayoutJob;
use egui::{Align, Id, Label, Rect, RichText, TextFormat, TextStyle, Ui, UiBuilder, pos2};
use mop_core::format::group_thousands;
use mop_core::industry::TypeId;
use mop_core::money::Money;
use mop_core::stock::{Asset, CharacterAssets, ItemId, LocationId, ResolvedAsset, resolve};
use mop_core::volume::Volume;
use mop_market::book::PriceRule;
use mop_market::fuzzwork::Side;
use mop_store::settings::{RuleTarget, RuleValue};

use super::tree::{self, CATEGORY_BLUEPRINT, Geo};
use super::value::stack_value;
use super::worth::Worth;
use crate::Shared;
use crate::build::prices::age_text;
use crate::characters::short_names;
use crate::dock;
use crate::status::Notice;
use crate::table::Table;
use crate::table::model::{Cell, Column, ColumnKind, Row, row_height};

const NO_CHARACTER: &str = "No character picked.";
const NO_GAME_DATA: &str = "The game data is not in yet.";
const VALUE_TOO_LARGE: &str = "Could not value the assets: a value is too large.";
const BLUEPRINT_HOVER: &str = "Blueprints are not valued.";
const GROUP_KEY: &str = "assets.group";
const COLUMNS_KEY: &str = "assets.columns";

/// How the table groups its stacks.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum Group {
    #[default]
    Nothing,
    Place,
    Character,
    Item,
}

impl Group {
    const ALL: [Group; 4] = [Group::Nothing, Group::Place, Group::Character, Group::Item];

    /// How the setting stores the choice.
    fn setting(self) -> &'static str {
        match self {
            Self::Nothing => "nothing",
            Self::Place => "place",
            Self::Character => "character",
            Self::Item => "item",
        }
    }

    /// The choice a saved setting names; any other text is `Nothing`.
    fn from_setting(text: &str) -> Group {
        match text {
            "place" => Self::Place,
            "character" => Self::Character,
            "item" => Self::Item,
            _ => Self::Nothing,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Nothing => "Nothing",
            Self::Place => "Place",
            Self::Character => "Character",
            Self::Item => "Item",
        }
    }
}

/// One optional price column.
#[derive(Debug, Clone, Copy)]
struct PriceColumn {
    id: &'static str,
    label: &'static str,
}

/// The optional price columns, in the order the setting stores them.
const PRICE_COLUMNS: [PriceColumn; 3] = [
    PriceColumn {
        id: "forge_sell",
        label: "Forge sell",
    },
    PriceColumn {
        id: "forge_buy",
        label: "Forge buy",
    },
    PriceColumn {
        id: "sell_average",
        label: "Sell average",
    },
];

/// The optional price columns this session shows.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct PriceColumns {
    forge_sell: bool,
    forge_buy: bool,
    sell_average: bool,
}

impl PriceColumns {
    /// The ticked columns a saved setting names; ids it does not know are left out.
    fn from_text(text: &str) -> Self {
        let mut columns = Self::default();
        for id in text.split(',') {
            columns.set(id, true);
        }
        columns
    }

    /// The setting's text: the ticked ids in [`PRICE_COLUMNS`] order, joined by commas.
    fn text(self) -> String {
        PRICE_COLUMNS
            .iter()
            .filter(|column| self.shown(column.id))
            .map(|column| column.id)
            .collect::<Vec<_>>()
            .join(",")
    }

    fn shown(self, id: &str) -> bool {
        match id {
            "forge_sell" => self.forge_sell,
            "forge_buy" => self.forge_buy,
            "sell_average" => self.sell_average,
            _ => false,
        }
    }

    fn set(&mut self, id: &str, shown: bool) {
        match id {
            "forge_sell" => self.forge_sell = shown,
            "forge_buy" => self.forge_buy = shown,
            "sell_average" => self.sell_average = shown,
            _ => {}
        }
    }

    fn any(self) -> bool {
        self.forge_sell || self.forge_buy || self.sell_average
    }
}

/// What the table needs of a type: its name, whether its stacks are valued, its price, the optional
/// Forge prices, and the volumes one unit takes up unpackaged and hauled.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TypeFacts {
    name: String,
    valued: bool,
    price: Option<Money>,
    forge_sell: Option<Money>,
    forge_buy: Option<Money>,
    sell_average: Option<Money>,
    volume: Option<Volume>,
    haul_volume: Option<Volume>,
}

/// What the table needs of a place: its label, and its system and region when the game data has them.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PlaceFacts {
    label: String,
    geo: Option<Geo>,
}

/// One stack, ready to show.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    item_id: ItemId,
    type_id: TypeId,
    name: String,
    character_id: i64,
    character_name: String,
    place_id: LocationId,
    place_name: String,
    /// The flag, or the container's type name and the flag when the stack sits inside another stack.
    inside: Option<String>,
    quantity: u64,
    volume: Option<Volume>,
    value: Option<Money>,
    forge_sell: Option<Money>,
    forge_buy: Option<Money>,
    sell_average: Option<Money>,
    /// The type is a blueprint: its stacks are not valued and not counted as unpriced.
    valued: bool,
    /// The type has no price.
    unpriced: bool,
    /// The stack's value does not fit in money.
    too_large: bool,
}

/// The footer's totals over the shown stacks.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Totals {
    stacks: u64,
    volume: Option<Volume>,
    value: Option<Money>,
    /// The names of the types with no price, by type.
    unpriced: BTreeMap<TypeId, String>,
    /// A value did not fit in money.
    too_large: bool,
}

impl Totals {
    fn new() -> Self {
        Self {
            stacks: 0,
            volume: Some(Volume::default()),
            value: Some(Money::ZERO),
            unpriced: BTreeMap::new(),
            too_large: false,
        }
    }

    /// Adds one stack: its volume, its value when it has one, or its type when it has no price.
    fn add(&mut self, entry: &Entry) {
        // Cannot saturate: a count of stacks is at most the number of assets, far below u64::MAX.
        self.stacks = self.stacks.saturating_add(1);
        if let Some(volume) = entry.volume {
            self.volume = self.volume.and_then(|sum| sum.checked_add(volume));
        }
        if entry.too_large {
            self.too_large = true;
            self.value = None;
        } else if let Some(value) = entry.value {
            self.value = self.value.and_then(|sum| sum.checked_add(value));
        }
        if entry.unpriced {
            self.unpriced.insert(entry.type_id, entry.name.clone());
        }
    }

    /// `7 stacks · m³ 27,298.52 · value 619,240.00`.
    fn base(&self) -> String {
        let stacks = if self.stacks == 1 {
            "1 stack".to_owned()
        } else {
            format!("{} stacks", self.stacks)
        };
        let volume = self
            .volume
            .map_or_else(|| "—".to_owned(), Volume::format_m3);
        let value = self.value.map_or_else(|| "—".to_owned(), Money::format_isk);
        format!("{stacks} · m³ {volume} · value {value}")
    }

    /// The amber part of the footer, when some valued types have no price.
    fn unpriced_text(&self) -> Option<String> {
        let count = self.unpriced.len();
        if count == 0 {
            return None;
        }
        let noun = if count == 1 { "type" } else { "types" };
        Some(format!(" · {count} {noun} not priced"))
    }

    /// The names of the types with no price, sorted ignoring case.
    fn unpriced_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.unpriced.values().cloned().collect();
        names.sort_by_key(|name| name.to_lowercase());
        names
    }
}

/// The built table: its entries, the rows drawn from them, the group rows' details, and their
/// totals.
#[derive(Debug, Clone, PartialEq)]
struct TableData {
    entries: Vec<Entry>,
    rows: Vec<Row>,
    /// The entry position by row key, for the stack rows only.
    index: HashMap<u64, usize>,
    /// The value and unpriced types of each group row, by its key.
    groups: HashMap<u64, GroupDetail>,
    totals: Totals,
}

/// What a group row adds up beyond its cells.
#[derive(Debug, Clone, PartialEq, Eq)]
struct GroupDetail {
    value: Option<Money>,
    /// The names of the types in the group with no price, by type.
    unpriced: BTreeMap<TypeId, String>,
}

impl GroupDetail {
    /// The hover text of the amber ` +`, or `None` when no type in the group is unpriced.
    fn marker_hover(&self) -> Option<String> {
        let count = self.unpriced.len();
        if count == 0 {
            return None;
        }
        let noun = if count == 1 { "type" } else { "types" };
        Some(format!("{count} item {noun} not priced"))
    }

    /// What the value cell shows before the amber marker.
    fn value_text(&self) -> String {
        self.value.map_or_else(|| "—".to_owned(), Money::format_isk)
    }
}

impl TableData {
    fn new(mut entries: Vec<Entry>, group: Group, prices: PriceColumns) -> Self {
        sort_entries(&mut entries);
        let mut totals = Totals::new();
        for entry in &entries {
            totals.add(entry);
        }
        let (rows, index, groups) = match group {
            Group::Nothing => {
                let rows = entries.iter().map(|entry| row(entry, 0, prices)).collect();
                let index = entries
                    .iter()
                    .enumerate()
                    .map(|(position, entry)| (key(entry), position))
                    .collect();
                (rows, index, HashMap::new())
            }
            Group::Place | Group::Character | Group::Item => grouped_rows(&entries, group, prices),
        };
        Self {
            entries,
            rows,
            index,
            groups,
            totals,
        }
    }
}

/// The rows of a grouped table: the groups by name ignoring case, each stack under its group in
/// the order the entries already have.
fn grouped_rows(
    entries: &[Entry],
    group: Group,
    prices: PriceColumns,
) -> (Vec<Row>, HashMap<u64, usize>, HashMap<u64, GroupDetail>) {
    // The groups in the order they first appear, so the stacks keep the entries' order.
    let mut order: Vec<(u64, String, Vec<usize>)> = Vec::new();
    let mut at: HashMap<u64, usize> = HashMap::new();
    for (position, entry) in entries.iter().enumerate() {
        let (key, label) = group_of(group, entry);
        match at.get(&key) {
            Some(&index) => {
                if let Some(group) = order.get_mut(index) {
                    group.2.push(position);
                }
            }
            None => {
                at.insert(key, order.len());
                order.push((key, label, vec![position]));
            }
        }
    }
    order.sort_by(|a, b| {
        a.1.to_lowercase()
            .cmp(&b.1.to_lowercase())
            .then_with(|| a.0.cmp(&b.0))
    });
    // Cannot saturate: order holds at most one group per entry, so the sum is at most twice
    // entries.len(), far below usize::MAX.
    let mut rows = Vec::with_capacity(entries.len().saturating_add(order.len()));
    let mut index = HashMap::new();
    let mut groups = HashMap::new();
    for (group_key, label, members) in order {
        let mut sum = GroupSum::new();
        for &position in &members {
            if let Some(entry) = entries.get(position) {
                sum.add(entry);
            }
        }
        rows.push(group_row(group_key, &label, &sum, group, prices));
        let detail = GroupDetail {
            value: sum.value,
            unpriced: sum.unpriced,
        };
        groups.insert(group_key, detail);
        for position in members {
            if let Some(entry) = entries.get(position) {
                index.insert(key(entry), position);
                rows.push(row(entry, 1, prices));
            }
        }
    }
    (rows, index, groups)
}

/// The key and label of the group an entry belongs to.
fn group_of(group: Group, entry: &Entry) -> (u64, String) {
    match group {
        Group::Place => (group_key("place", entry.place_id), entry.place_name.clone()),
        Group::Character => (
            group_key("character", entry.character_id),
            entry.character_name.clone(),
        ),
        Group::Item => (group_key("item", entry.type_id), entry.name.clone()),
        Group::Nothing => (0, String::new()),
    }
}

/// A group's row key: a hash of what the group is by, kept out of the item ids' range.
fn group_key(id: &str, value: impl Hash) -> u64 {
    let mut hasher = DefaultHasher::new();
    id.hash(&mut hasher);
    value.hash(&mut hasher);
    hasher.finish() | (1 << 63)
}

/// What a group adds up.
#[derive(Debug, Clone)]
struct GroupSum {
    volume: Option<Volume>,
    /// The price of the priced stacks; `None` until one is priced.
    value: Option<Money>,
    /// The units of the stacks, for the `Item` grouping.
    units: Option<u64>,
    /// A priced value did not fit in money.
    overflowed: bool,
    /// The names of the types with no price, by type.
    unpriced: BTreeMap<TypeId, String>,
}

impl GroupSum {
    fn new() -> Self {
        Self {
            volume: Some(Volume::default()),
            value: None,
            units: Some(0),
            overflowed: false,
            unpriced: BTreeMap::new(),
        }
    }

    fn add(&mut self, entry: &Entry) {
        self.volume = match (self.volume, entry.volume) {
            (Some(sum), Some(volume)) => sum.checked_add(volume),
            _ => None,
        };
        if entry.too_large {
            self.overflowed = true;
            self.value = None;
        } else if !self.overflowed
            && let Some(value) = entry.value
        {
            self.value = match self.value {
                Some(sum) => sum.checked_add(value),
                None => Some(value),
            };
            if self.value.is_none() {
                self.overflowed = true;
            }
        }
        self.units = self
            .units
            .and_then(|units| units.checked_add(entry.quantity));
        if entry.unpriced {
            self.unpriced.insert(entry.type_id, entry.name.clone());
        }
    }
}

/// The row key of a stack: its item id.
fn key(entry: &Entry) -> u64 {
    u64::try_from(entry.item_id).unwrap_or(u64::MAX)
}

#[cfg(test)]
fn build(
    assets: &[ResolvedAsset],
    types: &HashMap<TypeId, TypeFacts>,
    places: &HashMap<LocationId, PlaceFacts>,
    names: &HashMap<i64, String>,
    filter: Option<LocationId>,
) -> TableData {
    build_grouped(
        assets,
        types,
        places,
        names,
        filter,
        Group::Nothing,
        PriceColumns::default(),
    )
}

/// Sorts the stacks by value, biggest first and unpriced last, then by name ignoring case and by
/// item id.
fn sort_entries(entries: &mut [Entry]) {
    entries.sort_by(|a, b| {
        b.value
            .cmp(&a.value)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.item_id.cmp(&b.item_id))
    });
}

/// A quantity drawn grouped to thousands, and copied and sorted by its number.
fn units_cell(quantity: u64) -> Cell {
    let units = i64::try_from(quantity).map_or_else(|_| quantity.to_string(), group_thousands);
    Cell::int(i128::from(quantity), units)
}

/// A volume value drawn to two decimals, and copied and sorted by its ten-thousandths.
fn volume_cell(volume: Volume) -> Cell {
    Cell::int(i128::from(volume.ten_thousandths()), volume.format_m3())
}

/// The cells of one stack.
fn row(entry: &Entry, depth: u32, prices: PriceColumns) -> Row {
    let mut cells = vec![
        Cell::text(entry.name.clone()),
        Cell::text(entry.character_name.clone()),
        Cell::text(entry.place_name.clone()),
        entry
            .inside
            .as_ref()
            .map_or_else(Cell::missing, |inside| Cell::text(inside.clone())),
        units_cell(entry.quantity),
        entry.volume.map_or_else(Cell::missing, volume_cell),
        entry.value.map_or_else(Cell::missing, Cell::money),
    ];
    if prices.forge_sell {
        cells.push(entry.forge_sell.map_or_else(Cell::missing, Cell::money));
    }
    if prices.forge_buy {
        cells.push(entry.forge_buy.map_or_else(Cell::missing, Cell::money));
    }
    if prices.sell_average {
        cells.push(entry.sell_average.map_or_else(Cell::missing, Cell::money));
    }
    Row {
        key: key(entry),
        depth,
        cells,
    }
}

/// The cells of a group row: the name, the summed volume and value, and for `Item` the summed
/// units.
fn group_row(key: u64, label: &str, sum: &GroupSum, group: Group, prices: PriceColumns) -> Row {
    let units = if group == Group::Item {
        sum.units.map_or_else(Cell::missing, units_cell)
    } else {
        Cell::missing()
    };
    let mut cells = vec![
        Cell::text(label.to_owned()),
        Cell::text(""),
        Cell::text(""),
        Cell::text(""),
        units,
        sum.volume.map_or_else(Cell::missing, volume_cell),
        sum.value.map_or_else(Cell::missing, Cell::money),
    ];
    if prices.any() {
        cells.extend(
            [prices.forge_sell, prices.forge_buy, prices.sell_average]
                .into_iter()
                .filter(|shown| *shown)
                .map(|_| Cell::missing()),
        );
    }
    Row {
        key,
        depth: 0,
        cells,
    }
}

fn columns(prices: PriceColumns) -> Vec<Column> {
    let mut columns = vec![
        column("item", "Item", ColumnKind::Text, 190.0),
        column("character", "Character", ColumnKind::Text, 120.0),
        column("place", "Place", ColumnKind::Text, 280.0),
        column("in", "In", ColumnKind::Text, 140.0),
        column("units", "Units", ColumnKind::Number, 80.0),
        column("volume", "m³", ColumnKind::Number, 100.0),
        column("value", "Value", ColumnKind::Money, 130.0),
    ];
    for price in PRICE_COLUMNS {
        if prices.shown(price.id) {
            columns.push(column(price.id, price.label, ColumnKind::Money, 130.0));
        }
    }
    columns
}

fn column(id: &'static str, label: &str, kind: ColumnKind, width: f32) -> Column {
    Column {
        id,
        label: label.to_owned(),
        kind,
        width,
    }
}

/// Builds the table from the resolved assets, their type and place facts, and the owners' names.
/// `filter` keeps the stacks in one place, region, system or container.
fn build_grouped(
    assets: &[ResolvedAsset],
    types: &HashMap<TypeId, TypeFacts>,
    places: &HashMap<LocationId, PlaceFacts>,
    names: &HashMap<i64, String>,
    filter: Option<LocationId>,
    group: Group,
    prices: PriceColumns,
) -> TableData {
    let by_item = by_item(assets);
    let mut entries = Vec::new();
    for asset in assets {
        if !shown(asset, filter, places, assets, &by_item) {
            continue;
        }
        let facts = type_facts(types, asset.asset.type_id);
        let (value, unpriced, too_large) = value_of(&facts, asset.asset.quantity);
        let volume = volume_of(&facts, &asset.asset);
        let inside = inside_of(asset, types, assets, &by_item);
        entries.push(Entry {
            item_id: asset.asset.item_id,
            type_id: asset.asset.type_id,
            name: facts.name,
            character_id: asset.character_id,
            character_name: names
                .get(&asset.character_id)
                .cloned()
                .unwrap_or_else(|| asset.character_id.to_string()),
            place_id: asset.place,
            place_name: places
                .get(&asset.place)
                .map_or_else(|| asset.place.to_string(), |facts| facts.label.clone()),
            inside,
            quantity: asset.asset.quantity,
            volume,
            value,
            forge_sell: facts.forge_sell,
            forge_buy: facts.forge_buy,
            sell_average: facts.sell_average,
            valued: facts.valued,
            unpriced,
            too_large,
        });
    }
    TableData::new(entries, group, prices)
}

/// The index of each asset by its item id.
fn by_item(assets: &[ResolvedAsset]) -> HashMap<ItemId, usize> {
    assets
        .iter()
        .enumerate()
        .map(|(index, asset)| (asset.asset.item_id, index))
        .collect()
}

/// Whether the stack is in the filter: all stacks, a place, a region or system, or a container.
fn shown(
    asset: &ResolvedAsset,
    filter: Option<LocationId>,
    places: &HashMap<LocationId, PlaceFacts>,
    assets: &[ResolvedAsset],
    by_item: &HashMap<ItemId, usize>,
) -> bool {
    let Some(wanted) = filter else {
        return true;
    };
    if asset.place == wanted {
        return true;
    }
    if let Some(geo) = places
        .get(&asset.place)
        .and_then(|facts| facts.geo.as_ref())
        && (i64::from(geo.region_id) == wanted || i64::from(geo.system_id) == wanted)
    {
        return true;
    }
    inside(asset, assets, by_item, wanted)
}

/// Whether the stack sits inside the container with item id `wanted`, at any depth. The container
/// itself does not count as inside itself.
fn inside(
    asset: &ResolvedAsset,
    assets: &[ResolvedAsset],
    by_item: &HashMap<ItemId, usize>,
    wanted: LocationId,
) -> bool {
    let mut holder = asset.asset.location_id;
    let mut hops = 0_usize;
    while let Some(&index) = by_item.get(&holder) {
        if holder == wanted {
            return true;
        }
        hops += 1;
        if hops > assets.len() {
            return false;
        }
        holder = assets
            .get(index)
            .map_or(holder, |asset| asset.asset.location_id);
    }
    false
}

/// The facts of a type, or the stand-in for one the caller did not give: named by its id, valued,
/// unpriced and with no volume.
fn type_facts(types: &HashMap<TypeId, TypeFacts>, type_id: TypeId) -> TypeFacts {
    types.get(&type_id).cloned().unwrap_or_else(|| TypeFacts {
        name: type_id.to_string(),
        valued: true,
        price: None,
        forge_sell: None,
        forge_buy: None,
        sell_average: None,
        volume: None,
        haul_volume: None,
    })
}

/// The value of a stack, whether its type has no price, and whether its value does not fit.
fn value_of(facts: &TypeFacts, quantity: u64) -> (Option<Money>, bool, bool) {
    if !facts.valued {
        return (None, false, false);
    }
    match facts.price {
        Some(price) => match stack_value(price, quantity) {
            Some(value) => (Some(value), false, false),
            None => (None, false, true),
        },
        None => (None, true, false),
    }
}

/// The volume of a stack: its units times the volume of a singleton, or the haul volume otherwise.
fn volume_of(facts: &TypeFacts, asset: &Asset) -> Option<Volume> {
    let per_unit = if asset.singleton {
        facts.volume
    } else {
        facts.haul_volume
    };
    per_unit.and_then(|volume| volume.checked_mul_qty(asset.quantity))
}

/// What the `In` column shows: the flag, or the container's type name and the flag inside another
/// stack. `None` when the asset has neither.
fn inside_of(
    asset: &ResolvedAsset,
    types: &HashMap<TypeId, TypeFacts>,
    assets: &[ResolvedAsset],
    by_item: &HashMap<ItemId, usize>,
) -> Option<String> {
    let holder = by_item
        .get(&asset.asset.location_id)
        .and_then(|&index| assets.get(index))
        .map(|holder| type_facts(types, holder.asset.type_id).name);
    let flag = asset.asset.flag.as_deref().filter(|flag| !flag.is_empty());
    match (holder, flag) {
        (Some(holder), Some(flag)) => Some(format!("{holder} · {flag}")),
        (Some(holder), None) => Some(holder),
        (None, Some(flag)) => Some(flag.to_owned()),
        (None, None) => None,
    }
}

/// The panel's state, kept in the panel's temporary data.
#[derive(Debug, Default, Clone)]
struct State {
    built: Option<Arc<Built>>,
    choices: Option<Choices>,
}

/// The choices the player made, and the settings that did not read.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Choices {
    group: Group,
    prices: PriceColumns,
    errors: Vec<String>,
}

/// What the panel reads. The table is rebuilt only when one of these changes.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Key {
    /// Each stock character with its data revision.
    characters: Vec<(i64, u64)>,
    /// A hash of jEve's assets: jEve's data is replaced whole, and any change in it changes this.
    jeve_hash: u64,
    unticked: BTreeSet<i64>,
    /// The app-wide `Value items at` price.
    rule: PriceRule,
    /// The price rules in force when the table is built.
    rules: BTreeMap<RuleTarget, RuleValue>,
    typed: BTreeMap<i64, String>,
    /// When the held market file was published; a new file changes it.
    published_at: Option<i64>,
    /// The place, region, system or container picked.
    place: Option<LocationId>,
    group: Group,
    prices: PriceColumns,
}

/// The built table and the key it was built from.
#[derive(Debug)]
struct Built {
    key: Key,
    data: TableData,
}

/// The line a panel with nothing to show reads, or `None` when it can show the table.
fn blank(picked: bool, has_game_data: bool) -> Option<&'static str> {
    if !picked {
        Some(NO_CHARACTER)
    } else if !has_game_data {
        Some(NO_GAME_DATA)
    } else {
        None
    }
}

/// Draws the Assets panel.
pub(super) fn show(ui: &mut Ui, cx: &mut Shared) {
    if !cx.session.loaded() {
        dock::placeholder(ui, cx);
        return;
    }
    let id = Id::new("assets table panel");
    let mut state = ui
        .data(|data| data.get_temp::<State>(id))
        .unwrap_or_default();
    draw(&mut state, ui, cx);
    ui.data_mut(|data| data.insert_temp(id, state));
}

fn draw(state: &mut State, ui: &mut Ui, cx: &mut Shared) {
    let stock = cx.session.stock_characters();
    let unticked = match tree::unticked(cx) {
        Ok(unticked) => unticked,
        Err(message) => {
            ui.colored_label(cx.style.neg, message);
            BTreeSet::new()
        }
    };
    let picked: BTreeSet<i64> = stock
        .iter()
        .map(|character| character.character_id)
        .filter(|id| !unticked.contains(id))
        .collect();
    let mut choices = match state.choices.clone() {
        Some(choices) => choices,
        None => {
            let choices = read_choices(cx);
            state.choices = Some(choices.clone());
            choices
        }
    };
    for error in &choices.errors {
        ui.colored_label(cx.style.neg, error);
    }
    if let Some(message) = blank(!picked.is_empty(), cx.sde.is_some()) {
        ui.colored_label(cx.style.mute, message);
        return;
    }
    let now = (cx.clock)();
    let built = match current(
        state,
        cx,
        &stock,
        &unticked,
        &picked,
        now,
        choices.group,
        choices.prices,
    ) {
        Ok(built) => built,
        Err(message) => {
            ui.colored_label(cx.style.neg, message);
            return;
        }
    };
    let updates: Vec<Option<i64>> = stock
        .iter()
        .filter(|character| picked.contains(&character.character_id))
        .map(|character| character.updated)
        .collect();
    let oldest = oldest_text(&updates, now);

    let response = panel(ui, cx, &built, &mut choices, &oldest);
    if let Some(key) = response.clicked
        && let Some(&position) = built.data.index.get(&key)
        && let Some(entry) = built.data.entries.get(position)
    {
        cx.selection.item = Some(entry.type_id);
    }
    state.choices = Some(choices);
}

/// The saved group and price columns, read on the first frame.
fn read_choices(cx: &mut Shared) -> Choices {
    let mut errors = Vec::new();
    let group = match cx.store.setting(GROUP_KEY) {
        Ok(Some(text)) => Group::from_setting(&text),
        Ok(None) => Group::Nothing,
        Err(error) => {
            errors.push(format!("Could not read the settings: {error}"));
            Group::Nothing
        }
    };
    let prices = match cx.store.setting(COLUMNS_KEY) {
        Ok(Some(text)) => PriceColumns::from_text(&text),
        Ok(None) => PriceColumns::default(),
        Err(error) => {
            errors.push(format!("Could not read the settings: {error}"));
            PriceColumns::default()
        }
    };
    Choices {
        group,
        prices,
        errors,
    }
}

/// Saves the grouping; a failure is an ERROR notice.
fn save_group(cx: &mut Shared, group: Group) {
    if let Err(error) = cx.store.set_setting(GROUP_KEY, group.setting()) {
        cx.notices.push(Notice::error(format!(
            "Could not save the settings: {error}"
        )));
    }
}

/// Saves the price columns, clearing the setting when none is ticked; a failure is an ERROR notice.
fn save_columns(cx: &mut Shared, prices: PriceColumns) {
    let text = prices.text();
    let result = if text.is_empty() {
        cx.store.clear_setting(COLUMNS_KEY)
    } else {
        cx.store.set_setting(COLUMNS_KEY, &text)
    };
    if let Err(error) = result {
        cx.notices.push(Notice::error(format!(
            "Could not save the settings: {error}"
        )));
    }
}

/// The `Group by` dropdown and the `Columns` menu above the table.
fn toolbar(ui: &mut Ui, cx: &mut Shared, choices: &mut Choices) {
    ui.horizontal(|ui| {
        let group_before = choices.group;
        egui::ComboBox::from_id_salt("assets group by")
            .selected_text(format!("Group by: {}", choices.group.label()))
            .show_ui(ui, |ui| {
                for group in Group::ALL {
                    ui.selectable_value(&mut choices.group, group, group.label());
                }
            });
        if choices.group != group_before {
            save_group(cx, choices.group);
        }
        let prices_before = choices.prices;
        ui.menu_button("Columns", |ui| {
            for price in PRICE_COLUMNS {
                let mut shown = choices.prices.shown(price.id);
                if ui.checkbox(&mut shown, price.label).changed() {
                    choices.prices.set(price.id, shown);
                }
            }
        });
        if choices.prices != prices_before {
            save_columns(cx, choices.prices);
        }
    });
}

/// The table and its footer, with the footer kept at the bottom of the panel.
fn panel(
    ui: &mut Ui,
    cx: &mut Shared,
    built: &Built,
    choices: &mut Choices,
    oldest: &str,
) -> crate::table::TableResponse {
    // A small gap under the panel header, so the rows do not start tight against it.
    let row = row_height(cx.density);
    ui.add_space(row / 2.0);
    toolbar(ui, cx, choices);
    let rect = ui.available_rect_before_wrap();
    // The footer is two lines: the totals, then the age of the oldest data.
    let footer_height = row.max(20.0) * 2.0;
    let split = (rect.max.y - footer_height).max(rect.min.y);
    let table_rect = Rect::from_min_max(rect.min, pos2(rect.max.x, split));
    let footer_rect = Rect::from_min_max(pos2(rect.min.x, split), rect.max);
    let response = ui
        .scope_builder(
            UiBuilder::new()
                .max_rect(table_rect)
                .layout(egui::Layout::top_down(Align::LEFT)),
            |ui| table(ui, cx, built),
        )
        .inner;
    ui.scope_builder(
        UiBuilder::new()
            .max_rect(footer_rect)
            .layout(egui::Layout::top_down(Align::LEFT)),
        |ui| {
            footer(ui, cx, &built.data.totals);
            ui.colored_label(cx.style.mute, oldest);
        },
    );
    response
}

fn table(ui: &mut Ui, cx: &mut Shared, built: &Built) -> crate::table::TableResponse {
    let columns = columns(built.key.prices);
    let data = &built.data;
    // The Character cell's short name takes `&mut Shared`, so the closure owns its style.
    let style = cx.style;
    let density = cx.density;
    let mut draw = |ui: &mut Ui, key: u64, column: &'static str| -> bool {
        if column == "character" {
            let Some(&position) = data.index.get(&key) else {
                // A group row's Character cell is empty; its name is the group's own label.
                return false;
            };
            let Some(entry) = data.entries.get(position) else {
                return false;
            };
            return short_names::label(ui, cx, entry.character_name.as_str());
        }
        if column != "value" {
            return false;
        }
        if let Some(&position) = data.index.get(&key) {
            let Some(entry) = data.entries.get(position) else {
                return false;
            };
            if entry.valued {
                return false;
            }
            ui.label("—").on_hover_text(BLUEPRINT_HOVER);
            return true;
        }
        let Some(group) = data.groups.get(&key) else {
            return false;
        };
        let Some(hover) = group.marker_hover() else {
            return false;
        };
        // The Value column lays out right to left, so the row is pinned left to right: the marker
        // follows the number.
        ui.with_layout(egui::Layout::left_to_right(Align::Center), |ui| {
            ui.label(group.value_text());
            let marker = RichText::new(" +").color(style.stale);
            ui.add(Label::new(marker).selectable(false))
                .on_hover_text(hover);
        });
        true
    };
    let icons = |key: u64| {
        data.index
            .get(&key)
            .and_then(|&position| data.entries.get(position))
            .map(|entry| entry.type_id)
    };
    Table::new(Id::new(super::AssetsPanel::Assets.title()), &columns)
        .icons("item", &icons)
        .show_with(ui, &style, density, &data.rows, None, &mut draw)
}

/// The age of the oldest data among the picked characters, or `never` when one has no time.
fn oldest_text(updates: &[Option<i64>], now: i64) -> String {
    let mut oldest = None;
    for update in updates {
        let Some(update) = *update else {
            return "Oldest data: never".to_owned();
        };
        oldest = Some(oldest.map_or(update, |oldest: i64| oldest.min(update)));
    }
    match oldest {
        // A floor at 0 is meant here: a stored time later than now has no age to show, and
        // age_text prints a future time as 0 s.
        Some(oldest) => format!("Oldest data: {}", age_text(now.saturating_sub(oldest))),
        None => "Oldest data: never".to_owned(),
    }
}

/// The footer: the stack count, the volume and the value, then in amber the types with no price.
fn footer(ui: &mut Ui, cx: &Shared, totals: &Totals) {
    let font = TextStyle::Body.resolve(ui.style());
    let mut job = LayoutJob::default();
    job.append(
        &totals.base(),
        0.0,
        TextFormat {
            font_id: font.clone(),
            color: cx.style.mute,
            ..Default::default()
        },
    );
    let mut hover = None;
    if let Some(text) = totals.unpriced_text() {
        job.append(
            &text,
            0.0,
            TextFormat {
                font_id: font,
                color: cx.style.stale,
                ..Default::default()
            },
        );
        hover = Some(totals.unpriced_names().join("\n"));
    }
    let response = ui.add(Label::new(job));
    if let Some(hover) = hover {
        response.on_hover_text(hover);
    }
}

/// The built table for the picked characters, from the state when nothing it reads has changed.
#[expect(
    clippy::too_many_arguments,
    reason = "the cache key is the inputs the table reads, one argument each"
)]
fn current(
    state: &mut State,
    cx: &mut Shared,
    stock: &[CharacterAssets],
    unticked: &BTreeSet<i64>,
    picked: &BTreeSet<i64>,
    now: i64,
    group: Group,
    prices: PriceColumns,
) -> Result<Arc<Built>, String> {
    let key = cache_key(cx, stock, unticked, now, group, prices)?;
    if let Some(built) = state.built.as_ref().filter(|built| built.key == key) {
        return Ok(Arc::clone(built));
    }
    let built = Arc::new(build_for(cx, stock, picked, key, now)?);
    state.built = Some(Arc::clone(&built));
    Ok(built)
}

fn cache_key(
    cx: &mut Shared,
    stock: &[CharacterAssets],
    unticked: &BTreeSet<i64>,
    now: i64,
    group: Group,
    prices: PriceColumns,
) -> Result<Key, String> {
    let rule = cx
        .store
        .value_price()
        .map_err(|error| format!("Could not read the value price: {error}"))?;
    let rules = cx
        .store
        .price_rules_at(now)
        .map_err(|error| format!("Could not read the price rules: {error}"))?;
    let published_at = cx.market.held().published_at;
    let typed = cx
        .store
        .structure_names()
        .map_err(|error| format!("Could not read the structure names: {error}"))?;
    let characters = stock
        .iter()
        .map(|character| {
            let revision =
                u64::try_from(character.character_id).map_or(0, |id| cx.session.revision(id));
            (character.character_id, revision)
        })
        .collect();
    let jeve_hash = cx.session.jeve().map_or(0, |jeve| {
        let mut hasher = DefaultHasher::new();
        for held in &jeve.characters {
            for asset in &held.assets.assets {
                (
                    asset.item_id,
                    asset.type_id,
                    asset.location_id,
                    asset.quantity,
                    asset.singleton,
                    &asset.flag,
                )
                    .hash(&mut hasher);
            }
        }
        hasher.finish()
    });
    Ok(Key {
        characters,
        jeve_hash,
        unticked: unticked.clone(),
        rule,
        rules,
        typed,
        published_at,
        place: cx.selection.place,
        group,
        prices,
    })
}

/// Reads the game data and the held market file, and builds the table of the picked characters'
/// stacks in the picked place.
fn build_for(
    cx: &mut Shared,
    stock: &[CharacterAssets],
    picked: &BTreeSet<i64>,
    key: Key,
    now: i64,
) -> Result<Built, String> {
    let Some(sde) = cx.sde.as_ref() else {
        return Err(NO_GAME_DATA.to_owned());
    };
    let assets: Vec<ResolvedAsset> = resolve(stock)
        .assets
        .into_iter()
        .filter(|asset| picked.contains(&asset.character_id))
        .collect();

    let type_ids: BTreeSet<TypeId> = assets.iter().map(|asset| asset.asset.type_id).collect();
    let mut named = Vec::with_capacity(type_ids.len());
    for type_id in type_ids {
        let item = sde
            .item(type_id)
            .map_err(|error| tree::game_data_error(&error))?;
        let facts = match item {
            Some(item) => {
                let haul_volume = item.haul_volume();
                TypeFacts {
                    name: item.name,
                    valued: item.category_id != CATEGORY_BLUEPRINT,
                    price: None,
                    forge_sell: None,
                    forge_buy: None,
                    sell_average: None,
                    volume: item.volume,
                    haul_volume,
                }
            }
            None => TypeFacts {
                name: type_id.to_string(),
                valued: true,
                price: None,
                forge_sell: None,
                forge_buy: None,
                sell_average: None,
                volume: None,
                haul_volume: None,
            },
        };
        named.push((type_id, facts));
    }
    let valued: BTreeSet<TypeId> = named
        .iter()
        .filter(|(_, facts)| facts.valued)
        .map(|(type_id, _)| *type_id)
        .collect();
    let held = cx.market.held();
    let worth = Worth::load(&cx.store, &held, sde, &valued, now)?;
    let types: HashMap<TypeId, TypeFacts> = named
        .into_iter()
        .map(|(type_id, mut facts)| {
            if facts.valued {
                facts.price = worth.price(type_id);
                facts.forge_sell = held.rows.get(&(type_id, Side::Sell)).map(|row| row.min);
                facts.forge_buy = held.rows.get(&(type_id, Side::Buy)).map(|row| row.max);
                facts.sell_average = held
                    .rows
                    .get(&(type_id, Side::Sell))
                    .map(|row| row.weighted_average);
            } else {
                facts.price = None;
                facts.forge_sell = None;
                facts.forge_buy = None;
                facts.sell_average = None;
            }
            (type_id, facts)
        })
        .collect();

    let mut places = HashMap::new();
    for asset in &assets {
        if places.contains_key(&asset.place) {
            continue;
        }
        let label = cx.session.place_label(asset.place, Some(sde));
        let geo = tree::geo_of(sde, &cx.session, asset.place)
            .map_err(|error| tree::game_data_error(&error))?;
        places.insert(asset.place, PlaceFacts { label, geo });
    }
    let names: HashMap<i64, String> = picked
        .iter()
        .map(|&character| (character, tree::owner_name(&cx.session, character)))
        .collect();

    let data = build_grouped(
        &assets, &types, &places, &names, key.place, key.group, key.prices,
    );
    if data.totals.too_large {
        cx.notices.push(Notice::error(VALUE_TOO_LARGE));
    }
    Ok(Built { key, data })
}

#[cfg(test)]
mod tests {
    use mop_core::money::Money;

    use super::*;

    const MAIN: i64 = 2_112_000_001;
    const ALT: i64 = 2_112_000_002;
    const HAULER: i64 = 2_112_000_003;
    const JITA: LocationId = 60_003_760;
    const HOME_FORT: LocationId = 1_035_466_617_946;
    const REFUSED: LocationId = 1_035_466_617_999;
    const FORGE: i64 = 10_000_002;
    const JITA_SYSTEM: i64 = 30_000_142;
    const RAM: TypeId = 11_481;
    const RIFTER: TypeId = 587;
    const BLUEPRINT: TypeId = 691;
    /// 480.00 in ten-thousandths.
    const RAM_PRICE: i64 = 4_800_000;
    /// 450.00 in ten-thousandths.
    const RAM_BUY: i64 = 4_500_000;
    /// 505,000.00 in ten-thousandths.
    const RIFTER_PRICE: i64 = 5_050_000_000;
    /// The Forge sell row's lowest price: 478.60 in ten-thousandths.
    const RAM_FORGE_SELL: i64 = 4_786_000;
    /// The Forge sell row's volume-weighted average: 479.20 in ten-thousandths.
    const RAM_SELL_AVERAGE: i64 = 4_792_000;
    /// The Rifter's Forge sell row: lowest 500,000.00 and average 503,000.00 in ten-thousandths.
    const RIFTER_FORGE_SELL: i64 = 5_000_000_000;
    const RIFTER_SELL_AVERAGE: i64 = 5_030_000_000;

    fn money(ten_thousandths: i64) -> Money {
        Money::from_ten_thousandths(ten_thousandths)
    }

    fn volume(ten_thousandths: i64) -> Volume {
        Volume::from_ten_thousandths(ten_thousandths)
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "the fixture spells each field of a resolved asset out"
    )]
    fn asset(
        character: i64,
        item: ItemId,
        type_id: TypeId,
        location: LocationId,
        place: LocationId,
        quantity: u64,
        singleton: bool,
        flag: &str,
    ) -> ResolvedAsset {
        ResolvedAsset {
            character_id: character,
            asset: Asset {
                item_id: item,
                type_id,
                location_id: location,
                flag: Some(flag.to_owned()),
                quantity,
                singleton,
            },
            place,
            counts_as_stock: !singleton,
        }
    }

    /// The sample: Main's 40, 120, 3, the Rifter with 10 inside it; Alt's 60 and Hauler's 5.
    fn sample() -> Vec<ResolvedAsset> {
        vec![
            asset(MAIN, 1, RAM, JITA, JITA, 40, false, "Hangar"),
            asset(MAIN, 2, RAM, HOME_FORT, HOME_FORT, 120, false, "Hangar"),
            asset(MAIN, 3, RAM, REFUSED, REFUSED, 3, false, "Hangar"),
            asset(MAIN, 9_001, RIFTER, JITA, JITA, 1, true, "Hangar"),
            asset(MAIN, 9_002, RAM, 9_001, JITA, 10, false, "Cargo"),
            asset(ALT, 5_001, RAM, JITA, JITA, 60, false, "Hangar"),
            asset(HAULER, 6_001, RAM, JITA, JITA, 5, false, "Hangar"),
        ]
    }

    fn ram(price: Option<Money>) -> TypeFacts {
        TypeFacts {
            name: "R.A.M.- Robotics".to_owned(),
            valued: true,
            price,
            forge_sell: Some(money(RAM_FORGE_SELL)),
            forge_buy: Some(money(RAM_BUY)),
            sell_average: Some(money(RAM_SELL_AVERAGE)),
            volume: Some(volume(400)),
            haul_volume: Some(volume(400)),
        }
    }

    fn rifter(price: Option<Money>) -> TypeFacts {
        TypeFacts {
            name: "Rifter".to_owned(),
            valued: true,
            price,
            forge_sell: Some(money(RIFTER_FORGE_SELL)),
            forge_buy: None,
            sell_average: Some(money(RIFTER_SELL_AVERAGE)),
            volume: Some(volume(272_890_000)),
            haul_volume: Some(volume(25_000_000)),
        }
    }

    fn blueprint() -> TypeFacts {
        TypeFacts {
            name: "Rifter Blueprint".to_owned(),
            valued: false,
            price: None,
            forge_sell: None,
            forge_buy: None,
            sell_average: None,
            volume: Some(volume(100)),
            haul_volume: Some(volume(100)),
        }
    }

    fn type_map(
        ram_price: Option<Money>,
        rifter_price: Option<Money>,
    ) -> HashMap<TypeId, TypeFacts> {
        HashMap::from([(RAM, ram(ram_price)), (RIFTER, rifter(rifter_price))])
    }

    fn place_map() -> HashMap<LocationId, PlaceFacts> {
        let forge = Geo {
            region_id: 10_000_002,
            region: "The Forge".to_owned(),
            system_id: 30_000_142,
            system: "Jita".to_owned(),
        };
        HashMap::from([
            (
                JITA,
                PlaceFacts {
                    label: "Jita IV - Moon 4 - Caldari Navy Assembly Plant".to_owned(),
                    geo: Some(forge.clone()),
                },
            ),
            (
                HOME_FORT,
                PlaceFacts {
                    label: "Home Fort".to_owned(),
                    geo: Some(forge),
                },
            ),
            (
                REFUSED,
                PlaceFacts {
                    label: "Structure 1035466617999 (no access)".to_owned(),
                    geo: None,
                },
            ),
        ])
    }

    fn names() -> HashMap<i64, String> {
        HashMap::from([
            (MAIN, "Main".to_owned()),
            (ALT, "Alt (jEve)".to_owned()),
            (HAULER, "Hauler (jEve)".to_owned()),
        ])
    }

    fn all_prices() -> HashMap<TypeId, TypeFacts> {
        type_map(Some(money(RAM_PRICE)), Some(money(RIFTER_PRICE)))
    }

    fn text(row: &Row) -> Vec<&str> {
        row.cells.iter().map(|cell| cell.text.as_str()).collect()
    }

    fn find<'a>(data: &'a TableData, name: &str) -> &'a Entry {
        data.entries
            .iter()
            .find(|entry| entry.name == name)
            .unwrap_or_else(|| panic!("no entry named {name}"))
    }

    #[test]
    fn all_places_lists_each_stack_with_its_units_volume_and_value() {
        let data = build(&sample(), &all_prices(), &place_map(), &names(), None);
        assert_eq!(
            text(&data.rows[0]),
            vec![
                "Rifter",
                "Main",
                "Jita IV - Moon 4 - Caldari Navy Assembly Plant",
                "Hangar",
                "1",
                "27,289.00",
                "505,000.00",
            ]
        );
        assert_eq!(
            text(&data.rows[1]),
            vec![
                "R.A.M.- Robotics",
                "Main",
                "Home Fort",
                "Hangar",
                "120",
                "4.80",
                "57,600.00",
            ]
        );
        assert_eq!(
            text(&data.rows[2]),
            vec![
                "R.A.M.- Robotics",
                "Alt (jEve)",
                "Jita IV - Moon 4 - Caldari Navy Assembly Plant",
                "Hangar",
                "60",
                "2.40",
                "28,800.00",
            ]
        );
        assert_eq!(
            text(&data.rows[3]),
            vec![
                "R.A.M.- Robotics",
                "Main",
                "Jita IV - Moon 4 - Caldari Navy Assembly Plant",
                "Hangar",
                "40",
                "1.60",
                "19,200.00",
            ]
        );
        assert_eq!(
            text(&data.rows[4]),
            vec![
                "R.A.M.- Robotics",
                "Main",
                "Jita IV - Moon 4 - Caldari Navy Assembly Plant",
                "Rifter · Cargo",
                "10",
                "0.40",
                "4,800.00",
            ]
        );
        assert_eq!(
            text(&data.rows[5]),
            vec![
                "R.A.M.- Robotics",
                "Hauler (jEve)",
                "Jita IV - Moon 4 - Caldari Navy Assembly Plant",
                "Hangar",
                "5",
                "0.20",
                "2,400.00",
            ]
        );
        assert_eq!(
            text(&data.rows[6]),
            vec![
                "R.A.M.- Robotics",
                "Main",
                "Structure 1035466617999 (no access)",
                "Hangar",
                "3",
                "0.12",
                "1,440.00",
            ]
        );
        assert_eq!(
            data.totals.base(),
            "7 stacks · m³ 27,298.52 · value 619,240.00"
        );
        assert_eq!(data.totals.unpriced_text(), None);
    }

    #[test]
    fn a_place_region_system_or_container_filters_the_stacks() {
        let data = build(
            &sample(),
            &all_prices(),
            &place_map(),
            &names(),
            Some(HOME_FORT),
        );
        assert_eq!(data.rows.len(), 1);
        assert_eq!(data.rows[0].cells[4].text, "120");
        assert_eq!(data.totals.base(), "1 stack · m³ 4.80 · value 57,600.00");

        let data = build(
            &sample(),
            &all_prices(),
            &place_map(),
            &names(),
            Some(9_001),
        );
        assert_eq!(data.rows.len(), 1);
        assert_eq!(
            text(&data.rows[0]),
            vec![
                "R.A.M.- Robotics",
                "Main",
                "Jita IV - Moon 4 - Caldari Navy Assembly Plant",
                "Rifter · Cargo",
                "10",
                "0.40",
                "4,800.00",
            ]
        );
        assert_eq!(data.totals.base(), "1 stack · m³ 0.40 · value 4,800.00");

        for filter in [JITA_SYSTEM, FORGE] {
            let data = build(
                &sample(),
                &all_prices(),
                &place_map(),
                &names(),
                Some(filter),
            );
            assert_eq!(data.rows.len(), 6, "the refused structure is left out");
            assert_eq!(data.rows[0].cells[0].text, "Rifter");
            assert_eq!(
                data.totals.base(),
                "6 stacks · m³ 27,298.40 · value 617,800.00"
            );
        }
    }

    #[test]
    fn a_container_filter_includes_what_is_nested_at_any_depth_but_not_the_container() {
        let assets = vec![
            asset(MAIN, 9_001, RIFTER, JITA, JITA, 1, true, "Hangar"),
            asset(MAIN, 9_002, RIFTER, 9_001, JITA, 1, true, "Cargo"),
            asset(MAIN, 9_003, RAM, 9_002, JITA, 1, false, "Cargo"),
        ];
        let data = build(&assets, &all_prices(), &place_map(), &names(), Some(9_001));
        let items: BTreeSet<ItemId> = data.entries.iter().map(|entry| entry.item_id).collect();
        assert_eq!(items, BTreeSet::from([9_002, 9_003]));
    }

    #[test]
    fn an_unpriced_type_is_last_and_named_in_the_footer() {
        let data = build(
            &sample(),
            &type_map(Some(money(RAM_PRICE)), None),
            &place_map(),
            &names(),
            None,
        );
        let last = data.rows.last().unwrap();
        assert_eq!(last.cells[0].text, "Rifter");
        assert_eq!(last.cells[6].text, "—");
        assert_eq!(
            data.totals.base(),
            "7 stacks · m³ 27,298.52 · value 114,240.00"
        );
        assert_eq!(
            data.totals.unpriced_text().as_deref(),
            Some(" · 1 type not priced")
        );
        assert_eq!(data.totals.unpriced_names(), vec!["Rifter".to_owned()]);
    }

    #[test]
    fn a_blueprint_shows_a_dash_and_is_not_counted_as_unpriced() {
        let mut assets = sample();
        assets.push(asset(MAIN, 9_100, BLUEPRINT, JITA, JITA, 1, true, "Hangar"));
        let mut types = all_prices();
        types.insert(BLUEPRINT, blueprint());
        let data = build(&assets, &types, &place_map(), &names(), None);
        let entry = find(&data, "Rifter Blueprint");
        assert!(!entry.valued);
        assert_eq!(entry.value, None);
        assert!(data.totals.unpriced.is_empty());
        assert_eq!(data.totals.unpriced_text(), None);
        let row = data
            .rows
            .iter()
            .find(|row| row.cells[0].text == "Rifter Blueprint")
            .unwrap();
        assert_eq!(row.cells[6].text, "—");
    }

    #[test]
    fn the_highest_buy_price_prices_ram_and_leaves_the_rifter_unpriced() {
        let data = build(
            &sample(),
            &type_map(Some(money(RAM_BUY)), None),
            &place_map(),
            &names(),
            None,
        );
        let home = find(&data, "R.A.M.- Robotics");
        assert_eq!(home.value, Some(money(540_000_000)));
        let home_row = data
            .rows
            .iter()
            .find(|row| row.cells[2].text == "Home Fort")
            .unwrap();
        assert_eq!(home_row.cells[6].text, "54,000.00");
        let alt_row = data
            .rows
            .iter()
            .find(|row| row.cells[1].text == "Alt (jEve)")
            .unwrap();
        assert_eq!(alt_row.cells[6].text, "27,000.00");
        let last = data.rows.last().unwrap();
        assert_eq!(last.cells[0].text, "Rifter");
        assert_eq!(last.cells[6].text, "—");
    }

    #[test]
    fn a_singleton_uses_its_volume_and_a_stack_its_haul_volume() {
        let singleton = vec![asset(MAIN, 9_001, RIFTER, JITA, JITA, 1, true, "Hangar")];
        let data = build(&singleton, &all_prices(), &place_map(), &names(), None);
        assert_eq!(data.rows[0].cells[5].text, "27,289.00");

        let stack = vec![asset(MAIN, 9_001, RIFTER, JITA, JITA, 1, false, "Hangar")];
        let data = build(&stack, &all_prices(), &place_map(), &names(), None);
        assert_eq!(data.rows[0].cells[5].text, "2,500.00");
    }

    #[test]
    fn a_stack_with_no_volume_shows_a_dash() {
        let mut types = all_prices();
        types.get_mut(&RAM).unwrap().haul_volume = None;
        let assets = vec![asset(MAIN, 1, RAM, JITA, JITA, 40, false, "Hangar")];
        let data = build(&assets, &types, &place_map(), &names(), None);
        assert_eq!(data.rows[0].cells[5].text, "—");
    }

    #[test]
    fn a_value_too_large_for_money_shows_a_dash_in_the_footer() {
        let huge = type_map(Some(money(i64::MAX / 2)), Some(money(RIFTER_PRICE)));
        let data = build(&sample(), &huge, &place_map(), &names(), None);
        assert!(data.totals.too_large);
        assert_eq!(data.totals.base(), "7 stacks · m³ 27,298.52 · value —");
        assert_eq!(data.rows[0].cells[6].text, "505,000.00");
    }

    #[test]
    fn no_character_and_no_game_data_have_their_own_lines() {
        assert_eq!(blank(false, true), Some("No character picked."));
        assert_eq!(blank(true, false), Some("The game data is not in yet."));
        assert_eq!(blank(false, false), Some("No character picked."));
        assert_eq!(blank(true, true), None);
    }

    #[test]
    fn grouping_by_character_puts_the_groups_in_name_order_and_keeps_the_stacks() {
        let data = build_grouped(
            &sample(),
            &all_prices(),
            &place_map(),
            &names(),
            None,
            Group::Character,
            PriceColumns::default(),
        );
        let groups: Vec<&Row> = data.rows.iter().filter(|row| row.depth == 0).collect();
        assert_eq!(
            groups
                .iter()
                .map(|row| row.cells[0].text.as_str())
                .collect::<Vec<_>>(),
            vec!["Alt (jEve)", "Hauler (jEve)", "Main"]
        );
        assert_eq!(
            groups
                .iter()
                .map(|row| row.cells[6].text.as_str())
                .collect::<Vec<_>>(),
            vec!["28,800.00", "2,400.00", "588,040.00"]
        );
        assert_eq!(groups[0].cells[4].text, "—", "only Item groups sum units");
        assert_eq!(groups[0].cells[5].text, "2.40");

        let main: Vec<&Row> = data
            .rows
            .iter()
            .skip_while(|row| row.cells[0].text != "Main" || row.depth != 0)
            .skip(1)
            .take_while(|row| row.depth == 1)
            .collect();
        assert_eq!(main.len(), 5);
        assert_eq!(
            main.iter()
                .map(|row| row.cells[0].text.as_str())
                .collect::<Vec<_>>(),
            vec![
                "Rifter",
                "R.A.M.- Robotics",
                "R.A.M.- Robotics",
                "R.A.M.- Robotics",
                "R.A.M.- Robotics"
            ]
        );
        assert_eq!(main[0].cells[6].text, "505,000.00");
    }

    #[test]
    fn grouping_by_item_sums_units_and_value_and_a_missing_price_shows_a_dash() {
        let data = build_grouped(
            &sample(),
            &all_prices(),
            &place_map(),
            &names(),
            None,
            Group::Item,
            PriceColumns::default(),
        );
        let groups: Vec<&Row> = data.rows.iter().filter(|row| row.depth == 0).collect();
        assert_eq!(
            groups
                .iter()
                .map(|row| row.cells[0].text.as_str())
                .collect::<Vec<_>>(),
            vec!["R.A.M.- Robotics", "Rifter"]
        );
        assert_eq!(
            text(groups[0]),
            vec!["R.A.M.- Robotics", "", "", "", "238", "9.52", "114,240.00"]
        );
        assert_eq!(
            text(groups[1]),
            vec!["Rifter", "", "", "", "1", "27,289.00", "505,000.00"]
        );

        let without_rifter = build_grouped(
            &sample(),
            &type_map(Some(money(RAM_PRICE)), None),
            &place_map(),
            &names(),
            None,
            Group::Item,
            PriceColumns::default(),
        );
        let groups: Vec<&Row> = without_rifter
            .rows
            .iter()
            .filter(|row| row.depth == 0)
            .collect();
        assert_eq!(groups[0].cells[6].text, "114,240.00");
        assert_eq!(groups[1].cells[6].text, "—");
    }

    #[test]
    fn optional_price_columns_show_the_forge_rows_per_unit() {
        let prices = PriceColumns {
            forge_sell: true,
            forge_buy: true,
            sell_average: true,
        };
        let data = build_grouped(
            &sample(),
            &all_prices(),
            &place_map(),
            &names(),
            None,
            Group::Nothing,
            prices,
        );
        let ram = data
            .entries
            .iter()
            .find(|entry| entry.type_id == RAM && entry.quantity == 120)
            .expect("the 120 R.A.M. stack");
        let row = data
            .rows
            .iter()
            .find(|row| row.key == key(ram))
            .expect("its row");
        assert_eq!(
            text(row),
            vec![
                "R.A.M.- Robotics",
                "Main",
                "Home Fort",
                "Hangar",
                "120",
                "4.80",
                "57,600.00",
                "478.60",
                "450.00",
                "479.20",
            ]
        );
        let rifter = data
            .rows
            .iter()
            .find(|row| row.key == 9_001)
            .expect("the Rifter row");
        assert_eq!(
            text(rifter),
            vec![
                "Rifter",
                "Main",
                "Jita IV - Moon 4 - Caldari Navy Assembly Plant",
                "Hangar",
                "1",
                "27,289.00",
                "505,000.00",
                "500,000.00",
                "—",
                "503,000.00",
            ]
        );
    }

    #[test]
    fn a_blueprint_has_no_optional_price_even_with_a_market_row() {
        let mut assets = sample();
        assets.push(asset(MAIN, 9_100, BLUEPRINT, JITA, JITA, 1, true, "Hangar"));
        let mut types = all_prices();
        types.insert(BLUEPRINT, blueprint());
        let prices = PriceColumns {
            forge_sell: true,
            forge_buy: true,
            sell_average: true,
        };
        let data = build_grouped(
            &assets,
            &types,
            &place_map(),
            &names(),
            None,
            Group::Nothing,
            prices,
        );
        let row = data
            .rows
            .iter()
            .find(|row| row.cells[0].text == "Rifter Blueprint")
            .expect("the blueprint row");
        assert_eq!(
            &row.cells[7..],
            &[Cell::missing(), Cell::missing(), Cell::missing()]
        );
    }

    #[test]
    fn the_saved_choices_parse_and_a_bad_group_is_nothing() {
        assert_eq!(Group::from_setting("nothing"), Group::Nothing);
        assert_eq!(Group::from_setting("place"), Group::Place);
        assert_eq!(Group::from_setting("character"), Group::Character);
        assert_eq!(Group::from_setting("item"), Group::Item);
        assert_eq!(Group::from_setting("bad"), Group::Nothing);

        let all = PriceColumns::from_text("forge_sell,forge_buy,sell_average");
        assert_eq!(all.text(), "forge_sell,forge_buy,sell_average");
        assert_eq!(
            PriceColumns::from_text("bogus,forge_buy").text(),
            "forge_buy"
        );
        assert_eq!(PriceColumns::default().text(), "");
    }

    #[test]
    fn the_oldest_data_is_the_smallest_update_and_never_when_one_is_missing() {
        assert_eq!(
            oldest_text(&[Some(900_000), Some(950_400)], 1_000_000),
            "Oldest data: 1d"
        );
        assert_eq!(
            oldest_text(&[Some(900_000), None], 1_000_000),
            "Oldest data: never"
        );
        assert_eq!(oldest_text(&[], 1_000_000), "Oldest data: never");
    }
}

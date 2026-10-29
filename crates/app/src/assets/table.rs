//! The Assets panel of the Assets page: every stack in the picked place, with its units, volume
//! and value. [`build`] is pure; [`show`] draws the panel.

use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use egui::text::LayoutJob;
use egui::{Align, Id, Label, Rect, TextFormat, TextStyle, Ui, UiBuilder, pos2};
use mop_core::format::group_thousands;
use mop_core::industry::TypeId;
use mop_core::money::Money;
use mop_core::stock::{Asset, CharacterAssets, ItemId, LocationId, ResolvedAsset, resolve};
use mop_core::volume::Volume;
use mop_market::book::PriceRule;
use mop_store::settings::{RuleTarget, RuleValue};

use super::tree::{self, CATEGORY_BLUEPRINT, Geo};
use super::value::stack_value;
use super::worth::Worth;
use crate::Shared;
use crate::dock;
use crate::status::Notice;
use crate::table::Table;
use crate::table::model::{Cell, Column, ColumnKind, Row, row_height};

const NO_CHARACTER: &str = "No character picked.";
const NO_GAME_DATA: &str = "The game data is not in yet.";
const VALUE_TOO_LARGE: &str = "Could not value the assets: a value is too large.";
const BLUEPRINT_HOVER: &str = "Blueprints are not valued.";

/// What the table needs of a type: its name, whether its stacks are valued, its price, and the
/// volumes one unit takes up unpackaged and hauled.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TypeFacts {
    name: String,
    valued: bool,
    price: Option<Money>,
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
    character_name: String,
    place_name: String,
    /// The flag, or the container's type name and the flag when the stack sits inside another stack.
    inside: Option<String>,
    quantity: u64,
    volume: Option<Volume>,
    value: Option<Money>,
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

/// The built table: its entries in the order they show, the rows drawn from them, and their totals.
#[derive(Debug, Clone, PartialEq)]
struct TableData {
    entries: Vec<Entry>,
    rows: Vec<Row>,
    /// The entry position by row key.
    index: HashMap<u64, usize>,
    totals: Totals,
}

impl TableData {
    fn new(mut entries: Vec<Entry>) -> Self {
        sort_entries(&mut entries);
        let mut totals = Totals::new();
        for entry in &entries {
            totals.add(entry);
        }
        let rows: Vec<Row> = entries.iter().map(row).collect();
        let index = entries
            .iter()
            .enumerate()
            .map(|(position, entry)| (key(entry), position))
            .collect();
        Self {
            entries,
            rows,
            index,
            totals,
        }
    }
}

/// The row key of a stack: its item id.
fn key(entry: &Entry) -> u64 {
    u64::try_from(entry.item_id).unwrap_or(u64::MAX)
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

/// The cells of one stack.
fn row(entry: &Entry) -> Row {
    let units =
        i64::try_from(entry.quantity).map_or_else(|_| entry.quantity.to_string(), group_thousands);
    Row {
        key: key(entry),
        depth: 0,
        cells: vec![
            Cell::text(entry.name.clone()),
            Cell::text(entry.character_name.clone()),
            Cell::text(entry.place_name.clone()),
            entry
                .inside
                .as_ref()
                .map_or_else(Cell::missing, |inside| Cell::text(inside.clone())),
            Cell::int(i128::from(entry.quantity), units),
            entry.volume.map_or_else(Cell::missing, |volume| {
                Cell::int(i128::from(volume.ten_thousandths()), volume.format_m3())
            }),
            entry.value.map_or_else(Cell::missing, Cell::money),
        ],
    }
}

fn columns() -> Vec<Column> {
    vec![
        column("item", "Item", ColumnKind::Text, 190.0),
        column("character", "Character", ColumnKind::Text, 120.0),
        column("place", "Place", ColumnKind::Text, 280.0),
        column("in", "In", ColumnKind::Text, 140.0),
        column("units", "Units", ColumnKind::Number, 80.0),
        column("volume", "m³", ColumnKind::Number, 100.0),
        column("value", "Value", ColumnKind::Money, 130.0),
    ]
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
fn build(
    assets: &[ResolvedAsset],
    types: &HashMap<TypeId, TypeFacts>,
    places: &HashMap<LocationId, PlaceFacts>,
    names: &HashMap<i64, String>,
    filter: Option<LocationId>,
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
            character_name: names
                .get(&asset.character_id)
                .cloned()
                .unwrap_or_else(|| asset.character_id.to_string()),
            place_name: places
                .get(&asset.place)
                .map_or_else(|| asset.place.to_string(), |facts| facts.label.clone()),
            inside,
            quantity: asset.asset.quantity,
            volume,
            value,
            valued: facts.valued,
            unpriced,
            too_large,
        });
    }
    TableData::new(entries)
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
    if let Some(message) = blank(!picked.is_empty(), cx.sde.is_some()) {
        ui.colored_label(cx.style.mute, message);
        return;
    }
    let built = match current(state, cx, &stock, &unticked, &picked) {
        Ok(built) => built,
        Err(message) => {
            ui.colored_label(cx.style.neg, message);
            return;
        }
    };

    let response = panel(ui, cx, &built);
    if let Some(key) = response.clicked
        && let Some(&position) = built.data.index.get(&key)
        && let Some(entry) = built.data.entries.get(position)
    {
        cx.selection.item = Some(entry.type_id);
    }
}

/// The table and its footer, with the footer kept at the bottom of the panel.
fn panel(ui: &mut Ui, cx: &Shared, built: &Built) -> crate::table::TableResponse {
    // A small gap under the panel header, so the rows do not start tight against it.
    let row = row_height(cx.density);
    ui.add_space(row / 2.0);
    let rect = ui.available_rect_before_wrap();
    let footer_height = row.max(20.0);
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
            .layout(egui::Layout::left_to_right(Align::Center)),
        |ui| footer(ui, cx, &built.data.totals),
    );
    response
}

fn table(ui: &mut Ui, cx: &Shared, built: &Built) -> crate::table::TableResponse {
    let columns = columns();
    let data = &built.data;
    let mut draw = |ui: &mut Ui, key: u64, column: &'static str| -> bool {
        if column != "value" {
            return false;
        }
        let Some(&position) = data.index.get(&key) else {
            return false;
        };
        let Some(entry) = data.entries.get(position) else {
            return false;
        };
        if entry.valued {
            return false;
        }
        ui.label("—").on_hover_text(BLUEPRINT_HOVER);
        true
    };
    Table::new(Id::new(super::AssetsPanel::Assets.title()), &columns)
        .show_with(ui, &cx.style, cx.density, &data.rows, None, &mut draw)
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
fn current(
    state: &mut State,
    cx: &mut Shared,
    stock: &[CharacterAssets],
    unticked: &BTreeSet<i64>,
    picked: &BTreeSet<i64>,
) -> Result<Arc<Built>, String> {
    let now = (cx.clock)();
    let key = cache_key(cx, stock, unticked, now)?;
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
                    volume: item.volume,
                    haul_volume,
                }
            }
            None => TypeFacts {
                name: type_id.to_string(),
                valued: true,
                price: None,
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
            facts.price = if facts.valued {
                worth.price(type_id)
            } else {
                None
            };
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

    let data = build(&assets, &types, &places, &names, key.place);
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

    /// M1-163's sample: Main's 40, 120, 3, the Rifter with 10 inside it; Alt's 60 and Hauler's 5.
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
            volume: Some(volume(400)),
            haul_volume: Some(volume(400)),
        }
    }

    fn rifter(price: Option<Money>) -> TypeFacts {
        TypeFacts {
            name: "Rifter".to_owned(),
            valued: true,
            price,
            volume: Some(volume(272_890_000)),
            haul_volume: Some(volume(25_000_000)),
        }
    }

    fn blueprint() -> TypeFacts {
        TypeFacts {
            name: "Rifter Blueprint".to_owned(),
            valued: false,
            price: None,
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
}

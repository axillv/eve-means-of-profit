//! The Places panel of the Assets page: regions, systems, places and the containers that hold items,
//! with the stacks and the value under each. [`build`] is pure; [`show`] draws the panel.

use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use egui::{Color32, Id, Label, RichText, Sense, Ui};
use mop_core::format::group_thousands;
use mop_core::industry::TypeId;
use mop_core::money::Money;
use mop_core::net_worth::PriceBasis;
use mop_core::stock::{CharacterAssets, ItemId, LocationId, ResolvedAsset, resolve};
use mop_esi::places::FIRST_STRUCTURE_ID;
use mop_sde::{Sde, SdeError};

use super::name_dialog::{self, Dialog};
use super::value::{Prices, basis_label, stack_value};
use crate::Shared;
use crate::build::prices::age_text;
use crate::dock;
use crate::session::{Session, Source};
use crate::status::Notice;
use crate::table::Table;
use crate::table::model::{Cell, Column, ColumnKind, Row};

/// The top bits of a row key say what the row is; the rest is its id, which is below 2^61.
const KIND_SHIFT: u32 = 61;
const ID_LIMIT: u64 = 1 << KIND_SHIFT;
const ROOT: u64 = 0;
const REGION: u64 = 1;
const SYSTEM: u64 = 2;
const PLACE: u64 = 3;
const CONTAINER: u64 = 4;

/// The category of blueprints: their stacks are not valued.
const CATEGORY_BLUEPRINT: u32 = 9;
/// The id ranges of the stations and the solar systems in the game data.
const FIRST_STATION: LocationId = 60_000_000;
const LAST_STATION: LocationId = 63_999_999;
const FIRST_SYSTEM: LocationId = 30_000_000;
const LAST_SYSTEM: LocationId = 32_999_999;
/// The saved setting of the character filter: the ids of the characters left out, joined by `,`.
const FILTER_KEY: &str = "assets.characters";
const DAY: i64 = 86_400;

/// Where a place is in the game data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Geo {
    pub region_id: u32,
    pub region: String,
    pub system_id: u32,
    pub system: String,
}

/// What the tree needs of a place: its label, and its system and region when the game data has them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PlaceFacts {
    pub label: String,
    pub geo: Option<Geo>,
}

/// What the tree needs of a type: its name, whether its stacks are valued, and its price.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TypeFacts {
    pub name: String,
    pub valued: bool,
    pub price: Option<Money>,
}

/// One row of the tree and what it adds up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Entry {
    pub key: u64,
    pub depth: u32,
    pub label: String,
    /// What a click picks. `None` for the root and for the unknown region and system.
    pub select: Option<LocationId>,
    /// The asset rows under this row.
    pub stacks: u64,
    /// The value of the priced stacks under this row; `None` when none of them is priced.
    pub value: Option<Money>,
    /// How many item types under this row have no price.
    pub unpriced: usize,
}

/// The tree, its rows in the order they show, the root first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Tree {
    pub entries: Vec<Entry>,
    /// Some row's value does not fit in money; the player is told once when the tree is built.
    pub too_large: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TreeError {
    /// A value does not fit in a money amount.
    ValueTooLarge,
    /// An id is negative or too large to make a row key.
    IdOutOfRange,
    /// Items hold each other in a loop. The resolver leaves such items out, so this is a bug.
    Loop,
}

impl fmt::Display for TreeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ValueTooLarge => write!(f, "Could not value the assets: a value is too large."),
            Self::IdOutOfRange => write!(f, "Could not show the places: an id is out of range."),
            Self::Loop => write!(f, "Could not show the places: items hold each other."),
        }
    }
}

impl std::error::Error for TreeError {}

/// What adds up under one row.
#[derive(Debug, Default, Clone)]
struct Sum {
    stacks: u64,
    value: Option<Money>,
    /// The value does not fit in money: this row shows dashes, and the other rows are kept.
    too_large: bool,
    unpriced: BTreeSet<TypeId>,
}

impl Sum {
    /// Adds one stack: its value when it is priced, or its type when it has no price. A stack
    /// whose value does not fit is `too_large`; every row it counts under is too large too.
    fn add(&mut self, value: Option<Money>, unpriced: Option<TypeId>, too_large: bool) {
        // Cannot saturate: a count of stacks is at most the number of assets, far below u64::MAX.
        self.stacks = self.stacks.saturating_add(1);
        if let Some(value) = value {
            match self.value.map_or(Some(value), |sum| sum.checked_add(value)) {
                Some(sum) => self.value = Some(sum),
                None => self.too_large = true,
            }
        }
        self.too_large |= too_large;
        if let Some(type_id) = unpriced {
            self.unpriced.insert(type_id);
        }
    }
}

/// A row that sums up assets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Node {
    /// `None` for the unknown region.
    Region(Option<u32>),
    /// `None` for the unknown system.
    System(Option<u32>),
    Place(LocationId),
    Container(ItemId),
}

/// Regions and systems sort by name, the unknown one last: `(unknown, name, id)`.
type AreaKey = (bool, String, u32);

/// Each stack counted under every row it belongs to, and the places the assets sit in.
#[derive(Default)]
struct Sums {
    root: Sum,
    rows: BTreeMap<Node, Sum>,
    places: BTreeSet<LocationId>,
}

/// Which assets hold other assets: the containers inside a container, and the ones at the top of a place.
struct Holding {
    inside: BTreeMap<ItemId, Vec<usize>>,
    top: BTreeMap<LocationId, Vec<usize>>,
}

/// Builds the tree. `assets` are the resolved assets to count; `types` and `places` give what the tree
/// needs of each type and place. A type or place missing from them is named by its id: a type counts
/// as unpriced, and a place as in the unknown region.
pub(super) fn build(
    assets: &[ResolvedAsset],
    types: &HashMap<TypeId, TypeFacts>,
    places: &HashMap<LocationId, PlaceFacts>,
) -> Result<Tree, TreeError> {
    let by_item = by_item(assets);
    let sums = count(assets, types, places, &by_item)?;
    let too_large = sums.root.too_large || sums.rows.values().any(|sum| sum.too_large);
    let holding = holding(assets, types, &by_item);
    let regions = regions(&sums.places, places);
    let walk = Walk {
        assets,
        types,
        sums: &sums.rows,
        inside: &holding.inside,
    };
    let mut entries = vec![entry(ROOT, 0, "All places".to_owned(), None, &sums.root)];
    for (region, systems) in regions {
        entries.push(walk.row(Node::Region(known_id(&region)), 1, region.1)?);
        for (system, list) in systems {
            entries.push(walk.row(Node::System(known_id(&system)), 2, system.1)?);
            for (label, place) in list {
                entries.push(walk.row(Node::Place(place), 3, label)?);
                for &index in holding.top.get(&place).into_iter().flatten() {
                    walk.container(&mut entries, index, 4)?;
                }
            }
        }
    }
    Ok(Tree { entries, too_large })
}

/// The index of each asset by its item id.
fn by_item(assets: &[ResolvedAsset]) -> HashMap<ItemId, usize> {
    assets
        .iter()
        .enumerate()
        .map(|(index, resolved)| (resolved.asset.item_id, index))
        .collect()
}

/// Adds every asset to the rows it counts under.
fn count(
    assets: &[ResolvedAsset],
    types: &HashMap<TypeId, TypeFacts>,
    places: &HashMap<LocationId, PlaceFacts>,
    by_item: &HashMap<ItemId, usize>,
) -> Result<Sums, TreeError> {
    let mut sums = Sums::default();
    for resolved in assets {
        let facts = type_facts(types, resolved.asset.type_id);
        let (value, unpriced, too_large) =
            match stack_of(&facts, resolved.asset.type_id, resolved.asset.quantity) {
                Ok((value, unpriced)) => (value, unpriced, false),
                Err(TreeError::ValueTooLarge) => (None, None, true),
                Err(error) => return Err(error),
            };
        sums.places.insert(resolved.place);
        sums.root.add(value, unpriced, too_large);
        for node in nodes_of(assets, by_item, places, resolved)? {
            sums.rows
                .entry(node)
                .or_default()
                .add(value, unpriced, too_large);
        }
    }
    Ok(sums)
}

/// The value of a stack, and its type when it has no price. A blueprint is neither.
fn stack_of(
    facts: &TypeFacts,
    type_id: TypeId,
    quantity: u64,
) -> Result<(Option<Money>, Option<TypeId>), TreeError> {
    if !facts.valued {
        return Ok((None, None));
    }
    match facts.price {
        Some(price) => Ok((
            Some(stack_value(price, quantity).ok_or(TreeError::ValueTooLarge)?),
            None,
        )),
        None => Ok((None, Some(type_id))),
    }
}

/// The rows a stack counts under: its region, system and place, then the containers it sits in,
/// innermost first.
fn nodes_of(
    assets: &[ResolvedAsset],
    by_item: &HashMap<ItemId, usize>,
    places: &HashMap<LocationId, PlaceFacts>,
    resolved: &ResolvedAsset,
) -> Result<Vec<Node>, TreeError> {
    let geo = places
        .get(&resolved.place)
        .and_then(|facts| facts.geo.as_ref());
    let mut nodes = vec![
        Node::Region(geo.map(|geo| geo.region_id)),
        Node::System(geo.map(|geo| geo.system_id)),
        Node::Place(resolved.place),
    ];
    let mut holder = resolved.asset.location_id;
    let mut hops = 0_usize;
    while let Some(&index) = by_item.get(&holder) {
        nodes.push(Node::Container(holder));
        hops += 1;
        if hops > assets.len() {
            return Err(TreeError::Loop);
        }
        holder = assets.get(index).ok_or(TreeError::Loop)?.asset.location_id;
    }
    Ok(nodes)
}

/// Which assets are containers: an item that another asset sits in. Their contents are listed under them,
/// ordered by type name, then item id.
fn holding(
    assets: &[ResolvedAsset],
    types: &HashMap<TypeId, TypeFacts>,
    by_item: &HashMap<ItemId, usize>,
) -> Holding {
    let holders: HashSet<ItemId> = assets
        .iter()
        .map(|resolved| resolved.asset.location_id)
        .filter(|location| by_item.contains_key(location))
        .collect();
    let mut inside: BTreeMap<ItemId, Vec<usize>> = BTreeMap::new();
    let mut top: BTreeMap<LocationId, Vec<usize>> = BTreeMap::new();
    for (index, resolved) in assets.iter().enumerate() {
        if !holders.contains(&resolved.asset.item_id) {
            continue;
        }
        if by_item.contains_key(&resolved.asset.location_id) {
            inside
                .entry(resolved.asset.location_id)
                .or_default()
                .push(index);
        } else {
            top.entry(resolved.place).or_default().push(index);
        }
    }
    let order = |index: &usize| child_order(assets, types, *index);
    for children in inside.values_mut().chain(top.values_mut()) {
        children.sort_by_key(order);
    }
    Holding { inside, top }
}

/// The type name and item id that order containers under their holder.
fn child_order(
    assets: &[ResolvedAsset],
    types: &HashMap<TypeId, TypeFacts>,
    index: usize,
) -> (String, ItemId) {
    assets.get(index).map_or_else(
        || (String::new(), 0),
        |resolved| {
            (
                type_facts(types, resolved.asset.type_id).name,
                resolved.asset.item_id,
            )
        },
    )
}

/// The places by region and system, each system's places sorted by label ignoring case. The unknown
/// region and system come last.
fn regions(
    present: &BTreeSet<LocationId>,
    places: &HashMap<LocationId, PlaceFacts>,
) -> BTreeMap<AreaKey, BTreeMap<AreaKey, Vec<(String, LocationId)>>> {
    let mut regions: BTreeMap<AreaKey, BTreeMap<AreaKey, Vec<(String, LocationId)>>> =
        BTreeMap::new();
    for &place in present {
        let facts = place_facts(places, place);
        let (region, system) = match &facts.geo {
            Some(geo) => (
                (false, geo.region.clone(), geo.region_id),
                (false, geo.system.clone(), geo.system_id),
            ),
            None => (
                (true, "Unknown region".to_owned(), 0),
                (true, "Unknown system".to_owned(), 0),
            ),
        };
        regions
            .entry(region)
            .or_default()
            .entry(system)
            .or_default()
            .push((facts.label, place));
    }
    for systems in regions.values_mut() {
        for list in systems.values_mut() {
            list.sort_by_key(|(label, place)| (label.to_lowercase(), *place));
        }
    }
    regions
}

/// The facts of a type, or the stand-in for one the caller did not give: named by its id, valued, unpriced.
fn type_facts(types: &HashMap<TypeId, TypeFacts>, type_id: TypeId) -> TypeFacts {
    types.get(&type_id).cloned().unwrap_or_else(|| TypeFacts {
        name: type_id.to_string(),
        valued: true,
        price: None,
    })
}

/// The facts of a place, or the stand-in for one the caller did not give: named by its id, no geography.
fn place_facts(places: &HashMap<LocationId, PlaceFacts>, place: LocationId) -> PlaceFacts {
    places.get(&place).cloned().unwrap_or_else(|| PlaceFacts {
        label: place.to_string(),
        geo: None,
    })
}

/// The id of a known region or system, `None` for the unknown one.
fn known_id(key: &AreaKey) -> Option<u32> {
    (!key.0).then_some(key.2)
}

fn entry(key: u64, depth: u32, label: String, select: Option<LocationId>, sum: &Sum) -> Entry {
    Entry {
        key,
        depth,
        label,
        select,
        stacks: sum.stacks,
        value: if sum.too_large { None } else { sum.value },
        unpriced: sum.unpriced.len(),
    }
}

/// Walks the rows under a place: the containers in it, and their contents.
struct Walk<'a> {
    assets: &'a [ResolvedAsset],
    types: &'a HashMap<TypeId, TypeFacts>,
    sums: &'a BTreeMap<Node, Sum>,
    inside: &'a BTreeMap<ItemId, Vec<usize>>,
}

impl Walk<'_> {
    /// A row for `node`, with what it sums up.
    fn row(&self, node: Node, depth: u32, label: String) -> Result<Entry, TreeError> {
        let sum = self.sums.get(&node).cloned().unwrap_or_default();
        Ok(entry(node_key(node)?, depth, label, select(node), &sum))
    }

    /// The row of the container asset at `index`, then the containers inside it.
    fn container(&self, out: &mut Vec<Entry>, index: usize, depth: u32) -> Result<(), TreeError> {
        let asset = &self.assets.get(index).ok_or(TreeError::Loop)?.asset;
        let label = type_facts(self.types, asset.type_id).name;
        out.push(self.row(Node::Container(asset.item_id), depth, label)?);
        for &child in self.inside.get(&asset.item_id).into_iter().flatten() {
            self.container(out, child, depth + 1)?;
        }
        Ok(())
    }
}

fn node_key(node: Node) -> Result<u64, TreeError> {
    match node {
        Node::Region(id) => key(REGION, u64::from(id.unwrap_or(0))),
        Node::System(id) => key(SYSTEM, u64::from(id.unwrap_or(0))),
        Node::Place(place) => key(PLACE, to_id(place)?),
        Node::Container(item) => key(CONTAINER, to_id(item)?),
    }
}

fn key(kind: u64, id: u64) -> Result<u64, TreeError> {
    if id >= ID_LIMIT {
        return Err(TreeError::IdOutOfRange);
    }
    Ok(kind << KIND_SHIFT | id)
}

fn to_id(id: i64) -> Result<u64, TreeError> {
    u64::try_from(id).ok().ok_or(TreeError::IdOutOfRange)
}

/// What a click on a row picks. The unknown region and system pick nothing.
fn select(node: Node) -> Option<LocationId> {
    match node {
        Node::Region(id) | Node::System(id) => id.map(i64::from),
        Node::Place(place) => Some(place),
        Node::Container(item) => Some(item),
    }
}

/// The rows as the table draws them: the name, the stacks and the value.
pub(super) fn rows(tree: &Tree) -> Vec<Row> {
    tree.entries
        .iter()
        .map(|entry| {
            let shown = i64::try_from(entry.stacks)
                .map_or_else(|_| entry.stacks.to_string(), group_thousands);
            Row {
                key: entry.key,
                depth: entry.depth,
                cells: vec![
                    Cell::text(entry.label.clone()),
                    Cell::int(i128::from(entry.stacks), shown),
                    entry.value.map_or_else(Cell::missing, Cell::money),
                ],
            }
        })
        .collect()
}

fn columns() -> Vec<Column> {
    vec![
        Column {
            id: "place",
            label: "Place".to_owned(),
            kind: ColumnKind::Text,
            width: 420.0,
        },
        Column {
            id: "stacks",
            label: "Stacks".to_owned(),
            kind: ColumnKind::Number,
            width: 90.0,
        },
        Column {
            id: "value",
            label: "Value".to_owned(),
            kind: ColumnKind::Money,
            width: 160.0,
        },
    ]
}

/// What the panel reads. The tree is rebuilt only when one of these changes.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Key {
    /// Each stock character with its data revision.
    characters: Vec<(i64, u64)>,
    /// A hash of jEve's assets: jEve's data is replaced whole, and any change in it changes this.
    jeve_hash: u64,
    unticked: BTreeSet<i64>,
    basis: PriceBasis,
    typed: BTreeMap<i64, String>,
    today: i64,
}

/// The built tree and what the footer and the clicks need of it.
#[derive(Debug)]
struct Built {
    key: Key,
    tree: Result<Tree, TreeError>,
    rows: Vec<Row>,
    /// The entry index by row key.
    index: HashMap<u64, usize>,
    newest_day: Option<i64>,
}

impl Built {
    fn new(key: Key, tree: Result<Tree, TreeError>, newest_day: Option<i64>) -> Self {
        let rows = tree.as_ref().map(rows).unwrap_or_default();
        let index = tree.as_ref().map_or_else(
            |_| HashMap::new(),
            |tree| {
                tree.entries
                    .iter()
                    .enumerate()
                    .map(|(position, entry)| (entry.key, position))
                    .collect()
            },
        );
        Self {
            key,
            tree,
            rows,
            index,
            newest_day,
        }
    }
}

/// The Places panel's state, kept in the panel's temporary data.
#[derive(Debug, Default, Clone)]
struct State {
    built: Option<Arc<Built>>,
    dialog: Option<Dialog>,
}

/// What the table asked for this frame: a row picked, and a structure to name.
#[derive(Default)]
struct Outcome {
    pick: Option<Option<LocationId>>,
    name: Option<LocationId>,
}

/// Draws the Places panel: the character filter, the tree, and the footer.
pub(super) fn show(ui: &mut Ui, cx: &mut Shared) {
    if !cx.session.loaded() {
        dock::placeholder(ui, cx);
        return;
    }
    let id = Id::new("assets places panel");
    let mut state = ui
        .data(|data| data.get_temp::<State>(id))
        .unwrap_or_default();
    draw(&mut state, ui, cx);
    ui.data_mut(|data| data.insert_temp(id, state));
}

fn draw(state: &mut State, ui: &mut Ui, cx: &mut Shared) {
    let stock = cx.session.stock_characters();
    let unticked = match unticked(cx) {
        Ok(unticked) => unticked,
        Err(message) => {
            ui.colored_label(cx.style.neg, message);
            BTreeSet::new()
        }
    };
    filter(ui, cx, &stock, &unticked);

    let picked: BTreeSet<i64> = stock
        .iter()
        .map(|character| character.character_id)
        .filter(|id| !unticked.contains(id))
        .collect();
    if picked.is_empty() {
        ui.colored_label(cx.style.mute, "No character picked.");
        return;
    }
    if cx.sde.is_none() {
        ui.colored_label(cx.style.mute, "The game data is not in yet.");
        return;
    }
    let built = match current(state, cx, &stock, &unticked, &picked) {
        Ok(built) => built,
        Err(message) => {
            ui.colored_label(cx.style.neg, message);
            return;
        }
    };
    let tree = match &built.tree {
        Ok(tree) => tree,
        Err(error) => {
            ui.colored_label(cx.style.neg, error.to_string());
            return;
        }
    };

    let outcome = table(ui, cx, &built, tree);
    if let Some(pick) = outcome.pick {
        cx.selection.place = pick;
    }
    if let Some(id) = outcome.name {
        match cx.store.structure_name(id) {
            Ok(text) => state.dialog = Some(Dialog::new(id, text.unwrap_or_default())),
            Err(error) => cx.notices.push(Notice::error(format!(
                "Could not read the structure name: {error}"
            ))),
        }
    }
    let now = (cx.clock)();
    ui.label(footer(built.key.basis, built.newest_day, now));
    name_dialog::show(&mut state.dialog, ui.ctx(), cx);
}

/// The tree table: rows, the selected row, the marker for unpriced types, and the right-click menu of
/// a structure. Returns what the player asked for.
fn table(ui: &mut Ui, cx: &Shared, built: &Built, tree: &Tree) -> Outcome {
    let style = cx.style;
    let selected = cx
        .selection
        .place
        .and_then(|place| {
            tree.entries
                .iter()
                .find(|entry| entry.select == Some(place))
        })
        .map(|entry| entry.key);
    let columns = columns();
    let mut outcome = Outcome::default();
    let response = {
        let mut draw_cell = |ui: &mut Ui, key: u64, column: &'static str| -> bool {
            let Some(entry) = built
                .index
                .get(&key)
                .and_then(|&index| tree.entries.get(index))
            else {
                return false;
            };
            match column {
                "place" => place_cell(ui, entry, &mut outcome),
                "value" => value_cell(ui, entry, style.stale),
                _ => false,
            }
        };
        Table::new(Id::new("assets places"), &columns)
            .selected(selected)
            .show_with(ui, &style, cx.density, &built.rows, None, &mut draw_cell)
    };
    // A click on the place cell is caught by the cell; a click anywhere else comes from the table.
    if let Some(key) = response.clicked
        && outcome.pick.is_none()
        && let Some(entry) = built
            .index
            .get(&key)
            .and_then(|&index| tree.entries.get(index))
    {
        outcome.pick = Some(entry.select);
    }
    outcome
}

/// The place cell: its label, a click that picks the row, and for a structure the `Name this structure…`
/// menu. Returns that the cell was drawn here.
fn place_cell(ui: &mut Ui, entry: &Entry, outcome: &mut Outcome) -> bool {
    ui.label(entry.label.as_str());
    let response = ui.interact(
        ui.max_rect(),
        Id::new(("assets place", entry.key)),
        Sense::click(),
    );
    if response.clicked() {
        outcome.pick = Some(entry.select);
    }
    if let Some(structure) = structure_of(entry) {
        response.context_menu(|ui| {
            if ui.button("Name this structure…").clicked() {
                outcome.name = Some(structure);
                ui.close();
            }
        });
    }
    true
}

/// The id of the structure a row is, when it is a place with an id of a structure.
fn structure_of(entry: &Entry) -> Option<LocationId> {
    if entry.key >> KIND_SHIFT != PLACE {
        return None;
    }
    entry.select.filter(|place| *place >= FIRST_STRUCTURE_ID)
}

/// The value cell. A row with unpriced types shows its value followed by an amber ` +`; a row with no
/// value shows its dash alone.
fn value_cell(ui: &mut Ui, entry: &Entry, stale: Color32) -> bool {
    let Some(value) = entry.value else {
        return false;
    };
    if entry.unpriced == 0 {
        return false;
    }
    // The Value column lays out right to left, so the row is pinned left to right: the marker
    // follows the number.
    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
        ui.label(value.format_isk());
        let marker = RichText::new(" +").color(stale);
        ui.add(Label::new(marker).selectable(false))
            .on_hover_text(unpriced_text(entry.unpriced));
    });
    true
}

fn unpriced_text(count: usize) -> String {
    let noun = if count == 1 { "type" } else { "types" };
    format!("{count} item {noun} not priced")
}

/// The footer: the basis and the age of the newest saved day used.
fn footer(basis: PriceBasis, newest_day: Option<i64>, now: i64) -> String {
    let Some(day) = newest_day else {
        return "No saved prices yet. Refresh a character to save today's prices.".to_owned();
    };
    let saved = day
        .checked_mul(DAY)
        .and_then(|start| now.checked_sub(start))
        .map_or_else(|| "at an unknown time".to_owned(), age_text);
    format!("Prices: {} · saved {} ago", basis_label(basis), saved)
}

/// The character filter: a dropdown with a tick per stock character.
fn filter(ui: &mut Ui, cx: &mut Shared, stock: &[CharacterAssets], unticked: &BTreeSet<i64>) {
    let total = stock.len();
    let left_out = stock
        .iter()
        .filter(|character| unticked.contains(&character.character_id))
        .count();
    let shown = if left_out == 0 {
        "Characters: all".to_owned()
    } else {
        format!("Characters: {} of {total}", total - left_out)
    };
    let session = &cx.session;
    let mut next = unticked.clone();
    egui::ComboBox::from_id_salt("assets characters")
        .selected_text(shown)
        .show_ui(ui, |ui| {
            for character in stock {
                let id = character.character_id;
                let mut ticked = !next.contains(&id);
                if ui.checkbox(&mut ticked, owner_name(session, id)).changed() {
                    if ticked {
                        next.remove(&id);
                    } else {
                        next.insert(id);
                    }
                }
            }
        });
    if &next != unticked {
        let text = next
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(",");
        if let Err(error) = cx.store.set_setting(FILTER_KEY, &text) {
            cx.notices.push(Notice::error(format!(
                "Could not save the character filter: {error}"
            )));
        }
    }
}

/// The ids of the characters the player left out, as saved. A saved value that does not read is an error.
fn unticked(cx: &mut Shared) -> Result<BTreeSet<i64>, String> {
    let saved = cx
        .store
        .setting(FILTER_KEY)
        .map_err(|error| format!("Could not read the character filter: {error}"))?;
    let Some(saved) = saved else {
        return Ok(BTreeSet::new());
    };
    saved
        .split(',')
        .filter(|part| !part.is_empty())
        .map(|part| {
            part.parse::<i64>()
                .map_err(|error| format!("Could not read the character filter: {error}"))
        })
        .collect()
}

/// The owner's name for a stock character, as the Item panel names it.
fn owner_name(session: &Session, character: i64) -> String {
    if let Ok(id) = u64::try_from(character)
        && matches!(session.source(id), Some(Source::Login))
        && let Some(login) = session.characters().iter().find(|saved| saved.id == id)
    {
        return login.name.clone();
    }
    match session.jeve().and_then(|jeve| {
        jeve.characters
            .iter()
            .find(|held| held.character_id == character)
    }) {
        Some(held) => format!("{} (jEve)", held.name),
        None => character.to_string(),
    }
}

/// The built tree for the picked characters, from the state when nothing it reads has changed.
fn current(
    state: &mut State,
    cx: &mut Shared,
    stock: &[CharacterAssets],
    unticked: &BTreeSet<i64>,
    picked: &BTreeSet<i64>,
) -> Result<Arc<Built>, String> {
    let today = (cx.clock)().div_euclid(DAY);
    let key = cache_key(cx, stock, unticked, today)?;
    if let Some(built) = state.built.as_ref().filter(|built| built.key == key) {
        return Ok(Arc::clone(built));
    }
    let built = Arc::new(build_for(cx, stock, picked, key)?);
    state.built = Some(Arc::clone(&built));
    Ok(built)
}

fn cache_key(
    cx: &mut Shared,
    stock: &[CharacterAssets],
    unticked: &BTreeSet<i64>,
    today: i64,
) -> Result<Key, String> {
    let basis = cx
        .store
        .net_worth_settings()
        .map_err(|error| format!("Could not read the price basis: {error}"))?
        .basis;
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
        basis,
        typed,
        today,
    })
}

/// Reads the game data and the saved prices, and builds the tree of the picked characters' assets.
fn build_for(
    cx: &mut Shared,
    stock: &[CharacterAssets],
    picked: &BTreeSet<i64>,
    key: Key,
) -> Result<Built, String> {
    let Some(sde) = cx.sde.as_ref() else {
        return Err("The game data is not in yet.".to_owned());
    };
    let assets: Vec<ResolvedAsset> = resolve(stock)
        .assets
        .into_iter()
        .filter(|asset| picked.contains(&asset.character_id))
        .collect();

    let type_ids: BTreeSet<TypeId> = assets.iter().map(|asset| asset.asset.type_id).collect();
    let mut named = Vec::with_capacity(type_ids.len());
    for type_id in type_ids {
        let item = sde.item(type_id).map_err(|error| game_data_error(&error))?;
        let (name, category) = item.map_or_else(
            || (type_id.to_string(), 0),
            |item| (item.name, item.category_id),
        );
        named.push((type_id, name, category != CATEGORY_BLUEPRINT));
    }
    let valued: BTreeSet<TypeId> = named
        .iter()
        .filter(|(_, _, valued)| *valued)
        .map(|(type_id, _, _)| *type_id)
        .collect();
    let prices = Prices::load(&mut cx.store, &valued, key.today)
        .map_err(|error| format!("Could not read the saved prices: {error}"))?;
    let newest_day = prices.newest_day(valued.iter().copied());
    let types: HashMap<TypeId, TypeFacts> = named
        .into_iter()
        .map(|(type_id, name, valued)| {
            let price = if valued { prices.price(type_id) } else { None };
            (
                type_id,
                TypeFacts {
                    name,
                    valued,
                    price,
                },
            )
        })
        .collect();

    let mut places = HashMap::new();
    for asset in &assets {
        if places.contains_key(&asset.place) {
            continue;
        }
        let label = cx.session.place_label(asset.place, Some(sde));
        let geo = geo_of(sde, &cx.session, asset.place).map_err(|error| game_data_error(&error))?;
        places.insert(asset.place, PlaceFacts { label, geo });
    }

    let tree = build(&assets, &types, &places);
    if tree.as_ref().is_ok_and(|tree| tree.too_large) {
        cx.notices
            .push(Notice::error(TreeError::ValueTooLarge.to_string()));
    }
    Ok(Built::new(key, tree, newest_day))
}

fn game_data_error(error: &SdeError) -> String {
    format!("Could not read the game data: {error}")
}

/// Where a place is: its system and region, from the game data for a station or a system, and from
/// the structure's entry for a structure. `None` when the game data does not know its system.
fn geo_of(sde: &Sde, session: &Session, place: LocationId) -> Result<Option<Geo>, SdeError> {
    let system = if (FIRST_SYSTEM..=LAST_SYSTEM).contains(&place) {
        u32::try_from(place).ok()
    } else if (FIRST_STATION..=LAST_STATION).contains(&place) {
        sde.station(place)?.map(|station| station.system_id)
    } else if place >= FIRST_STRUCTURE_ID {
        session
            .structures()
            .get(&place)
            .and_then(|entry| entry.system_id)
    } else {
        None
    };
    let Some(system) = system else {
        return Ok(None);
    };
    Ok(sde.system(system)?.map(|solar| Geo {
        region_id: solar.region_id,
        region: solar.region_name,
        system_id: solar.id,
        system: solar.name,
    }))
}

#[cfg(test)]
mod tests {
    use mop_core::stock::Asset;

    use super::*;

    const JITA_IV: LocationId = 60_003_760;
    const HOME_FORT: LocationId = 1_035_466_617_946;
    const REFUSED: LocationId = 1_035_466_617_999;
    const RAM: TypeId = 11_481;
    const RIFTER: TypeId = 587;
    const BLUEPRINT: TypeId = 690;
    /// 464.30 in ten-thousandths: the midpoint of the saved prices of R.A.M.
    const RAM_PRICE: i64 = 4_643_000;
    /// 450,000.00 in ten-thousandths: the Rifter.
    const RIFTER_PRICE: i64 = 4_500_000_000;

    fn asset(
        item: ItemId,
        type_id: TypeId,
        location: LocationId,
        place: LocationId,
        quantity: u64,
    ) -> ResolvedAsset {
        ResolvedAsset {
            character_id: 1,
            asset: Asset {
                item_id: item,
                type_id,
                location_id: location,
                flag: None,
                quantity,
                singleton: false,
            },
            place,
            counts_as_stock: true,
        }
    }

    /// The spec's sample: a Rifter with 10 units in it, 40, 60 and 5 units in Jita 4-4, 120 units in
    /// the named structure, and 3 units in a structure nobody names.
    fn sample() -> Vec<ResolvedAsset> {
        vec![
            asset(1, RAM, JITA_IV, JITA_IV, 40),
            asset(2, RAM, HOME_FORT, HOME_FORT, 120),
            asset(9001, RIFTER, JITA_IV, JITA_IV, 1),
            asset(3, RAM, 9001, JITA_IV, 10),
            asset(4, RAM, REFUSED, REFUSED, 3),
            asset(5, RAM, JITA_IV, JITA_IV, 60),
            asset(6, RAM, JITA_IV, JITA_IV, 5),
        ]
    }

    fn type_map(ram: Option<i64>, rifter: Option<i64>) -> HashMap<TypeId, TypeFacts> {
        HashMap::from([
            (
                RAM,
                TypeFacts {
                    name: "R.A.M.- Robotics".to_owned(),
                    valued: true,
                    price: ram.map(Money::from_ten_thousandths),
                },
            ),
            (
                RIFTER,
                TypeFacts {
                    name: "Rifter".to_owned(),
                    valued: true,
                    price: rifter.map(Money::from_ten_thousandths),
                },
            ),
        ])
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
                JITA_IV,
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

    /// Depth, label, stacks, value in ten-thousandths, and unpriced types, row by row.
    fn shape(tree: &Tree) -> Vec<(u32, String, u64, Option<i64>, usize)> {
        tree.entries
            .iter()
            .map(|entry| {
                (
                    entry.depth,
                    entry.label.clone(),
                    entry.stacks,
                    entry.value.map(|value| value.ten_thousandths()),
                    entry.unpriced,
                )
            })
            .collect()
    }

    fn row(
        depth: u32,
        label: &str,
        stacks: u64,
        value: Option<i64>,
        unpriced: usize,
    ) -> (u32, String, u64, Option<i64>, usize) {
        (depth, label.to_owned(), stacks, value, unpriced)
    }

    fn root(tree: &Tree) -> &Entry {
        tree.entries.first().unwrap()
    }

    fn jita_row(tree: &Tree) -> &Entry {
        let wanted = key(PLACE, JITA_IV as u64).unwrap();
        tree.entries
            .iter()
            .find(|entry| entry.key == wanted)
            .unwrap()
    }

    #[test]
    fn rows_sum_their_stacks_and_values_in_order() {
        let tree = build(
            &sample(),
            &type_map(Some(RAM_PRICE), Some(RIFTER_PRICE)),
            &place_map(),
        )
        .unwrap();
        assert_eq!(
            shape(&tree),
            vec![
                row(0, "All places", 7, Some(5_605_034_000), 0),
                row(1, "The Forge", 6, Some(5_591_105_000), 0),
                row(2, "Jita", 6, Some(5_591_105_000), 0),
                row(3, "Home Fort", 1, Some(557_160_000), 0),
                row(
                    3,
                    "Jita IV - Moon 4 - Caldari Navy Assembly Plant",
                    5,
                    Some(5_033_945_000),
                    0,
                ),
                row(4, "Rifter", 1, Some(46_430_000), 0),
                row(1, "Unknown region", 1, Some(13_929_000), 0),
                row(2, "Unknown system", 1, Some(13_929_000), 0),
                row(
                    3,
                    "Structure 1035466617999 (no access)",
                    1,
                    Some(13_929_000),
                    0
                ),
            ]
        );
    }

    #[test]
    fn a_row_selects_its_place_and_the_root_selects_nothing() {
        let tree = build(
            &sample(),
            &type_map(Some(RAM_PRICE), Some(RIFTER_PRICE)),
            &place_map(),
        )
        .unwrap();
        let selects: Vec<Option<LocationId>> =
            tree.entries.iter().map(|entry| entry.select).collect();
        assert_eq!(
            selects,
            vec![
                None,
                Some(10_000_002),
                Some(30_000_142),
                Some(HOME_FORT),
                Some(JITA_IV),
                Some(9001),
                None,
                None,
                Some(REFUSED),
            ]
        );
    }

    #[test]
    fn an_unpriced_type_leaves_its_value_short_and_is_counted() {
        let tree = build(&sample(), &type_map(Some(RAM_PRICE), None), &place_map()).unwrap();
        let jita = jita_row(&tree);
        assert_eq!(
            jita.value.map(|value| value.ten_thousandths()),
            Some(533_945_000)
        );
        assert_eq!(jita.unpriced, 1);
        assert_eq!(root(&tree).unpriced, 1);
    }

    #[test]
    fn a_blueprint_is_not_valued_and_not_counted_as_unpriced() {
        let mut assets = sample();
        assets.push(asset(7, BLUEPRINT, JITA_IV, JITA_IV, 1));
        let mut types = type_map(Some(RAM_PRICE), Some(RIFTER_PRICE));
        types.insert(
            BLUEPRINT,
            TypeFacts {
                name: "Some Blueprint".to_owned(),
                valued: false,
                price: None,
            },
        );
        let tree = build(&assets, &types, &place_map()).unwrap();
        let jita = jita_row(&tree);
        assert_eq!(jita.stacks, 6);
        assert_eq!(
            jita.value.map(|value| value.ten_thousandths()),
            Some(5_033_945_000)
        );
        assert_eq!(jita.unpriced, 0);
    }

    #[test]
    fn with_no_prices_every_value_is_missing() {
        let tree = build(&sample(), &type_map(None, None), &place_map()).unwrap();
        assert!(tree.entries.iter().all(|entry| entry.value.is_none()));
        // Both types are counted as unpriced; a missing value shows no " +".
        assert_eq!(root(&tree).unpriced, 2);
    }

    #[test]
    fn a_value_too_large_for_money_shows_dashes_and_keeps_the_tree() {
        let huge = type_map(Some(i64::MAX / 2), Some(RIFTER_PRICE));
        let tree = build(&sample(), &huge, &place_map()).unwrap();
        assert!(tree.too_large);
        assert_eq!(root(&tree).value, None);
        assert!(tree.entries.iter().all(|entry| entry.value.is_none()));
        assert_eq!(root(&tree).stacks, 7);
    }

    #[test]
    fn items_that_hold_each_other_are_an_error() {
        let assets = vec![asset(1, RAM, 2, JITA_IV, 1), asset(2, RAM, 1, JITA_IV, 1)];
        assert_eq!(
            build(&assets, &type_map(Some(RAM_PRICE), None), &place_map()),
            Err(TreeError::Loop)
        );
    }
}

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use mop_core::industry::TypeId;
use mop_core::money::Money;
use mop_core::stock::{ItemId, LocationId, ResolvedAsset};

use super::{
    AreaKey, CONTAINER, Entry, Holding, ID_LIMIT, KIND_SHIFT, Node, PLACE, PlaceFacts, REGION,
    ROOT, SYSTEM, Sum, Sums, Tree, TreeError, TypeFacts,
};
use crate::assets::value::stack_value;

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

pub(super) fn key(kind: u64, id: u64) -> Result<u64, TreeError> {
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

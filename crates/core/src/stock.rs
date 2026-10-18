//! Counts the units of each item a user owns in chosen characters and places,
//! from the assets of every character. Running jobs are not counted here.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::industry::TypeId;

pub type ItemId = i64;
pub type LocationId = i64;
pub type CharacterId = i64;

/// One item as the source lists it. `location_id` is another asset's `item_id`
/// when the item sits inside that asset, otherwise a place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    pub item_id: ItemId,
    pub type_id: TypeId,
    pub location_id: LocationId,
    /// ESI location flag text (`Hangar`, `Cargo`, `HiSlot0`, ...); `None` when the source has none.
    pub flag: Option<String>,
    pub quantity: u64,
    pub singleton: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CharacterAssets {
    pub character_id: CharacterId,
    /// When the source last fetched this character's assets, Unix seconds; `None` = unknown.
    pub updated: Option<i64>,
    pub assets: Vec<Asset>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StockScope {
    pub characters: BTreeSet<CharacterId>,
    pub places: Places,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Places {
    All,
    Only(BTreeSet<LocationId>),
}

impl Places {
    fn contains(&self, place: LocationId) -> bool {
        match self {
            Self::All => true,
            Self::Only(places) => places.contains(&place),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedAsset {
    pub character_id: CharacterId,
    pub asset: Asset,
    /// The place the asset sits in, after following containers.
    pub place: LocationId,
    pub counts_as_stock: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub assets: Vec<ResolvedAsset>,
    /// Items whose location chain never reaches a place; left out of `assets`.
    pub looped: Vec<ItemId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StockError {
    Overflow { type_id: TypeId },
}

impl fmt::Display for StockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Overflow { type_id } => {
                write!(
                    f,
                    "the units of type {type_id} add up to more than fit in a count"
                )
            }
        }
    }
}

impl std::error::Error for StockError {}

const FITTED_PREFIXES: [&str; 7] = [
    "HiSlot",
    "MedSlot",
    "LoSlot",
    "RigSlot",
    "SubSystemSlot",
    "ServiceSlot",
    "FighterTube",
];

/// True when the flag means the item is fitted to a ship or a structure, not stored.
pub fn is_fitted(flag: &str) -> bool {
    flag == "Implant"
        || FITTED_PREFIXES
            .iter()
            .any(|prefix| flag.starts_with(prefix))
}

/// An asset kept after the duplicate rule, with the owner it came from.
struct Candidate<'a> {
    character_id: CharacterId,
    updated: Option<i64>,
    asset: &'a Asset,
}

/// Whether `new` replaces `current` for the same item: the newer character data wins,
/// then the smaller character id. Equal owners keep the row seen first.
fn beats(new: &Candidate<'_>, current: &Candidate<'_>) -> bool {
    new.updated > current.updated
        || (new.updated == current.updated && new.character_id < current.character_id)
}

/// Where each kept item sits: `Some(place)`, or `None` when its chain loops.
/// Walks iteratively and remembers every answer, so a long chain costs one pass.
fn places(kept: &BTreeMap<ItemId, Candidate<'_>>) -> BTreeMap<ItemId, Option<LocationId>> {
    let mut known: BTreeMap<ItemId, Option<LocationId>> = BTreeMap::new();
    for &start in kept.keys() {
        if known.contains_key(&start) {
            continue;
        }
        let mut path: Vec<ItemId> = Vec::new();
        let mut on_path: BTreeSet<ItemId> = BTreeSet::new();
        let mut current = start;
        let outcome = loop {
            if let Some(&done) = known.get(&current) {
                break done;
            }
            if !on_path.insert(current) {
                break None;
            }
            path.push(current);
            // Every id reached here is a kept item: the start is one, and so is each
            // location followed below only when it is kept.
            let Some(row) = kept.get(&current) else {
                break None;
            };
            let location = row.asset.location_id;
            if !kept.contains_key(&location) {
                break Some(location);
            }
            current = location;
        };
        for item in path {
            known.insert(item, outcome);
        }
    }
    known
}

/// Joins the assets of all characters: one row per item, its place, and whether it counts.
pub fn resolve(characters: &[CharacterAssets]) -> Resolved {
    let mut kept: BTreeMap<ItemId, Candidate<'_>> = BTreeMap::new();
    for owner in characters {
        for asset in &owner.assets {
            let candidate = Candidate {
                character_id: owner.character_id,
                updated: owner.updated,
                asset,
            };
            let wins = kept
                .get(&asset.item_id)
                .is_none_or(|current| beats(&candidate, current));
            if wins {
                kept.insert(asset.item_id, candidate);
            }
        }
    }

    let found = places(&kept);
    let mut assets = Vec::new();
    let mut looped = Vec::new();
    for (&item_id, candidate) in &kept {
        match found.get(&item_id).copied().flatten() {
            Some(place) => assets.push(ResolvedAsset {
                character_id: candidate.character_id,
                asset: candidate.asset.clone(),
                place,
                counts_as_stock: !candidate.asset.singleton
                    && !candidate.asset.flag.as_deref().is_some_and(is_fitted),
            }),
            None => looped.push(item_id),
        }
    }
    Resolved { assets, looped }
}

impl Resolved {
    /// Units per type over the assets that count, in the chosen characters and places.
    pub fn owned(&self, scope: &StockScope) -> Result<BTreeMap<TypeId, u64>, StockError> {
        let mut totals: BTreeMap<TypeId, u64> = BTreeMap::new();
        for row in &self.assets {
            let in_scope =
                scope.characters.contains(&row.character_id) && scope.places.contains(row.place);
            if !row.counts_as_stock || !in_scope || row.asset.quantity == 0 {
                continue;
            }
            let type_id = row.asset.type_id;
            let so_far = totals.get(&type_id).copied().unwrap_or(0);
            let total = so_far
                .checked_add(row.asset.quantity)
                .ok_or(StockError::Overflow { type_id })?;
            totals.insert(type_id, total);
        }
        Ok(totals)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATION: LocationId = 60_003_760;
    const STRUCTURE: LocationId = 1_035_466_617_946;
    const JITA_SYSTEM: LocationId = 30_000_142;
    const CHAR_A: CharacterId = 9001;
    const CHAR_B: CharacterId = 9002;

    fn row(
        item_id: ItemId,
        type_id: TypeId,
        location_id: LocationId,
        flag: Option<&str>,
        quantity: u64,
        singleton: bool,
    ) -> Asset {
        Asset {
            item_id,
            type_id,
            location_id,
            flag: flag.map(str::to_owned),
            quantity,
            singleton,
        }
    }

    fn character_a(updated: Option<i64>) -> CharacterAssets {
        CharacterAssets {
            character_id: CHAR_A,
            updated,
            assets: vec![
                row(1, 34, STATION, Some("Hangar"), 1_000, false),
                row(2, 3466, STATION, Some("Hangar"), 1, true),
                row(3, 34, 2, Some("Unlocked"), 500, false),
                row(4, 587, STRUCTURE, Some("Hangar"), 1, true),
                row(5, 35, 4, Some("Cargo"), 300, false),
                row(6, 185, 4, Some("HiSlot0"), 100, false),
                row(7, 2454, 4, Some("DroneBay"), 5, false),
                row(8, 34, JITA_SYSTEM, None, 50, false),
                row(20, 34, 21, Some("Hangar"), 7, false),
                row(21, 34, 20, Some("Hangar"), 7, false),
                row(22, 35, 20, Some("Unlocked"), 9, false),
            ],
        }
    }

    fn character_b(updated: Option<i64>) -> CharacterAssets {
        CharacterAssets {
            character_id: CHAR_B,
            updated,
            assets: vec![
                row(9, 34, STATION, Some("Hangar"), 2_000, false),
                row(3, 34, STATION, Some("Hangar"), 450, false),
            ],
        }
    }

    fn fixture() -> Vec<CharacterAssets> {
        vec![character_a(Some(1_000)), character_b(Some(2_000))]
    }

    fn find(resolved: &Resolved, item_id: ItemId) -> &ResolvedAsset {
        resolved
            .assets
            .iter()
            .find(|found| found.asset.item_id == item_id)
            .expect("item is resolved")
    }

    fn scope(characters: &[CharacterId], places: Places) -> StockScope {
        StockScope {
            characters: characters.iter().copied().collect(),
            places,
        }
    }

    fn only(places: &[LocationId]) -> Places {
        Places::Only(places.iter().copied().collect())
    }

    #[test]
    fn fixture_resolves_places_loops_and_counting() {
        let resolved = resolve(&fixture());
        assert_eq!(resolved.looped, vec![20, 21, 22]);
        let ids: Vec<ItemId> = resolved.assets.iter().map(|r| r.asset.item_id).collect();
        assert_eq!(ids, vec![1, 2, 3, 4, 5, 6, 7, 8, 9]);

        let item_three = find(&resolved, 3);
        assert_eq!(item_three.character_id, CHAR_B);
        assert_eq!(item_three.asset.quantity, 450);
        assert_eq!(item_three.place, STATION);

        let places: Vec<(ItemId, LocationId)> = resolved
            .assets
            .iter()
            .map(|r| (r.asset.item_id, r.place))
            .collect();
        assert_eq!(
            places,
            vec![
                (1, STATION),
                (2, STATION),
                (3, STATION),
                (4, STRUCTURE),
                (5, STRUCTURE),
                (6, STRUCTURE),
                (7, STRUCTURE),
                (8, JITA_SYSTEM),
                (9, STATION),
            ]
        );

        let counting: Vec<(ItemId, bool)> = resolved
            .assets
            .iter()
            .map(|r| (r.asset.item_id, r.counts_as_stock))
            .collect();
        assert_eq!(
            counting,
            vec![
                (1, true),
                (2, false),
                (3, true),
                (4, false),
                (5, true),
                (6, false),
                (7, true),
                (8, true),
                (9, true),
            ]
        );
    }

    #[test]
    fn newest_character_data_wins_a_duplicate_item() {
        let newer_a = [character_a(Some(3_000)), character_b(Some(2_000))];
        let item_three = find(&resolve(&newer_a), 3).clone();
        assert_eq!(item_three.character_id, CHAR_A);
        assert_eq!(item_three.asset.quantity, 500);
        assert_eq!(item_three.place, STATION);

        let both_unknown = [character_a(None), character_b(None)];
        assert_eq!(find(&resolve(&both_unknown), 3).character_id, CHAR_A);

        let both_unknown_reversed = [character_b(None), character_a(None)];
        assert_eq!(
            find(&resolve(&both_unknown_reversed), 3).character_id,
            CHAR_A
        );

        let only_b_known = [character_a(None), character_b(Some(1))];
        assert_eq!(find(&resolve(&only_b_known), 3).character_id, CHAR_B);
    }

    #[test]
    fn owned_sums_every_character_and_place() {
        let resolved = resolve(&fixture());
        let all = scope(&[CHAR_A, CHAR_B], Places::All);
        let owned = resolved.owned(&all).expect("no overflow");
        assert_eq!(owned, BTreeMap::from([(34, 3_500), (35, 300), (2454, 5)]));
    }

    #[test]
    fn owned_respects_character_and_place_scope() {
        let resolved = resolve(&fixture());

        let only_a = resolved
            .owned(&scope(&[CHAR_A], Places::All))
            .expect("no overflow");
        assert_eq!(only_a, BTreeMap::from([(34, 1_050), (35, 300), (2454, 5)]));

        let only_a_station = resolved
            .owned(&scope(&[CHAR_A], only(&[STATION])))
            .expect("no overflow");
        assert_eq!(only_a_station, BTreeMap::from([(34, 1_000)]));

        let structure = resolved
            .owned(&scope(&[CHAR_A, CHAR_B], only(&[STRUCTURE])))
            .expect("no overflow");
        assert_eq!(structure, BTreeMap::from([(35, 300), (2454, 5)]));

        let empty_cases = [
            scope(&[CHAR_B], only(&[JITA_SYSTEM])),
            scope(&[], Places::All),
            scope(&[CHAR_A, CHAR_B], only(&[])),
        ];
        for case in &empty_cases {
            assert!(resolved.owned(case).expect("no overflow").is_empty());
        }
    }

    #[test]
    fn input_order_does_not_change_the_result() {
        let mut a_reversed = character_a(Some(1_000));
        a_reversed.assets.reverse();
        let reordered = [character_b(Some(2_000)), a_reversed];
        assert_eq!(resolve(&reordered), resolve(&fixture()));

        let all = scope(&[CHAR_A, CHAR_B], Places::All);
        assert_eq!(
            resolve(&reordered).owned(&all),
            resolve(&fixture()).owned(&all)
        );
    }

    #[test]
    fn self_reference_is_a_loop() {
        let characters = [CharacterAssets {
            character_id: CHAR_A,
            updated: None,
            assets: vec![row(50, 34, 50, Some("Hangar"), 1, false)],
        }];
        let resolved = resolve(&characters);
        assert_eq!(resolved.looped, vec![50]);
        assert!(resolved.assets.is_empty());
    }

    #[test]
    fn deep_chain_resolves_without_recursion() {
        const DEPTH: i64 = 10_000;
        let assets = (1..=DEPTH)
            .map(|item_id| {
                let location = if item_id == DEPTH {
                    STATION
                } else {
                    item_id + 1
                };
                row(item_id, 34, location, Some("Hangar"), 1, false)
            })
            .collect();
        let characters = [CharacterAssets {
            character_id: CHAR_A,
            updated: None,
            assets,
        }];
        let resolved = resolve(&characters);
        assert!(resolved.looped.is_empty());
        assert_eq!(resolved.assets.len(), 10_000);
        assert!(resolved.assets.iter().all(|r| r.place == STATION));
    }

    #[test]
    fn owned_reports_overflow_by_type() {
        let characters = [CharacterAssets {
            character_id: CHAR_A,
            updated: None,
            assets: vec![
                row(1, 34, STATION, Some("Hangar"), u64::MAX, false),
                row(2, 34, STATION, Some("Hangar"), 1, false),
            ],
        }];
        let resolved = resolve(&characters);
        let all = scope(&[CHAR_A], Places::All);
        assert_eq!(
            resolved.owned(&all),
            Err(StockError::Overflow { type_id: 34 })
        );

        let alone = [CharacterAssets {
            character_id: CHAR_A,
            updated: None,
            assets: vec![row(1, 34, STATION, Some("Hangar"), u64::MAX, false)],
        }];
        assert_eq!(
            resolve(&alone).owned(&all),
            Ok(BTreeMap::from([(34, u64::MAX)]))
        );
    }

    #[test]
    fn is_fitted_matches_slot_flags_and_implants_exactly() {
        for flag in [
            "HiSlot7",
            "MedSlot0",
            "LoSlot6",
            "RigSlot2",
            "SubSystemSlot3",
            "ServiceSlot0",
            "FighterTube4",
            "Implant",
        ] {
            assert!(is_fitted(flag), "{flag} should be fitted");
        }
        for flag in [
            "Hangar",
            "Cargo",
            "DroneBay",
            "Unlocked",
            "AutoFit",
            "FleetHangar",
            "SpecializedCommandCenterHold",
            "hislot0",
            "",
        ] {
            assert!(!is_fitted(flag), "{flag} should not be fitted");
        }
    }

    #[test]
    fn repeated_item_inside_one_character_keeps_the_first_row() {
        let characters = [CharacterAssets {
            character_id: CHAR_A,
            updated: Some(1_000),
            assets: vec![
                row(1, 34, STATION, Some("Hangar"), 10, false),
                row(1, 34, STATION, Some("Hangar"), 20, false),
            ],
        }];
        let resolved = resolve(&characters);
        assert_eq!(resolved.assets.len(), 1);
        assert_eq!(find(&resolved, 1).asset.quantity, 10);
    }

    #[test]
    fn overflow_error_names_the_type() {
        let error = StockError::Overflow { type_id: 34 };
        assert_eq!(
            error.to_string(),
            "the units of type 34 add up to more than fit in a count"
        );
    }

    #[test]
    fn zero_quantity_adds_no_entry() {
        let characters = [CharacterAssets {
            character_id: CHAR_A,
            updated: None,
            assets: vec![row(1, 34, STATION, Some("Hangar"), 0, false)],
        }];
        let owned = resolve(&characters)
            .owned(&scope(&[CHAR_A], Places::All))
            .expect("no overflow");
        assert!(owned.is_empty());
    }
}

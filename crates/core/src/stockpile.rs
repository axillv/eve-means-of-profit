//! Stockpiles: named lists of targets, which can include other stockpiles, and what the user has
//! against them. Pure: the caller passes in the stock and the prices.
//!
//! A stockpile counts every unit in its scope. Two stockpiles may count the same units; reservations
//! are what stop them being used twice.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::industry::TypeId;
use crate::money::Money;
use crate::stock::{CharacterId, LocationId, ResolvedAsset};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stockpile {
    pub id: i64,
    pub name: String,
    pub group: Option<String>,
    pub multiplier: u32,
    pub targets: Vec<Target>,
    pub includes: Vec<Include>,
    pub filters: Vec<Filter>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub type_id: TypeId,
    pub units: u64,
    /// The units are wanted as they are, not multiplied by the stockpile's multiplier.
    pub ignore_multiplier: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Include {
    pub stockpile_id: i64,
    pub count: u32,
}

/// Which stock counts: a character, a place and a source. An entry counts when one filter matches it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filter {
    pub characters: Characters,
    pub place: Place,
    pub sources: BTreeSet<Source>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Characters {
    All,
    Only(BTreeSet<CharacterId>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    Everywhere,
    Location(LocationId),
    System(u32),
    Region(u32),
}

/// Where stock can be. A filter lists the ones it counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Source {
    Assets,
    SellOrders,
    BuyOrders,
    Jobs,
}

/// One unit count the caller found for an order or a job: the units sit at `place`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Held {
    pub character_id: CharacterId,
    pub type_id: TypeId,
    pub place: LocationId,
    pub units: u64,
}

/// The stock the caller found, one list per source.
#[derive(Debug, Clone, Copy)]
pub struct Stock<'a> {
    pub assets: &'a [ResolvedAsset],
    pub sell_orders: &'a [Held],
    pub buy_orders: &'a [Held],
    pub jobs: &'a [Held],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StockpileError {
    /// An include names a stockpile that is not in the set.
    Missing(i64),
    /// The ids from a stockpile around an include loop back to it, e.g. `[3, 1, 3]`.
    Loop(Vec<i64>),
    OutOfRange,
}

impl fmt::Display for StockpileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing(id) => write!(f, "stockpile {id} does not exist"),
            Self::Loop(path) => {
                let path: Vec<String> = path.iter().map(i64::to_string).collect();
                write!(f, "stockpiles include themselves: {}", path.join(" → "))
            }
            Self::OutOfRange => {
                write!(f, "the counts add up to more than fit in a count")
            }
        }
    }
}

impl std::error::Error for StockpileError {}

/// The ids from `id` to the first id that repeats, when an include leads back to a stockpile
/// already on the way there. `None` when no loop is reachable from `id`.
pub fn include_loop(id: i64, all: &BTreeMap<i64, Stockpile>) -> Option<Vec<i64>> {
    visit(id, all, &mut Vec::new(), &mut BTreeSet::new())
}

fn visit(
    id: i64,
    all: &BTreeMap<i64, Stockpile>,
    path: &mut Vec<i64>,
    done: &mut BTreeSet<i64>,
) -> Option<Vec<i64>> {
    if let Some(at) = path.iter().position(|&on_path| on_path == id) {
        let mut found: Vec<i64> = path.iter().skip(at).copied().collect();
        found.push(id);
        return Some(found);
    }
    if done.contains(&id) {
        return None;
    }
    // A missing stockpile has no includes to follow; `targets` reports it.
    let stockpile = all.get(&id)?;
    path.push(id);
    for include in &stockpile.includes {
        if let Some(found) = visit(include.stockpile_id, all, path, done) {
            return Some(found);
        }
    }
    path.pop();
    done.insert(id);
    None
}

/// The units wanted per type for stockpile `id`, with its includes expanded.
pub fn targets(
    id: i64,
    all: &BTreeMap<i64, Stockpile>,
) -> Result<BTreeMap<TypeId, u64>, StockpileError> {
    if let Some(path) = include_loop(id, all) {
        return Err(StockpileError::Loop(path));
    }
    expand(id, all)
}

/// `targets` once there is known to be no loop.
fn expand(
    id: i64,
    all: &BTreeMap<i64, Stockpile>,
) -> Result<BTreeMap<TypeId, u64>, StockpileError> {
    let stockpile = all.get(&id).ok_or(StockpileError::Missing(id))?;
    let multiplier = u64::from(stockpile.multiplier);
    let mut wanted = BTreeMap::new();
    for target in &stockpile.targets {
        let units = if target.ignore_multiplier {
            target.units
        } else {
            target
                .units
                .checked_mul(multiplier)
                .ok_or(StockpileError::OutOfRange)?
        };
        add(&mut wanted, target.type_id, units)?;
    }
    for include in &stockpile.includes {
        let factor = multiplier
            .checked_mul(u64::from(include.count))
            .ok_or(StockpileError::OutOfRange)?;
        for (type_id, units) in expand(include.stockpile_id, all)? {
            let units = units
                .checked_mul(factor)
                .ok_or(StockpileError::OutOfRange)?;
            add(&mut wanted, type_id, units)?;
        }
    }
    Ok(wanted)
}

fn add(
    wanted: &mut BTreeMap<TypeId, u64>,
    type_id: TypeId,
    units: u64,
) -> Result<(), StockpileError> {
    let total = wanted.entry(type_id).or_insert(0);
    *total = total.checked_add(units).ok_or(StockpileError::OutOfRange)?;
    Ok(())
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Have {
    pub assets: u64,
    pub sell_orders: u64,
    pub buy_orders: u64,
    pub jobs: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Ok,
    Part,
    Short,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub type_id: TypeId,
    pub target: u64,
    pub have: Have,
    pub total: u64,
    /// The units still missing; 0 when the total reaches the target.
    pub short: u64,
    pub status: Status,
}

/// What the user has against stockpile `id`, one line per wanted type. Only `id`'s own filters
/// apply: the stockpiles it includes are a list of targets here.
pub fn have(
    id: i64,
    all: &BTreeMap<i64, Stockpile>,
    stock: &Stock<'_>,
    locate: &dyn Fn(LocationId) -> Option<(u32, u32)>,
) -> Result<Vec<Line>, StockpileError> {
    let wanted = targets(id, all)?;
    let filters = &all.get(&id).ok_or(StockpileError::Missing(id))?.filters;
    let mut haves: BTreeMap<TypeId, Have> = wanted.keys().map(|&t| (t, Have::default())).collect();

    for asset in stock.assets.iter().filter(|a| a.counts_as_stock) {
        if matches_any(
            filters,
            asset.character_id,
            asset.place,
            locate,
            Source::Assets,
        ) {
            credit(&mut haves, asset.asset.type_id, asset.asset.quantity, |h| {
                &mut h.assets
            })?;
        }
    }
    for held in stock.sell_orders {
        if matches_any(
            filters,
            held.character_id,
            held.place,
            locate,
            Source::SellOrders,
        ) {
            credit(&mut haves, held.type_id, held.units, |h| &mut h.sell_orders)?;
        }
    }
    for held in stock.buy_orders {
        if matches_any(
            filters,
            held.character_id,
            held.place,
            locate,
            Source::BuyOrders,
        ) {
            credit(&mut haves, held.type_id, held.units, |h| &mut h.buy_orders)?;
        }
    }
    for held in stock.jobs {
        if matches_any(filters, held.character_id, held.place, locate, Source::Jobs) {
            credit(&mut haves, held.type_id, held.units, |h| &mut h.jobs)?;
        }
    }

    wanted
        .into_iter()
        .map(|(type_id, target)| {
            let have = haves.remove(&type_id).unwrap_or_default();
            let total = [have.assets, have.sell_orders, have.buy_orders, have.jobs]
                .into_iter()
                .try_fold(0_u64, u64::checked_add)
                .ok_or(StockpileError::OutOfRange)?;
            // A floor at 0 is meant here: a stockpile holding more than its target is short 0, never a negative count.
            let short = target.saturating_sub(total);
            let status = if total >= target {
                Status::Ok
            } else if total == 0 {
                Status::Short
            } else {
                Status::Part
            };
            Ok(Line {
                type_id,
                target,
                have,
                total,
                short,
                status,
            })
        })
        .collect()
}

/// True when some filter takes this entry: its character, its place and the entry's source.
pub(crate) fn matches_any(
    filters: &[Filter],
    character: CharacterId,
    place: LocationId,
    locate: &dyn Fn(LocationId) -> Option<(u32, u32)>,
    source: Source,
) -> bool {
    filters.iter().any(|filter| {
        let by_character = match &filter.characters {
            Characters::All => true,
            Characters::Only(set) => set.contains(&character),
        };
        by_character
            && place_matches(filter.place, place, locate)
            && filter.sources.contains(&source)
    })
}

fn place_matches(
    wanted: Place,
    place: LocationId,
    locate: &dyn Fn(LocationId) -> Option<(u32, u32)>,
) -> bool {
    match wanted {
        Place::Everywhere => true,
        Place::Location(id) => id == place,
        Place::System(system) => locate(place).is_some_and(|(found, _)| found == system),
        Place::Region(region) => locate(place).is_some_and(|(_, found)| found == region),
    }
}

fn credit(
    haves: &mut BTreeMap<TypeId, Have>,
    type_id: TypeId,
    units: u64,
    slot: fn(&mut Have) -> &mut u64,
) -> Result<(), StockpileError> {
    let Some(have) = haves.get_mut(&type_id) else {
        return Ok(());
    };
    let field = slot(have);
    *field = field.checked_add(units).ok_or(StockpileError::OutOfRange)?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shortfall {
    pub value: Money,
    /// The types that are short but have no price; they add nothing to `value`.
    pub unpriced: Vec<TypeId>,
}

/// What the missing units cost at the given prices.
pub fn shortfall(
    lines: &[Line],
    price: &dyn Fn(TypeId) -> Option<Money>,
) -> Result<Shortfall, StockpileError> {
    let mut value: i128 = 0;
    let mut unpriced = Vec::new();
    for line in lines.iter().filter(|line| line.short > 0) {
        match price(line.type_id) {
            Some(each) => {
                let part = i128::from(line.short)
                    .checked_mul(i128::from(each.ten_thousandths()))
                    .ok_or(StockpileError::OutOfRange)?;
                value = value.checked_add(part).ok_or(StockpileError::OutOfRange)?;
            }
            None => unpriced.push(line.type_id),
        }
    }
    let ten_thousandths = i64::try_from(value)
        .ok()
        .ok_or(StockpileError::OutOfRange)?;
    Ok(Shortfall {
        value: Money::from_ten_thousandths(ten_thousandths),
        unpriced,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAIN: CharacterId = 2_112_000_001;
    const ALT: CharacterId = 2_112_000_002;
    const JITA_4_4: LocationId = 60_003_760;
    const JITA_SYSTEM: u32 = 30_000_142;
    const THE_FORGE: u32 = 10_000_002;
    const STRUCTURE: LocationId = 1_035_466_617_946;
    const ELSEWHERE: LocationId = 60_008_494;

    fn locate(place: LocationId) -> Option<(u32, u32)> {
        match place {
            JITA_4_4 | STRUCTURE => Some((JITA_SYSTEM, THE_FORGE)),
            ELSEWHERE => Some((30_002_187, 10_000_043)),
            _ => None,
        }
    }

    fn target(type_id: TypeId, units: u64) -> Target {
        Target {
            type_id,
            units,
            ignore_multiplier: false,
        }
    }

    fn stockpile(
        id: i64,
        multiplier: u32,
        targets: Vec<Target>,
        includes: Vec<Include>,
        filters: Vec<Filter>,
    ) -> Stockpile {
        Stockpile {
            id,
            name: format!("Stockpile {id}"),
            group: None,
            multiplier,
            targets,
            includes,
            filters,
        }
    }

    fn include(stockpile_id: i64, count: u32) -> Include {
        Include {
            stockpile_id,
            count,
        }
    }

    fn all(list: Vec<Stockpile>) -> BTreeMap<i64, Stockpile> {
        list.into_iter().map(|s| (s.id, s)).collect()
    }

    /// Kit (1), Hammer (2) and Weekly (3), as the spec's acceptance set has them.
    fn set() -> BTreeMap<i64, Stockpile> {
        all(vec![
            stockpile(
                1,
                1,
                vec![target(100, 10), target(200, 50), target(300, 5)],
                vec![],
                vec![],
            ),
            stockpile(2, 1, vec![target(100, 20), target(300, 30)], vec![], vec![]),
            stockpile(
                3,
                2,
                vec![
                    target(100, 1),
                    Target {
                        type_id: 400,
                        units: 7,
                        ignore_multiplier: true,
                    },
                ],
                vec![include(1, 3), include(2, 5)],
                vec![],
            ),
        ])
    }

    fn asset(
        character: CharacterId,
        type_id: TypeId,
        place: LocationId,
        quantity: u64,
        counts: bool,
    ) -> ResolvedAsset {
        ResolvedAsset {
            character_id: character,
            asset: crate::stock::Asset {
                item_id: 0,
                type_id,
                location_id: place,
                flag: None,
                quantity,
                singleton: false,
            },
            place,
            counts_as_stock: counts,
        }
    }

    fn held(character: CharacterId, type_id: TypeId, place: LocationId, units: u64) -> Held {
        Held {
            character_id: character,
            type_id,
            place,
            units,
        }
    }

    fn sources(listed: &[Source]) -> BTreeSet<Source> {
        listed.iter().copied().collect()
    }

    fn only(characters: &[CharacterId]) -> Characters {
        Characters::Only(characters.iter().copied().collect())
    }

    #[test]
    fn targets_scale_by_the_multiplier_and_expand_includes() {
        let all = set();

        assert_eq!(
            targets(1, &all),
            Ok(BTreeMap::from([(100, 10), (200, 50), (300, 5)]))
        );
        assert_eq!(
            targets(3, &all),
            Ok(BTreeMap::from([
                (100, 262),
                (200, 300),
                (300, 330),
                (400, 7)
            ]))
        );
    }

    #[test]
    fn a_loop_through_includes_is_reported_with_its_path() {
        let mut all = set();
        all.get_mut(&1).expect("kit").includes.push(include(3, 1));

        assert_eq!(targets(3, &all), Err(StockpileError::Loop(vec![3, 1, 3])));
    }

    #[test]
    fn an_include_of_an_unknown_stockpile_is_reported() {
        let mut all = set();
        all.get_mut(&3)
            .expect("weekly")
            .includes
            .push(include(9, 1));

        assert_eq!(targets(3, &all), Err(StockpileError::Missing(9)));
    }

    #[test]
    fn a_count_that_overflows_is_out_of_range() {
        let all = all(vec![stockpile(
            1,
            4_294_967_295,
            vec![target(100, u64::MAX)],
            vec![],
            vec![],
        )]);

        assert_eq!(targets(1, &all), Err(StockpileError::OutOfRange));
    }

    #[test]
    fn a_multiplier_of_zero_gives_zero_units_except_ignored_targets() {
        let all = all(vec![stockpile(
            1,
            0,
            vec![
                target(100, 10),
                Target {
                    type_id: 400,
                    units: 7,
                    ignore_multiplier: true,
                },
            ],
            vec![],
            vec![],
        )]);

        assert_eq!(targets(1, &all), Ok(BTreeMap::from([(100, 0), (400, 7)])));
    }

    #[test]
    fn a_type_listed_twice_is_summed() {
        let all = all(vec![stockpile(
            1,
            1,
            vec![target(100, 10), target(100, 5)],
            vec![],
            vec![],
        )]);

        assert_eq!(targets(1, &all), Ok(BTreeMap::from([(100, 15)])));
    }

    #[test]
    fn stock_counts_once_per_entry_through_the_filters_that_match_it() {
        let mut all = set();
        all.get_mut(&1).expect("kit").filters = vec![
            Filter {
                characters: Characters::All,
                place: Place::Location(JITA_4_4),
                sources: sources(&[Source::Assets, Source::SellOrders]),
            },
            Filter {
                characters: only(&[MAIN]),
                place: Place::System(JITA_SYSTEM),
                sources: sources(&[Source::Assets, Source::Jobs]),
            },
        ];
        let assets = [
            asset(MAIN, 100, JITA_4_4, 4, true),
            asset(ALT, 100, JITA_4_4, 3, true),
            asset(ALT, 100, STRUCTURE, 5, true),
            asset(MAIN, 100, JITA_4_4, 1, false),
            asset(MAIN, 200, JITA_4_4, 20, true),
        ];
        let stock = Stock {
            assets: &assets,
            sell_orders: &[held(MAIN, 100, JITA_4_4, 6)],
            buy_orders: &[held(MAIN, 100, JITA_4_4, 100)],
            jobs: &[held(MAIN, 100, STRUCTURE, 10)],
        };

        let lines = have(1, &all, &stock, &locate).expect("have");

        assert_eq!(
            lines,
            vec![
                Line {
                    type_id: 100,
                    target: 10,
                    have: Have {
                        assets: 7,
                        sell_orders: 6,
                        buy_orders: 0,
                        jobs: 10,
                    },
                    total: 23,
                    short: 0,
                    status: Status::Ok,
                },
                Line {
                    type_id: 200,
                    target: 50,
                    have: Have {
                        assets: 20,
                        ..Have::default()
                    },
                    total: 20,
                    short: 30,
                    status: Status::Part,
                },
                Line {
                    type_id: 300,
                    target: 5,
                    have: Have::default(),
                    total: 0,
                    short: 5,
                    status: Status::Short,
                },
            ]
        );
    }

    #[test]
    fn a_region_filter_takes_the_places_in_it() {
        let mut all = set();
        all.get_mut(&1).expect("kit").filters = vec![Filter {
            characters: Characters::All,
            place: Place::Region(THE_FORGE),
            sources: sources(&[Source::Assets]),
        }];
        let assets = [
            asset(MAIN, 100, JITA_4_4, 4, true),
            asset(ALT, 100, JITA_4_4, 3, true),
            asset(ALT, 100, STRUCTURE, 5, true),
            asset(MAIN, 100, JITA_4_4, 1, false),
        ];
        let stock = Stock {
            assets: &assets,
            sell_orders: &[],
            buy_orders: &[],
            jobs: &[],
        };

        let forge = have(1, &all, &stock, &locate).expect("have");
        assert_eq!(forge[0].have.assets, 12);

        all.get_mut(&1).expect("kit").filters[0].place = Place::Region(10_000_043);
        let elsewhere = have(1, &all, &stock, &locate).expect("have");
        assert_eq!(elsewhere[0].have.assets, 0);
    }

    #[test]
    fn a_place_the_locator_does_not_know_counts_only_for_everywhere() {
        let mut all = set();
        let unknown = 42;
        let assets = [asset(MAIN, 100, unknown, 9, true)];
        let stock = Stock {
            assets: &assets,
            sell_orders: &[],
            buy_orders: &[],
            jobs: &[],
        };
        let filter = |place| Filter {
            characters: Characters::All,
            place,
            sources: sources(&[Source::Assets]),
        };

        all.get_mut(&1).expect("kit").filters = vec![filter(Place::Region(THE_FORGE))];
        assert_eq!(have(1, &all, &stock, &locate).expect("have")[0].total, 0);

        all.get_mut(&1).expect("kit").filters = vec![filter(Place::Everywhere)];
        assert_eq!(have(1, &all, &stock, &locate).expect("have")[0].total, 9);
    }

    #[test]
    fn without_filters_every_line_is_short() {
        let all = set();
        let stock = Stock {
            assets: &[asset(MAIN, 100, JITA_4_4, 4, true)],
            sell_orders: &[held(MAIN, 100, JITA_4_4, 6)],
            buy_orders: &[],
            jobs: &[],
        };

        let lines = have(1, &all, &stock, &locate).expect("have");

        assert!(
            lines
                .iter()
                .all(|line| line.total == 0 && line.status == Status::Short)
        );
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn shortfall_prices_the_missing_units_and_names_the_unpriced_types() {
        let all = set();
        let stock = Stock {
            assets: &[],
            sell_orders: &[],
            buy_orders: &[],
            jobs: &[],
        };
        let lines = have(1, &all, &stock, &locate).expect("have");
        let only_200 = |type_id| (type_id == 200).then_some(Money::from_ten_thousandths(5_500_000));
        let priced = |type_id| match type_id {
            100 => Some(Money::from_ten_thousandths(797_500_000)),
            200 => Some(Money::from_ten_thousandths(5_500_000)),
            _ => None,
        };

        assert_eq!(
            shortfall(&lines, &only_200),
            Ok(Shortfall {
                value: Money::from_ten_thousandths(275_000_000),
                unpriced: vec![100, 300],
            })
        );
        assert_eq!(
            shortfall(&lines, &priced),
            Ok(Shortfall {
                value: Money::from_ten_thousandths(8_250_000_000),
                unpriced: vec![300],
            })
        );
    }

    #[test]
    fn a_shortfall_too_large_for_money_is_out_of_range() {
        let line = Line {
            type_id: 100,
            target: 2,
            have: Have::default(),
            total: 0,
            short: 2,
            status: Status::Short,
        };

        let result = shortfall(&[line], &|_| Some(Money::from_ten_thousandths(i64::MAX)));

        assert_eq!(result, Err(StockpileError::OutOfRange));
    }

    #[test]
    fn errors_say_what_failed() {
        assert_eq!(
            StockpileError::Missing(9).to_string(),
            "stockpile 9 does not exist"
        );
        assert_eq!(
            StockpileError::Loop(vec![3, 1, 3]).to_string(),
            "stockpiles include themselves: 3 → 1 → 3"
        );
        assert_eq!(
            StockpileError::OutOfRange.to_string(),
            "the counts add up to more than fit in a count"
        );
    }

    #[test]
    fn a_line_with_nothing_short_is_not_listed_as_unpriced() {
        let covered = Line {
            type_id: 100,
            target: 10,
            have: Have {
                assets: 10,
                ..Have::default()
            },
            total: 10,
            short: 0,
            status: Status::Ok,
        };
        let missing = Line {
            type_id: 200,
            target: 5,
            have: Have::default(),
            total: 0,
            short: 5,
            status: Status::Short,
        };

        let result = shortfall(&[covered, missing], &|_| None).expect("shortfall");

        assert_eq!(result.unpriced, vec![200]);
        assert_eq!(result.value, Money::ZERO);
    }
}

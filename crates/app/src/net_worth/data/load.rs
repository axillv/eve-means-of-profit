//! Reading the net worth history: a range of stored records, one valued point for each day, each
//! at the prices, rules and PLEX count of its own moment.

use std::collections::{BTreeMap, BTreeSet};

use mop_core::industry::TypeId;
use mop_core::net_worth::{
    CharacterKey, Part, Point, Snapshot, daily, leave_out, long_carried, points,
};
use mop_market::book::PriceRule;
use mop_market::fuzzwork::{FORGE, Side, SummaryRow};
use mop_market::plex::PLEX;
use mop_sde::query::Sde;
use mop_store::Store;
use mop_store::price_history::{MarketKind, PointPrices};
use mop_store::settings::{RuleTarget, RuleValue};

use super::pricer::{History, Pricer, TypeGroups};
use super::{ValuedPoint, value_point};

/// A character whose data is at least this old at a point is left out of it when asked.
const LEFT_OUT_AGE: i64 = 604_800;

/// The points of a date range, valued, and the point before the last at the last point's prices.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loaded {
    /// One valued point per day, oldest first.
    pub points: Vec<ValuedPoint>,
    /// The point before the last, valued at the last point's prices; `None` with fewer than two.
    pub previous_at_latest_prices: Option<ValuedPoint>,
    /// Why the prices could not be read; the points are then valued without any.
    pub error: Option<String>,
}

/// Reads the records from `from` to `to`, keeps the last point of each day, drops points before
/// `from`, and values each at its own moment's prices, rules and PLEX count.
///
/// A character in `left_out` is removed from a point only when its data there is at least
/// 604,800 seconds old, so a fresher day still counts it. A price read that fails leaves `error`
/// set and values every point with no copy, history, rules, groups or PLEX units.
pub fn load(
    store: &mut Store,
    sde: Option<&Sde>,
    from: i64,
    to: i64,
    shown: &BTreeSet<Part>,
    show_plex: bool,
    left_out: &BTreeSet<CharacterKey>,
) -> Result<Loaded, String> {
    let records = store
        .records(from, to)
        .map_err(|error| format!("Could not read net worth: {error}"))?;
    let built = points(&records);
    let mut days = daily(&built.points);
    days.retain(|point| point.at >= from);
    for point in &mut days {
        let carried: BTreeSet<CharacterKey> = long_carried(point, LEFT_OUT_AGE)
            .into_iter()
            .filter(|key| left_out.contains(key))
            .collect();
        if !carried.is_empty() {
            *point = leave_out(point, &carried);
        }
    }
    if days.is_empty() {
        return Ok(Loaded {
            points: Vec::new(),
            previous_at_latest_prices: None,
            error: None,
        });
    }

    let (inputs, error) = match PriceInputs::read(store, sde, &days, to) {
        Ok(inputs) => (inputs, None),
        Err(message) => (
            PriceInputs::empty(),
            Some(format!("Could not read the prices: {message}")),
        ),
    };

    let empty_rules = BTreeMap::new();
    let mut valued = Vec::with_capacity(days.len());
    for point in &days {
        let rules = inputs.rules.get(&point.at).unwrap_or(&empty_rules);
        let pricer = Pricer::new(
            point.at,
            inputs.copies.get(&point.at),
            &inputs.history,
            rules,
            inputs.app_wide,
            &inputs.groups,
        );
        valued.push(
            value_point(
                point,
                &pricer,
                inputs.plex_units(point.at),
                shown,
                show_plex,
            )
            .map_err(|error| format!("Could not value net worth: {error}"))?,
        );
    }

    let previous_at_latest_prices = match days.as_slice() {
        [.., previous, last] => {
            let rules = inputs.rules.get(&last.at).unwrap_or(&empty_rules);
            let pricer = Pricer::new(
                last.at,
                inputs.copies.get(&last.at),
                &inputs.history,
                rules,
                inputs.app_wide,
                &inputs.groups,
            );
            Some(
                value_point(
                    previous,
                    &pricer,
                    inputs.plex_units(previous.at),
                    shown,
                    show_plex,
                )
                .map_err(|error| format!("Could not value net worth: {error}"))?,
            )
        }
        _ => None,
    };

    Ok(Loaded {
        points: valued,
        previous_at_latest_prices,
        error,
    })
}

/// Everything the pricer needs, read once for the whole range.
struct PriceInputs {
    /// The price copies kept with the points, by the point's time.
    copies: BTreeMap<i64, PointPrices>,
    /// The price rules in force at each point, by the point's time.
    rules: BTreeMap<i64, BTreeMap<RuleTarget, RuleValue>>,
    /// The PLEX units of each point, summed over accounts, by the point's time.
    plex: BTreeMap<i64, u64>,
    /// The app-wide price for a type with no rule of its own.
    app_wide: PriceRule,
    /// The group and category of each type the game data holds.
    groups: TypeGroups,
    /// The Forge's daily readings, when a point needs them.
    history: History,
}

impl PriceInputs {
    /// Reads every price input, stopping at the first failure. A point with no copy or only PLEX
    /// rows makes the Forge's history needed; without a Forge market it is empty.
    fn read(
        store: &mut Store,
        sde: Option<&Sde>,
        points: &[Point],
        to: i64,
    ) -> Result<Self, String> {
        let Some(first) = points.first() else {
            return Ok(Self::empty());
        };
        let copies: BTreeMap<i64, PointPrices> = store
            .point_prices(first.at, to)
            .map_err(|error| error.to_string())?
            .into_iter()
            .map(|copy| (copy.at, copy))
            .collect();
        let mut rules = BTreeMap::new();
        let mut plex = BTreeMap::new();
        for point in points {
            rules.insert(
                point.at,
                store
                    .price_rules_at(point.at)
                    .map_err(|error| error.to_string())?,
            );
            let counts = store.plex_at(point.at).map_err(|error| error.to_string())?;
            let units = counts
                .values()
                .try_fold(0u64, |sum, count| sum.checked_add(*count))
                .ok_or_else(|| "the PLEX count is too large".to_owned())?;
            plex.insert(point.at, units);
        }
        let app_wide = store.value_price().map_err(|error| error.to_string())?;
        let types = priced_types(points);
        let groups = type_groups(sde, &types)?;
        let needs_history = points
            .iter()
            .any(|point| !has_market_copy(copies.get(&point.at)));
        let history = if needs_history {
            forge_history(store, &types)?
        } else {
            History::new()
        };
        Ok(Self {
            copies,
            rules,
            plex,
            app_wide,
            groups,
            history,
        })
    }

    /// No prices at all, for when reading them failed.
    fn empty() -> Self {
        Self {
            copies: BTreeMap::new(),
            rules: BTreeMap::new(),
            plex: BTreeMap::new(),
            app_wide: PriceRule::SellFivePercent,
            groups: TypeGroups::new(),
            history: History::new(),
        }
    }

    /// The point's PLEX units; 0 when the reading failed or the point had none.
    fn plex_units(&self, at: i64) -> u64 {
        self.plex.get(&at).copied().unwrap_or(0)
    }
}

/// Whether a copy prices types other than PLEX, as the pricer decides it.
fn has_market_copy(copy: Option<&PointPrices>) -> bool {
    copy.is_some_and(|copy| copy.rows.keys().any(|(type_id, _)| *type_id != PLEX))
}

/// The types of the assets and job outputs of every point, plus PLEX.
fn priced_types(points: &[Point]) -> BTreeSet<TypeId> {
    let mut types = BTreeSet::from([PLEX]);
    for point in points {
        for member in point.members.values() {
            if let Snapshot::Seen(holdings) = &member.snapshot {
                types.extend(holdings.assets.keys().copied());
                types.extend(holdings.jobs_output.keys().copied());
            }
        }
    }
    types
}

/// The group and category of each type the game data holds; an unknown type is absent.
fn type_groups(sde: Option<&Sde>, types: &BTreeSet<TypeId>) -> Result<TypeGroups, String> {
    let mut groups = TypeGroups::new();
    let Some(sde) = sde else {
        return Ok(groups);
    };
    for type_id in types {
        if let Some(item) = sde.item(*type_id).map_err(|error| error.to_string())? {
            groups.insert(*type_id, (item.group_id, item.category_id));
        }
    }
    Ok(groups)
}

/// The Forge's readings of `types`, both sides, from the Region market at the Forge's place.
/// The market's enabled flag is not read: a disabled market keeps its history.
fn forge_history(store: &Store, types: &BTreeSet<TypeId>) -> Result<History, String> {
    let markets = store.history_markets().map_err(|error| error.to_string())?;
    let Some(market) = markets
        .iter()
        .find(|market| market.kind == MarketKind::Region && market.place == u64::from(FORGE))
    else {
        return Ok(History::new());
    };
    let mut history = History::new();
    for type_id in types {
        for side in [Side::Sell, Side::Buy] {
            let readings: Vec<(i64, SummaryRow)> = store
                .series(market.id, *type_id, side)
                .map_err(|error| error.to_string())?;
            history.insert((*type_id, side), readings);
        }
    }
    Ok(history)
}

//! Net worth over time: what each character held at each refresh, valued at one price basis.
//! Pure logic, no UI and no storage: the caller supplies the records, the prices and the parts shown.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::industry::TypeId;
use crate::money::Money;

/// A character whose last data is older than this many seconds is drawn as stale (one day).
pub const STALE_AFTER: i64 = 86_400;

const SECONDS_PER_DAY: i64 = 86_400;

/// Who a record belongs to: a character id, or the name of a jEve owner that is not a number.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum CharacterKey {
    Id(u64),
    Name(String),
}

/// One part of net worth, in the order the chart lists them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Part {
    Wallet,
    Assets,
    SellOrders,
    BuyEscrow,
    IndustryJobs,
    ContractCollateral,
    Contracts,
    Implants,
}

impl Part {
    pub const ALL: [Part; 8] = [
        Part::Wallet,
        Part::Assets,
        Part::SellOrders,
        Part::BuyEscrow,
        Part::IndustryJobs,
        Part::ContractCollateral,
        Part::Contracts,
        Part::Implants,
    ];
}

/// The price of an item that counts. The user picks one and the whole history is re-priced with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PriceBasis {
    #[default]
    Midpoint,
    HighestBuy,
    LowestSell,
    Average,
}

/// The prices the app saved for one type on one day.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemPrices {
    pub lowest_sell: Option<Money>,
    pub highest_buy: Option<Money>,
    pub average: Option<Money>,
}

impl ItemPrices {
    /// The price at `basis`; `None` when the app does not know it.
    pub fn price(&self, basis: PriceBasis) -> Option<Money> {
        match basis {
            PriceBasis::Midpoint => {
                // Summed in i128 so two large prices cannot overflow; the half of two i64 values fits i64.
                let sum = i128::from(self.lowest_sell?.ten_thousandths())
                    + i128::from(self.highest_buy?.ten_thousandths());
                let half = sum / 2;
                // Round half away from zero: any remainder moves the half one step further from zero.
                let rounded = if sum % 2 == 0 {
                    half
                } else {
                    half + sum.signum()
                };
                i64::try_from(rounded).ok().map(Money::from_ten_thousandths)
            }
            PriceBasis::HighestBuy => self.highest_buy,
            PriceBasis::LowestSell => self.lowest_sell,
            PriceBasis::Average => self.average,
        }
    }
}

/// What the app saw of one character at one refresh.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Holdings {
    pub wallet: Option<Money>,
    /// Counted units by type. The caller decides what counts as assets.
    pub assets: BTreeMap<TypeId, u64>,
    /// Sum of price times remaining quantity of the open sell orders.
    pub sell_orders: Option<Money>,
    /// Sum of the escrow of the open buy orders.
    pub buy_escrow: Option<Money>,
    /// Units the character's active, paused or ready manufacturing and reaction jobs will deliver.
    pub jobs_output: BTreeMap<TypeId, u64>,
}

/// One record of a character at one time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Snapshot {
    /// Seen by the app; priced on demand.
    Seen(Holdings),
    /// Fixed values from jEve's own pricing; never re-priced.
    Imported(BTreeMap<Part, Money>),
    /// The character was removed from the app at that time.
    Removed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub character: CharacterKey,
    /// Unix seconds.
    pub at: i64,
    pub snapshot: Snapshot,
}

/// The points of the chart, and how many imported records were dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Points {
    pub points: Vec<Point>,
    /// Imported records at or after the character's first seen record; the app's own data wins.
    pub ignored_imported: usize,
}

/// Every character's latest data at one time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Point {
    pub at: i64,
    pub members: BTreeMap<CharacterKey, Member>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    /// When the data used for this point was recorded.
    pub record_at: i64,
    pub snapshot: Snapshot,
}

impl Point {
    /// The characters whose data is older than `STALE_AFTER`, with its age in seconds.
    pub fn stale(&self) -> Vec<(CharacterKey, i64)> {
        self.members
            .iter()
            .filter_map(|(key, member)| {
                let age = self.at.checked_sub(member.record_at)?;
                (age > STALE_AFTER).then(|| (key.clone(), age))
            })
            .collect()
    }
}

/// Builds the chart points from records in any order.
pub fn points(records: &[Record]) -> Points {
    let mut first_seen: BTreeMap<&CharacterKey, i64> = BTreeMap::new();
    for record in records {
        if matches!(record.snapshot, Snapshot::Seen(_)) {
            first_seen
                .entry(&record.character)
                .and_modify(|at| *at = (*at).min(record.at))
                .or_insert(record.at);
        }
    }

    let mut ignored_imported = 0;
    let mut kept: Vec<&Record> = Vec::new();
    for record in records {
        let superseded = matches!(record.snapshot, Snapshot::Imported(_))
            && first_seen
                .get(&record.character)
                .is_some_and(|&first| record.at >= first);
        if superseded {
            ignored_imported += 1;
        } else {
            kept.push(record);
        }
    }

    // Stable sort: of two records at the same time, the later one in the input is applied last and wins.
    kept.sort_by_key(|record| record.at);
    let times: BTreeSet<i64> = kept.iter().map(|record| record.at).collect();

    let mut latest: BTreeMap<&CharacterKey, &Record> = BTreeMap::new();
    let mut next = 0;
    let mut out = Vec::with_capacity(times.len());
    for at in times {
        while let Some(&record) = kept.get(next).filter(|record| record.at <= at) {
            latest.insert(&record.character, record);
            next += 1;
        }
        let members = latest
            .iter()
            .filter(|&(_, &record)| !matches!(record.snapshot, Snapshot::Removed))
            .map(|(&key, &record)| {
                let member = Member {
                    record_at: record.at,
                    snapshot: record.snapshot.clone(),
                };
                (key.clone(), member)
            })
            .collect();
        out.push(Point { at, members });
    }

    Points {
        points: out,
        ignored_imported,
    }
}

/// One part of a character's value, and the units of each type that had no price.
struct Valued {
    parts: BTreeMap<Part, Money>,
    unpriced: BTreeMap<Part, BTreeMap<TypeId, u64>>,
}

/// The value of one point: every part for every character, and the sum over the shown parts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PointValue {
    pub at: i64,
    /// All eight parts, shown or not; 0 where nothing.
    pub by_part: BTreeMap<Part, Money>,
    /// Sum over the shown parts of each character.
    pub by_character: BTreeMap<CharacterKey, Money>,
    /// Sum over the shown parts of all characters.
    pub total: Money,
    /// Units of the shown parts that had no price at the basis, summed over characters. Left out of the value.
    pub unpriced: BTreeMap<TypeId, u64>,
    pub stale: Vec<(CharacterKey, i64)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetWorthError {
    Overflow,
}

impl fmt::Display for NetWorthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NetWorthError::Overflow => write!(f, "net worth is too large to add up"),
        }
    }
}

impl std::error::Error for NetWorthError {}

/// Values one point at `basis`. `prices` gives the prices saved for a type on or before a UTC day;
/// the day is the point's own.
pub fn value(
    point: &Point,
    basis: PriceBasis,
    prices: &dyn Fn(TypeId, i64) -> Option<ItemPrices>,
    shown: &BTreeSet<Part>,
) -> Result<PointValue, NetWorthError> {
    let day = point.at.div_euclid(SECONDS_PER_DAY);
    let mut by_part: BTreeMap<Part, Money> =
        Part::ALL.iter().map(|&part| (part, Money::ZERO)).collect();
    let mut by_character = BTreeMap::new();
    let mut total = Money::ZERO;
    let mut unpriced: BTreeMap<TypeId, u64> = BTreeMap::new();

    for (key, member) in &point.members {
        let valued = value_member(&member.snapshot, basis, prices, day)?;
        let mut character = Money::ZERO;
        for (&part, &amount) in &valued.parts {
            let sum = by_part.entry(part).or_insert(Money::ZERO);
            *sum = sum.checked_add(amount).ok_or(NetWorthError::Overflow)?;
            if shown.contains(&part) {
                character = character
                    .checked_add(amount)
                    .ok_or(NetWorthError::Overflow)?;
                total = total.checked_add(amount).ok_or(NetWorthError::Overflow)?;
            }
        }
        for (part, units) in &valued.unpriced {
            if shown.contains(part) {
                for (&type_id, &count) in units {
                    let sum = unpriced.entry(type_id).or_insert(0);
                    *sum = sum.checked_add(count).ok_or(NetWorthError::Overflow)?;
                }
            }
        }
        by_character.insert(key.clone(), character);
    }

    Ok(PointValue {
        at: point.at,
        by_part,
        by_character,
        total,
        unpriced,
        stale: point.stale(),
    })
}

fn value_member(
    snapshot: &Snapshot,
    basis: PriceBasis,
    prices: &dyn Fn(TypeId, i64) -> Option<ItemPrices>,
    day: i64,
) -> Result<Valued, NetWorthError> {
    let mut parts: BTreeMap<Part, Money> =
        Part::ALL.iter().map(|&part| (part, Money::ZERO)).collect();
    let mut unpriced = BTreeMap::new();
    match snapshot {
        Snapshot::Seen(holdings) => {
            parts.insert(Part::Wallet, holdings.wallet.unwrap_or(Money::ZERO));
            parts.insert(
                Part::SellOrders,
                holdings.sell_orders.unwrap_or(Money::ZERO),
            );
            parts.insert(Part::BuyEscrow, holdings.buy_escrow.unwrap_or(Money::ZERO));
            let (assets, assets_unpriced) = priced_units(&holdings.assets, basis, prices, day)?;
            let (jobs, jobs_unpriced) = priced_units(&holdings.jobs_output, basis, prices, day)?;
            parts.insert(Part::Assets, assets);
            parts.insert(Part::IndustryJobs, jobs);
            unpriced.insert(Part::Assets, assets_unpriced);
            unpriced.insert(Part::IndustryJobs, jobs_unpriced);
        }
        Snapshot::Imported(amounts) => {
            for (&part, &amount) in amounts {
                parts.insert(part, amount);
            }
        }
        // Removed characters are left out of points, so there is nothing to value.
        Snapshot::Removed => {}
    }
    Ok(Valued { parts, unpriced })
}

/// The value of counted units at `basis`, and the units of each type that had no price.
fn priced_units(
    units: &BTreeMap<TypeId, u64>,
    basis: PriceBasis,
    prices: &dyn Fn(TypeId, i64) -> Option<ItemPrices>,
    day: i64,
) -> Result<(Money, BTreeMap<TypeId, u64>), NetWorthError> {
    let mut total = Money::ZERO;
    let mut unpriced = BTreeMap::new();
    for (&type_id, &count) in units {
        match prices(type_id, day).and_then(|item| item.price(basis)) {
            Some(price) => {
                let value = price
                    .checked_mul_qty(count)
                    .ok_or(NetWorthError::Overflow)?;
                total = total.checked_add(value).ok_or(NetWorthError::Overflow)?;
            }
            None if count > 0 => {
                unpriced.insert(type_id, count);
            }
            None => {}
        }
    }
    Ok((total, unpriced))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn isk(text: &str) -> Money {
        Money::parse_isk(text).unwrap()
    }

    fn a() -> CharacterKey {
        CharacterKey::Id(2112625428)
    }

    fn b() -> CharacterKey {
        CharacterKey::Id(95465499)
    }

    fn all_parts() -> BTreeSet<Part> {
        Part::ALL.iter().copied().collect()
    }

    fn seen(wallet: &str, assets: &[(TypeId, u64)]) -> Snapshot {
        Snapshot::Seen(Holdings {
            wallet: Some(isk(wallet)),
            assets: assets.iter().copied().collect(),
            sell_orders: None,
            buy_escrow: None,
            jobs_output: BTreeMap::new(),
        })
    }

    fn rec(character: CharacterKey, at: i64, snapshot: Snapshot) -> Record {
        Record {
            character,
            at,
            snapshot,
        }
    }

    /// Tritanium (34) and Morphite (11399) on day 0 only.
    fn market(type_id: TypeId, day: i64) -> Option<ItemPrices> {
        match (type_id, day) {
            (34, 0) => Some(ItemPrices {
                lowest_sell: Some(isk("4.00")),
                highest_buy: Some(isk("3.00")),
                average: Some(isk("3.50")),
            }),
            (11399, 0) => Some(ItemPrices {
                lowest_sell: Some(isk("17,860.00")),
                highest_buy: Some(isk("17,300.00")),
                average: Some(isk("17,322.24")),
            }),
            _ => None,
        }
    }

    fn point_at(records: &[Record], at: i64) -> Point {
        points(records)
            .points
            .into_iter()
            .find(|point| point.at == at)
            .unwrap()
    }

    fn worth(records: &[Record], at: i64, basis: PriceBasis) -> PointValue {
        let point = point_at(records, at);
        value(&point, basis, &market, &all_parts()).unwrap()
    }

    /// Carry-over: A at 1,000 and 3,000, B at 2,000.
    fn carry_over() -> Vec<Record> {
        vec![
            rec(a(), 1_000, seen("100.00", &[(34, 1_000)])),
            rec(b(), 2_000, seen("50.00", &[])),
            rec(a(), 3_000, seen("120.00", &[(34, 1_000)])),
        ]
    }

    #[test]
    fn price_at_each_basis() {
        let prices = ItemPrices {
            lowest_sell: Some(isk("17,860.00")),
            highest_buy: Some(isk("17,300.00")),
            average: Some(isk("17,322.24")),
        };
        assert_eq!(prices.price(PriceBasis::Midpoint), Some(isk("17,580.00")));
        assert_eq!(prices.price(PriceBasis::HighestBuy), Some(isk("17,300.00")));
        assert_eq!(prices.price(PriceBasis::LowestSell), Some(isk("17,860.00")));
        assert_eq!(prices.price(PriceBasis::Average), Some(isk("17,322.24")));

        let thousand = |money: Option<Money>| money.unwrap().checked_mul_qty(1_000);
        assert_eq!(
            thousand(prices.price(PriceBasis::Midpoint)),
            Some(isk("17,580,000.00"))
        );
        assert_eq!(
            thousand(prices.price(PriceBasis::HighestBuy)),
            Some(isk("17,300,000.00"))
        );
        assert_eq!(
            thousand(prices.price(PriceBasis::LowestSell)),
            Some(isk("17,860,000.00"))
        );
        assert_eq!(
            thousand(prices.price(PriceBasis::Average)),
            Some(isk("17,322,240.00"))
        );
    }

    #[test]
    fn midpoint_rounds_half_away_from_zero() {
        let prices = ItemPrices {
            lowest_sell: Some(isk("3.99")),
            highest_buy: Some(isk("3.9801")),
            average: None,
        };
        assert_eq!(prices.price(PriceBasis::Midpoint), Some(isk("3.9851")));
    }

    #[test]
    fn midpoint_of_huge_prices_does_not_overflow() {
        let max = Money::from_ten_thousandths(i64::MAX);
        let prices = ItemPrices {
            lowest_sell: Some(max),
            highest_buy: Some(max),
            average: None,
        };
        assert_eq!(prices.price(PriceBasis::Midpoint), Some(max));
    }

    #[test]
    fn midpoint_needs_both_sides() {
        let prices = ItemPrices {
            lowest_sell: Some(isk("3.99")),
            highest_buy: None,
            average: None,
        };
        assert_eq!(prices.price(PriceBasis::Midpoint), None);
        assert_eq!(prices.price(PriceBasis::LowestSell), Some(isk("3.99")));
    }

    #[test]
    fn characters_not_refreshed_keep_their_last_data() {
        let records = carry_over();
        let result = points(&records);
        let times: Vec<i64> = result.points.iter().map(|point| point.at).collect();
        assert_eq!(times, vec![1_000, 2_000, 3_000]);

        assert_eq!(
            worth(&records, 1_000, PriceBasis::Midpoint).total,
            isk("3,600.00")
        );
        assert_eq!(
            worth(&records, 2_000, PriceBasis::Midpoint).total,
            isk("3,650.00")
        );
        let at_3000 = worth(&records, 3_000, PriceBasis::Midpoint);
        assert_eq!(at_3000.total, isk("3,670.00"));
        assert_eq!(at_3000.by_character[&a()], isk("3,620.00"));
        assert_eq!(at_3000.by_character[&b()], isk("50.00"));
        assert_eq!(
            worth(&records, 3_000, PriceBasis::HighestBuy).total,
            isk("3,170.00")
        );
        assert!(at_3000.stale.is_empty());

        let point = point_at(&records, 3_000);
        assert_eq!(point.members[&a()].record_at, 3_000);
        assert_eq!(point.members[&b()].record_at, 2_000);
    }

    #[test]
    fn stale_after_one_day_and_not_at_exactly_one_day() {
        let mut records = carry_over();
        records.retain(|record| record.at != 3_000);
        records.push(rec(a(), 88_401, seen("1.00", &[])));
        let point = point_at(&records, 88_401);
        assert_eq!(point.stale(), vec![(b(), 86_401)]);

        let mut records = carry_over();
        records.retain(|record| record.at != 3_000);
        records.push(rec(a(), 88_400, seen("1.00", &[])));
        assert!(point_at(&records, 88_400).stale().is_empty());
    }

    #[test]
    fn prices_are_asked_for_the_point_day() {
        let records = vec![rec(a(), 200_000, seen("0.00", &[(34, 1_000)]))];
        let only_day_two = |type_id: TypeId, day: i64| -> Option<ItemPrices> {
            assert_eq!((type_id, day), (34, 2));
            None
        };
        let point = point_at(&records, 200_000);
        let value = value(&point, PriceBasis::Midpoint, &only_day_two, &all_parts()).unwrap();
        assert_eq!(value.by_part[&Part::Assets], Money::ZERO);
        assert_eq!(value.unpriced, BTreeMap::from([(34, 1_000)]));
    }

    #[test]
    fn hidden_parts_leave_the_total_but_keep_their_value() {
        let records = carry_over();
        let point = point_at(&records, 3_000);
        let shown: BTreeSet<Part> = Part::ALL
            .iter()
            .copied()
            .filter(|&part| part != Part::Wallet)
            .collect();
        let value = value(&point, PriceBasis::Midpoint, &market, &shown).unwrap();
        assert_eq!(value.total, isk("3,500.00"));
        assert_eq!(value.by_part[&Part::Wallet], isk("170.00"));
        assert_eq!(value.by_character[&a()], isk("3,500.00"));
        assert_eq!(value.by_character[&b()], Money::ZERO);
    }

    #[test]
    fn hidden_parts_are_not_counted_as_unpriced() {
        let records = vec![rec(a(), 200_000, seen("0.00", &[(34, 1_000)]))];
        let point = point_at(&records, 200_000);
        let shown: BTreeSet<Part> = Part::ALL
            .iter()
            .copied()
            .filter(|&part| part != Part::Assets)
            .collect();
        let value = value(&point, PriceBasis::Midpoint, &|_, _| None, &shown).unwrap();
        assert!(value.unpriced.is_empty());
    }

    #[test]
    fn removed_characters_leave_the_points_until_seen_again() {
        let records = vec![
            rec(a(), 1_000, seen("100.00", &[(34, 1_000)])),
            rec(b(), 2_000, seen("50.00", &[])),
            rec(a(), 3_000, seen("120.00", &[(34, 1_000)])),
            rec(b(), 2_500, Snapshot::Removed),
            rec(b(), 4_000, seen("70.00", &[])),
        ];
        let at_2500 = worth(&records, 2_500, PriceBasis::Midpoint);
        assert_eq!(at_2500.total, isk("3,600.00"));
        assert_eq!(at_2500.by_character.len(), 1);
        assert_eq!(
            worth(&records, 3_000, PriceBasis::Midpoint).total,
            isk("3,620.00")
        );
        assert_eq!(
            worth(&records, 4_000, PriceBasis::Midpoint).total,
            isk("3,690.00")
        );
    }

    #[test]
    fn imported_history_is_fixed_until_the_app_sees_the_character() {
        let imported = Snapshot::Imported(BTreeMap::from([
            (Part::Wallet, isk("90.00")),
            (Part::Assets, isk("3,000.00")),
            (Part::IndustryJobs, isk("10.00")),
            (Part::ContractCollateral, isk("433.00")),
            (Part::Implants, isk("7.00")),
        ]));
        let mut records = carry_over();
        records.push(rec(a(), 500, imported.clone()));
        for basis in [
            PriceBasis::Midpoint,
            PriceBasis::HighestBuy,
            PriceBasis::LowestSell,
            PriceBasis::Average,
        ] {
            assert_eq!(worth(&records, 500, basis).total, isk("3,540.00"));
        }
        let point = point_at(&records, 500);
        let shown: BTreeSet<Part> = Part::ALL
            .iter()
            .copied()
            .filter(|&part| part != Part::ContractCollateral)
            .collect();
        assert_eq!(
            value(&point, PriceBasis::Midpoint, &market, &shown)
                .unwrap()
                .total,
            isk("3,107.00")
        );

        records.push(rec(a(), 1_500, imported.clone()));
        let result = points(&records);
        assert_eq!(result.ignored_imported, 1);
        assert!(result.points.iter().all(|point| point.at != 1_500));

        records.push(rec(CharacterKey::Name("TEST-NAME".into()), 1_500, imported));
        let result = points(&records);
        assert_eq!(result.ignored_imported, 1);
        let at_1500 = result
            .points
            .iter()
            .find(|point| point.at == 1_500)
            .unwrap();
        assert!(
            at_1500
                .members
                .contains_key(&CharacterKey::Name("TEST-NAME".into()))
        );
        assert!(
            result
                .points
                .iter()
                .filter(|point| point.at >= 1_500)
                .all(|point| {
                    point
                        .members
                        .contains_key(&CharacterKey::Name("TEST-NAME".into()))
                })
        );
    }

    #[test]
    fn sell_orders_escrow_and_jobs_are_valued() {
        let records = vec![rec(
            b(),
            5_000,
            Snapshot::Seen(Holdings {
                wallet: None,
                assets: BTreeMap::new(),
                sell_orders: Some(isk("200.00")),
                buy_escrow: Some(isk("30.00")),
                jobs_output: BTreeMap::from([(11399, 2)]),
            }),
        )];
        let value = worth(&records, 5_000, PriceBasis::Midpoint);
        assert_eq!(value.by_part[&Part::SellOrders], isk("200.00"));
        assert_eq!(value.by_part[&Part::BuyEscrow], isk("30.00"));
        assert_eq!(value.by_part[&Part::IndustryJobs], isk("35,160.00"));
    }

    #[test]
    fn last_record_at_the_same_time_wins() {
        let records = vec![
            rec(a(), 1_000, seen("100.00", &[])),
            rec(a(), 1_000, seen("110.00", &[])),
        ];
        assert_eq!(
            worth(&records, 1_000, PriceBasis::Midpoint).total,
            isk("110.00")
        );
    }

    #[test]
    fn totals_that_do_not_fit_are_an_error() {
        let max = Money::from_ten_thousandths(i64::MAX);
        let records = vec![
            rec(
                a(),
                1_000,
                Snapshot::Seen(Holdings {
                    wallet: Some(max),
                    assets: BTreeMap::new(),
                    sell_orders: None,
                    buy_escrow: None,
                    jobs_output: BTreeMap::new(),
                }),
            ),
            rec(
                b(),
                1_000,
                Snapshot::Seen(Holdings {
                    wallet: Some(max),
                    assets: BTreeMap::new(),
                    sell_orders: None,
                    buy_escrow: None,
                    jobs_output: BTreeMap::new(),
                }),
            ),
        ];
        let point = point_at(&records, 1_000);
        assert_eq!(
            value(&point, PriceBasis::Midpoint, &market, &all_parts()),
            Err(NetWorthError::Overflow)
        );
    }

    #[test]
    fn zero_units_without_a_price_are_not_counted_as_unpriced() {
        let records = vec![rec(a(), 200_000, seen("0.00", &[(34, 0)]))];
        let point = point_at(&records, 200_000);
        let value = value(&point, PriceBasis::Midpoint, &|_, _| None, &all_parts()).unwrap();
        assert!(value.unpriced.is_empty());
    }

    #[test]
    fn overflow_message_says_what_failed() {
        assert_eq!(
            NetWorthError::Overflow.to_string(),
            "net worth is too large to add up"
        );
    }

    #[test]
    fn negative_wallet_is_summed_as_given() {
        let records = vec![
            rec(a(), 1_000, seen("-5.00", &[])),
            rec(b(), 1_000, seen("100.00", &[])),
        ];
        assert_eq!(
            worth(&records, 1_000, PriceBasis::Midpoint).total,
            isk("95.00")
        );
    }
}

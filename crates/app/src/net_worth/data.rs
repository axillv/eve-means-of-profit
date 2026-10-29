pub mod pricer;

use std::collections::{BTreeMap, BTreeSet};

use mop_core::industry::TypeId;
use mop_core::money::Money;
use mop_core::net_worth::{
    CharacterKey, Member, NetWorthError, Part, Point, PointValue, Snapshot, value_with,
};
use mop_market::plex::PLEX;

use self::pricer::{PriceSource, Pricer};

/// One net worth point valued at the prices of its own moment, with the PLEX vault of that day
/// added at that moment's PLEX price.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValuedPoint {
    /// The point's parts and characters, priced with the pricer.
    pub value: PointValue,
    /// The typed PLEX count of the point's day, as given.
    pub plex_units: u64,
    /// The vault's value: the PLEX price times the units, or 0 when it is not shown or unpriced.
    pub plex_value: Money,
    /// The value of the shown parts plus the vault.
    pub total: Money,
    /// The unpriced units of the shown parts, plus the vault's units when they have no price.
    pub unpriced: BTreeMap<TypeId, u64>,
    /// Where the pricer's prices came from, read after the valuing.
    pub source: PriceSource,
    /// Each character's own values, shown or not.
    pub members: BTreeMap<CharacterKey, MemberValue>,
}

/// What one character was worth at one point.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberValue {
    /// All eight parts, 0 where nothing.
    pub parts: BTreeMap<Part, Money>,
    /// When the data used for this point was recorded.
    pub record_at: i64,
    /// Whether the values are jEve's own, never re-priced.
    pub imported: bool,
}

/// Values one point with `pricer`, whose day is the point's own, and adds the PLEX vault.
///
/// `plex_units` is the typed PLEX count of the point's day, summed over accounts by the caller.
/// With `show_plex` true and units to add, the vault is priced like any type: no price leaves its
/// units in `unpriced` instead. PLEX items in the point's own assets are valued apart from the
/// vault, which is extra.
pub fn value_point(
    point: &Point,
    pricer: &Pricer,
    plex_units: u64,
    shown: &BTreeSet<Part>,
    show_plex: bool,
) -> Result<ValuedPoint, NetWorthError> {
    let price = |type_id: TypeId, _day: i64| pricer.price(type_id);
    let value = value_with(point, &price, shown)?;

    let mut plex_value = Money::ZERO;
    let mut unpriced = value.unpriced.clone();
    if show_plex && plex_units > 0 {
        match pricer.price(PLEX) {
            Some(price) => {
                plex_value = price
                    .checked_mul_qty(plex_units)
                    .ok_or(NetWorthError::Overflow)?;
            }
            None => {
                let units = unpriced.entry(PLEX).or_insert(0);
                *units = units
                    .checked_add(plex_units)
                    .ok_or(NetWorthError::Overflow)?;
            }
        }
    }
    let total = value
        .total
        .checked_add(plex_value)
        .ok_or(NetWorthError::Overflow)?;

    let members = member_values(point, &price)?;
    // Read after every price, so a history source names the newest day the valuing used.
    let source = pricer.source();

    Ok(ValuedPoint {
        value,
        plex_units,
        plex_value,
        total,
        unpriced,
        source,
        members,
    })
}

/// Each member of the point valued on its own, with all eight parts shown.
fn member_values(
    point: &Point,
    price: &dyn Fn(TypeId, i64) -> Option<Money>,
) -> Result<BTreeMap<CharacterKey, MemberValue>, NetWorthError> {
    let all_shown: BTreeSet<Part> = Part::ALL.iter().copied().collect();
    let mut values = BTreeMap::new();
    for (key, member) in &point.members {
        let one = Point {
            at: point.at,
            members: BTreeMap::from([(key.clone(), member.clone())]),
        };
        let valued = value_with(&one, price, &all_shown)?;
        values.insert(
            key.clone(),
            MemberValue {
                parts: valued.by_part,
                record_at: member.record_at,
                imported: is_imported(member),
            },
        );
    }
    Ok(values)
}

/// Whether the member's values are jEve's own.
fn is_imported(member: &Member) -> bool {
    matches!(member.snapshot, Snapshot::Imported(_))
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use mop_core::industry::TypeId;
    use mop_core::money::Money;
    use mop_core::net_worth::{
        CharacterKey, Holdings, Member, NetWorthError, Part, Point, Snapshot,
    };
    use mop_market::book::PriceRule;
    use mop_market::fuzzwork::{Side, SummaryRow};
    use mop_market::plex::PLEX;
    use mop_store::price_history::PointPrices;
    use mop_store::settings::{RuleTarget, RuleValue};

    use super::pricer::{History, PriceSource, Pricer, TypeGroups};
    use super::{ValuedPoint, value_point};

    /// The point's moment, and the moment the copied market prices were published.
    const NOW: i64 = 1_000_000_000;
    const PUBLISHED_AT: i64 = 999_999_000;
    /// The day of `NOW`, the newest history reading used by the tests.
    const NOW_DAY: i64 = 11_574;
    /// PAU.
    const PAU: TypeId = 12_612;
    /// The imported character's record time.
    const IMPORTED_AT: i64 = 1_552_492_124;

    fn isk(text: &str) -> Money {
        Money::parse_isk(text).expect("test price")
    }

    fn all_parts() -> BTreeSet<Part> {
        Part::ALL.iter().copied().collect()
    }

    fn row(five_percent: &str) -> SummaryRow {
        SummaryRow {
            min: isk(five_percent),
            max: isk(five_percent),
            weighted_average: isk(five_percent),
            median: isk(five_percent),
            five_percent: isk(five_percent),
            volume: 1_000,
            orders: 7,
        }
    }

    /// The market copy of the point: PAU at 61,200.00, and PLEX's row when asked for.
    fn copy(with_plex: bool) -> PointPrices {
        let mut rows = BTreeMap::from([((PAU, Side::Sell), row("61,200.00"))]);
        if with_plex {
            rows.insert((PLEX, Side::Sell), row("5,200,000.00"));
        }
        PointPrices {
            at: NOW,
            published_at: Some(PUBLISHED_AT),
            rows,
        }
    }

    fn main_key() -> CharacterKey {
        CharacterKey::Id(2_112_000_001)
    }

    fn main_snapshot() -> Snapshot {
        Snapshot::Seen(Holdings {
            wallet: Some(isk("1,234,567.89")),
            assets: BTreeMap::from([(PAU, 160)]),
            sell_orders: Some(isk("6,120,000.00")),
            buy_escrow: Some(isk("550,100.00")),
            jobs_output: BTreeMap::from([(PAU, 10)]),
        })
    }

    fn point_with(snapshot: Snapshot) -> Point {
        Point {
            at: NOW,
            members: BTreeMap::from([(
                main_key(),
                Member {
                    record_at: NOW,
                    snapshot,
                },
            )]),
        }
    }

    fn main_point() -> Point {
        point_with(main_snapshot())
    }

    /// Values the point from a copy priced under the app-wide sell rule, with no other rules.
    fn price_point(
        point: &Point,
        copy: &PointPrices,
        plex_units: u64,
        shown: &BTreeSet<Part>,
        show_plex: bool,
    ) -> Result<ValuedPoint, NetWorthError> {
        let history = History::new();
        let rules = BTreeMap::new();
        let groups = TypeGroups::new();
        let pricer = Pricer::new(
            point.at,
            Some(copy),
            &history,
            &rules,
            PriceRule::SellFivePercent,
            &groups,
        );
        value_point(point, &pricer, plex_units, shown, show_plex)
    }

    #[test]
    fn the_vault_is_priced_at_the_points_moment() {
        let point = main_point();
        let copy = copy(true);
        let all = all_parts();
        let valued = price_point(&point, &copy, 500, &all, true).expect("value");

        assert_eq!(valued.value.total, isk("18,308,667.89"));
        assert_eq!(valued.plex_units, 500);
        assert_eq!(valued.plex_value, isk("2,600,000,000.00"));
        assert_eq!(valued.total, isk("2,618,308,667.89"));
        assert!(valued.unpriced.is_empty());
        assert_eq!(
            valued.source,
            PriceSource::Copy {
                published_at: Some(PUBLISHED_AT)
            }
        );

        // Sell orders unticked: the part leaves both totals, the vault stays.
        let shown: BTreeSet<Part> = all
            .iter()
            .copied()
            .filter(|&part| part != Part::SellOrders)
            .collect();
        let valued = price_point(&point, &copy, 500, &shown, true).expect("value");
        assert_eq!(valued.value.total, isk("12,188,667.89"));
        assert_eq!(valued.total, isk("2,612,188,667.89"));
        assert_eq!(valued.value.by_part[&Part::SellOrders], isk("6,120,000.00"));
    }

    #[test]
    fn the_vault_is_optional_and_no_units_add_nothing() {
        let point = main_point();
        let copy = copy(true);
        let all = all_parts();

        let hidden = price_point(&point, &copy, 500, &all, false).expect("value");
        assert_eq!(hidden.plex_units, 500);
        assert_eq!(hidden.plex_value, Money::ZERO);
        assert_eq!(hidden.total, isk("18,308,667.89"));
        assert!(hidden.unpriced.is_empty());

        for show_plex in [false, true] {
            let none = price_point(&point, &copy, 0, &all, show_plex).expect("value");
            assert_eq!(none.plex_units, 0);
            assert_eq!(none.plex_value, Money::ZERO);
            assert_eq!(none.total, isk("18,308,667.89"));
            assert!(!none.unpriced.contains_key(&PLEX));
        }
    }

    #[test]
    fn an_unpriced_vault_is_named_with_its_units() {
        let point = main_point();
        let all = all_parts();
        let unpriced_copy = copy(false);

        let valued = price_point(&point, &unpriced_copy, 500, &all, true).expect("value");
        assert_eq!(valued.plex_value, Money::ZERO);
        assert_eq!(valued.total, isk("18,308,667.89"));
        assert_eq!(valued.unpriced, BTreeMap::from([(PLEX, 500)]));

        // Two PLEX items in the assets join the vault's units under the same type.
        let with_plex_assets = point_with(Snapshot::Seen(Holdings {
            wallet: Some(isk("1,234,567.89")),
            assets: BTreeMap::from([(PAU, 160), (PLEX, 2)]),
            sell_orders: Some(isk("6,120,000.00")),
            buy_escrow: Some(isk("550,100.00")),
            jobs_output: BTreeMap::from([(PAU, 10)]),
        }));
        let valued =
            price_point(&with_plex_assets, &unpriced_copy, 500, &all, true).expect("value");
        assert_eq!(valued.unpriced, BTreeMap::from([(PLEX, 502)]));

        // With PLEX's row back the asset items are priced like any other, and the vault too.
        let valued = price_point(&with_plex_assets, &copy(true), 500, &all, true).expect("value");
        assert!(valued.unpriced.is_empty());
        assert_eq!(valued.plex_value, isk("2,600,000,000.00"));
        assert_eq!(valued.value.by_part[&Part::Assets], isk("20,192,000.00"));
        assert_eq!(
            valued.total,
            valued
                .value
                .total
                .checked_add(isk("2,600,000,000.00"))
                .expect("sum")
        );
    }

    #[test]
    fn every_member_is_valued_on_its_own() {
        let mut point = main_point();
        let copy = copy(true);
        let all = all_parts();
        let valued = price_point(&point, &copy, 500, &all, true).expect("value");

        let main = &valued.members[&main_key()];
        assert_eq!(main.parts.len(), 8);
        assert_eq!(main.parts[&Part::Wallet], isk("1,234,567.89"));
        assert_eq!(main.parts[&Part::Assets], isk("9,792,000.00"));
        assert_eq!(main.parts[&Part::SellOrders], isk("6,120,000.00"));
        assert_eq!(main.parts[&Part::BuyEscrow], isk("550,100.00"));
        assert_eq!(main.parts[&Part::IndustryJobs], isk("612,000.00"));
        assert_eq!(main.parts[&Part::ContractCollateral], Money::ZERO);
        assert_eq!(main.parts[&Part::Contracts], Money::ZERO);
        assert_eq!(main.parts[&Part::Implants], Money::ZERO);
        assert_eq!(main.record_at, NOW);
        assert!(!main.imported);

        let name = CharacterKey::Name("TEST-NAME".into());
        point.members.insert(
            name.clone(),
            Member {
                record_at: IMPORTED_AT,
                snapshot: Snapshot::Imported(BTreeMap::from([
                    (Part::Wallet, isk("10.00")),
                    (Part::Assets, isk("9.00")),
                ])),
            },
        );
        let valued = price_point(&point, &copy, 500, &all, true).expect("value");

        let imported = &valued.members[&name];
        assert_eq!(imported.parts.len(), 8);
        assert_eq!(imported.parts[&Part::Wallet], isk("10.00"));
        assert_eq!(imported.parts[&Part::Assets], isk("9.00"));
        assert_eq!(imported.parts[&Part::SellOrders], Money::ZERO);
        assert_eq!(imported.parts[&Part::BuyEscrow], Money::ZERO);
        assert_eq!(imported.parts[&Part::IndustryJobs], Money::ZERO);
        assert_eq!(imported.parts[&Part::ContractCollateral], Money::ZERO);
        assert_eq!(imported.parts[&Part::Contracts], Money::ZERO);
        assert_eq!(imported.parts[&Part::Implants], Money::ZERO);
        assert_eq!(imported.record_at, IMPORTED_AT);
        assert!(imported.imported);
        assert_eq!(valued.value.by_part[&Part::Wallet], isk("1,234,577.89"));

        // The members' parts add up to the point's, each part shown or not.
        for part in Part::ALL {
            let sum = valued.members.values().fold(Money::ZERO, |sum, member| {
                sum.checked_add(member.parts[&part]).expect("member sum")
            });
            assert_eq!(sum, valued.value.by_part[&part], "{part:?}");
        }
        // The vault is not in the members.
        assert_eq!(valued.members.len(), point.members.len());
        assert_eq!(
            valued.members[&main_key()].parts[&Part::Assets],
            isk("9,792,000.00")
        );
    }

    #[test]
    fn the_source_is_read_after_the_valuing() {
        let point = main_point();
        let history = BTreeMap::from([((PAU, Side::Sell), vec![(NOW_DAY, row("61,200.00"))])]);
        let rules = BTreeMap::new();
        let groups = TypeGroups::new();
        let pricer = Pricer::new(
            point.at,
            None,
            &history,
            &rules,
            PriceRule::SellFivePercent,
            &groups,
        );
        let valued = value_point(&point, &pricer, 0, &all_parts(), true).expect("value");
        assert_eq!(
            valued.source,
            PriceSource::History {
                newest_day: Some(NOW_DAY)
            }
        );
    }

    #[test]
    fn a_vault_too_large_to_multiply_is_an_error() {
        let point = main_point();
        let copy = copy(true);
        assert_eq!(
            price_point(&point, &copy, 1 << 40, &all_parts(), true),
            Err(NetWorthError::Overflow)
        );
    }

    #[test]
    fn a_vault_that_pushes_the_total_over_is_an_error() {
        let point = main_point();
        let copy = copy(true);
        let history = History::new();
        let groups = TypeGroups::new();
        let rules = BTreeMap::from([(
            RuleTarget::Item(PLEX),
            RuleValue::Fixed(Money::from_ten_thousandths(i64::MAX)),
        )]);
        let pricer = Pricer::new(
            point.at,
            Some(&copy),
            &history,
            &rules,
            PriceRule::SellFivePercent,
            &groups,
        );
        assert_eq!(
            value_point(&point, &pricer, 1, &all_parts(), true),
            Err(NetWorthError::Overflow)
        );
    }
}

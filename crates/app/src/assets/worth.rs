//! What one unit of any type is worth on the Assets page: the held Forge market file, read under
//! the player's app-wide `Value items at` price and the item, group and category rules. Reads the
//! store and the game data only; fetches nothing.

use std::collections::{BTreeMap, BTreeSet};

use mop_core::industry::TypeId;
use mop_core::money::Money;
use mop_market::book::PriceRule;
use mop_sde::query::Sde;
use mop_store::Store;
use mop_store::price_history::PointPrices;
use mop_store::settings::{RuleTarget, RuleValue};

use crate::build::prices::Held;
use crate::net_worth::data::pricer::{History, Pricer, TypeGroups};

/// What one unit of each asked type is worth under the held market file and the price rules.
pub struct Worth {
    /// The price of each type that could be priced; a type with no price is absent.
    prices: BTreeMap<TypeId, Money>,
    /// The app-wide `Value items at` price.
    rule: PriceRule,
    /// When the market published the held file, if it has rows.
    published_at: Option<i64>,
}

impl Worth {
    /// Prices every type in `types` once, from `held` and `rules` under `app_wide`. A held file
    /// with rows is the price copy; with no rows there is no copy and no history, so only fixed
    /// rules price.
    pub fn new(
        held: &Held,
        rules: &BTreeMap<RuleTarget, RuleValue>,
        app_wide: PriceRule,
        groups: &TypeGroups,
        types: &BTreeSet<TypeId>,
        now: i64,
    ) -> Self {
        let copy = (!held.rows.is_empty()).then(|| PointPrices {
            at: now,
            published_at: held.published_at,
            rows: held.rows.clone(),
        });
        let history = History::new();
        let pricer = Pricer::new(now, copy.as_ref(), &history, rules, app_wide, groups);
        let prices = types
            .iter()
            .filter_map(|type_id| pricer.price(*type_id).map(|price| (*type_id, price)))
            .collect();
        Self {
            prices,
            rule: app_wide,
            published_at: copy.as_ref().and_then(|copy| copy.published_at),
        }
    }

    /// Reads the app-wide price, the rules at `now` and each type's group and category, then
    /// prices the types. An unknown type is left out of the map, as is one with no price.
    pub fn load(
        store: &Store,
        held: &Held,
        sde: &Sde,
        types: &BTreeSet<TypeId>,
        now: i64,
    ) -> Result<Self, String> {
        let app_wide = store
            .value_price()
            .map_err(|error| format!("Could not read the value price: {error}"))?;
        let rules = store
            .price_rules_at(now)
            .map_err(|error| format!("Could not read the price rules: {error}"))?;
        let mut groups = TypeGroups::new();
        for &type_id in types {
            let item = sde
                .item(type_id)
                .map_err(|error| format!("Could not read the game data: {error}"))?;
            if let Some(item) = item {
                groups.insert(type_id, (item.group_id, item.category_id));
            }
        }
        Ok(Self::new(held, &rules, app_wide, &groups, types, now))
    }

    /// What one unit of `type_id` is worth; `None` for a type not asked for or one with no price.
    pub fn price(&self, type_id: TypeId) -> Option<Money> {
        self.prices.get(&type_id).copied()
    }

    /// The app-wide `Value items at` price.
    pub fn rule(&self) -> PriceRule {
        self.rule
    }

    /// When the market published the held file; `None` when it has no rows.
    pub fn published_at(&self) -> Option<i64> {
        self.published_at
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::Path;

    use mop_core::money::Money;
    use mop_market::book::PriceRule;
    use mop_market::fuzzwork::{Side, SummaryRow};
    use mop_sde::{Sde, import};
    use mop_store::Store;
    use mop_store::settings::{RuleTarget, RuleValue};
    use tempfile::TempDir;

    use super::Worth;
    use crate::build::prices::Held;

    /// The acceptance market's publication time.
    const PUBLISHED_AT: i64 = 992_800;
    /// The point the prices are read at.
    const NOW: i64 = 1_000_000;
    /// R.A.M.- Robotics, a Rifter, and a type with no market row.
    const ROBOTICS: u32 = 11_481;
    const RIFTER: u32 = 587;
    const UNPRICED: u32 = 35;

    fn isk(text: &str) -> Money {
        Money::parse_isk(text).expect("test price")
    }

    fn row(prices: [&str; 5]) -> SummaryRow {
        let [min, max, weighted_average, median, five_percent] = prices;
        SummaryRow {
            min: isk(min),
            max: isk(max),
            weighted_average: isk(weighted_average),
            median: isk(median),
            five_percent: isk(five_percent),
            volume: 1_000,
            orders: 7,
        }
    }

    fn held() -> Held {
        Held {
            rows: BTreeMap::from([
                (
                    (ROBOTICS, Side::Sell),
                    row(["478.60", "500.00", "479.20", "481.00", "480.00"]),
                ),
                (
                    (ROBOTICS, Side::Buy),
                    row(["440.00", "450.00", "445.00", "447.00", "449.00"]),
                ),
                (
                    (RIFTER, Side::Sell),
                    row([
                        "500,000.00",
                        "510,000.00",
                        "503,000.00",
                        "502,000.00",
                        "505,000.00",
                    ]),
                ),
            ]),
            published_at: Some(PUBLISHED_AT),
            next_refresh_at: None,
        }
    }

    fn types() -> BTreeSet<u32> {
        BTreeSet::from([ROBOTICS, RIFTER, UNPRICED])
    }

    fn no_rules() -> BTreeMap<RuleTarget, RuleValue> {
        BTreeMap::new()
    }

    #[test]
    fn the_app_wide_price_prices_every_type_it_has_a_row_for() {
        let worth = Worth::new(
            &held(),
            &no_rules(),
            PriceRule::SellFivePercent,
            &BTreeMap::new(),
            &types(),
            NOW,
        );
        assert_eq!(worth.price(ROBOTICS), Some(isk("480.00")));
        assert_eq!(worth.price(RIFTER), Some(isk("505,000.00")));
        assert_eq!(worth.price(UNPRICED), None);
        assert_eq!(worth.rule(), PriceRule::SellFivePercent);
        assert_eq!(worth.published_at(), Some(PUBLISHED_AT));
    }

    #[test]
    fn a_buy_rule_does_not_fall_back_to_the_sell_side() {
        let worth = Worth::new(
            &held(),
            &no_rules(),
            PriceRule::HighestBuy,
            &BTreeMap::new(),
            &types(),
            NOW,
        );
        assert_eq!(worth.price(ROBOTICS), Some(isk("450.00")));
        assert_eq!(worth.price(RIFTER), None);
    }

    #[test]
    fn an_item_rule_beats_the_app_wide_price() {
        let rules = BTreeMap::from([(
            RuleTarget::Item(RIFTER),
            RuleValue::Fixed(isk("400,000.00")),
        )]);
        let worth = Worth::new(
            &held(),
            &rules,
            PriceRule::SellFivePercent,
            &BTreeMap::new(),
            &types(),
            NOW,
        );
        assert_eq!(worth.price(RIFTER), Some(isk("400,000.00")));
        assert_eq!(worth.price(ROBOTICS), Some(isk("480.00")));
    }

    #[test]
    fn a_category_rule_reaches_a_type_through_its_group_and_category() {
        let rules = BTreeMap::from([(
            RuleTarget::Category(6),
            RuleValue::Market(PriceRule::LowestSell),
        )]);
        let groups = BTreeMap::from([(RIFTER, (25, 6))]);
        let worth = Worth::new(
            &held(),
            &rules,
            PriceRule::SellFivePercent,
            &groups,
            &types(),
            NOW,
        );
        assert_eq!(worth.price(RIFTER), Some(isk("500,000.00")));
        assert_eq!(worth.price(ROBOTICS), Some(isk("480.00")));
    }

    #[test]
    fn a_held_file_with_no_rows_prices_only_fixed_rules() {
        let empty = Held {
            rows: BTreeMap::new(),
            published_at: Some(PUBLISHED_AT),
            next_refresh_at: None,
        };
        let worth = Worth::new(
            &empty,
            &no_rules(),
            PriceRule::SellFivePercent,
            &BTreeMap::new(),
            &types(),
            NOW,
        );
        assert_eq!(worth.price(ROBOTICS), None);
        assert_eq!(worth.price(RIFTER), None);
        assert_eq!(worth.published_at(), None);

        let rules = BTreeMap::from([(
            RuleTarget::Item(RIFTER),
            RuleValue::Fixed(isk("400,000.00")),
        )]);
        let worth = Worth::new(
            &empty,
            &rules,
            PriceRule::SellFivePercent,
            &BTreeMap::new(),
            &types(),
            NOW,
        );
        assert_eq!(worth.price(RIFTER), Some(isk("400,000.00")));
    }

    #[test]
    fn load_reads_the_value_price_the_rules_and_the_game_data() {
        let dir = TempDir::new().expect("a temp folder");
        let source =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../sde/tests/fixtures/sde-3607705");
        let db = dir.path().join("sde.sqlite");
        import(&source, &db).expect("the fixture imports");
        let sde = Sde::open(&db).expect("the fixture opens");

        let mut store = Store::open(&dir.path().join("store.sqlite"), NOW)
            .expect("the store opens")
            .store;
        store
            .set_value_price(PriceRule::BuyMedian)
            .expect("the value price saves");
        store
            .set_price_rule(
                RuleTarget::Category(6),
                Some(RuleValue::Market(PriceRule::LowestSell)),
                NOW,
            )
            .expect("the category rule saves");

        let types = BTreeSet::from([RIFTER]);
        let worth = Worth::load(&store, &held(), &sde, &types, NOW).expect("the worth loads");
        assert_eq!(worth.rule(), PriceRule::BuyMedian);
        assert_eq!(worth.price(RIFTER), Some(isk("500,000.00")));
    }
}

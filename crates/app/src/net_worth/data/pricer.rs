//! What one unit of any type was worth at one net worth point: from the price copy kept with the
//! point, else from the daily Forge history, under the rule that stood on the point's day.
//!
//! A point saved with only PLEX's price (no market download) is not a market copy: only PLEX reads
//! it, and nothing ever falls back from a held price to the history.

use std::cell::Cell;
use std::collections::BTreeMap;

use mop_core::industry::TypeId;
use mop_core::money::Money;
use mop_core::price_history::day_of;
use mop_market::book::PriceRule;
use mop_market::fuzzwork::{Side, Stat, SummaryRow};
use mop_market::plex::PLEX;
use mop_store::price_history::PointPrices;
use mop_store::settings::{RuleTarget, RuleValue, resolve};

/// The daily readings of the Forge, by type and side: the day and the summary read that day. Any
/// order within one day's readings; the pricer reads the newest day at or before the point's day.
pub type History = BTreeMap<(TypeId, Side), Vec<(i64, SummaryRow)>>;

/// A type's group id and category id, by type. A type the game data does not hold is absent.
pub type TypeGroups = BTreeMap<TypeId, (u32, u32)>;

/// Where a point's prices come from: the copy kept with it, or the daily history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PriceSource {
    /// The market prices copied when the point was recorded.
    Copy {
        /// When the market published those prices, if it was known.
        published_at: Option<i64>,
    },
    /// The daily history under the rule that stood on the point's day.
    History {
        /// The newest reading day used so far; `None` until one is used.
        newest_day: Option<i64>,
    },
}

/// The group and category ids given to [`resolve`] for a type the game data does not hold: no rule
/// names them, so only the type's own rule or the app-wide price can price it.
const UNKNOWN_IDS: u32 = u32::MAX;

/// Prices the items of one net worth point, from its price copy and the daily history, under the
/// price rules that were set.
pub struct Pricer<'a> {
    /// The day the point was recorded, from [`day_of`].
    day: i64,
    /// The prices kept with the point; `None` when the point has none, and a PLEX-only copy counts
    /// as none for every type but PLEX.
    copy: Option<&'a PointPrices>,
    /// Whether the copy has a row of any type but PLEX: such a copy prices every type, a
    /// PLEX-only one (no market download) PLEX alone.
    market_copy: bool,
    /// The daily Forge readings, by type and side.
    history: &'a History,
    /// The price rules that were set, by what they are about.
    rules: &'a BTreeMap<RuleTarget, RuleValue>,
    /// The rule for a type with no rule of its own, its group's or its category's.
    app_wide: PriceRule,
    /// Group id and category id by type.
    groups: &'a TypeGroups,
    /// The newest history reading day used so far, for [`Self::source`].
    newest_day: Cell<Option<i64>>,
}

impl<'a> Pricer<'a> {
    /// A pricer for the point at `at`: its day is `day_of(at)`. A copy with no row of a
    /// type but PLEX is PLEX-only and counts as no copy, except that PLEX reads it.
    pub fn new(
        at: i64,
        copy: Option<&'a PointPrices>,
        history: &'a History,
        rules: &'a BTreeMap<RuleTarget, RuleValue>,
        app_wide: PriceRule,
        groups: &'a TypeGroups,
    ) -> Self {
        let market_copy =
            copy.is_some_and(|copy| copy.rows.keys().any(|(type_id, _)| *type_id != PLEX));
        Self {
            day: day_of(at),
            copy,
            market_copy,
            history,
            rules,
            app_wide,
            groups,
            newest_day: Cell::new(None),
        }
    }

    /// What one unit of `type_id` was worth at the point. A fixed price is that price; a market
    /// rule reads the summary price from the copy when the point has one (no row is `None`, with
    /// no fallback to the history nor to another rule), else the newest history reading at or
    /// before the point's day.
    pub fn price(&self, type_id: TypeId) -> Option<Money> {
        let (group_id, category_id) = self
            .groups
            .get(&type_id)
            .copied()
            .unwrap_or((UNKNOWN_IDS, UNKNOWN_IDS));
        let (value, _priced_by) =
            resolve(self.rules, self.app_wide, type_id, group_id, category_id);
        match value {
            RuleValue::Fixed(price) => Some(price),
            RuleValue::Market(rule) => self.market_price(type_id, rule),
        }
    }

    /// Where this pricer's prices come from: the copy when the point kept one, else the history,
    /// whose `newest_day` is the newest reading day used so far.
    pub fn source(&self) -> PriceSource {
        if self.market_copy {
            PriceSource::Copy {
                published_at: self.copy.and_then(|copy| copy.published_at),
            }
        } else {
            PriceSource::History {
                newest_day: self.newest_day.get(),
            }
        }
    }

    /// One summary price of a market rule: from the copy when it prices this type, else from the
    /// history.
    fn market_price(&self, type_id: TypeId, rule: PriceRule) -> Option<Money> {
        let (side, stat) = rule.summary()?;
        let held = self.copy.filter(|_| self.market_copy || type_id == PLEX);
        match held {
            Some(copy) => copy
                .rows
                .get(&(type_id, side))
                .map(|row| row.price(side, stat)),
            None => self.history_price(type_id, side, stat),
        }
    }

    /// The price of the newest reading at or before the point's day, noted as used.
    fn history_price(&self, type_id: TypeId, side: Side, stat: Stat) -> Option<Money> {
        let readings = self.history.get(&(type_id, side))?;
        let (day, row) = readings
            .iter()
            .filter(|(day, _)| *day <= self.day)
            .max_by_key(|(day, _)| *day)?;
        self.note_newest(*day);
        Some(row.price(side, stat))
    }

    /// Keeps the newest day any reading has been used from.
    fn note_newest(&self, day: i64) {
        if self.newest_day.get().is_none_or(|newest| newest < day) {
            self.newest_day.set(Some(day));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{History, PriceSource, Pricer, TypeGroups};
    use mop_core::money::Money;
    use mop_market::book::PriceRule;
    use mop_market::fuzzwork::{Side, SummaryRow};
    use mop_market::plex::PLEX;
    use mop_store::price_history::PointPrices;
    use mop_store::settings::{RuleTarget, RuleValue};

    /// The point's time: day 11,574.
    const NOW: i64 = 1_000_000_000;
    const PUBLISHED_AT: i64 = 999_999_000;
    /// PAU, Tritanium, a Steel Plate, and a type nothing knows.
    const PAU: u32 = 12_612;
    const TRITANIUM: u32 = 34;
    const PLATE: u32 = 11_551;
    const UNKNOWN_TYPE: u32 = 99_999;

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

    /// A history reading whose five prices are the one price: the tests read one stat at a time.
    fn day_row(price: &str) -> SummaryRow {
        row([price; 5])
    }

    fn pau_copy() -> PointPrices {
        PointPrices {
            at: NOW,
            published_at: Some(PUBLISHED_AT),
            rows: BTreeMap::from([
                (
                    (PAU, Side::Sell),
                    row(["61,000", "75,000", "63,100", "66,000", "61,200"]),
                ),
                (
                    (PAU, Side::Buy),
                    row(["50,000", "55,500", "53,000", "51,000", "55,010"]),
                ),
                (
                    (TRITANIUM, Side::Sell),
                    row(["3.85", "4.10", "3.92", "3.90", "3.89"]),
                ),
                (
                    (TRITANIUM, Side::Buy),
                    row(["3.40", "3.60", "3.55", "3.52", "3.50"]),
                ),
            ]),
        }
    }

    /// PAU's Forge history: 5% prices of days 11,567 and 11,574.
    fn history() -> History {
        BTreeMap::from([
            (
                (PAU, Side::Sell),
                vec![(11_567, day_row("60,000")), (11_574, day_row("59,500"))],
            ),
            (
                (PAU, Side::Buy),
                vec![(11_567, day_row("54,000")), (11_574, day_row("53,000"))],
            ),
        ])
    }

    fn groups() -> TypeGroups {
        BTreeMap::from([(PAU, (100, 18)), (TRITANIUM, (18, 4)), (PLATE, (432, 18))])
    }

    #[test]
    fn a_copy_prices_each_summary_rule_it_reads() {
        let copy = pau_copy();
        let history = history();
        let rules = BTreeMap::new();
        let groups = groups();
        for (rule, expected) in [
            (PriceRule::SellFivePercent, "61,200"),
            (PriceRule::BuyFivePercent, "55,010"),
            (PriceRule::LowestSell, "61,000"),
            (PriceRule::HighestBuy, "55,500"),
            (PriceRule::SellMedian, "66,000"),
        ] {
            let pricer = Pricer::new(NOW, Some(&copy), &history, &rules, rule, &groups);
            assert_eq!(pricer.price(PAU), Some(isk(expected)), "{rule:?}");
        }
        let pricer = Pricer::new(
            NOW,
            Some(&copy),
            &history,
            &rules,
            PriceRule::BuyFivePercent,
            &groups,
        );
        assert_eq!(pricer.price(TRITANIUM), Some(isk("3.50")));
        assert_eq!(
            pricer.source(),
            PriceSource::Copy {
                published_at: Some(PUBLISHED_AT)
            }
        );
    }

    #[test]
    fn a_copy_row_is_the_only_price_and_a_missing_row_is_none() {
        let copy = pau_copy();
        let history = history();
        let groups = groups();
        let rules = BTreeMap::new();
        // The history's 59,500.00 is not read while the copy stands, and type 35 has no row.
        let pricer = Pricer::new(
            NOW,
            Some(&copy),
            &history,
            &rules,
            PriceRule::SellFivePercent,
            &groups,
        );
        assert_eq!(pricer.price(PAU), Some(isk("61,200")));
        assert_eq!(pricer.price(35), None);

        // A copy without PAU's Buy row prices its buy side nothing, not the history's 53,000.00.
        let mut thin = pau_copy();
        thin.rows.remove(&(PAU, Side::Buy));
        let pricer = Pricer::new(
            NOW,
            Some(&thin),
            &history,
            &rules,
            PriceRule::BuyFivePercent,
            &groups,
        );
        assert_eq!(pricer.price(PAU), None);
        assert_eq!(
            pricer.source(),
            PriceSource::Copy {
                published_at: Some(PUBLISHED_AT)
            }
        );
    }

    #[test]
    fn without_a_copy_the_newest_reading_at_or_before_the_point_day_prices() {
        let history = history();
        let rules = BTreeMap::new();
        let groups = groups();
        for (at, expected, newest_day) in [
            // Day 11,567: both readings of that day, the older day not read.
            (999_395_200, Some("60,000"), Some(11_567)),
            (NOW, Some("59,500"), Some(11_574)),
            // Day 11,570: the newest reading at or before it is day 11,567's.
            (999_650_000, Some("60,000"), Some(11_567)),
            // Day 11,566: before every reading.
            (999_302_400, None, None),
        ] {
            let pricer = Pricer::new(
                at,
                None,
                &history,
                &rules,
                PriceRule::SellFivePercent,
                &groups,
            );
            assert_eq!(pricer.price(PAU), expected.map(isk), "at {at}");
            assert_eq!(
                pricer.source(),
                PriceSource::History { newest_day },
                "at {at}"
            );
        }
    }

    #[test]
    fn the_newest_day_used_stays_when_a_later_type_has_no_reading() {
        let history = history();
        let rules = BTreeMap::new();
        let groups = groups();
        let pricer = Pricer::new(
            NOW,
            None,
            &history,
            &rules,
            PriceRule::SellFivePercent,
            &groups,
        );
        assert_eq!(pricer.price(PAU), Some(isk("59,500")));
        assert_eq!(pricer.price(35), None);
        assert_eq!(
            pricer.source(),
            PriceSource::History {
                newest_day: Some(11_574)
            }
        );
    }

    #[test]
    fn an_empty_history_prices_nothing() {
        let history = History::new();
        let rules = BTreeMap::new();
        let groups = groups();
        let pricer = Pricer::new(
            NOW,
            None,
            &history,
            &rules,
            PriceRule::SellFivePercent,
            &groups,
        );
        assert_eq!(pricer.price(PAU), None);
        assert_eq!(pricer.source(), PriceSource::History { newest_day: None });
    }

    #[test]
    fn the_rule_that_wins_says_the_price() {
        let copy = pau_copy();
        let history = history();
        let groups = groups();

        let fixed_plate =
            BTreeMap::from([(RuleTarget::Item(PLATE), RuleValue::Fixed(isk("29,000,000")))]);
        let pricer = Pricer::new(
            NOW,
            Some(&copy),
            &history,
            &fixed_plate,
            PriceRule::SellFivePercent,
            &groups,
        );
        assert_eq!(pricer.price(PLATE), Some(isk("29,000,000")));

        let group_rule = BTreeMap::from([(
            RuleTarget::Group(18),
            RuleValue::Market(PriceRule::BuyFivePercent),
        )]);
        let pricer = Pricer::new(
            NOW,
            Some(&copy),
            &history,
            &group_rule,
            PriceRule::SellFivePercent,
            &groups,
        );
        assert_eq!(pricer.price(TRITANIUM), Some(isk("3.50")));
        assert_eq!(pricer.price(PAU), Some(isk("61,200")));

        // No Tritanium Buy row in the copy: the group rule prices nothing, with no fallback to
        // the app-wide sell price.
        let mut thin = pau_copy();
        thin.rows.remove(&(TRITANIUM, Side::Buy));
        let pricer = Pricer::new(
            NOW,
            Some(&thin),
            &history,
            &group_rule,
            PriceRule::SellFivePercent,
            &groups,
        );
        assert_eq!(pricer.price(TRITANIUM), None);

        // A type the game data does not hold: a group rule does not reach it, so only its own
        // rule or the app-wide price can price it.
        let pricer = Pricer::new(
            NOW,
            Some(&copy),
            &history,
            &group_rule,
            PriceRule::SellFivePercent,
            &groups,
        );
        assert_eq!(pricer.price(UNKNOWN_TYPE), None);
        let own_rule =
            BTreeMap::from([(RuleTarget::Item(UNKNOWN_TYPE), RuleValue::Fixed(isk("1")))]);
        let pricer = Pricer::new(
            NOW,
            Some(&copy),
            &history,
            &own_rule,
            PriceRule::SellFivePercent,
            &groups,
        );
        assert_eq!(pricer.price(UNKNOWN_TYPE), Some(isk("1")));

        // WalkSell needs the order list the copy and the history do not hold.
        let no_rules = BTreeMap::new();
        let with_copy = Pricer::new(
            NOW,
            Some(&copy),
            &history,
            &no_rules,
            PriceRule::WalkSell,
            &groups,
        );
        assert_eq!(with_copy.price(PAU), None);
        let without_copy =
            Pricer::new(NOW, None, &history, &no_rules, PriceRule::WalkSell, &groups);
        assert_eq!(without_copy.price(PAU), None);
    }

    #[test]
    fn a_plex_only_copy_is_no_copy_except_for_plex() {
        let plex_copy = PointPrices {
            at: NOW,
            published_at: None,
            rows: BTreeMap::from([((PLEX, Side::Sell), day_row("5,200,000"))]),
        };
        let history = history();
        let rules = BTreeMap::new();
        let groups = groups();
        let pricer = Pricer::new(
            NOW,
            Some(&plex_copy),
            &history,
            &rules,
            PriceRule::SellFivePercent,
            &groups,
        );
        assert_eq!(pricer.price(PLEX), Some(isk("5,200,000")));
        assert_eq!(pricer.price(PAU), Some(isk("59,500")));
        assert_eq!(
            pricer.source(),
            PriceSource::History {
                newest_day: Some(11_574)
            }
        );

        // PLEX has no Buy row and never falls back to the history.
        let pricer = Pricer::new(
            NOW,
            Some(&plex_copy),
            &history,
            &rules,
            PriceRule::BuyFivePercent,
            &groups,
        );
        assert_eq!(pricer.price(PLEX), None);

        // A copy without a single row is no copy either.
        let empty = PointPrices {
            at: NOW,
            published_at: Some(PUBLISHED_AT),
            rows: BTreeMap::new(),
        };
        let pricer = Pricer::new(
            NOW,
            Some(&empty),
            &history,
            &rules,
            PriceRule::SellFivePercent,
            &groups,
        );
        assert_eq!(pricer.price(PAU), Some(isk("59,500")));
        assert_eq!(
            pricer.source(),
            PriceSource::History {
                newest_day: Some(11_574)
            }
        );
    }
}

//! Blueprints that The Forge sells, and how many units pay for one.
//!
//! A blueprint is buyable when the market has a sell price for its type: a copy has no market
//! row, and a faction blueprint is LP-store only. The price is what a buyer pays, and the payback
//! is the smallest whole number of units whose profit covers it.

#![deny(
    clippy::arithmetic_side_effects,
    clippy::float_arithmetic,
    clippy::as_conversions,
    clippy::integer_division
)]

use crate::industry::TypeId;
use crate::money::Money;

/// A blueprint on sale for one product.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlueprintOffer {
    pub product: TypeId,
    pub blueprint: TypeId,
    pub price: Money,
}

impl BlueprintOffer {
    /// The smallest number of units whose profit covers the blueprint.
    ///
    /// `None` when the price or the profit is zero or negative, or the answer does not fit `u64`.
    pub fn payback_units(&self, profit_each: Money) -> Option<u64> {
        let price = self.price.ten_thousandths();
        let profit = profit_each.ten_thousandths();
        if price <= 0 || profit <= 0 {
            return None;
        }
        let price = i128::from(price);
        let profit = i128::from(profit);
        let units = price
            .checked_add(profit.checked_sub(1)?)?
            .checked_div(profit)?;
        u64::try_from(units).ok()
    }
}

/// The offers for the pairs whose blueprint has a positive sell price, in input order.
///
/// A pair keeps its product and blueprint ids and gets the price `price(blueprint)` returned.
/// Pairs without a price, or with zero or negative one, are left out.
pub fn buyable_blueprints(
    pairs: &[(TypeId, TypeId)],
    price: impl Fn(TypeId) -> Option<Money>,
) -> Vec<BlueprintOffer> {
    pairs
        .iter()
        .filter_map(|&(product, blueprint)| {
            let quoted = price(blueprint)?;
            (quoted > Money::ZERO).then_some(BlueprintOffer {
                product,
                blueprint,
                price: quoted,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{BlueprintOffer, buyable_blueprints};
    use crate::money::Money;

    fn isk(text: &str) -> Money {
        Money::parse_isk(text).unwrap()
    }

    #[test]
    fn an_offer_keeps_the_pair_and_the_price() {
        let offer = BlueprintOffer {
            product: 587,
            blueprint: 691,
            price: isk("3,500,000.00"),
        };
        assert_eq!(offer.product, 587);
        assert_eq!(offer.blueprint, 691);
        assert_eq!(offer.price, isk("3,500,000.00"));
    }

    #[test]
    fn buyable_blueprints_keeps_input_order_and_skips_missing_prices() {
        let pairs = [(587, 691), (2454, 2455), (2456, 2457)];
        let offers = buyable_blueprints(&pairs, |blueprint| match blueprint {
            691 => Some(isk("3,500,000.00")),
            2455 => Some(isk("499,639.39")),
            _ => None,
        });
        assert_eq!(
            offers,
            vec![
                BlueprintOffer {
                    product: 587,
                    blueprint: 691,
                    price: isk("3,500,000.00"),
                },
                BlueprintOffer {
                    product: 2454,
                    blueprint: 2455,
                    price: isk("499,639.39"),
                },
            ]
        );
    }

    #[test]
    fn buyable_blueprints_leaves_out_zero_and_negative_prices() {
        let pairs = [(1, 10), (2, 20), (3, 30)];
        let offers = buyable_blueprints(&pairs, |blueprint| match blueprint {
            10 => Some(Money::ZERO),
            20 => Some(isk("-1.00")),
            _ => Some(isk("1.00")),
        });
        assert_eq!(
            offers,
            vec![BlueprintOffer {
                product: 3,
                blueprint: 30,
                price: isk("1.00"),
            }]
        );
    }

    #[test]
    fn buyable_blueprints_of_nothing_is_empty() {
        assert!(buyable_blueprints(&[], |_| Some(isk("1.00"))).is_empty());
    }

    #[test]
    fn payback_units_rounds_the_ceiling_up() {
        let offer = |price: &str| BlueprintOffer {
            product: 1,
            blueprint: 2,
            price: isk(price),
        };
        assert_eq!(
            offer("500,000.00").payback_units(isk("100.00")),
            Some(5_000)
        );
        assert_eq!(
            offer("500,000.01").payback_units(isk("100.00")),
            Some(5_001)
        );
        assert_eq!(offer("1.00").payback_units(isk("3.00")), Some(1));
        assert_eq!(offer("0.01").payback_units(isk("3.00")), Some(1));
    }

    #[test]
    fn payback_units_needs_a_positive_price_and_profit() {
        let offer = |price: &str| BlueprintOffer {
            product: 1,
            blueprint: 2,
            price: isk(price),
        };
        assert_eq!(offer("0.00").payback_units(isk("100.00")), None);
        assert_eq!(offer("-1.00").payback_units(isk("100.00")), None);
        assert_eq!(offer("500,000.00").payback_units(isk("0.00")), None);
        assert_eq!(offer("500,000.00").payback_units(isk("-100.00")), None);
    }
}

//! Order books: reading ESI's order list and pricing a quantity at one location.

#![deny(
    clippy::arithmetic_side_effects,
    clippy::as_conversions,
    clippy::integer_division
)]

use mop_core::industry::TypeId;
use mop_core::money::Money;
use serde::Deserialize;

use crate::error::MarketError;
use crate::fuzzwork::{Side, Stat};

/// One buy or sell order from a region's order book.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Order {
    pub order_id: u64,
    pub type_id: TypeId,
    pub location_id: u64,
    pub is_buy: bool,
    pub price: Money,
    pub volume_remain: u64,
}

/// The order fields we read. ESI's other fields are ignored.
#[derive(Deserialize)]
struct RawOrder {
    order_id: u64,
    type_id: TypeId,
    location_id: u64,
    is_buy_order: bool,
    price: serde_json::Number,
    volume_remain: u64,
}

impl RawOrder {
    fn into_order(self) -> Result<Order, MarketError> {
        // With arbitrary_precision the number keeps its literal text, so the price is never read as f64.
        let text = self.price.to_string();
        let order_id = self.order_id;
        let price = Money::parse_decimal(&text)
            .ok()
            .ok_or_else(|| MarketError::BadPrice {
                order_id,
                text: text.clone(),
            })?;
        Ok(Order {
            order_id,
            type_id: self.type_id,
            location_id: self.location_id,
            is_buy: self.is_buy_order,
            price,
            volume_remain: self.volume_remain,
        })
    }
}

/// Reads ESI's order list for one type. Fails on the first order with an unreadable price.
pub fn parse_orders(json: &str) -> Result<Vec<Order>, MarketError> {
    let raw: Vec<RawOrder> =
        serde_json::from_str(json).map_err(|error| MarketError::Json(error.to_string()))?;
    raw.into_iter().map(RawOrder::into_order).collect()
}

/// Which price a quote is built from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PriceRule {
    /// Buy the quantity by walking up the sell orders, cheapest first.
    WalkSell,
    /// The lowest sell price times the quantity, whether or not that many units are for sale.
    LowestSell,
    /// The highest buy price times the quantity.
    HighestBuy,
    /// The sell side's 5% price times the quantity, from the summary file.
    #[default]
    SellFivePercent,
    /// The sell side's volume-weighted average price times the quantity, from the summary file.
    SellWeightedAverage,
    /// The sell side's median price times the quantity, from the summary file.
    SellMedian,
    /// The buy side's 5% price times the quantity, from the summary file.
    BuyFivePercent,
    /// The buy side's volume-weighted average price times the quantity, from the summary file.
    BuyWeightedAverage,
    /// The buy side's median price times the quantity, from the summary file.
    BuyMedian,
}

impl PriceRule {
    /// The side and statistic of the summary file this rule reads. `None` for [`PriceRule::WalkSell`],
    /// which needs the order list the summary file does not hold.
    pub fn summary(self) -> Option<(Side, Stat)> {
        match self {
            Self::WalkSell => None,
            Self::LowestSell => Some((Side::Sell, Stat::Best)),
            Self::HighestBuy => Some((Side::Buy, Stat::Best)),
            Self::SellFivePercent => Some((Side::Sell, Stat::FivePercent)),
            Self::SellWeightedAverage => Some((Side::Sell, Stat::WeightedAverage)),
            Self::SellMedian => Some((Side::Sell, Stat::Median)),
            Self::BuyFivePercent => Some((Side::Buy, Stat::FivePercent)),
            Self::BuyWeightedAverage => Some((Side::Buy, Stat::WeightedAverage)),
            Self::BuyMedian => Some((Side::Buy, Stat::Median)),
        }
    }
}

/// The price of a quantity under one rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quote {
    /// The quantity is priced. `available` counts every unit on the side the rule reads.
    Priced {
        total: Money,
        per_unit: Money,
        available: u64,
    },
    /// Not enough units are for sale to buy `wanted`. `total_for_available` buys all of them.
    Thin {
        wanted: u64,
        available: u64,
        total_for_available: Money,
    },
    /// The side the rule reads has no orders at the location.
    NoOrders,
}

/// Prices `qty` units at `location_id` under `rule`. Only orders located there count.
///
/// An empty side gives [`Quote::NoOrders`] for every quantity, including zero.
pub fn quote(
    orders: &[Order],
    location_id: u64,
    rule: PriceRule,
    qty: u64,
) -> Result<Quote, MarketError> {
    let mut here: Vec<&Order> = orders
        .iter()
        .filter(|order| order.location_id == location_id)
        .collect();
    here.sort_by_key(|order| order.price);
    let sells: Vec<&Order> = here.iter().copied().filter(|order| !order.is_buy).collect();
    let buys: Vec<&Order> = here
        .iter()
        .copied()
        .rev()
        .filter(|order| order.is_buy)
        .collect();
    match rule {
        PriceRule::WalkSell => walk_sell(&sells, qty),
        PriceRule::LowestSell => best_price(&sells, qty),
        PriceRule::HighestBuy => best_price(&buys, qty),
        PriceRule::SellFivePercent
        | PriceRule::SellWeightedAverage
        | PriceRule::SellMedian
        | PriceRule::BuyFivePercent
        | PriceRule::BuyWeightedAverage
        | PriceRule::BuyMedian => Err(MarketError::SummaryRule),
    }
}

/// Buys `qty` from `sells` (cheapest first). Fewer units for sale than `qty` gives [`Quote::Thin`].
fn walk_sell(sells: &[&Order], qty: u64) -> Result<Quote, MarketError> {
    if sells.is_empty() {
        return Ok(Quote::NoOrders);
    }
    let available = total_volume(sells)?;
    if available < qty {
        let total_for_available = walk(sells, available)?;
        return Ok(Quote::Thin {
            wanted: qty,
            available,
            total_for_available,
        });
    }
    let total = walk(sells, qty)?;
    Ok(priced(total, qty, available))
}

/// `qty` times the best price in `side` (sorted best first), whatever the volume behind it.
fn best_price(side: &[&Order], qty: u64) -> Result<Quote, MarketError> {
    let Some(best) = side.first() else {
        return Ok(Quote::NoOrders);
    };
    let available = total_volume(side)?;
    let total = best
        .price
        .checked_mul_qty(qty)
        .ok_or(MarketError::Overflow)?;
    Ok(priced(total, qty, available))
}

/// What `qty` units cost when taken from `sells` in order. Fails when fewer than `qty` are listed.
fn walk(sells: &[&Order], qty: u64) -> Result<Money, MarketError> {
    let mut left = qty;
    let mut total = Money::ZERO;
    for order in sells {
        if left == 0 {
            break;
        }
        let take = left.min(order.volume_remain);
        let cost = order
            .price
            .checked_mul_qty(take)
            .ok_or(MarketError::Overflow)?;
        total = total.checked_add(cost).ok_or(MarketError::Overflow)?;
        // `take` never exceeds `left`, so this cannot saturate.
        left = left.saturating_sub(take);
    }
    Ok(total)
}

fn total_volume(orders: &[&Order]) -> Result<u64, MarketError> {
    orders
        .iter()
        .try_fold(0u64, |sum, order| sum.checked_add(order.volume_remain))
        .ok_or(MarketError::Overflow)
}

fn priced(total: Money, qty: u64, available: u64) -> Quote {
    // `checked_div_qty` is `None` only for zero units, and a zero quantity has a zero per-unit price.
    let per_unit = total.checked_div_qty(qty).unwrap_or(Money::ZERO);
    Quote::Priced {
        total,
        per_unit,
        available,
    }
}

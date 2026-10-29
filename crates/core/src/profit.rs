//! Fees, costs and profit for one sale: exact ISK arithmetic, no screen and no storage.
//!
//! The caller works out the four costs and the sale value; this module only charges the fees the
//! chosen sell mode pays and subtracts everything. A loss is a result, not an error.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::float_arithmetic,
    clippy::as_conversions,
    clippy::integer_division
)]

use std::fmt;

use crate::fees::{BrokerFee, FeeError, instant_sell, sell_order};
use crate::money::{Money, Rate};

/// How the sale is made, which decides the fees.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SellMode {
    /// A sell order: the broker fee and the structure's SCC when placed, sales tax on the sale.
    SellOrder(BrokerFee),
    /// An instant sell into a buy order: sales tax only.
    BuyOrders,
}

/// The plan's cost lines, each already worked out by the caller.
///
/// `materials` never holds the invention datacores or decryptors: the buy list adds them as
/// lines, and `invention` carries them here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Costs {
    pub materials: Money,
    pub jobs: Money,
    pub invention: Money,
    pub freight: Money,
}

impl Costs {
    /// The four costs summed; a cost below zero or a sum that does not fit is an error.
    fn total(&self) -> Result<Money, ProfitError> {
        for cost in [self.materials, self.jobs, self.invention, self.freight] {
            if cost < Money::ZERO {
                return Err(ProfitError::Negative);
            }
        }
        self.materials
            .checked_add(self.jobs)
            .and_then(|sum| sum.checked_add(self.invention))
            .and_then(|sum| sum.checked_add(self.freight))
            .ok_or(ProfitError::Overflow)
    }
}

/// What one sale leaves: the sale value, every fee, every cost and the difference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Profit {
    pub gross: Money,
    /// Broker plus SCC plus sales tax.
    pub fees: Money,
    /// The four cost lines summed.
    pub costs: Money,
    /// `gross` minus `fees` minus `costs`; negative is a loss.
    pub profit: Money,
}

/// Fees, costs and the profit of a sale worth `gross`.
pub fn profit(
    gross: Money,
    mode: SellMode,
    tax: Rate,
    costs: &Costs,
) -> Result<Profit, ProfitError> {
    let costs = costs.total()?;
    let sale = match mode {
        SellMode::SellOrder(fee) => sell_order(gross, fee, tax),
        SellMode::BuyOrders => instant_sell(gross, tax),
    }
    .map_err(ProfitError::Fee)?;
    let fees = sale
        .broker
        .checked_add(sale.scc)
        .and_then(|fees| fees.checked_add(sale.tax))
        .ok_or(ProfitError::Overflow)?;
    let profit = gross
        .checked_sub(fees)
        .and_then(|profit| profit.checked_sub(costs))
        .ok_or(ProfitError::Overflow)?;
    Ok(Profit {
        gross,
        fees,
        costs,
        profit,
    })
}

/// Why a profit could not be worked out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfitError {
    /// A cost line is below zero.
    Negative,
    /// A sum or difference does not fit.
    Overflow,
    /// The sale itself was refused.
    Fee(FeeError),
}

impl fmt::Display for ProfitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProfitError::Negative => write!(f, "a cost is below zero"),
            ProfitError::Overflow => write!(f, "the profit is too large"),
            ProfitError::Fee(fee) => write!(f, "{fee}"),
        }
    }
}

impl std::error::Error for ProfitError {}

#[cfg(test)]
mod tests {
    use super::{Costs, Profit, ProfitError, SellMode, profit};
    use crate::fees::{BrokerFee, FeeError};
    use crate::money::{Money, Rate};

    fn isk(text: &str) -> Money {
        Money::parse_isk(text).unwrap()
    }

    fn npc() -> BrokerFee {
        BrokerFee::Npc(Rate::from_ppm(30_000))
    }

    fn tax() -> Rate {
        Rate::from_ppm(75_000)
    }

    fn costs() -> Costs {
        Costs {
            materials: isk("500,000.00"),
            jobs: isk("20,000.00"),
            invention: isk("0"),
            freight: isk("10,000.00"),
        }
    }

    #[test]
    fn a_sell_order_pays_broker_and_tax() {
        let result = profit(
            isk("1,000,000.00"),
            SellMode::SellOrder(npc()),
            tax(),
            &costs(),
        )
        .unwrap();
        assert_eq!(
            result,
            Profit {
                gross: isk("1,000,000.00"),
                fees: isk("105,000.00"),
                costs: isk("530,000.00"),
                profit: Money::from_ten_thousandths(3_650_000_000),
            }
        );
        assert_eq!(result.fees.ten_thousandths(), 1_050_000_000);
        assert_eq!(result.costs.ten_thousandths(), 5_300_000_000);
        assert_eq!(result.profit.format_isk(), "365,000.00");
    }

    #[test]
    fn a_loss_is_a_result_not_an_error() {
        let result = profit(
            isk("100,000.00"),
            SellMode::SellOrder(npc()),
            tax(),
            &costs(),
        )
        .unwrap();
        assert_eq!(result.fees.ten_thousandths(), 105_000_000);
        assert_eq!(result.profit.ten_thousandths(), -4_405_000_000);
    }

    #[test]
    fn buy_orders_pay_the_sales_tax_only() {
        let result = profit(isk("1,000,000.00"), SellMode::BuyOrders, tax(), &costs()).unwrap();
        assert_eq!(result.fees, isk("75,000.00"));
        assert_eq!(result.profit.ten_thousandths(), 3_950_000_000);
    }

    #[test]
    fn a_structure_pays_its_scc_as_well() {
        let mode = SellMode::SellOrder(BrokerFee::Structure {
            owner: Rate::from_ppm(30_000),
            scc: Rate::from_ppm(5_000),
        });
        let result = profit(isk("1,000,000.00"), mode, tax(), &costs()).unwrap();
        assert_eq!(result.fees, isk("110,000.00"));
        assert_eq!(result.profit.ten_thousandths(), 3_600_000_000);
    }

    #[test]
    fn a_small_sale_keeps_the_minimum_broker_fee() {
        let result = profit(
            isk("1,000.00"),
            SellMode::SellOrder(npc()),
            tax(),
            &Costs::default(),
        )
        .unwrap();
        assert_eq!(result.fees, isk("175.00"));
        assert_eq!(result.profit.ten_thousandths(), 8_250_000);
    }

    #[test]
    fn every_cost_line_must_not_be_negative() {
        for costs in [
            Costs {
                materials: Money::from_ten_thousandths(-1),
                ..Costs::default()
            },
            Costs {
                jobs: Money::from_ten_thousandths(-1),
                ..Costs::default()
            },
            Costs {
                invention: Money::from_ten_thousandths(-1),
                ..Costs::default()
            },
            Costs {
                freight: Money::from_ten_thousandths(-1),
                ..Costs::default()
            },
        ] {
            assert_eq!(
                profit(Money::ZERO, SellMode::BuyOrders, tax(), &costs),
                Err(ProfitError::Negative)
            );
        }
    }

    #[test]
    fn costs_that_do_not_add_up_are_an_overflow() {
        let huge = Costs {
            materials: Money::from_ten_thousandths(i64::MAX),
            jobs: Money::from_ten_thousandths(1),
            ..Costs::default()
        };
        assert_eq!(
            profit(Money::ZERO, SellMode::SellOrder(npc()), tax(), &huge),
            Err(ProfitError::Overflow)
        );
    }

    #[test]
    fn a_negative_sale_is_the_fee_error() {
        for mode in [SellMode::SellOrder(npc()), SellMode::BuyOrders] {
            assert_eq!(
                profit(
                    Money::from_ten_thousandths(-1),
                    mode,
                    tax(),
                    &Costs::default()
                ),
                Err(ProfitError::Fee(FeeError::Negative))
            );
        }
    }

    #[test]
    fn errors_say_what_is_wrong() {
        assert_eq!(ProfitError::Negative.to_string(), "a cost is below zero");
        assert_eq!(ProfitError::Overflow.to_string(), "the profit is too large");
        assert_eq!(
            ProfitError::Fee(FeeError::Negative).to_string(),
            "amount is negative"
        );
        let as_error: &dyn std::error::Error = &ProfitError::Overflow;
        assert_eq!(as_error.to_string(), "the profit is too large");
    }
}

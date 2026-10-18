//! Sales tax, broker fees and structure SCC for buy and sell orders, to the game's rounding.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::float_arithmetic,
    clippy::as_conversions,
    clippy::integer_division
)]

use std::fmt;

use crate::decimal::div_round;
use crate::money::{Money, Rate};

/// Ten-thousandths of ISK per 0.01 ISK, times 1,000,000 ppm: divides a product to get cents.
const CENT_RATE_DIVISOR: i128 = 100 * 1_000_000;
/// The broker part of an order is never below 100.00 ISK.
const MINIMUM_BROKER_FEE: Money = Money::from_ten_thousandths(1_000_000);
/// The structure SCC part, 0.5% unless the owner's setting says otherwise.
const DEFAULT_SCC_PPM: u32 = 5_000;
/// The highest Accounting, Broker Relations level in the game.
const MAX_SKILL_LEVEL: u8 = 5;

/// A standing in hundredths: `5.23` is 523, `-10.00` is -1,000.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Standing(i16);

impl Standing {
    pub fn from_hundredths(hundredths: i16) -> Result<Standing, FeeError> {
        if (-1_000..=1_000).contains(&hundredths) {
            Ok(Standing(hundredths))
        } else {
            Err(FeeError::Standing)
        }
    }

    /// Parses a standing typed by a person: `5.23`, `-1.25`, `10`, `0.5`. Two decimals at most.
    pub fn parse(text: &str) -> Result<Standing, FeeError> {
        let (negative, unsigned) = match text.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, text),
        };
        let (whole, fraction) = match unsigned.split_once('.') {
            Some((whole, fraction)) => (whole, Some(fraction)),
            None => (unsigned, None),
        };
        if !is_digits(whole) {
            return Err(FeeError::Standing);
        }
        let fraction = match fraction {
            None => 0,
            Some(digits) if (1..=2).contains(&digits.len()) && is_digits(digits) => {
                // Pad to two places, so "5" reads as 50 hundredths.
                format!("{digits:0<2}")
                    .parse::<i32>()
                    .ok()
                    .ok_or(FeeError::Standing)?
            }
            Some(_) => return Err(FeeError::Standing),
        };
        let whole: i32 = whole.parse().ok().ok_or(FeeError::Standing)?;
        let hundredths = whole
            .checked_mul(100)
            .and_then(|whole| whole.checked_add(fraction))
            .ok_or(FeeError::Standing)?;
        let hundredths = if negative {
            hundredths.checked_neg().ok_or(FeeError::Standing)?
        } else {
            hundredths
        };
        let hundredths = i16::try_from(hundredths).ok().ok_or(FeeError::Standing)?;
        Standing::from_hundredths(hundredths)
    }
}

fn is_digits(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit())
}

/// Sales tax for a seller with this Accounting level: 7.5% at level 0, less 0.11 points per level.
pub fn sales_tax_rate(accounting: u8) -> Result<Rate, FeeError> {
    check_level(accounting)?;
    let discount = 11u32
        .checked_mul(u32::from(accounting))
        .ok_or(FeeError::Overflow)?;
    let percent = 100u32.checked_sub(discount).ok_or(FeeError::Overflow)?;
    750u32
        .checked_mul(percent)
        .map(Rate::from_ppm)
        .ok_or(FeeError::Overflow)
}

/// The NPC broker rate: 3%, less 0.3% per Broker Relations level, less standing discounts.
///
/// The standings part is not checked against the game; callers may pass any `Rate` instead.
pub fn npc_broker_rate(
    broker_relations: u8,
    faction: Standing,
    corporation: Standing,
) -> Result<Rate, FeeError> {
    check_level(broker_relations)?;
    let level = i64::from(broker_relations);
    let faction = i64::from(faction.0);
    let corporation = i64::from(corporation.0);
    // In ppm: 3% is 30,000, each skill level is 0.3% (3,000), each standing hundredth is 0.03% (3) or 0.02% (2).
    let ppm = level
        .checked_mul(3_000)
        .and_then(|discount| discount.checked_add(faction.checked_mul(3)?))
        .and_then(|discount| discount.checked_add(corporation.checked_mul(2)?))
        .and_then(|discount| 30_000i64.checked_sub(discount))
        .ok_or(FeeError::Overflow)?;
    u32::try_from(ppm)
        .ok()
        .map(Rate::from_ppm)
        .ok_or(FeeError::Overflow)
}

fn check_level(level: u8) -> Result<(), FeeError> {
    if level > MAX_SKILL_LEVEL {
        Err(FeeError::SkillLevel(level))
    } else {
        Ok(())
    }
}

/// How the broker and the structure's SCC are charged on an order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrokerFee {
    /// An NPC station: one broker rate, no SCC.
    Npc(Rate),
    /// A player structure: the owner's broker rate and the SCC rate.
    Structure { owner: Rate, scc: Rate },
}

impl BrokerFee {
    /// A structure with the owner's broker rate and the default 0.5% SCC.
    pub fn structure(owner: Rate) -> BrokerFee {
        BrokerFee::Structure {
            owner,
            scc: Rate::from_ppm(DEFAULT_SCC_PPM),
        }
    }
}

/// The broker and SCC charged on one order value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BrokerCharge {
    pub broker: Money,
    pub scc: Money,
}

impl BrokerCharge {
    /// Broker plus SCC; `None` if the sum does not fit.
    pub fn total(self) -> Option<Money> {
        self.broker.checked_add(self.scc)
    }
}

/// The broker and SCC on an order worth `order_value`.
///
/// The broker part is at least 100.00 ISK; the SCC part has no minimum. Both are rounded to 0.01 ISK.
pub fn broker_charge(order_value: Money, fee: BrokerFee) -> Result<BrokerCharge, FeeError> {
    if order_value < Money::ZERO {
        return Err(FeeError::Negative);
    }
    let (broker_rate, scc_rate) = match fee {
        BrokerFee::Npc(rate) => (rate, None),
        BrokerFee::Structure { owner, scc } => (owner, Some(scc)),
    };
    let broker = round_to_cents(order_value, broker_rate)?.max(MINIMUM_BROKER_FEE);
    let scc = match scc_rate {
        Some(rate) => round_to_cents(order_value, rate)?,
        None => Money::ZERO,
    };
    Ok(BrokerCharge { broker, scc })
}

/// The sales tax on a sale worth `gross`, rounded to 0.0001 ISK.
pub fn sales_tax(gross: Money, rate: Rate) -> Result<Money, FeeError> {
    if gross < Money::ZERO {
        return Err(FeeError::Negative);
    }
    gross.checked_mul_rate(rate).ok_or(FeeError::Overflow)
}

/// What a sale costs the seller, and what is left.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SellFees {
    pub broker: Money,
    pub scc: Money,
    pub tax: Money,
    /// Gross minus broker, SCC and tax. Negative when the fees are more than the sale.
    pub net: Money,
}

/// A sell order: the broker fee when placed, sales tax when it sells.
pub fn sell_order(gross: Money, fee: BrokerFee, tax: Rate) -> Result<SellFees, FeeError> {
    let charge = broker_charge(gross, fee)?;
    let tax = sales_tax(gross, tax)?;
    let net = gross
        .checked_sub(charge.broker)
        .and_then(|net| net.checked_sub(charge.scc))
        .and_then(|net| net.checked_sub(tax))
        .ok_or(FeeError::Overflow)?;
    Ok(SellFees {
        broker: charge.broker,
        scc: charge.scc,
        tax,
        net,
    })
}

/// An instant sell into a buy order: sales tax only.
pub fn instant_sell(gross: Money, tax: Rate) -> Result<SellFees, FeeError> {
    let tax = sales_tax(gross, tax)?;
    let net = gross.checked_sub(tax).ok_or(FeeError::Overflow)?;
    Ok(SellFees {
        broker: Money::ZERO,
        scc: Money::ZERO,
        tax,
        net,
    })
}

/// A buy order: the broker fee when placed, nothing more.
pub fn buy_order(value: Money, fee: BrokerFee) -> Result<BrokerCharge, FeeError> {
    broker_charge(value, fee)
}

/// `value` times `rate`, rounded half away from zero to 0.01 ISK.
fn round_to_cents(value: Money, rate: Rate) -> Result<Money, FeeError> {
    let product = i128::from(value.ten_thousandths())
        .checked_mul(i128::from(rate.ppm()))
        .ok_or(FeeError::Overflow)?;
    let cents = div_round(product, CENT_RATE_DIVISOR).ok_or(FeeError::Overflow)?;
    let ten_thousandths = cents.checked_mul(100).ok_or(FeeError::Overflow)?;
    i64::try_from(ten_thousandths)
        .ok()
        .map(Money::from_ten_thousandths)
        .ok_or(FeeError::Overflow)
}

/// Why a fee could not be worked out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeeError {
    /// The skill level is above 5.
    SkillLevel(u8),
    /// The standing is not a number in -10.00 to 10.00 with at most two decimals.
    Standing,
    /// An order value or sale is below zero.
    Negative,
    /// The result does not fit.
    Overflow,
}

impl fmt::Display for FeeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FeeError::SkillLevel(level) => write!(f, "skill level {level} is above 5"),
            FeeError::Standing => write!(f, "standing must be between -10.00 and 10.00"),
            FeeError::Negative => write!(f, "amount is negative"),
            FeeError::Overflow => write!(f, "fee does not fit"),
        }
    }
}

impl std::error::Error for FeeError {}

#[cfg(test)]
mod tests {
    use super::{BrokerCharge, BrokerFee, FeeError, SellFees, Standing};
    use crate::money::{Money, Rate};

    fn isk(text: &str) -> Money {
        Money::parse_isk(text).unwrap()
    }

    fn ppm(value: u32) -> Rate {
        Rate::from_ppm(value)
    }

    fn standing(text: &str) -> Standing {
        Standing::parse(text).unwrap()
    }

    #[test]
    fn sales_tax_rate_falls_with_accounting() {
        let expected = [75_000, 66_750, 58_500, 50_250, 42_000, 33_750];
        for (level, value) in expected.into_iter().enumerate() {
            let level = u8::try_from(level).unwrap();
            assert_eq!(
                super::sales_tax_rate(level),
                Ok(ppm(value)),
                "level {level}"
            );
        }
        assert_eq!(super::sales_tax_rate(6), Err(FeeError::SkillLevel(6)));
    }

    #[test]
    fn sales_tax_rounds_to_ten_thousandths() {
        assert_eq!(
            super::sales_tax(isk("749,684.00"), ppm(42_000)),
            Ok(Money::from_ten_thousandths(314_867_280))
        );
        assert_eq!(
            super::sales_tax(isk("2,059.79"), ppm(75_000)),
            Ok(Money::from_ten_thousandths(1_544_843))
        );
        assert_eq!(super::sales_tax(isk("0.00"), ppm(75_000)), Ok(Money::ZERO));
        assert_eq!(
            super::sales_tax(isk("-1.00"), ppm(75_000)),
            Err(FeeError::Negative)
        );
    }

    #[test]
    fn npc_broker_rate_takes_skills_and_standings() {
        let zero = Standing::default();
        let cases = [
            (0, zero, zero, 30_000),
            (2, zero, zero, 24_000),
            (5, standing("10.00"), standing("10.00"), 10_000),
            (4, standing("2.50"), standing("-1.25"), 17_500),
            (5, standing("5.23"), zero, 13_431),
            (0, standing("-10.00"), standing("-10.00"), 35_000),
        ];
        for (level, faction, corporation, expected) in cases {
            assert_eq!(
                super::npc_broker_rate(level, faction, corporation),
                Ok(ppm(expected)),
                "level {level}"
            );
        }
        assert_eq!(
            super::npc_broker_rate(6, Standing::default(), Standing::default()),
            Err(FeeError::SkillLevel(6))
        );
    }

    #[test]
    fn standing_parses_typed_values() {
        assert_eq!(
            Standing::parse("5.23"),
            Ok(Standing::from_hundredths(523).unwrap())
        );
        assert_eq!(
            Standing::parse("-1.25"),
            Ok(Standing::from_hundredths(-125).unwrap())
        );
        assert_eq!(
            Standing::parse("10"),
            Ok(Standing::from_hundredths(1_000).unwrap())
        );
        assert_eq!(
            Standing::parse("-10.00"),
            Ok(Standing::from_hundredths(-1_000).unwrap())
        );
        assert_eq!(
            Standing::parse("0.5"),
            Ok(Standing::from_hundredths(50).unwrap())
        );
    }

    #[test]
    fn standing_rejects_what_is_not_a_standing() {
        for text in [
            "10.01", "1.234", "+1", "1,5", "", "abc", "-", ".5", "10.", "11", "-10.01",
        ] {
            assert_eq!(Standing::parse(text), Err(FeeError::Standing), "{text}");
        }
        assert_eq!(Standing::from_hundredths(1_001), Err(FeeError::Standing));
        assert_eq!(
            Standing::from_hundredths(-1_000),
            Ok(Standing::parse("-10").unwrap())
        );
    }

    #[test]
    fn npc_broker_fee_has_a_minimum_and_rounds_to_cents() {
        let npc = |ppm_value| BrokerFee::Npc(ppm(ppm_value));
        let charge = |value: &str, fee| super::broker_charge(isk(value), fee);
        assert_eq!(
            charge("1,234,567.00", npc(15_000)),
            Ok(BrokerCharge {
                broker: isk("18,518.51"),
                scc: Money::ZERO
            })
        );
        assert_eq!(
            charge("2,000,000.00", npc(30_000)),
            Ok(BrokerCharge {
                broker: isk("60,000.00"),
                scc: Money::ZERO
            })
        );
        assert_eq!(
            charge("2,000.00", npc(30_000)),
            Ok(BrokerCharge {
                broker: isk("100.00"),
                scc: Money::ZERO
            })
        );
        assert_eq!(
            charge("6,000.00", npc(15_000)),
            Ok(BrokerCharge {
                broker: isk("100.00"),
                scc: Money::ZERO
            })
        );
        assert_eq!(
            charge("7,000.00", npc(15_000)),
            Ok(BrokerCharge {
                broker: isk("105.00"),
                scc: Money::ZERO
            })
        );
        assert_eq!(
            charge("10,000,000.01", npc(13_431)),
            Ok(BrokerCharge {
                broker: isk("134,310.00"),
                scc: Money::ZERO
            })
        );
    }

    #[test]
    fn structure_fee_adds_scc_without_minimum() {
        let owner = |ppm_value| BrokerFee::structure(ppm(ppm_value));
        assert_eq!(
            super::broker_charge(isk("74,000,000.00"), owner(15_000)),
            Ok(BrokerCharge {
                broker: isk("1,110,000.00"),
                scc: isk("370,000.00")
            })
        );
        assert_eq!(
            super::broker_charge(isk("74,000,000.00"), owner(15_000))
                .unwrap()
                .total(),
            Some(isk("1,480,000.00"))
        );
        assert_eq!(
            super::broker_charge(isk("150,000,000.00"), owner(0)),
            Ok(BrokerCharge {
                broker: isk("100.00"),
                scc: isk("750,000.00")
            })
        );
        assert_eq!(
            super::broker_charge(isk("1,000.00"), owner(10_000)),
            Ok(BrokerCharge {
                broker: isk("100.00"),
                scc: isk("5.00")
            })
        );
        assert_eq!(
            super::broker_charge(isk("1,234,567.00"), owner(0)).map(|charge| charge.scc),
            Ok(isk("6,172.84"))
        );
        assert_eq!(
            super::broker_charge(isk("1,234.57"), owner(0)).map(|charge| charge.scc),
            Ok(isk("6.17"))
        );
    }

    #[test]
    fn sell_order_takes_broker_scc_and_tax() {
        assert_eq!(
            super::sell_order(isk("749,684.00"), BrokerFee::Npc(ppm(18_000)), ppm(42_000)),
            Ok(SellFees {
                broker: isk("13,494.31"),
                scc: Money::ZERO,
                tax: Money::from_ten_thousandths(314_867_280),
                net: Money::from_ten_thousandths(7_047_029_620),
            })
        );
        assert_eq!(
            super::sell_order(isk("50.00"), BrokerFee::Npc(ppm(30_000)), ppm(75_000)),
            Ok(SellFees {
                broker: isk("100.00"),
                scc: Money::ZERO,
                tax: isk("3.75"),
                net: isk("-53.75"),
            })
        );
    }

    #[test]
    fn instant_sell_pays_tax_only() {
        assert_eq!(
            super::instant_sell(isk("749,684.00"), ppm(42_000)),
            Ok(SellFees {
                broker: Money::ZERO,
                scc: Money::ZERO,
                tax: Money::from_ten_thousandths(314_867_280),
                net: isk("718,197.272"),
            })
        );
    }

    #[test]
    fn buy_order_pays_broker_only() {
        assert_eq!(
            super::buy_order(isk("2,000,000.00"), BrokerFee::Npc(ppm(30_000))),
            Ok(BrokerCharge {
                broker: isk("60,000.00"),
                scc: Money::ZERO
            })
        );
    }

    #[test]
    fn zero_order_pays_the_minimum_broker_fee() {
        assert_eq!(
            super::broker_charge(isk("0.00"), BrokerFee::Npc(ppm(30_000))),
            Ok(BrokerCharge {
                broker: isk("100.00"),
                scc: Money::ZERO
            })
        );
    }

    #[test]
    fn errors_say_what_is_wrong() {
        assert_eq!(
            FeeError::SkillLevel(6).to_string(),
            "skill level 6 is above 5"
        );
        assert_eq!(
            FeeError::Standing.to_string(),
            "standing must be between -10.00 and 10.00"
        );
        assert_eq!(FeeError::Negative.to_string(), "amount is negative");
        assert_eq!(FeeError::Overflow.to_string(), "fee does not fit");
    }

    #[test]
    fn negative_orders_are_errors() {
        assert_eq!(
            super::broker_charge(isk("-0.01"), BrokerFee::Npc(ppm(30_000))),
            Err(FeeError::Negative)
        );
        assert_eq!(
            super::sell_order(isk("-1.00"), BrokerFee::Npc(ppm(30_000)), ppm(75_000)),
            Err(FeeError::Negative)
        );
        assert_eq!(
            super::instant_sell(isk("-1.00"), ppm(75_000)),
            Err(FeeError::Negative)
        );
    }

    #[test]
    fn largest_values_round_and_overflow_without_wrapping() {
        let max = Money::from_ten_thousandths(i64::MAX);
        let max_cents = Money::from_ten_thousandths(9_223_372_036_854_775_800);
        assert_eq!(
            super::broker_charge(max, BrokerFee::Npc(ppm(1_000_000))),
            Ok(BrokerCharge {
                broker: max_cents,
                scc: Money::ZERO
            })
        );
        let full_structure = BrokerFee::Structure {
            owner: ppm(1_000_000),
            scc: ppm(1_000_000),
        };
        let charge = super::broker_charge(max, full_structure).unwrap();
        assert_eq!(
            charge,
            BrokerCharge {
                broker: max_cents,
                scc: max_cents
            }
        );
        assert_eq!(charge.total(), None);
        assert_eq!(
            super::sell_order(max, full_structure, ppm(1_000_000)),
            Err(FeeError::Overflow)
        );
    }
}

//! The buy list of a plan: each line's volume and value, the totals, and the delivered cost.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::float_arithmetic,
    clippy::as_conversions,
    clippy::integer_division
)]

use crate::freight::{CargoLine, FreightQuote};
use crate::industry::TypeId;
use crate::money::Money;
use crate::volume::Volume;

/// What the market charges for a line, as the app maps it from a market quote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinePrice {
    /// Enough is on the market for the whole line.
    Priced {
        total: Money,
        per_unit: Money,
        priced_at: i64,
    },
    /// Only part of the line is on the market.
    Thin {
        available: u64,
        total_for_available: Money,
        priced_at: i64,
    },
    Missing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuyInput {
    pub type_id: TypeId,
    pub name: String,
    pub needed: u64,
    pub from_stock: u64,
    pub to_buy: u64,
    pub unit_volume: Option<Volume>,
    pub price: LinePrice,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuyRow {
    pub type_id: TypeId,
    pub name: String,
    pub needed: u64,
    pub from_stock: u64,
    pub to_buy: u64,
    pub unit_volume: Option<Volume>,
    pub price: LinePrice,
    /// `None` when the unit volume is unknown.
    pub volume: Option<Volume>,
    /// `None` unless the line is priced.
    pub value: Option<Money>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuyList {
    pub rows: Vec<BuyRow>,
    pub volume: Volume,
    pub value: Money,
    /// Lines still to buy whose value or volume is unknown.
    pub incomplete: Vec<TypeId>,
    /// The oldest price among the lines still to buy.
    pub oldest_priced_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delivered {
    pub value: Money,
    pub collateral: Money,
    pub freight: Money,
    pub total: Money,
    pub incomplete: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuyListError {
    Overflow,
}

/// Builds the buy list, adding up the volumes and values that are known.
pub fn buy_list(inputs: Vec<BuyInput>) -> Result<BuyList, BuyListError> {
    let rows = inputs.into_iter().map(row).collect::<Result<Vec<_>, _>>()?;
    let mut volume = Volume::default();
    let mut value = Money::ZERO;
    for row in &rows {
        if let Some(known) = row.volume {
            volume = volume.checked_add(known).ok_or(BuyListError::Overflow)?;
        }
        if let Some(known) = row.value {
            value = value.checked_add(known).ok_or(BuyListError::Overflow)?;
        }
    }
    let incomplete: Vec<TypeId> = rows
        .iter()
        .filter(|row| row.to_buy > 0 && (row.volume.is_none() || row.value.is_none()))
        .map(|row| row.type_id)
        .collect();
    let oldest_priced_at = rows
        .iter()
        .filter(|row| row.to_buy > 0)
        .filter_map(|row| priced_at(&row.price))
        .min();
    Ok(BuyList {
        rows,
        volume,
        value,
        incomplete,
        oldest_priced_at,
    })
}

fn row(input: BuyInput) -> Result<BuyRow, BuyListError> {
    let volume = if input.to_buy == 0 {
        Some(Volume::default())
    } else {
        input
            .unit_volume
            .map(|unit| {
                unit.checked_mul_qty(input.to_buy)
                    .ok_or(BuyListError::Overflow)
            })
            .transpose()?
    };
    let value = if input.to_buy == 0 {
        Some(Money::ZERO)
    } else {
        priced_total(&input.price)
    };
    let BuyInput {
        type_id,
        name,
        needed,
        from_stock,
        to_buy,
        unit_volume,
        price,
    } = input;
    Ok(BuyRow {
        type_id,
        name,
        needed,
        from_stock,
        to_buy,
        unit_volume,
        price,
        volume,
        value,
    })
}

fn priced_total(price: &LinePrice) -> Option<Money> {
    match price {
        LinePrice::Priced { total, .. } => Some(*total),
        LinePrice::Thin { .. } | LinePrice::Missing => None,
    }
}

fn priced_at(price: &LinePrice) -> Option<i64> {
    match price {
        LinePrice::Priced { priced_at, .. } | LinePrice::Thin { priced_at, .. } => Some(*priced_at),
        LinePrice::Missing => None,
    }
}

impl BuyList {
    /// The lines to carry: bought, with a known volume and a full price.
    pub fn cargo(&self) -> Vec<CargoLine> {
        self.rows
            .iter()
            .filter_map(|row| {
                let LinePrice::Priced { per_unit, .. } = row.price else {
                    return None;
                };
                Some(CargoLine {
                    type_id: row.type_id,
                    units: row.to_buy,
                    unit_volume: row.unit_volume?,
                    unit_value: per_unit,
                })
            })
            .filter(|line| line.units > 0)
            .collect()
    }

    /// The value of the lines plus the freight. Without a freight quote the freight is 0 and the
    /// result is incomplete when there is cargo to carry.
    pub fn delivered(&self, freight: Option<&FreightQuote>) -> Result<Delivered, BuyListError> {
        let collateral = match freight {
            Some(quote) => quote
                .contracts
                .iter()
                .try_fold(Money::ZERO, |sum, contract| {
                    sum.checked_add(contract.value)
                        .ok_or(BuyListError::Overflow)
                })?,
            None => Money::ZERO,
        };
        let freight_cost = freight.map_or(Money::ZERO, |quote| quote.total);
        let total = self
            .value
            .checked_add(freight_cost)
            .ok_or(BuyListError::Overflow)?;
        let incomplete =
            !self.incomplete.is_empty() || (freight.is_none() && !self.cargo().is_empty());
        Ok(Delivered {
            value: self.value,
            collateral,
            freight: freight_cost,
            total,
            incomplete,
        })
    }

    /// One `name<TAB>quantity` line per line to buy, in row order.
    pub fn multibuy_text(&self) -> String {
        self.rows
            .iter()
            .filter(|row| row.to_buy > 0)
            .map(|row| format!("{}\t{}", row.name, row.to_buy))
            .collect::<Vec<String>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::{BuyInput, BuyList, LinePrice, buy_list};
    use crate::freight::{CargoLine, Packing, RateCard, quote};
    use crate::money::{Money, Rate};
    use crate::volume::Volume;

    fn isk(text: &str) -> Money {
        Money::parse_isk(text).unwrap()
    }

    fn m3(text: &str) -> Volume {
        Volume::parse_decimal(text).unwrap()
    }

    fn input(
        type_id: u32,
        name: &str,
        needed: u64,
        from_stock: u64,
        to_buy: u64,
        unit_volume: Option<&str>,
        price: LinePrice,
    ) -> BuyInput {
        BuyInput {
            type_id,
            name: name.to_string(),
            needed,
            from_stock,
            to_buy,
            unit_volume: unit_volume.map(m3),
            price,
        }
    }

    fn priced(total: &str, per_unit: &str, priced_at: i64) -> LinePrice {
        LinePrice::Priced {
            total: isk(total),
            per_unit: isk(per_unit),
            priced_at,
        }
    }

    fn five_row_list() -> BuyList {
        buy_list(vec![
            input(
                34,
                "Tritanium",
                4307,
                1000,
                3307,
                Some("0.01"),
                priced("14054.75", "4.25", 1000),
            ),
            input(
                11481,
                "R.A.M.- Robotics",
                10,
                0,
                10,
                Some("0.04"),
                priced("4786.00", "478.60", 900),
            ),
            input(
                2454,
                "Hobgoblin I",
                10,
                10,
                0,
                Some("5"),
                LinePrice::Missing,
            ),
            input(
                9834,
                "Guidance Systems",
                10,
                0,
                10,
                Some("3"),
                LinePrice::Thin {
                    available: 4,
                    total_for_available: isk("2000000"),
                    priced_at: 950,
                },
            ),
            input(
                16682,
                "Hypersynaptic Fibers",
                6,
                0,
                6,
                None,
                LinePrice::Missing,
            ),
        ])
        .unwrap()
    }

    fn two_priced_rows() -> BuyList {
        buy_list(vec![
            input(
                34,
                "Tritanium",
                4307,
                1000,
                3307,
                Some("0.01"),
                priced("14054.75", "4.25", 1000),
            ),
            input(
                11481,
                "R.A.M.- Robotics",
                10,
                0,
                10,
                Some("0.04"),
                priced("4786.00", "478.60", 900),
            ),
        ])
        .unwrap()
    }

    fn card_850_half_percent() -> RateCard {
        RateCard {
            per_m3: Some(isk("850")),
            collateral: Some(Rate::parse_percent("0.5").unwrap()),
            ..RateCard::default()
        }
    }

    #[test]
    fn rows_show_volume_and_value_where_known() {
        let list = five_row_list();
        let rows = &list.rows;
        assert_eq!(rows[0].volume, Some(m3("33.07")));
        assert_eq!(rows[1].volume, Some(m3("0.40")));
        assert_eq!(rows[2].volume, Some(m3("0")));
        assert_eq!(rows[3].volume, Some(m3("30.00")));
        assert_eq!(rows[4].volume, None);
        assert_eq!(rows[0].value, Some(isk("14054.75")));
        assert_eq!(rows[1].value, Some(isk("4786")));
        assert_eq!(rows[2].value, Some(isk("0")));
        assert_eq!(rows[3].value, None);
        assert_eq!(rows[4].value, None);
    }

    #[test]
    fn totals_add_only_the_known_values() {
        let list = five_row_list();
        assert_eq!(list.volume, m3("63.47"));
        assert_eq!(list.value, isk("18840.75"));
        assert_eq!(list.incomplete, vec![9834, 16682]);
        assert_eq!(list.oldest_priced_at, Some(900));
    }

    #[test]
    fn a_line_with_nothing_to_buy_is_never_incomplete() {
        let list = buy_list(vec![input(
            2454,
            "Hobgoblin I",
            10,
            10,
            0,
            None,
            LinePrice::Missing,
        )])
        .unwrap();
        assert_eq!(list.rows[0].volume, Some(m3("0")));
        assert_eq!(list.rows[0].value, Some(isk("0")));
        assert!(list.incomplete.is_empty());
        assert_eq!(list.oldest_priced_at, None);
        assert_eq!(list.multibuy_text(), "");
    }

    #[test]
    fn a_line_with_nothing_to_buy_is_not_priced_or_carried() {
        let list = buy_list(vec![input(
            34,
            "Tritanium",
            100,
            100,
            0,
            Some("0.01"),
            priced("100", "1", 1),
        )])
        .unwrap();
        assert_eq!(list.oldest_priced_at, None);
        assert!(list.cargo().is_empty());
        assert_eq!(list.multibuy_text(), "");
    }

    #[test]
    fn cargo_carries_only_fully_priced_lines_to_buy() {
        let list = five_row_list();
        assert_eq!(
            list.cargo(),
            vec![
                CargoLine {
                    type_id: 34,
                    units: 3307,
                    unit_volume: m3("0.01"),
                    unit_value: isk("4.25"),
                },
                CargoLine {
                    type_id: 11481,
                    units: 10,
                    unit_volume: m3("0.04"),
                    unit_value: isk("478.60"),
                },
            ]
        );
    }

    #[test]
    fn delivered_adds_the_freight_and_flags_missing_prices() {
        let list = five_row_list();
        let freight = quote(&card_850_half_percent(), 0, &list.cargo(), Packing::TopUp).unwrap();
        assert_eq!(freight.total, isk("28543.7038"));
        let delivered = list.delivered(Some(&freight)).unwrap();
        assert_eq!(delivered.value, isk("18840.75"));
        assert_eq!(delivered.collateral, isk("18840.75"));
        assert_eq!(delivered.freight, isk("28543.7038"));
        assert_eq!(delivered.total, isk("47384.4538"));
        assert!(delivered.incomplete);
    }

    #[test]
    fn delivered_without_a_freight_quote_has_no_freight() {
        let list = five_row_list();
        let delivered = list.delivered(None).unwrap();
        assert_eq!(delivered.freight, isk("0"));
        assert_eq!(delivered.collateral, isk("0"));
        assert_eq!(delivered.total, isk("18840.75"));
        assert!(delivered.incomplete);
    }

    #[test]
    fn a_complete_list_is_complete_only_with_a_freight_quote() {
        let list = two_priced_rows();
        assert!(list.incomplete.is_empty());
        let freight = quote(&card_850_half_percent(), 0, &list.cargo(), Packing::TopUp).unwrap();
        let delivered = list.delivered(Some(&freight)).unwrap();
        assert!(!delivered.incomplete);
        assert_eq!(delivered.total, isk("47384.4538"));
        assert!(list.delivered(None).unwrap().incomplete);
    }

    #[test]
    fn multibuy_text_lists_the_lines_to_buy_in_row_order() {
        assert_eq!(
            five_row_list().multibuy_text(),
            "Tritanium\t3307\nR.A.M.- Robotics\t10\nGuidance Systems\t10\nHypersynaptic Fibers\t6"
        );
    }
}

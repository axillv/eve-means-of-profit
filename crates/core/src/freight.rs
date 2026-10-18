//! Freight: what a hauling service charges to carry a cargo, packed into contracts.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::float_arithmetic,
    clippy::as_conversions,
    clippy::integer_division
)]

use std::fmt;

use crate::industry::TypeId;
use crate::money::{Money, Rate};
use crate::volume::Volume;

/// What a freight service charges. Every part is optional: `None` means "not charged" or "no limit".
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RateCard {
    /// Charge per m³ carried.
    pub per_m3: Option<Money>,
    /// Charge per jump.
    pub per_jump: Option<Money>,
    /// Share of the contract's value that is charged.
    pub collateral: Option<Rate>,
    /// Least a contract is charged.
    pub minimum: Option<Money>,
    /// Largest volume of one contract.
    pub max_m3: Option<Volume>,
    /// Largest value of one contract.
    pub max_collateral: Option<Money>,
}

/// A stack of one item: `units` of it, each with a volume and a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CargoLine {
    pub type_id: TypeId,
    pub units: u64,
    pub unit_volume: Volume,
    pub unit_value: Money,
}

/// How units are packed. `FillOnly` fills contracts in order. `TopUp` also moves units into the
/// last contract so that it reaches the minimum charge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Packing {
    FillOnly,
    #[default]
    TopUp,
}

/// One contract: the pieces it carries and what it costs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contract {
    pub pieces: Vec<(TypeId, u64)>,
    pub volume: Volume,
    pub value: Money,
    pub charge: Money,
    pub pays_minimum: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FreightQuote {
    pub contracts: Vec<Contract>,
    pub total: Money,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FreightError {
    CannotCarry(TypeId),
    NegativeRate,
    Overflow,
}

impl fmt::Display for FreightError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FreightError::CannotCarry(type_id) => write!(
                f,
                "item {type_id} cannot be carried: too big, too valuable, or a negative size or value"
            ),
            FreightError::NegativeRate => {
                write!(f, "a price or limit on the rate card is negative")
            }
            FreightError::Overflow => write!(f, "the freight amount is too large to count"),
        }
    }
}

impl std::error::Error for FreightError {}

/// Packs `cargo` into contracts and prices each one.
pub fn quote(
    card: &RateCard,
    jumps: u32,
    cargo: &[CargoLine],
    packing: Packing,
) -> Result<FreightQuote, FreightError> {
    check_rate_card(card)?;
    let lines = packable(card, cargo)?;
    let mut loads = fill(card, &lines)?;
    if packing == Packing::TopUp {
        top_up(card, jumps, &mut loads)?;
    }
    let contracts = loads
        .iter()
        .map(|load| build_contract(card, jumps, load))
        .collect::<Result<Vec<_>, _>>()?;
    let total = contracts.iter().try_fold(Money::ZERO, |sum, contract| {
        sum.checked_add(contract.charge)
            .ok_or(FreightError::Overflow)
    })?;
    Ok(FreightQuote { contracts, total })
}

/// The index of the cheapest quote; the lower index wins a tie. `None` when no quote is `Ok`.
pub fn cheapest(quotes: &[Result<FreightQuote, FreightError>]) -> Option<usize> {
    let mut best: Option<(usize, Money)> = None;
    for (index, quote) in quotes.iter().enumerate() {
        let Ok(quote) = quote else { continue };
        if best.is_none_or(|(_, cheapest_so_far)| quote.total < cheapest_so_far) {
            best = Some((index, quote.total));
        }
    }
    best.map(|(index, _)| index)
}

fn check_rate_card(card: &RateCard) -> Result<(), FreightError> {
    let negative_money = [
        card.per_m3,
        card.per_jump,
        card.minimum,
        card.max_collateral,
    ]
    .into_iter()
    .flatten()
    .any(|amount| amount < Money::ZERO);
    let negative_volume = card.max_m3.is_some_and(|max| max < Volume::default());
    if negative_money || negative_volume {
        return Err(FreightError::NegativeRate);
    }
    Ok(())
}

/// The lines with units to carry, biggest first. A line that cannot fit one contract is an error.
fn packable(card: &RateCard, cargo: &[CargoLine]) -> Result<Vec<CargoLine>, FreightError> {
    let mut lines = Vec::new();
    for line in cargo.iter().filter(|line| line.units > 0) {
        let too_big = card.max_m3.is_some_and(|max| line.unit_volume > max);
        let too_valuable = card.max_collateral.is_some_and(|max| line.unit_value > max);
        let negative = line.unit_volume < Volume::default() || line.unit_value < Money::ZERO;
        if too_big || too_valuable || negative {
            return Err(FreightError::CannotCarry(line.type_id));
        }
        lines.push(*line);
    }
    lines.sort_by(|a, b| {
        b.unit_volume
            .cmp(&a.unit_volume)
            .then(b.unit_value.cmp(&a.unit_value))
            .then(a.type_id.cmp(&b.type_id))
    });
    Ok(lines)
}

/// The contents of one contract being packed. Its totals always match its pieces.
#[derive(Debug, Clone, Default)]
struct Load {
    pieces: Vec<CargoLine>,
    volume: Volume,
    value: Money,
}

impl Load {
    /// Adds `units` of the item of `unit`; a piece of the same type joins the first one.
    fn add_units(&mut self, unit: &CargoLine, units: u64) -> Result<(), FreightError> {
        if units == 0 {
            return Ok(());
        }
        match self
            .pieces
            .iter_mut()
            .find(|piece| piece.type_id == unit.type_id)
        {
            Some(piece) => {
                piece.units = piece
                    .units
                    .checked_add(units)
                    .ok_or(FreightError::Overflow)?;
            }
            None => self.pieces.push(CargoLine { units, ..*unit }),
        }
        self.recount()
    }

    /// Takes `units` out of the piece at `index`, dropping the piece when it is empty.
    fn remove_units(&mut self, index: usize, units: u64) -> Result<(), FreightError> {
        if let Some(piece) = self.pieces.get_mut(index) {
            piece.units = piece
                .units
                .checked_sub(units)
                .ok_or(FreightError::Overflow)?;
        }
        self.pieces.retain(|piece| piece.units > 0);
        self.recount()
    }

    fn recount(&mut self) -> Result<(), FreightError> {
        let mut volume = Volume::default();
        let mut value = Money::ZERO;
        for piece in &self.pieces {
            let piece_volume = piece
                .unit_volume
                .checked_mul_qty(piece.units)
                .ok_or(FreightError::Overflow)?;
            let piece_value = piece
                .unit_value
                .checked_mul_qty(piece.units)
                .ok_or(FreightError::Overflow)?;
            volume = volume
                .checked_add(piece_volume)
                .ok_or(FreightError::Overflow)?;
            value = value
                .checked_add(piece_value)
                .ok_or(FreightError::Overflow)?;
        }
        self.volume = volume;
        self.value = value;
        Ok(())
    }
}

/// Step one: put each line into the open contracts in order, then into new ones.
fn fill(card: &RateCard, lines: &[CargoLine]) -> Result<Vec<Load>, FreightError> {
    let mut loads: Vec<Load> = Vec::new();
    for line in lines {
        let mut left = line.units;
        for load in &mut loads {
            if left == 0 {
                break;
            }
            let units = fit_count(card, load, line)?.min(left);
            load.add_units(line, units)?;
            left = left.checked_sub(units).ok_or(FreightError::Overflow)?;
        }
        while left > 0 {
            let mut load = Load::default();
            let units = fit_count(card, &load, line)?.min(left);
            if units == 0 {
                return Err(FreightError::CannotCarry(line.type_id));
            }
            load.add_units(line, units)?;
            left = left.checked_sub(units).ok_or(FreightError::Overflow)?;
            loads.push(load);
        }
    }
    Ok(loads)
}

/// How many more units of `line` fit in `load` under the card's volume and value limits.
fn fit_count(card: &RateCard, load: &Load, line: &CargoLine) -> Result<u64, FreightError> {
    let by_volume = match card.max_m3 {
        Some(max) => fit_under(
            max.ten_thousandths(),
            load.volume.ten_thousandths(),
            line.unit_volume.ten_thousandths(),
        )?,
        None => u64::MAX,
    };
    let by_value = match card.max_collateral {
        Some(max) => fit_under(
            max.ten_thousandths(),
            load.value.ten_thousandths(),
            line.unit_value.ten_thousandths(),
        )?,
        None => u64::MAX,
    };
    Ok(by_volume.min(by_value))
}

/// Whole units of size `per_unit` that fit in `limit - used`. A unit of no size never runs out.
fn fit_under(limit: i64, used: i64, per_unit: i64) -> Result<u64, FreightError> {
    if per_unit == 0 {
        return Ok(u64::MAX);
    }
    let room = limit.checked_sub(used).ok_or(FreightError::Overflow)?;
    let count = room.checked_div(per_unit).ok_or(FreightError::Overflow)?;
    u64::try_from(count).ok().ok_or(FreightError::Overflow)
}

/// Step two, only with a minimum: fills the last contract from the others until it pays the minimum.
fn top_up(card: &RateCard, jumps: u32, loads: &mut Vec<Load>) -> Result<(), FreightError> {
    let Some(minimum) = card.minimum else {
        return Ok(());
    };
    if loads.len() < 2 {
        return Ok(());
    }
    let Some(mut last) = loads.pop() else {
        return Ok(());
    };
    let moved = move_into_last(card, jumps, minimum, loads, &mut last);
    loads.push(last);
    moved
}

/// Takes units from the donors, from the second-to-last contract back to the first, and from each
/// donor's pieces from the last to the first, until `last` reaches the minimum.
fn move_into_last(
    card: &RateCard,
    jumps: u32,
    minimum: Money,
    donors: &mut [Load],
    last: &mut Load,
) -> Result<(), FreightError> {
    for donor in donors.iter_mut().rev() {
        for index in (0..donor.pieces.len()).rev() {
            if linear_charge(card, jumps, last.volume, last.value)? >= minimum {
                return Ok(());
            }
            move_piece(card, jumps, minimum, donor, last, index)?;
        }
    }
    Ok(())
}

fn move_piece(
    card: &RateCard,
    jumps: u32,
    minimum: Money,
    donor: &mut Load,
    last: &mut Load,
    index: usize,
) -> Result<(), FreightError> {
    let Some(piece) = donor.pieces.get(index).copied() else {
        return Ok(());
    };
    let units = units_to_move(card, jumps, minimum, donor, last, &piece)?;
    if units == 0 {
        return Ok(());
    }
    donor.remove_units(index, units)?;
    last.add_units(&piece, units)
}

/// The units of `piece` to move from `donor` to `last`: the fewest that bring `last` to the
/// minimum, capped by the limits of `last` and by the donor staying at or above the minimum.
fn units_to_move(
    card: &RateCard,
    jumps: u32,
    minimum: Money,
    donor: &Load,
    last: &Load,
    piece: &CargoLine,
) -> Result<u64, FreightError> {
    let holds = |count: u64| -> Result<bool, FreightError> {
        Ok(count <= fit_count(card, last, piece)?
            && donor_after(card, jumps, donor, piece, count)? >= minimum)
    };
    let cap = largest_holding(piece.units, holds)?;
    units_to_reach(card, jumps, minimum, last, piece, cap)
}

/// The linear charge of `donor` once `count` units of `piece` have left it.
fn donor_after(
    card: &RateCard,
    jumps: u32,
    donor: &Load,
    piece: &CargoLine,
    count: u64,
) -> Result<Money, FreightError> {
    let taken_volume = piece
        .unit_volume
        .checked_mul_qty(count)
        .ok_or(FreightError::Overflow)?;
    let taken_value = piece
        .unit_value
        .checked_mul_qty(count)
        .ok_or(FreightError::Overflow)?;
    let volume = volume_less(donor.volume, taken_volume).ok_or(FreightError::Overflow)?;
    let value = donor
        .value
        .checked_sub(taken_value)
        .ok_or(FreightError::Overflow)?;
    linear_charge(card, jumps, volume, value)
}

fn volume_less(volume: Volume, taken: Volume) -> Option<Volume> {
    volume
        .ten_thousandths()
        .checked_sub(taken.ten_thousandths())
        .map(Volume::from_ten_thousandths)
}

/// The fewest units in `1..=top` that bring `last` to the minimum; `top` when none does.
fn units_to_reach(
    card: &RateCard,
    jumps: u32,
    minimum: Money,
    last: &Load,
    piece: &CargoLine,
    top: u64,
) -> Result<u64, FreightError> {
    let reaches = |count: u64| -> Result<bool, FreightError> {
        Ok(linear_with(card, jumps, last, piece, count)? >= minimum)
    };
    if top == 0 || !reaches(top)? {
        return Ok(top);
    }
    let mut low: u64 = 1;
    let mut high = top;
    while low < high {
        let mid = low.midpoint(high);
        if reaches(mid)? {
            high = mid;
        } else {
            low = mid.checked_add(1).ok_or(FreightError::Overflow)?;
        }
    }
    Ok(low)
}

/// The linear charge of `load` once `count` more units of `piece` have joined it.
fn linear_with(
    card: &RateCard,
    jumps: u32,
    load: &Load,
    piece: &CargoLine,
    count: u64,
) -> Result<Money, FreightError> {
    let added_volume = piece
        .unit_volume
        .checked_mul_qty(count)
        .ok_or(FreightError::Overflow)?;
    let added_value = piece
        .unit_value
        .checked_mul_qty(count)
        .ok_or(FreightError::Overflow)?;
    let volume = load
        .volume
        .checked_add(added_volume)
        .ok_or(FreightError::Overflow)?;
    let value = load
        .value
        .checked_add(added_value)
        .ok_or(FreightError::Overflow)?;
    linear_charge(card, jumps, volume, value)
}

/// The largest count in `0..=high` for which `holds` is true. `holds` is true for a run of counts
/// starting at 0; when it is false at 0 the answer is 0.
fn largest_holding(
    high: u64,
    holds: impl Fn(u64) -> Result<bool, FreightError>,
) -> Result<u64, FreightError> {
    if !holds(0)? {
        return Ok(0);
    }
    let (mut low, mut end) = (0_u64, high);
    while low < end {
        let mid = low
            .midpoint(end)
            .checked_add(1)
            .ok_or(FreightError::Overflow)?;
        if holds(mid)? {
            low = mid;
        } else {
            end = mid.checked_sub(1).ok_or(FreightError::Overflow)?;
        }
    }
    Ok(low)
}

fn build_contract(card: &RateCard, jumps: u32, load: &Load) -> Result<Contract, FreightError> {
    let linear = linear_charge(card, jumps, load.volume, load.value)?;
    let (charge, pays_minimum) = charge_for(card, linear);
    Ok(Contract {
        pieces: load
            .pieces
            .iter()
            .map(|piece| (piece.type_id, piece.units))
            .collect(),
        volume: load.volume,
        value: load.value,
        charge,
        pays_minimum,
    })
}

/// The charge before the minimum: per m³, per jump and collateral, each part rounded on its own.
fn linear_charge(
    card: &RateCard,
    jumps: u32,
    volume: Volume,
    value: Money,
) -> Result<Money, FreightError> {
    let parts = [
        card.per_m3.map(|price| price.checked_mul_volume(volume)),
        card.per_jump
            .map(|fee| fee.checked_mul_qty(u64::from(jumps))),
        card.collateral.map(|rate| value.checked_mul_rate(rate)),
    ];
    parts
        .into_iter()
        .flatten()
        .try_fold(Money::ZERO, |sum, part| {
            let part = part.ok_or(FreightError::Overflow)?;
            sum.checked_add(part).ok_or(FreightError::Overflow)
        })
}

fn charge_for(card: &RateCard, linear: Money) -> (Money, bool) {
    match card.minimum {
        Some(minimum) if minimum > linear => (minimum, true),
        Some(_) | None => (linear, false),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CargoLine, Contract, FreightError, FreightQuote, Packing, RateCard, cheapest, quote,
    };
    use crate::industry::TypeId;
    use crate::money::{Money, Rate};
    use crate::volume::Volume;

    fn isk(text: &str) -> Money {
        Money::parse_isk(text).unwrap()
    }

    fn m3(text: &str) -> Volume {
        Volume::parse_decimal(text).unwrap()
    }

    fn rate(text: &str) -> Rate {
        Rate::parse_percent(text).unwrap()
    }

    fn line(type_id: TypeId, units: u64, unit_volume: &str, unit_value: &str) -> CargoLine {
        CargoLine {
            type_id,
            units,
            unit_volume: m3(unit_volume),
            unit_value: isk(unit_value),
        }
    }

    fn contract_of(
        pieces: &[(TypeId, u64)],
        volume: &str,
        value: &str,
        charge: &str,
        pays_minimum: bool,
    ) -> Contract {
        Contract {
            pieces: pieces.to_vec(),
            volume: m3(volume),
            value: isk(value),
            charge: isk(charge),
            pays_minimum,
        }
    }

    fn both_packings(
        card: &RateCard,
        jumps: u32,
        cargo: &[CargoLine],
    ) -> (FreightQuote, FreightQuote) {
        (
            quote(card, jumps, cargo, Packing::FillOnly).unwrap(),
            quote(card, jumps, cargo, Packing::TopUp).unwrap(),
        )
    }

    fn total_only(total: &str) -> FreightQuote {
        FreightQuote {
            contracts: Vec::new(),
            total: isk(total),
        }
    }

    #[test]
    fn top_up_is_the_default_packing() {
        assert_eq!(Packing::default(), Packing::TopUp);
    }

    #[test]
    fn owner_example_fills_then_tops_up() {
        let card = RateCard {
            per_m3: Some(isk("1000")),
            minimum: Some(isk("10000000")),
            max_m3: Some(m3("30000")),
            ..RateCard::default()
        };
        let cargo = [line(34, 3_200_000, "0.01", "4")];
        let (fill_only, top_up) = both_packings(&card, 0, &cargo);
        assert_eq!(
            fill_only,
            FreightQuote {
                contracts: vec![
                    contract_of(&[(34, 3_000_000)], "30000", "12000000", "30000000", false),
                    contract_of(&[(34, 200_000)], "2000", "800000", "10000000", true),
                ],
                total: isk("40000000"),
            }
        );
        assert_eq!(
            top_up,
            FreightQuote {
                contracts: vec![
                    contract_of(&[(34, 2_200_000)], "22000", "8800000", "22000000", false),
                    contract_of(&[(34, 1_000_000)], "10000", "4000000", "10000000", false),
                ],
                total: isk("32000000"),
            }
        );
    }

    #[test]
    fn donor_is_not_pushed_below_the_minimum() {
        let card = RateCard {
            per_m3: Some(isk("1000")),
            minimum: Some(isk("10000000")),
            max_m3: Some(m3("12000")),
            ..RateCard::default()
        };
        let cargo = [line(34, 1_500_000, "0.01", "4")];
        let (fill_only, top_up) = both_packings(&card, 0, &cargo);
        assert_eq!(
            fill_only,
            FreightQuote {
                contracts: vec![
                    contract_of(&[(34, 1_200_000)], "12000", "4800000", "12000000", false),
                    contract_of(&[(34, 300_000)], "3000", "1200000", "10000000", true),
                ],
                total: isk("22000000"),
            }
        );
        assert_eq!(
            top_up,
            FreightQuote {
                contracts: vec![
                    contract_of(&[(34, 1_000_000)], "10000", "4000000", "10000000", false),
                    contract_of(&[(34, 500_000)], "5000", "2000000", "10000000", true),
                ],
                total: isk("20000000"),
            }
        );
    }

    #[test]
    fn nothing_to_move_when_the_donor_would_drop_below_the_minimum() {
        let card = RateCard {
            per_m3: Some(isk("500")),
            per_jump: Some(isk("1000000")),
            minimum: Some(isk("10000000")),
            max_m3: Some(m3("10000")),
            ..RateCard::default()
        };
        let cargo = [line(34, 1_200_000, "0.01", "4")];
        let (fill_only, top_up) = both_packings(&card, 5, &cargo);
        let expected = FreightQuote {
            contracts: vec![
                contract_of(&[(34, 1_000_000)], "10000", "4000000", "10000000", false),
                contract_of(&[(34, 200_000)], "2000", "800000", "10000000", true),
            ],
            total: isk("20000000"),
        };
        assert_eq!(fill_only, expected);
        assert_eq!(top_up, expected);
    }

    #[test]
    fn biggest_lines_go_first_and_fill_the_first_contract() {
        let card = RateCard {
            per_m3: Some(isk("1000")),
            max_m3: Some(m3("1000")),
            ..RateCard::default()
        };
        let cargo = [
            line(10, 4, "300", "1000000"),
            line(20, 1000, "0.5", "100"),
            line(30, 1, "300", "2000000"),
        ];
        let (fill_only, top_up) = both_packings(&card, 0, &cargo);
        let expected = FreightQuote {
            contracts: vec![
                contract_of(
                    &[(30, 1), (10, 2), (20, 200)],
                    "1000",
                    "4020000",
                    "1000000",
                    false,
                ),
                contract_of(&[(10, 2), (20, 800)], "1000", "2080000", "1000000", false),
            ],
            total: isk("2000000"),
        };
        assert_eq!(fill_only, expected);
        assert_eq!(top_up, expected);
    }

    #[test]
    fn collateral_limit_splits_the_stack() {
        let card = RateCard {
            collateral: Some(rate("1")),
            minimum: Some(isk("5000000")),
            max_collateral: Some(isk("1000000000")),
            ..RateCard::default()
        };
        let cargo = [line(40, 5, "10", "300000000")];
        let expected = FreightQuote {
            contracts: vec![
                contract_of(&[(40, 3)], "30", "900000000", "9000000", false),
                contract_of(&[(40, 2)], "20", "600000000", "6000000", false),
            ],
            total: isk("15000000"),
        };
        let (fill_only, top_up) = both_packings(&card, 0, &cargo);
        assert_eq!(fill_only, expected);
        assert_eq!(top_up, expected);
    }

    #[test]
    fn both_limits_apply_together() {
        let card = RateCard {
            per_m3: Some(isk("1000")),
            collateral: Some(rate("1")),
            minimum: Some(isk("10000000")),
            max_m3: Some(m3("10000")),
            max_collateral: Some(isk("50000000")),
            ..RateCard::default()
        };
        let cargo = [line(1, 800, "10", "100000"), line(2, 3000, "1", "1000")];
        let expected = FreightQuote {
            contracts: vec![
                contract_of(&[(1, 500)], "5000", "50000000", "10000000", true),
                contract_of(&[(1, 300), (2, 3000)], "6000", "33000000", "10000000", true),
            ],
            total: isk("20000000"),
        };
        let (fill_only, top_up) = both_packings(&card, 0, &cargo);
        assert_eq!(fill_only, expected);
        assert_eq!(top_up, expected);
    }

    #[test]
    fn rounding_keeps_every_amount_exact_to_a_ten_thousandth() {
        let card = RateCard {
            per_m3: Some(isk("850")),
            collateral: Some(rate("0.5")),
            ..RateCard::default()
        };
        let cargo = [
            line(16, 7051, "0.01", "284.1"),
            line(17, 1, "0.0025", "0.5"),
        ];
        let expected = FreightQuote {
            contracts: vec![contract_of(
                &[(16, 7051), (17, 1)],
                "70.5125",
                "2003189.6",
                "69951.573",
                false,
            )],
            total: isk("69951.573"),
        };
        let (fill_only, top_up) = both_packings(&card, 0, &cargo);
        assert_eq!(fill_only, expected);
        assert_eq!(top_up, expected);
    }

    #[test]
    fn jumps_are_charged_per_contract() {
        let card = RateCard {
            per_jump: Some(isk("250000")),
            collateral: Some(rate("1")),
            ..RateCard::default()
        };
        let cargo = [
            line(587, 4, "2500", "500000"),
            line(34, 1_000_000, "0.01", "4.25"),
        ];
        let expected = FreightQuote {
            contracts: vec![contract_of(
                &[(587, 4), (34, 1_000_000)],
                "20000",
                "6250000",
                "8562500",
                false,
            )],
            total: isk("8562500"),
        };
        let (fill_only, top_up) = both_packings(&card, 34, &cargo);
        assert_eq!(fill_only, expected);
        assert_eq!(top_up, expected);
    }

    #[test]
    fn a_single_contract_under_the_minimum_pays_the_minimum() {
        let card = RateCard {
            per_m3: Some(isk("1000")),
            minimum: Some(isk("10000000")),
            ..RateCard::default()
        };
        let cargo = [line(34, 200_000, "0.01", "4")];
        let expected = FreightQuote {
            contracts: vec![contract_of(
                &[(34, 200_000)],
                "2000",
                "800000",
                "10000000",
                true,
            )],
            total: isk("10000000"),
        };
        let (fill_only, top_up) = both_packings(&card, 0, &cargo);
        assert_eq!(fill_only, expected);
        assert_eq!(top_up, expected);
    }

    #[test]
    fn empty_cargo_has_no_contracts_and_no_minimum() {
        let card = RateCard {
            minimum: Some(isk("10000000")),
            ..RateCard::default()
        };
        let (fill_only, top_up) = both_packings(&card, 0, &[]);
        assert_eq!(fill_only, total_only("0"));
        assert_eq!(top_up, total_only("0"));
    }

    #[test]
    fn lines_with_no_units_are_ignored() {
        let card = RateCard {
            max_m3: Some(m3("1000")),
            ..RateCard::default()
        };
        let cargo = [line(587, 0, "2500", "1")];
        assert_eq!(quote(&card, 0, &cargo, Packing::TopUp), Ok(total_only("0")));
    }

    #[test]
    fn an_item_too_big_for_one_contract_is_named() {
        let card = RateCard {
            max_m3: Some(m3("1000")),
            ..RateCard::default()
        };
        let cargo = [line(587, 1, "2500", "1")];
        assert_eq!(
            quote(&card, 0, &cargo, Packing::FillOnly),
            Err(FreightError::CannotCarry(587))
        );
    }

    #[test]
    fn an_item_too_valuable_for_one_contract_is_named() {
        let card = RateCard {
            max_collateral: Some(isk("1000000000")),
            ..RateCard::default()
        };
        let cargo = [line(40, 1, "10", "1000000001")];
        assert_eq!(
            quote(&card, 0, &cargo, Packing::FillOnly),
            Err(FreightError::CannotCarry(40))
        );
    }

    #[test]
    fn negative_rates_and_limits_are_refused() {
        let cargo = [line(34, 1, "0.01", "1")];
        let negative_price = RateCard {
            per_m3: Some(isk("-1")),
            ..RateCard::default()
        };
        assert_eq!(
            quote(&negative_price, 0, &cargo, Packing::TopUp),
            Err(FreightError::NegativeRate)
        );
        let negative_limit = RateCard {
            max_m3: Some(Volume::from_ten_thousandths(-10_000)),
            ..RateCard::default()
        };
        assert_eq!(
            quote(&negative_limit, 0, &cargo, Packing::TopUp),
            Err(FreightError::NegativeRate)
        );
    }

    #[test]
    fn negative_cargo_is_refused_as_an_item_that_cannot_be_carried() {
        let card = RateCard {
            per_m3: Some(isk("1000")),
            ..RateCard::default()
        };
        let cargo = [CargoLine {
            type_id: 34,
            units: 1,
            unit_volume: Volume::from_ten_thousandths(-100),
            unit_value: isk("1"),
        }];
        assert_eq!(
            quote(&card, 0, &cargo, Packing::TopUp),
            Err(FreightError::CannotCarry(34))
        );
    }

    #[test]
    fn overflowing_charges_are_an_error() {
        let card = RateCard {
            per_m3: Some(isk("922337203685477")),
            ..RateCard::default()
        };
        let cargo = [line(34, 1_000_000, "0.01", "1")];
        assert_eq!(
            quote(&card, 0, &cargo, Packing::FillOnly),
            Err(FreightError::Overflow)
        );
    }

    #[test]
    fn cheapest_takes_the_lowest_total_and_the_lower_index_on_a_tie() {
        let quotes = [
            Ok(total_only("40000000")),
            Err(FreightError::CannotCarry(1)),
            Ok(total_only("32000000")),
            Ok(total_only("32000000")),
        ];
        assert_eq!(cheapest(&quotes), Some(2));
    }

    #[test]
    fn cheapest_is_none_when_no_quote_is_ok() {
        let quotes = [
            Err(FreightError::CannotCarry(1)),
            Err(FreightError::Overflow),
        ];
        assert_eq!(cheapest(&quotes), None);
        assert_eq!(cheapest(&[]), None);
    }

    #[test]
    fn zero_rates_and_limits_are_allowed() {
        let card = RateCard {
            per_m3: Some(isk("0")),
            max_m3: Some(m3("0")),
            max_collateral: Some(isk("0")),
            ..RateCard::default()
        };
        let cargo = [line(34, 5, "0", "0")];
        let expected = FreightQuote {
            contracts: vec![contract_of(&[(34, 5)], "0", "0", "0", false)],
            total: isk("0"),
        };
        let (fill_only, top_up) = both_packings(&card, 0, &cargo);
        assert_eq!(fill_only, expected);
        assert_eq!(top_up, expected);
    }

    #[test]
    fn a_unit_exactly_at_the_limits_still_fits() {
        let card = RateCard {
            max_m3: Some(m3("1000")),
            max_collateral: Some(isk("1000000000")),
            ..RateCard::default()
        };
        let cargo = [line(1, 2, "1000", "1000000000")];
        let one = contract_of(&[(1, 1)], "1000", "1000000000", "0", false);
        let expected = FreightQuote {
            contracts: vec![one.clone(), one],
            total: isk("0"),
        };
        assert_eq!(quote(&card, 0, &cargo, Packing::FillOnly), Ok(expected));
    }

    #[test]
    fn the_first_oversize_line_in_input_order_is_named() {
        let by_volume = RateCard {
            max_m3: Some(m3("1000")),
            ..RateCard::default()
        };
        let cargo = [line(10, 1, "1500", "1"), line(20, 1, "2000", "1")];
        assert_eq!(
            quote(&by_volume, 0, &cargo, Packing::TopUp),
            Err(FreightError::CannotCarry(10))
        );
        let by_value = RateCard {
            max_collateral: Some(isk("1000")),
            ..RateCard::default()
        };
        let cargo = [line(10, 1, "1", "1001"), line(20, 1, "1", "2000")];
        assert_eq!(
            quote(&by_value, 0, &cargo, Packing::TopUp),
            Err(FreightError::CannotCarry(10))
        );
    }

    #[test]
    fn a_donor_piece_moved_whole_is_dropped_from_the_donor() {
        let card = RateCard {
            per_m3: Some(isk("1000")),
            minimum: Some(isk("10000000")),
            max_m3: Some(m3("12000")),
            ..RateCard::default()
        };
        let cargo = [line(1, 11_500, "1", "1"), line(2, 1_500, "1", "1")];
        let (fill_only, top_up) = both_packings(&card, 0, &cargo);
        assert_eq!(
            fill_only,
            FreightQuote {
                contracts: vec![
                    contract_of(
                        &[(1, 11_500), (2, 500)],
                        "12000",
                        "12000",
                        "12000000",
                        false
                    ),
                    contract_of(&[(2, 1_000)], "1000", "1000", "10000000", true),
                ],
                total: isk("22000000"),
            }
        );
        assert_eq!(
            top_up,
            FreightQuote {
                contracts: vec![
                    contract_of(&[(1, 10_000)], "10000", "10000", "10000000", false),
                    contract_of(&[(2, 1_500), (1, 1_500)], "3000", "3000", "10000000", true),
                ],
                total: isk("20000000"),
            }
        );
    }

    #[test]
    fn top_up_works_with_three_contracts() {
        let card = RateCard {
            per_m3: Some(isk("1000")),
            minimum: Some(isk("10000000")),
            max_m3: Some(m3("12000")),
            ..RateCard::default()
        };
        let cargo = [line(34, 2_700_000, "0.01", "4")];
        let (fill_only, top_up) = both_packings(&card, 0, &cargo);
        assert_eq!(
            fill_only,
            FreightQuote {
                contracts: vec![
                    contract_of(&[(34, 1_200_000)], "12000", "4800000", "12000000", false),
                    contract_of(&[(34, 1_200_000)], "12000", "4800000", "12000000", false),
                    contract_of(&[(34, 300_000)], "3000", "1200000", "10000000", true),
                ],
                total: isk("34000000"),
            }
        );
        assert_eq!(
            top_up,
            FreightQuote {
                contracts: vec![
                    contract_of(&[(34, 1_000_000)], "10000", "4000000", "10000000", false),
                    contract_of(&[(34, 1_000_000)], "10000", "4000000", "10000000", false),
                    contract_of(&[(34, 700_000)], "7000", "2800000", "10000000", true),
                ],
                total: isk("30000000"),
            }
        );
    }

    #[test]
    fn errors_say_what_is_wrong() {
        assert_eq!(
            FreightError::CannotCarry(587).to_string(),
            "item 587 cannot be carried: too big, too valuable, or a negative size or value"
        );
        assert_eq!(
            FreightError::NegativeRate.to_string(),
            "a price or limit on the rate card is negative"
        );
        assert_eq!(
            FreightError::Overflow.to_string(),
            "the freight amount is too large to count"
        );
    }
}

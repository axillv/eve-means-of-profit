use super::{CargoLine, Contract, FreightError, FreightQuote, Packing, RateCard, cheapest, quote};
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

fn both_packings(card: &RateCard, jumps: u32, cargo: &[CargoLine]) -> (FreightQuote, FreightQuote) {
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

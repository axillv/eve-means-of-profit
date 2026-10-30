use super::*;

use super::copy::{copy_multibuy, paste};

/// The value in the Buy list's footer row.
fn footer_value(harness: &Harness<'_, App>) -> Money {
    let total = harness.get_by_label("Total");
    let cells = line_cells(harness, total);
    let (_, value) = cells.last().expect("the value in the footer row");
    isk(value)
}

/// The two datacores the invention buys for the whole attempts of Hobgoblin II.
pub(super) fn assert_datacore_rows(harness: &Harness<'_, App>) {
    assert_row(
        harness,
        "Datacore - Electronic Engineering",
        &[
            "Datacore - Electronic Engineering",
            "3",
            "0",
            "3",
            "50,000.00",
            "0.30",
            "150,000.00",
            "50,000.00",
            "4m",
        ],
    );
    assert_row(
        harness,
        "Datacore - Graviton Physics",
        &[
            "Datacore - Graviton Physics",
            "3",
            "0",
            "3",
            "43,750.00",
            "0.30",
            "131,250.00",
            "43,750.00",
            "4m",
        ],
    );
}

/// The six lines of the Hobgoblin II × 8 plan, as the multibuy copy writes them.
const PLAN_COPY: &str = "Hobgoblin I\t8\nGuidance Systems\t8\nRobotics\t8\nMorphite\t8\nR.A.M.- Robotics\t8\nParticle Accelerator Unit\t8";

#[test]
fn the_rows_follow_the_plan_order() {
    let world = World::new();
    let harness = complete_list(&world, &standard_services());
    let names = [
        "Hobgoblin I",
        "Guidance Systems",
        "Robotics",
        "Morphite",
        "R.A.M.- Robotics",
        "Particle Accelerator Unit",
        "Datacore - Electronic Engineering",
        "Datacore - Graviton Physics",
    ];
    let tops: Vec<f32> = names
        .iter()
        .map(|name| row_anchor(&harness, name).1)
        .collect();
    assert!(
        tops.windows(2).all(|pair| pair[0] < pair[1]),
        "the rows are out of order: {tops:?}"
    );
}

#[test]
fn the_buy_list_says_it_has_a_table() {
    let world = World::new();
    let harness = complete_list(&world, &standard_services());
    assert_eq!(
        harness
            .ctx
            .data(|data| data.get_temp::<bool>(has_table(Id::new("Buy list")))),
        Some(true)
    );
}

#[test]
fn the_datacores_of_the_whole_attempts_join_the_buy_list() {
    let world = World::new();
    let unticked = invention_list(
        &world,
        InventionChoice {
            invent: false,
            decryptor: None,
        },
    );
    let harness = invention_list(
        &world,
        InventionChoice {
            invent: true,
            decryptor: None,
        },
    );
    assert_datacore_rows(&harness);
    // The datacores come after the plan's lines.
    let tops: Vec<f32> = [
        "Particle Accelerator Unit",
        "Datacore - Electronic Engineering",
        "Datacore - Graviton Physics",
    ]
    .iter()
    .map(|name| row_anchor(&harness, name).1)
    .collect();
    assert!(
        tops.windows(2).all(|pair| pair[0] < pair[1]),
        "the datacores are out of order: {tops:?}"
    );
    assert_eq!(
        footer_value(&harness).checked_sub(footer_value(&unticked)),
        Some(isk("281250.00"))
    );
}

#[test]
fn the_attainment_decryptor_is_bought_for_the_attempts_it_shortens() {
    let world = World::new();
    let unticked = invention_list(
        &world,
        InventionChoice {
            invent: false,
            decryptor: None,
        },
    );
    let harness = invention_list(
        &world,
        InventionChoice {
            invent: true,
            decryptor: Some(34202),
        },
    );
    assert_row(
        &harness,
        "Datacore - Electronic Engineering",
        &["2", "0.20", "50,000.00", "100,000.00"],
    );
    assert_row(
        &harness,
        "Datacore - Graviton Physics",
        &["2", "0.20", "43,750.00", "87,500.00"],
    );
    assert_row(
        &harness,
        "Attainment Decryptor",
        &["2", "0.20", "1,200,000.00", "2,400,000.00"],
    );
    // The decryptor comes after the datacores, which come after the plan's lines.
    let tops: Vec<f32> = [
        "Particle Accelerator Unit",
        "Datacore - Electronic Engineering",
        "Datacore - Graviton Physics",
        "Attainment Decryptor",
    ]
    .iter()
    .map(|name| row_anchor(&harness, name).1)
    .collect();
    assert!(
        tops.windows(2).all(|pair| pair[0] < pair[1]),
        "the invention lines are out of order: {tops:?}"
    );
    assert_eq!(
        footer_value(&harness).checked_sub(footer_value(&unticked)),
        Some(isk("2587500.00"))
    );
}

#[test]
fn a_datacore_in_stock_lowers_what_the_buy_list_and_the_copy_ask_for() {
    let world = World::new();
    let mut harness = invention_list(
        &world,
        InventionChoice {
            invent: true,
            decryptor: None,
        },
    );
    assert!(
        copy_multibuy(&mut harness)
            .ends_with("Datacore - Electronic Engineering\t3\nDatacore - Graviton Physics\t3")
    );
    paste(&mut harness, "Datacore - Electronic Engineering\t1");
    assert_eq!(
        cells(&harness, "Datacore - Electronic Engineering"),
        [
            "Datacore - Electronic Engineering",
            "3",
            "1",
            "2",
            "50,000.00",
            "0.20",
            "100,000.00",
            "50,000.00",
            "4m",
        ]
    );
    assert!(
        copy_multibuy(&mut harness)
            .ends_with("Datacore - Electronic Engineering\t2\nDatacore - Graviton Physics\t3")
    );
}

#[test]
fn an_uninvented_part_buys_no_datacores_or_decryptor() {
    let world = World::new();
    let mut harness = invention_list(
        &world,
        InventionChoice {
            invent: false,
            decryptor: None,
        },
    );
    assert_eq!(
        harness.query_all(by().label_contains("Datacore")).count(),
        0
    );
    assert_eq!(
        harness
            .query_all(by().label_contains("Attainment Decryptor"))
            .count(),
        0
    );
    assert_eq!(copy_multibuy(&mut harness), PLAN_COPY);
}

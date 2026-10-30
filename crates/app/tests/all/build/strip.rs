use super::*;

#[test]
fn the_plan_strip_at_1280_by_720_reads_across_and_matches_its_snapshot() {
    let env = make_env(&standard_facilities());
    let mut harness = start_sized(&env, vec2(1280.0, 720.0));
    harness.run();
    hobgoblin_ii_plan(&mut harness);

    let centre_y = |label: &str| harness.get_by_label(label).rect().center().y;
    let (product, quantity, manufacturing) = (
        centre_y("Product"),
        centre_y("Quantity"),
        centre_y("Manufacturing"),
    );
    assert!(
        (product - quantity).abs() <= 2.0 && (quantity - manufacturing).abs() <= 2.0,
        "Product, Quantity and Manufacturing share a line"
    );
    assert!(
        harness.get_by_label("Manufacturing").rect().min.x
            > harness.get_by_label("Quantity").rect().min.x,
        "Manufacturing is right of Quantity"
    );

    harness.remove_cursor();
    harness.run();
    harness.snapshot("build-1280x720");
}

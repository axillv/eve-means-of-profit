use super::*;

#[test]
fn the_tree_table_with_footer_matches_its_screenshot() {
    let seen = Arc::new(Mutex::new(Seen::default()));
    let mut harness = table_harness(
        test_rows(),
        Some(test_footer()),
        Density::Normal,
        vec2(800.0, 400.0),
        None,
        &seen,
    );
    harness.run();
    harness.snapshot("table-tree");
}

#[test]
fn the_dense_table_matches_its_screenshot() {
    let seen = Arc::new(Mutex::new(Seen::default()));
    let mut harness = table_harness(
        test_rows(),
        Some(test_footer()),
        Density::Dense,
        vec2(800.0, 400.0),
        None,
        &seen,
    );
    harness.run();
    harness.snapshot("table-dense");
}

#[test]
fn the_table_sorted_by_value_matches_its_screenshot() {
    let seen = Arc::new(Mutex::new(Seen::default()));
    let mut harness = table_harness(
        test_rows(),
        Some(test_footer()),
        Density::Normal,
        vec2(800.0, 400.0),
        None,
        &seen,
    );
    harness.run();
    harness.get_by_label("Value").click();
    harness.run();
    harness.snapshot("table-sorted");
}

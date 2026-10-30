use super::*;

fn fold(keys: &[u64]) -> BTreeSet<u64> {
    keys.iter().copied().collect()
}

#[test]
fn sort_without_a_sort_keeps_the_given_order() {
    let rows = test_rows();
    assert_eq!(
        sort_order(&rows, None, Direction::Ascending),
        vec![0, 1, 2, 3, 4, 5, 6]
    );
}

#[test]
fn sort_by_item_keeps_children_under_their_parent() {
    let rows = test_rows();
    assert_eq!(
        sort_order(&rows, Some(0), Direction::Ascending),
        vec![0, 4, 5, 1, 2, 3, 6]
    );
    assert_eq!(
        sort_order(&rows, Some(0), Direction::Descending),
        vec![6, 0, 1, 3, 2, 5, 4]
    );
}

#[test]
fn sort_by_value_puts_missing_last_in_both_directions() {
    let rows = test_rows();
    assert_eq!(
        sort_order(&rows, Some(2), Direction::Descending),
        vec![0, 1, 3, 2, 4, 5, 6]
    );
    assert_eq!(
        sort_order(&rows, Some(2), Direction::Ascending),
        vec![6, 0, 4, 1, 3, 2, 5]
    );
}

#[test]
fn sort_is_stable_for_equal_keys() {
    let rows = test_rows();
    assert_eq!(
        sort_order(&rows, Some(1), Direction::Ascending),
        vec![0, 1, 3, 2, 4, 5, 6]
    );
    assert_eq!(
        sort_order(&rows, Some(1), Direction::Descending),
        vec![6, 0, 1, 2, 3, 4, 5]
    );
}

#[test]
fn a_sort_saves_as_its_column_id_and_direction() {
    let columns = columns();
    assert_eq!(
        sort_to_text(&columns, Some((1, Direction::Descending))),
        "need:desc"
    );
    assert_eq!(
        sort_to_text(&columns, Some((0, Direction::Ascending))),
        "item:asc"
    );
    assert_eq!(sort_to_text(&columns, None), "");
    assert_eq!(sort_to_text(&columns, Some((9, Direction::Ascending))), "");
}

#[test]
fn a_saved_sort_reads_back_by_column_id() {
    let columns = columns();
    assert_eq!(
        sort_from_text(&columns, "value:asc"),
        Some((2, Direction::Ascending))
    );
    assert_eq!(sort_from_text(&columns, "bogus:asc"), None);
    assert_eq!(sort_from_text(&columns, "need"), None);
    assert_eq!(sort_from_text(&columns, "need:up"), None);
    assert_eq!(sort_from_text(&columns, ""), None);
}

#[test]
fn visible_hides_the_descendants_of_collapsed_rows() {
    let rows = test_rows();
    let unsorted: Vec<usize> = (0..7).collect();
    assert_eq!(visible(&rows, &unsorted, &fold(&[2])), vec![0, 1, 4, 5, 6]);
    assert_eq!(visible(&rows, &unsorted, &fold(&[1])), vec![0, 6]);
    assert_eq!(visible(&rows, &unsorted, &fold(&[7])), unsorted);
    let by_item = sort_order(&rows, Some(0), Direction::Ascending);
    assert_eq!(visible(&rows, &by_item, &fold(&[2])), vec![0, 4, 5, 1, 6]);
}

#[test]
fn sort_keeps_missing_siblings_in_their_given_order_in_both_directions() {
    // Under one parent: a present value, then two missing ones, in that order.
    let rows = vec![
        row(1, 0, "Parent", 1, None),
        row(2, 1, "First missing", 1, None),
        row(3, 1, "Second missing", 1, None),
        row(4, 1, "Present", 1, Some(1)),
    ];
    assert_eq!(
        sort_order(&rows, Some(2), Direction::Ascending),
        vec![0, 3, 1, 2]
    );
    assert_eq!(
        sort_order(&rows, Some(2), Direction::Descending),
        vec![0, 3, 1, 2]
    );
}

#[test]
fn sort_keeps_many_equal_missing_siblings_in_their_given_order() {
    let mut rows = vec![row(1, 0, "Parent", 1, None)];
    for key in 2..=26 {
        rows.push(row(key, 1, "Missing", 1, None));
    }
    let given: Vec<usize> = std::iter::once(0).chain(1..26).collect();
    assert_eq!(sort_order(&rows, Some(2), Direction::Ascending), given);
    assert_eq!(sort_order(&rows, Some(2), Direction::Descending), given);
}

#[test]
fn sort_puts_missing_siblings_last_among_present_ones_in_both_directions() {
    // Under one parent, present and missing values interleaved.
    let rows = vec![
        row(1, 0, "Parent", 1, None),
        row(2, 1, "Missing a", 1, None),
        row(3, 1, "Five", 1, Some(50_000)),
        row(4, 1, "Missing b", 1, None),
        row(5, 1, "Three", 1, Some(30_000)),
        row(6, 1, "Missing c", 1, None),
    ];
    assert_eq!(
        sort_order(&rows, Some(2), Direction::Ascending),
        vec![0, 4, 2, 1, 3, 5]
    );
    assert_eq!(
        sort_order(&rows, Some(2), Direction::Descending),
        vec![0, 2, 4, 1, 3, 5]
    );
}

#[test]
fn span_gives_the_top_of_the_first_row_in_view_and_the_total_height() {
    let view = span(2_200.0, 600.0, 22.0, 50_000);
    assert_eq!(view.rows, 99..129);
    assert_eq!(view.top, 2_178.0);
    assert_eq!(view.total, 1_100_000.0);
    assert_eq!(span(0.0, 600.0, 0.0, 10).rows, 0..0);
}

#[test]
fn row_range_covers_the_view_with_one_row_either_side() {
    assert_eq!(row_range(0.0, 600.0, 22.0, 50_000), 0..29);
    assert_eq!(row_range(2_200.0, 600.0, 22.0, 50_000), 99..129);
    assert_eq!(row_range(0.0, 600.0, 22.0, 10), 0..10);
    assert_eq!(row_range(0.0, 600.0, 18.0, 0), 0..0);
}

#[test]
fn to_tsv_copies_the_rows_and_the_footer_as_plain_values() {
    let columns = columns();
    let rows = test_rows();
    let shown: Vec<usize> = (0..7).collect();
    let footer = test_footer();
    assert_eq!(
        to_tsv(&columns, &rows, &shown, Some(&footer)),
        "Item\tNeed\tValue\nHobgoblin II\t10\t1000\nParticle Accelerator Unit\t10\t309000\nCrystalline Carbonide\t166\t\nFullerides\t59\t150\nHobgoblin I\t10\t275000\nMorphite\t10\t\nRifter\t31\t2.125\nTotal\t\t585152.125"
    );
}

#[test]
fn to_tsv_turns_tabs_and_newlines_in_a_field_into_spaces() {
    let columns = columns();
    let rows = vec![Row {
        key: 1,
        depth: 0,
        cells: vec![Cell::text("a\tb\nc"), Cell::int(1, "1"), Cell::missing()],
    }];
    assert_eq!(
        to_tsv(&columns, &rows, &[0], None),
        "Item\tNeed\tValue\na b c\t1\t"
    );
}

#[test]
fn row_heights_follow_the_density() {
    assert_eq!(row_height(Density::Sparse), 28.0);
    assert_eq!(row_height(Density::Normal), 24.0);
    assert_eq!(row_height(Density::Dense), 20.0);
}

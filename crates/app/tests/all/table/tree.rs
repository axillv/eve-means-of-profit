use super::*;

fn tree_harness() -> (Harness<'static>, Arc<Mutex<Seen>>) {
    let seen = Arc::new(Mutex::new(Seen::default()));
    let harness = table_harness(
        test_rows(),
        None,
        Density::Normal,
        vec2(800.0, 400.0),
        None,
        &seen,
    );
    (harness, seen)
}

#[test]
fn rows_without_children_sit_at_their_depth_like_their_siblings() {
    let (mut harness, _seen) = tree_harness();
    harness.run();
    let x = |label: &str| harness.get_by_label(label).rect().min.x;
    let parent = x("Hobgoblin II");
    assert!((x("Crystalline Carbonide") - parent - 28.0).abs() <= 0.5);
    assert!((x("Fullerides") - x("Crystalline Carbonide")).abs() <= 0.5);
    assert!((x("Particle Accelerator Unit") - parent - 14.0).abs() <= 0.5);
    assert!((x("Hobgoblin I") - x("Particle Accelerator Unit")).abs() <= 0.5);
    assert!((x("Morphite") - x("Particle Accelerator Unit")).abs() <= 0.5);
    assert!((x("Rifter") - parent).abs() <= 0.5);
}

#[test]
fn a_click_on_a_leaf_text_at_its_new_place_reports_the_row() {
    let (mut harness, seen) = tree_harness();
    harness.run();
    let text = harness.get_by_label("Fullerides").rect();
    click_at(&harness, pos2(text.min.x + 2.0, text.center().y));
    harness.run();
    assert!(
        seen.lock()
            .unwrap()
            .responses
            .iter()
            .any(|response| response.clicked == Some(4))
    );
}

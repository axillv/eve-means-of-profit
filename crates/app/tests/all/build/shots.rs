use super::*;

use egui_kittest::SnapshotResults;

// Record: the empty states re-record the Build snapshots build-new-plan (taken below),
// pages-build and shell-build-empty (taken in pages.rs and shell.rs).
#[test]
fn the_snapshots_for_the_owner_are_taken() {
    let mut results = SnapshotResults::new();

    let env = make_env(&standard_facilities());
    let mut harness = start(&env);
    harness.run();
    click(&mut harness, "New plan");
    harness.remove_cursor();
    harness.run();
    harness.snapshot("build-new-plan");
    results.extend_harness(&mut harness);

    let env = make_env(&standard_facilities());
    let mut harness = start(&env);
    harness.run();
    p3_plan(&mut harness);
    harness.snapshot("build-p3");
    results.extend_harness(&mut harness);

    let env = make_env(&standard_facilities());
    let mut harness = start(&env);
    harness.run();
    hobgoblin_ii_plan(&mut harness);
    paste(
        &mut harness,
        "Particle Accelerator Unit\t4\nNo Such Thing\t4\nRifter\t2",
    );
    harness.snapshot("build-paste-problems");
    results.extend_harness(&mut harness);

    let env = make_env(&standard_facilities());
    let mut harness = start(&env);
    harness.run();
    hobgoblin_ii_plan(&mut harness);
    harness
        .get_by(|node| {
            node.value()
                .is_some_and(|value| value == "Forge sell, 5% price")
        })
        .click();
    harness.run();
    harness.snapshot("build-price-rule-open");
    results.extend_harness(&mut harness);

    results.unwrap();
}

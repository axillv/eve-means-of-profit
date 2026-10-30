use super::*;

#[test]
fn the_screenshots_of_the_buy_list_are_taken() {
    let mut results = SnapshotResults::new();

    let world = World::new();
    let mut harness = complete_list(&world, &standard_services());
    harness.snapshot("buylist-complete");
    results.extend_harness(&mut harness);

    let world = World::new();
    {
        let mut forge = lock(&world.forge);
        forge.line_mut(11688, false).volume = 4;
        forge.lines.retain(|line| line.type_id != 9848);
    }
    let env = make_env(&standard_services());
    let mut harness = start(&env, &world);
    harness.run();
    buy_list_for(&mut harness);
    settle(&mut harness, "687,036.00");
    harness.snapshot("buylist-incomplete");
    results.extend_harness(&mut harness);

    let world = World::new();
    let mut harness = complete_list_sized(&world, &standard_services(), 1600.0);
    hover(&mut harness, "110 K");
    harness.snapshot("buylist-freight-hover");
    results.extend_harness(&mut harness);

    let world = World::new();
    let mut harness = complete_list(&world, &standard_services());
    type_price(&mut harness, "Robotics", "1,000,000,000.00");
    // The Price column is in view at 1280 px, so the picture needs no sideways scroll.
    // The pointer rests over the Plans panel, so no tooltip covers the row.
    harness.hover_at(egui::pos2(300.0, 600.0));
    harness.run();
    harness.snapshot("buylist-manual-price");
    results.extend_harness(&mut harness);

    results.unwrap();
}

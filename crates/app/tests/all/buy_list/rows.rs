use super::*;

use super::plans::assert_datacore_rows;

/// The six rows of the complete plan, at the prices of the standard file.
fn assert_complete_rows(harness: &Harness<'_, App>) {
    assert_row(
        harness,
        "Hobgoblin I",
        &[
            "Hobgoblin I",
            "10",
            "0",
            "10",
            "27,500.00",
            "50.00",
            "275,000.00",
            "27,500.00",
            "4m",
        ],
    );
    assert_row(
        harness,
        "Guidance Systems",
        &[
            "Guidance Systems",
            "10",
            "0",
            "10",
            "10,450.00",
            "30.00",
            "104,500.00",
            "10,450.00",
            "4m",
        ],
    );
    assert_row(
        harness,
        "Robotics",
        &[
            "Robotics",
            "10",
            "0",
            "10",
            "39,990.00",
            "30.00",
            "399,900.00",
            "39,990.00",
            "4m",
        ],
    );
    assert_row(
        harness,
        "Morphite",
        &[
            "Morphite",
            "10",
            "0",
            "10",
            "2,150.00",
            "0.10",
            "21,500.00",
            "2,150.00",
            "4m",
        ],
    );
    assert_row(
        harness,
        "R.A.M.- Robotics",
        &[
            "R.A.M.- Robotics",
            "10",
            "0",
            "10",
            "478.60",
            "0.40",
            "4,786.00",
            "478.60",
            "4m",
        ],
    );
    assert_row(
        harness,
        "Particle Accelerator Unit",
        &[
            "Particle Accelerator Unit",
            "10",
            "0",
            "10",
            "30,900.00",
            "10.00",
            "309,000.00",
            "30,900.00",
            "4m",
        ],
    );
}

#[test]
fn the_buy_list_rows_and_the_footer_are_complete() {
    let world = World::new();
    let mut harness = complete_list(&world, &standard_services());
    assert!(shown(&harness, "Price rule: Forge sell, 5% price"));
    assert_complete_rows(&harness);
    assert_datacore_rows(&harness);
    assert!(shown(&harness, "freight "));
    assert!(shown(&harness, "110 K"));
    assert!(shown(&harness, " · collateral 1.40 M · delivered 1.51 M"));
    assert!(!shown(&harness, " incomplete"));
    assert!(shown(&harness, "Oldest: 4m"));
    assert!(!shown(&harness, "Refresh automatically"));
    assert!(!shown(&harness, "Next refresh in 15m"));
    assert!(has_value(&harness, "Cheapest (Alliance)"));
    assert!(shown(&harness, "1 contract"));
    let footer: Vec<String> = line_cells(&harness, harness.get_by_label("Total"))
        .into_iter()
        .map(|(_, value)| value)
        .collect();
    assert_eq!(footer, ["Total", "66", "0", "66", "121.10", "1,395,936.00"]);
    hover(&mut harness, " · collateral 1.40 M · delivered 1.51 M");
    assert!(shown(&harness, "1,395,936.00 · 1,505,850.68 ISK"));
}

#[test]
fn the_need_own_and_price_headers_are_in_view_at_1280() {
    let world = World::new();
    let harness = complete_list(&world, &standard_services());
    let item = rightmost(&harness, "Item").rect();
    let need = rightmost(&harness, "Need").rect();
    let own = rightmost(&harness, "Own").rect();
    let price = rightmost(&harness, "Price").rect();
    let volume = rightmost(&harness, "m³").rect();
    // The Buy list is the rightmost panel of the Build page, so its content ends at the window's
    // right edge; the Item header sits at its left edge.
    let panel = egui::Rect::from_min_max(
        egui::pos2(item.min.x, item.min.y),
        egui::pos2(harness.ctx.content_rect().max.x, item.max.y),
    );
    assert!(need.min.x > item.min.x, "Need is right of Item");
    assert!(own.min.x > need.min.x, "Own is right of Need");
    assert!(price.min.x > own.min.x, "Price is right of Own");
    assert!(volume.min.x >= price.max.x, "m³ starts right of Price");
    for (name, cell) in [("Need", need), ("Own", own), ("Price", price)] {
        assert!(
            cell.min.x >= panel.min.x - 1.0,
            "{name} starts outside the Buy list panel: {cell:?} vs {panel:?}"
        );
        assert!(
            cell.max.x <= panel.max.x + 1.0,
            "{name} ends outside the Buy list panel: {cell:?} vs {panel:?}"
        );
    }
}

#[test]
fn the_footer_row_lines_up_with_the_headers_at_1600() {
    let world = World::new();
    let harness = complete_list_sized(&world, &standard_services(), 1600.0);
    let headers = line_cells(&harness, rightmost(&harness, "Item"));
    let footers = line_cells(&harness, harness.get_by_label("Total"));
    // Item, Need, Own, Buy, Price, m³, Value, Live, Diff and Age.
    assert_eq!(headers.len(), 10, "{headers:?}");
    // The footer carries the counts, the volume and the value; Price has no footer text.
    let values: Vec<&str> = footers.iter().map(|(_, value)| value.as_str()).collect();
    assert_eq!(values, ["Total", "66", "0", "66", "121.10", "1,395,936.00"]);
    // Item's footer text starts at its header's left edge.
    assert!((footers[0].0.min.x - headers[0].0.min.x).abs() <= 1.0);
    // The count and money columns end at their header's right edge. Price has no footer text,
    // so m³ and Value follow Buy.
    for (footer, header) in [(1, 1), (2, 2), (3, 3), (4, 5), (5, 6)] {
        let (rect, text) = &footers[footer];
        let off = (rect.max.x - headers[header].0.max.x).abs();
        assert!(off <= 1.0, "{text} is {off} points off its header");
    }
}

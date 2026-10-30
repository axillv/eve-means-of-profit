use super::*;

use super::copy::copied_text;

/// Points at `at` and runs enough frames for the tooltip there to appear.
fn hover_at_point(harness: &mut Harness<'_, App>, at: egui::Pos2) {
    harness.hover_at(at);
    for _ in 0..60 {
        harness.run();
    }
}

/// The Age cell of the row labelled `row`, which reads `text`, with the table scrolled sideways
/// so the age column is in view.
fn age_cell(harness: &mut Harness<'_, App>, row: &str, text: &str) -> egui::Rect {
    let (left, y) = row_anchor(harness, row);
    scroll_table(harness, left, y, -700.0);
    harness
        .get_all_by_label(text)
        .map(|node| node.rect())
        .find(|rect| (rect.center().y - y).abs() < 4.0)
        .expect("the age cell of the row")
}

/// Sets the price rule that new plans start with, in the store the app opens.
fn default_rule(env: &Env, rule: PriceRule) {
    let mut store = Store::open(&env.data.join("user.sqlite"), NOW)
        .expect("the store opens")
        .store;
    let mut settings = store.settings().expect("settings");
    settings.default_price_rule = rule;
    store.save_settings(&settings).expect("settings saved");
}

/// The Buy list of a new plan under `rule`, once its prices are in.
fn list_under(world: &World, rule: PriceRule) -> (Env, Harness<'static, App>) {
    let env = make_env(&standard_services());
    default_rule(&env, rule);
    let mut harness = start(&env, world);
    harness.run();
    buy_list_for(&mut harness);
    settle(&mut harness, "Oldest: 4m");
    (env, harness)
}

#[test]
fn the_median_rule_prices_each_line_at_its_median() {
    let (_env, harness) = list_under(&World::new(), PriceRule::SellMedian);
    assert_row(&harness, "Robotics", &["40,100.00", "401,000.00"]);
}

#[test]
fn the_lowest_sell_rule_prices_each_line_at_its_lowest_sell() {
    let (_env, harness) = list_under(&World::new(), PriceRule::LowestSell);
    assert_row(&harness, "Robotics", &["39,500.00", "395,000.00"]);
}

#[test]
fn the_weighted_average_rule_prices_each_line_at_its_average() {
    let (_env, harness) = list_under(&World::new(), PriceRule::SellWeightedAverage);
    assert_row(&harness, "Robotics", &["40,500.00", "405,000.00"]);
}

#[test]
fn an_old_walk_the_book_plan_reads_the_five_percent_price() {
    let (_env, harness) = list_under(&World::new(), PriceRule::WalkSell);
    assert!(shown(&harness, "Price rule: Forge sell, 5% price"));
    assert_row(&harness, "Robotics", &["39,990.00", "399,900.00"]);
}

#[test]
fn highest_buy_prices_only_the_line_with_buy_orders() {
    let (_env, mut harness) = list_under(&World::new(), PriceRule::HighestBuy);
    assert_row(&harness, "R.A.M.- Robotics", &["431.60", "4,316.00"]);
    let rows = [
        "Hobgoblin I",
        "Guidance Systems",
        "Robotics",
        "Morphite",
        "R.A.M.- Robotics",
        "Particle Accelerator Unit",
    ];
    let boxes = price_boxes(&mut harness, &rows);
    for (row, (_, value)) in rows.iter().zip(&boxes) {
        if *row != "R.A.M.- Robotics" {
            assert_eq!(value.as_deref(), Some("—"), "{row}");
        }
    }
    let robotics = boxes[2].0;
    hover_at_point(&mut harness, robotics.center());
    assert!(shown(&harness, "no Forge buy orders"));
    assert!(shown(&harness, " incomplete"));
}

#[test]
fn a_thin_side_and_a_missing_side_are_marked_and_left_out_of_the_value() {
    let world = World::new();
    {
        let mut forge = lock(&world.forge);
        forge.line_mut(11688, false).volume = 4;
        forge.lines.retain(|line| line.type_id != 9848);
    }
    let env = make_env(&standard_services());
    let mut harness = start_window(&env, &world, vec2(1280.0, 1000.0));
    harness.run();
    buy_list_for(&mut harness);
    settle(&mut harness, "only 4 on market");
    // A thin line keeps the summary price; only its value is cut to the units on the market.
    assert_row(
        &harness,
        "Particle Accelerator Unit",
        &["30,900.00", "only 4 on market"],
    );
    assert_row(&harness, "Robotics", &["—"]);
    assert!(shown(&harness, " incomplete"));
    // The missing price leaves Robotics' value out of the sum, while the known lines' total
    // still lands in the footer row; the footer line sums the delivery of what is known.
    let footer: Vec<String> = line_cells(&harness, harness.get_by_label("Total"))
        .into_iter()
        .map(|(_, value)| value)
        .collect();
    assert!(
        footer.iter().any(|value| value == "687,036.00"),
        "{footer:?}"
    );
    assert!(shown(&harness, " · collateral 687 K · delivered 759 K"));
    // The wider list wraps the footer so the incomplete label starts on a later line than
    // the label's own rect centre; hover the run of text itself.
    let incomplete = harness
        .query_all(by().predicate(|node| {
            node.value()
                .is_some_and(|value| value.trim() == "incomplete")
        }))
        .next()
        .expect("the incomplete label")
        .rect();
    hover_at_point(&mut harness, incomplete.center());
    assert!(shown(&harness, "Robotics\nParticle Accelerator Unit"));
}

#[test]
fn a_missing_price_shows_a_dash_and_says_why_on_hover() {
    let world = World::new();
    lock(&world.forge).lines.retain(|line| line.type_id != 9848);
    let env = make_env(&standard_services());
    let mut harness = start(&env, &world);
    harness.run();
    buy_list_for(&mut harness);
    settle(&mut harness, "Oldest: 4m");
    let boxes = price_boxes(&mut harness, &["Robotics"]);
    let (at, value) = &boxes[0];
    assert_eq!(value.as_deref(), Some("—"));
    hover_at_point(&mut harness, at.center());
    assert!(shown(&harness, "no Forge sell orders"));
}

#[test]
fn old_prices_show_their_age_and_the_status_line_names_the_oldest() {
    let world = World::new();
    lock(&world.forge).published = NOW - 21_601;
    let env = make_env(&standard_services());
    let mut harness = start_window(&env, &world, vec2(1280.0, 1000.0));
    harness.run();
    buy_list_for(&mut harness);
    settle(&mut harness, "Oldest: 6h");
    assert!(shown(&harness, "Market data: oldest 6h"));
    // The age of the oldest price is in the stale colour.
    let old = harness.get_by_label("Oldest: 6h").rect();
    assert!(coloured(&mut harness, old));

    // A file made 4 minutes ago is not stale: its age is in the text colour.
    world.at(NOW + 900);
    lock(&world.forge).publish(standard_lines(), NOW + 660);
    click(&mut harness, "Refresh now");
    settle(&mut harness, "Oldest: 4m");
    let fresh = harness.get_by_label("Oldest: 4m").rect();
    assert!(!coloured(&mut harness, fresh));
}

#[test]
fn a_price_exactly_six_hours_old_keeps_the_text_colour() {
    let world = World::new();
    lock(&world.forge).published = NOW - 21_600;
    let env = make_env(&standard_services());
    let mut harness = start(&env, &world);
    harness.run();
    buy_list_for(&mut harness);
    settle(&mut harness, "Oldest: 6h");
    assert!(shown(&harness, "Market data: oldest 6h"));
    // Six hours is the limit itself, not past it: the age is in the text colour.
    let limit = harness.get_by_label("Oldest: 6h").rect();
    assert!(!coloured(&mut harness, limit));
}

#[test]
fn a_price_exactly_six_hours_old_keeps_the_age_cell_in_the_text_colour() {
    let world = World::new();
    let mut harness = aged_list(&world, 21_600, "6h");
    assert_row(&harness, "Robotics", &["6h"]);
    // Six hours is the limit itself, not past it: the cell is in the text colour.
    let cell = age_cell(&mut harness, "Robotics", "6h");
    assert!(!coloured(&mut harness, cell));
}

#[test]
fn an_age_past_a_day_says_old_and_warns_before_buying() {
    // A second past six hours: the cell is amber.
    let world = World::new();
    let mut harness = aged_list(&world, 21_601, "6h");
    let cell = age_cell(&mut harness, "Robotics", "6h");
    assert!(coloured(&mut harness, cell));

    // Exactly a day is still `1d`, in amber.
    let world = World::new();
    let mut harness = aged_list(&world, 86_400, "1d");
    assert_row(&harness, "Robotics", &["1d"]);
    let cell = age_cell(&mut harness, "Robotics", "1d");
    assert!(coloured(&mut harness, cell));

    // A second past a day: the cell says `old`, in amber, and warns on hover.
    let world = World::new();
    let mut harness = aged_list(&world, 86_401, "1d old");
    assert_row(&harness, "Robotics", &["1d old"]);
    let cell = age_cell(&mut harness, "Robotics", "1d old");
    assert!(coloured(&mut harness, cell));
    hover_at_point(&mut harness, cell.center());
    assert!(shown(
        &harness,
        "Over a day old. Refresh before you rely on it."
    ));
    // The copied list says `old` too, not only the drawn cell. Age is the last column.
    harness
        .ctx
        .data_mut(|data| data.insert_temp(copy_request(Id::new("Buy list")), true));
    harness.step();
    let copied = copied_text(&harness).expect("the list is copied");
    assert!(
        copied.lines().any(|line| line.ends_with("\t1d old")),
        "the copied age reads old: {copied}"
    );
}

#[test]
fn the_oldest_line_asks_for_a_refresh_once_the_prices_are_over_a_day_old() {
    let world = World::new();
    let mut harness = aged_list(&world, 172_800, "2d old");
    let text = "Oldest: 2d · Prices over a day old: refresh before you buy.";
    settle(&mut harness, text);
    // The line sits below an 800 px window at the 14 px metrics; the colour is only on screen
    // once it is scrolled into view.
    harness.get_by_label(text).scroll_to_me();
    harness.run();
    let old = harness.get_by_label(text).rect();
    assert!(coloured(&mut harness, old));

    // An hour old is not stale: the line gives no refresh advice and is in the text colour.
    let world = World::new();
    let mut harness = aged_list(&world, 3_600, "1h");
    settle(&mut harness, "Oldest: 1h");
    let fresh = harness.get_by_label("Oldest: 1h").rect();
    assert!(!coloured(&mut harness, fresh));
}

#[test]
fn a_rate_limit_keeps_the_prices_and_names_the_wait() {
    let world = World::new();
    let mut harness = complete_list(&world, &standard_services());
    world.at(NOW + 900);
    lock(&world.forge).fault = Fault::RateLimit;
    click(&mut harness, "Refresh now");
    settle(&mut harness, "ERROR");
    assert!(shown(
        &harness,
        "Market prices paused for 30 s (rate limited)."
    ));
    assert!(shown(&harness, "27,500.00"));
}

#[test]
fn an_unavailable_file_keeps_the_prices_and_names_the_error() {
    let world = World::new();
    let mut harness = complete_list(&world, &standard_services());
    world.at(NOW + 900);
    lock(&world.forge).fault = Fault::Unavailable;
    click(&mut harness, "Refresh now");
    settle(&mut harness, "ERROR");
    assert!(shown(
        &harness,
        "Could not get market prices: ESI answered with status 503"
    ));
    assert!(shown(&harness, "27,500.00"));
}

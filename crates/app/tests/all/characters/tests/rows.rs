use super::*;

#[test]
fn rows_are_sorted_by_name_with_the_login_and_the_age_of_each_kind() {
    let env = env();
    // Nothing is due, so the start-up refresh leaves the saved ages as they are.
    let eve = FakeEve::new(&env, all_not_yet);
    let mut harness = open(&env, &eve);
    until(&mut harness, "the rows", |h| {
        shown(h, "login expired: log in again")
    });
    assert!(row_y(&harness, "Alt") < row_y(&harness, "Main"));
    assert!(shown(&harness, "ok"));
    assert!(shown(&harness, "Log in again"));
    for age in ["1h", "5m", "30s"] {
        assert_eq!(count(&harness, age), 1, "{age}");
    }
    // Alt has fetched nothing (four `never`); Main's jobs make the fifth.
    assert_eq!(count(&harness, "never"), 5);
}

#[test]
fn each_age_and_the_login_carry_their_colour() {
    let style = Style::eve_dense();
    let assets = kind_view(Some((NOW - 4_000, Ok(NOW - 400))), NOW, &style);
    assert_eq!((assets.text.as_str(), assets.color), ("1h", style.stale));
    assert_eq!(assets.hover, "Can be refreshed now");
    let orders = kind_view(Some((NOW - 300, Ok(NOW + 900))), NOW, &style);
    assert_eq!((orders.text.as_str(), orders.color), ("5m", style.text));
    assert_eq!(orders.hover, "Next refresh allowed in 15m");
    let jobs = kind_view(None, NOW, &style);
    assert_eq!((jobs.text.as_str(), jobs.color), ("never", style.text));
    let wallet = kind_view(Some((NOW - 30, Ok(NOW + 90))), NOW, &style);
    assert_eq!((wallet.text.as_str(), wallet.color), ("30s", style.text));
    assert_eq!(login_view(false, &style), ("ok", style.text));
    assert_eq!(
        login_view(true, &style),
        ("login expired: log in again", style.neg)
    );
}

#[test]
fn a_next_refresh_that_cannot_be_worked_out_shows_the_error() {
    let style = Style::eve_dense();
    let broken = kind_view(Some((NOW - 30, Err("bad time".to_owned()))), NOW, &style);
    assert_eq!(broken.text, "30s");
    assert_eq!(broken.color, style.neg);
    assert_eq!(
        broken.hover,
        "Could not work out the next refresh: bad time"
    );
}

#[test]
fn hovering_an_age_tells_when_eve_allows_the_next_refresh() {
    let env = env();
    let eve = FakeEve::new(&env, partial);
    let mut harness = open(&env, &eve);
    until(&mut harness, "the rows", |h| shown(h, "5m"));
    harness
        .get_all_by_label("5m")
        .next()
        .expect("Main's orders age")
        .hover();
    frames(&mut harness);
    assert!(shown(&harness, "Next refresh allowed in 15m"));
}

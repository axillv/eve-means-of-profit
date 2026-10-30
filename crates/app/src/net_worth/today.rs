//! The Today page's Net worth block: the latest total, its change since the point before it, and
//! the types the basis could not price. The body is one cached set of strings, so it changes only
//! when the data it describes does.

use std::collections::BTreeSet;

use egui::{
    Align, Button, Id, Label, Layout, RichText, TextWrapMode, Ui, WidgetInfo, WidgetType, vec2,
};
use mop_core::money::Money;
use mop_core::net_worth::Part;
use mop_core::price_history::day_of;
use mop_market::book::PriceRule;
use mop_store::StoreError;

use crate::Shared;
use crate::build::inputs::price_text;
use crate::net_worth::Range;
use crate::net_worth::data::load::{Loaded, load};
use crate::net_worth::summary::{Since, Summary, SummaryError, summarise};
use crate::net_worth::view::basis_text;
use crate::orders::rows::utc_text;
use crate::rail::Page;
use crate::status::Notice;

/// The key of the block's cached lines in the frame memory.
const CACHE_KEY: &str = "today.net_worth";
/// The block's title, which every state starts with.
const TITLE: &str = "Net worth";
/// What the block says when there are no points or characters.
const STARTS_WITH_CHARACTER: &str = "Starts with your first character.";
/// The button that adds the first character.
const ADD_CHARACTER: &str = "Add character";
/// What the block says when there are no points at all.
const NOTHING: &str = "No net worth points yet. Refresh your characters to record the first one.";
/// What the block says when the points could not be read.
const FAILED: &str = "Could not read net worth.";
/// The space between two parts of one row.
const PART_GAP: f32 = 8.0;
/// The height of one block's row.
const ROW_HEIGHT: f32 = 38.0;
/// What the split line counts, on hover.
const SPLIT_HOVER: &str = "Price moves: the same items repriced at both points. Everything else: quantity changes, items that appeared or left, the wallet and PLEX.";

/// The block's lines and the state they were read in.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Cached {
    /// The frame that built the lines, from `cumulative_frame_nr`.
    frame: u64,
    /// The login characters' `(id, revision)` when the lines were built.
    revisions: Vec<(u64, u64)>,
    body: Body,
}

/// What the block draws.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Body {
    /// There are no points yet.
    Nothing,
    /// The points or the settings could not be read.
    Failed,
    /// The latest point, with the lines that describe it.
    Shown(Lines),
}

/// The texts of one shown block.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Lines {
    /// The latest total as `<format_isk_short> ISK`.
    total: String,
    /// The total's hover text: the full amount and the basis.
    hover: String,
    /// The change and the day it is since, with the colour its size asks for.
    change: Option<(String, String, Sign)>,
    /// The change's hover text: its amount, the basis and the earlier point.
    change_hover: Option<String>,
    /// The change split into price moves and everything else, when there is a change to split.
    split: Option<String>,
    /// The split's hover text: what each part counts.
    split_hover: Option<String>,
    /// How many item types have no price, when there are any.
    unpriced: Option<String>,
}

/// Which colour a change asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sign {
    Up,
    Down,
    None,
}

/// Draws the whole block. One load builds every line; later frames read the cache. Returns the page
/// its empty row asks for, when it was clicked.
pub fn body(ui: &mut Ui, cx: &mut Shared) -> Option<Page> {
    let frame = ui.ctx().cumulative_frame_nr();
    let revisions = login_revisions(cx);
    let cached = ui.ctx().data(|data| data.get_temp::<Cached>(cache_id()));
    let hidden = cached
        .as_ref()
        .is_some_and(|cached| frame > cached.frame.saturating_add(1));
    let due = cached
        .as_ref()
        .is_none_or(|cached| cached.revisions != revisions || hidden);
    let cached = match cached {
        Some(mut cached) if !due => {
            cached.frame = frame;
            cached
        }
        _ => {
            let now = (cx.clock)();
            reload(cx, now, frame, revisions)
        }
    };
    ui.ctx()
        .data_mut(|data| data.insert_temp(cache_id(), cached.clone()));
    show(ui, cx, &cached.body)
}

/// The name of the block's cache entry.
fn cache_id() -> Id {
    Id::new(CACHE_KEY)
}

/// The login characters and their revisions, in the order the login lists them.
fn login_revisions(cx: &Shared) -> Vec<(u64, u64)> {
    cx.session
        .characters()
        .iter()
        .map(|character| (character.id, cx.session.revision(character.id)))
        .collect()
}

/// Reads the settings, the points and their summary, and leaves the lines to draw.
fn reload(cx: &mut Shared, now: i64, frame: u64, revisions: Vec<(u64, u64)>) -> Cached {
    let (shown, price, plex) = read_settings(cx);
    let sde = cx.sde.as_ref();
    let loaded = load(
        &mut cx.store,
        sde,
        Range::Days30.from(now),
        now,
        &shown,
        plex,
        &BTreeSet::new(),
    );
    let body = match loaded {
        Err(text) => {
            cx.notices.push(Notice::error(text));
            Body::Failed
        }
        Ok(loaded) => loaded_body(cx, &loaded, price),
    };
    Cached {
        frame,
        revisions,
        body,
    }
}

/// The shown parts, the app-wide price and whether the vault counts. A failed read is one ERROR
/// notice, and every value falls back to the defaults.
fn read_settings(cx: &mut Shared) -> (BTreeSet<Part>, PriceRule, bool) {
    match try_read_settings(cx) {
        Ok(settings) => settings,
        Err(error) => {
            cx.notices.push(Notice::error(format!(
                "Could not read the net worth settings: {error}"
            )));
            (
                Part::ALL.into_iter().collect(),
                PriceRule::SellFivePercent,
                true,
            )
        }
    }
}

/// Reads the three settings in order, stopping at the first failure.
fn try_read_settings(cx: &mut Shared) -> Result<(BTreeSet<Part>, PriceRule, bool), StoreError> {
    let shown = cx.store.net_worth_settings()?.shown;
    let price = cx.store.value_price()?;
    // An unset or unknown text keeps the vault on.
    let plex = cx.store.setting(super::PLEX_SETTING)?.as_deref() != Some("0");
    Ok((shown, price, plex))
}

/// Turns a loaded range into the body, reporting the reads and the sums that failed.
fn loaded_body(cx: &mut Shared, loaded: &Loaded, price: PriceRule) -> Body {
    if let Some(error) = loaded.error.as_ref() {
        cx.notices
            .push(Notice::error(format!("Could not price net worth: {error}")));
        return Body::Failed;
    }
    let Some(last) = loaded.points.last() else {
        return Body::Nothing;
    };
    let summary: Option<Summary> =
        match summarise(&loaded.points, loaded.previous_at_latest_prices.as_ref()) {
            Ok(summary) => summary,
            Err(SummaryError::Overflow) => {
                cx.notices.push(Notice::error(
                    "Could not add up net worth: the sum is too large",
                ));
                None
            }
        };

    let total = format!("{} ISK", last.total.format_isk_short());
    let hover = format!(
        "{} ISK · {}",
        last.total.format_isk(),
        basis_text(price_text(price), last.source),
    );
    let since = summary.and_then(|summary| summary.since);
    let change = since.map(|since| {
        let amount = since.change.amount;
        let phrase = since_phrase(since.from_at, since.from_day, day_of(last.value.at));
        (
            amount.format_isk_short_signed(),
            format!("since {phrase}"),
            sign(amount),
        )
    });
    let (change_hover, split_hover) = hover_texts(since, price);
    let split = since.map(|since| {
        format!(
            "Price moves {}, everything else {}",
            since.price_moves.format_isk_short_signed(),
            since.everything_else.format_isk_short_signed(),
        )
    });
    let unpriced = (!last.unpriced.is_empty()).then(|| {
        format!(
            "{} item types have no price at this basis and are not counted.",
            last.unpriced.len(),
        )
    });

    Body::Shown(Lines {
        total,
        hover,
        change,
        change_hover,
        split,
        split_hover,
        unpriced,
    })
}

/// The change and split hover texts, when there is a change to describe.
fn hover_texts(since: Option<Since>, price: PriceRule) -> (Option<String>, Option<String>) {
    let Some(since) = since else {
        return (None, None);
    };
    (
        Some(change_hover_text(since, price)),
        Some(SPLIT_HOVER.to_owned()),
    )
}

/// The change's hover text: its amount, the basis and the point it is since.
fn change_hover_text(since: Since, price: PriceRule) -> String {
    format!(
        "{} ISK at {}, since {}",
        since.change.amount.format_isk(),
        price_text(price),
        utc_text(since.from_at),
    )
}

#[cfg(test)]
impl Body {
    /// The hover texts this body would draw; the empty and failed states draw none.
    fn hovers(&self) -> (Option<&str>, Option<&str>) {
        match self {
            Body::Shown(lines) => (lines.change_hover.as_deref(), lines.split_hover.as_deref()),
            Body::Nothing | Body::Failed => (None, None),
        }
    }
}

/// Whether a change is a gain, a loss or neither.
fn sign(amount: Money) -> Sign {
    match amount.ten_thousandths() {
        value if value > 0 => Sign::Up,
        value if value < 0 => Sign::Down,
        _ => Sign::None,
    }
}

/// Whether the block draws its empty row with the Add character button: only when it has no
/// points and the login has no characters.
fn add_character(body: &Body, no_characters: bool) -> bool {
    matches!(body, Body::Nothing) && no_characters
}

/// Draws the block's title as one row, so every state starts at the same height.
fn title(ui: &mut Ui) {
    let width = ui.available_width();
    ui.allocate_ui_with_layout(
        vec2(width, ROW_HEIGHT),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.label(RichText::new(TITLE).strong());
        },
    );
}

/// Draws the empty row: the title, the copy and the Add character button on one line. Returns
/// `Page::Characters` when the button was clicked.
fn empty_row(ui: &mut Ui, cx: &Shared) -> Option<Page> {
    let width = ui.available_width();
    let mut target = None;
    ui.allocate_ui_with_layout(
        vec2(width, ROW_HEIGHT),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.label(RichText::new(TITLE).strong());
            ui.add_space(PART_GAP);
            ui.add(
                Label::new(RichText::new(STARTS_WITH_CHARACTER).color(cx.style.mute))
                    .wrap_mode(TextWrapMode::Extend),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let response = ui.add(Button::new(ADD_CHARACTER));
                let name = format!("{TITLE}: {ADD_CHARACTER}");
                response.widget_info(|| {
                    WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), name.clone())
                });
                if response.clicked() {
                    target = Some(Page::Characters);
                }
            });
        },
    );
    target
}

/// Draws one body: the title, then its content or empty copy.
fn show(ui: &mut Ui, cx: &Shared, body: &Body) -> Option<Page> {
    if add_character(body, cx.session.characters().is_empty()) {
        return empty_row(ui, cx);
    }
    title(ui);
    match body {
        Body::Nothing => {
            ui.colored_label(cx.style.mute, NOTHING);
        }
        Body::Failed => {
            ui.colored_label(cx.style.mute, FAILED);
        }
        Body::Shown(lines) => {
            ui.heading(lines.total.as_str())
                .on_hover_text(lines.hover.as_str());
            if let Some((amount, since, sign)) = &lines.change {
                let color = match sign {
                    Sign::Up => cx.style.pos,
                    Sign::Down => cx.style.neg,
                    Sign::None => cx.style.mute,
                };
                ui.horizontal(|ui| {
                    let amount = ui.label(RichText::new(amount).color(color));
                    if let Some(hover) = &lines.change_hover {
                        amount.on_hover_text(hover.as_str());
                    }
                    ui.add_space(4.0);
                    let since = ui.colored_label(cx.style.mute, since);
                    if let Some(hover) = &lines.change_hover {
                        since.on_hover_text(hover.as_str());
                    }
                });
            }
            if let Some(split) = &lines.split {
                let split = ui.colored_label(cx.style.mute, split);
                if let Some(hover) = &lines.split_hover {
                    split.on_hover_text(hover.as_str());
                }
            }
            if let Some(unpriced) = &lines.unpriced {
                ui.colored_label(cx.style.stale, unpriced);
            }
        }
    }
    None
}

/// The day text after "since": `yesterday` when the earlier point is the last one's day before,
/// else the short date.
fn since_phrase(from_at: i64, from_day: i64, last_day: i64) -> String {
    if last_day.checked_sub(1) == Some(from_day) {
        "yesterday".to_owned()
    } else {
        date_text(from_at)
    }
}

/// A moment as `<day> <Mon>`: `2 Sep`, with no leading zero. Text that is not a date is unchanged.
fn date_text(at: i64) -> String {
    let text = utc_text(at);
    let date = text
        .split_once(' ')
        .map_or(text.as_str(), |(date, _time)| date);
    let mut parts = date.split('-');
    let (_year, Some(month), Some(day)) = (parts.next(), parts.next(), parts.next()) else {
        return text;
    };
    let Some(month) = month_name(month) else {
        return text;
    };
    let Ok(day) = day.parse::<i64>() else {
        return text;
    };
    format!("{day} {month}")
}

/// The EVE style month name of a two-digit month.
fn month_name(month: &str) -> Option<&'static str> {
    Some(match month {
        "01" => "Jan",
        "02" => "Feb",
        "03" => "Mar",
        "04" => "Apr",
        "05" => "May",
        "06" => "Jun",
        "07" => "Jul",
        "08" => "Aug",
        "09" => "Sep",
        "10" => "Oct",
        "11" => "Nov",
        "12" => "Dec",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use mop_core::money::Money;
    use mop_market::book::PriceRule;

    use super::{
        Body, Lines, SPLIT_HOVER, Sign, add_character, change_hover_text, date_text, hover_texts,
        sign, since_phrase,
    };
    use crate::net_worth::summary::{Change, Since};

    fn isk(text: &str) -> Money {
        Money::parse_isk(text).expect("a test amount")
    }

    fn since(from_at: i64, amount: &str, price_moves: &str, everything_else: &str) -> Since {
        Since {
            from_at,
            from_day: 0,
            change: Change {
                amount: isk(amount),
                percent: None,
            },
            price_moves: isk(price_moves),
            everything_else: isk(everything_else),
        }
    }

    #[test]
    fn the_add_character_row_appears_exactly_without_points_and_characters() {
        assert!(
            add_character(&Body::Nothing, true),
            "no characters and no points asks for a character"
        );
        assert!(
            !add_character(&Body::Nothing, false),
            "a character without points asks for a refresh, not a character"
        );
        assert!(
            !add_character(&Body::Failed, true),
            "a failed read keeps its error, not the Add character row"
        );
    }

    #[test]
    fn the_change_hover_names_the_amount_the_basis_and_the_earlier_point() {
        let since = since(999_395_200, "7,708,667.89", "192,000.00", "7,516,667.89");
        let (change, split) = hover_texts(Some(since), PriceRule::SellFivePercent);
        assert_eq!(
            change.as_deref(),
            Some("7,708,667.89 ISK at Forge sell, 5% price, since 2001-09-02 01:46")
        );
        assert_eq!(
            split.as_deref(),
            Some(
                "Price moves: the same items repriced at both points. Everything else: quantity changes, items that appeared or left, the wallet and PLEX."
            )
        );
        assert_eq!(
            SPLIT_HOVER,
            "Price moves: the same items repriced at both points. Everything else: quantity changes, items that appeared or left, the wallet and PLEX."
        );
    }

    #[test]
    fn a_single_point_has_no_hover_texts() {
        assert_eq!(hover_texts(None, PriceRule::SellFivePercent), (None, None));
    }

    #[test]
    fn the_empty_and_failed_states_have_no_hovers() {
        assert_eq!(Body::Nothing.hovers(), (None, None));
        assert_eq!(Body::Failed.hovers(), (None, None));
        let shown = Lines {
            total: String::new(),
            hover: String::new(),
            change: None,
            change_hover: None,
            split: None,
            split_hover: None,
            unpriced: None,
        };
        assert_eq!(Body::Shown(shown).hovers(), (None, None));
    }

    #[test]
    fn the_change_hover_of_yesterdays_points_names_their_time() {
        let since = since(999_914_400, "804,000.00", "192,000.00", "612,000.00");
        assert_eq!(
            change_hover_text(since, PriceRule::SellFivePercent),
            "804,000.00 ISK at Forge sell, 5% price, since 2001-09-08 02:00"
        );
    }

    #[test]
    fn a_short_date_has_no_leading_zero_and_names_the_month() {
        assert_eq!(date_text(999_395_200), "2 Sep");
        assert_eq!(date_text(1_000_000_000), "9 Sep");
        assert_eq!(date_text(0), "1 Jan");
    }

    #[test]
    fn since_says_yesterday_only_for_the_day_before_the_last() {
        assert_eq!(since_phrase(999_914_400, 11_573, 11_574), "yesterday");
        assert_eq!(since_phrase(999_395_200, 11_567, 11_574), "2 Sep");
        assert_eq!(since_phrase(999_395_200, 11_574, 11_574), "2 Sep");
    }

    #[test]
    fn the_sign_follows_the_change() {
        assert_eq!(sign(isk("804,000.00")), Sign::Up);
        assert_eq!(sign(isk("-804,000.00")), Sign::Down);
        assert_eq!(sign(Money::ZERO), Sign::None);
    }
}

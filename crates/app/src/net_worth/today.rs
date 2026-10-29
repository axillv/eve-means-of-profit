//! The Today page's Net worth block: the latest total, its change since the point before it, and
//! the types the basis could not price. The body is one cached set of strings, so it changes only
//! when the data it describes does.

use std::collections::BTreeSet;

use egui::{Id, RichText, Ui};
use mop_core::money::Money;
use mop_core::net_worth::Part;
use mop_core::price_history::day_of;
use mop_market::book::PriceRule;
use mop_store::StoreError;

use crate::Shared;
use crate::build::inputs::price_text;
use crate::net_worth::Range;
use crate::net_worth::data::load::{Loaded, load};
use crate::net_worth::summary::{Summary, SummaryError, summarise};
use crate::net_worth::view::basis_text;
use crate::orders::rows::utc_text;
use crate::status::Notice;

/// The key of the block's cached lines in the frame memory.
const CACHE_KEY: &str = "today.net_worth";
/// What the block says when there are no points at all.
const NOTHING: &str = "No net worth points yet. Refresh your characters to record the first one.";
/// What the block says when the points could not be read.
const FAILED: &str = "Could not read net worth.";

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
    /// The change split into price moves and everything else, when there is a change to split.
    split: Option<String>,
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

/// Draws the block's body. One load builds every line; later frames read the cache.
pub fn body(ui: &mut Ui, cx: &mut Shared) {
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
    show(ui, cx, &cached.body);
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
    let change = summary.and_then(|summary| summary.since).map(|since| {
        let amount = since.change.amount;
        let phrase = since_phrase(since.from_at, since.from_day, day_of(last.value.at));
        (
            amount.format_isk_short_signed(),
            format!("since {phrase}"),
            sign(amount),
        )
    });
    let split = summary.and_then(|summary| summary.since).map(|since| {
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
        split,
        unpriced,
    })
}

/// Whether a change is a gain, a loss or neither.
fn sign(amount: Money) -> Sign {
    match amount.ten_thousandths() {
        value if value > 0 => Sign::Up,
        value if value < 0 => Sign::Down,
        _ => Sign::None,
    }
}

/// Draws one body.
fn show(ui: &mut Ui, cx: &Shared, body: &Body) {
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
                    ui.label(RichText::new(amount).color(color));
                    ui.add_space(4.0);
                    ui.colored_label(cx.style.mute, since);
                });
            }
            if let Some(split) = &lines.split {
                ui.colored_label(cx.style.mute, split);
            }
            if let Some(unpriced) = &lines.unpriced {
                ui.colored_label(cx.style.stale, unpriced);
            }
        }
    }
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

    use super::{Sign, date_text, sign, since_phrase};

    fn isk(text: &str) -> Money {
        Money::parse_isk(text).expect("a test amount")
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

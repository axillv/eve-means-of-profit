//! The Net worth panel's loaded points: reading the picked range off the UI thread's store and
//! showing the headline numbers.

use std::collections::BTreeSet;

use egui::{RichText, Ui};
use mop_core::industry::TypeId;
use mop_core::price_history::day_of;

use crate::Shared;
use crate::build::inputs::price_text;
use crate::net_worth::Choices;
use crate::net_worth::data::ValuedPoint;
use crate::net_worth::data::load::{Loaded, load};
use crate::net_worth::data::pricer::PriceSource;
use crate::net_worth::summary::{Summary, SummaryError, summarise};
use crate::orders::rows::utc_text;
use crate::status::Notice;

/// How long a loaded range stays fresh while the page keeps being drawn, in seconds.
const REFRESH_SECS: i64 = 60;
/// The size of the headline's total.
const HEADLINE_SIZE: f32 = 24.0;

/// The loaded points of the picked range and the numbers that describe them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shown {
    pub loaded: Loaded,
    pub summary: Option<Summary>,
    pub choices: Choices,
}

/// What the panel has loaded. The page's controls live in [`crate::net_worth::NetWorthPage`];
/// this only remembers the result and when to read it again.
#[derive(Debug, Default)]
pub struct View {
    shown: Option<Shown>,
    /// Set by [`Self::invalidate`]; cleared by the next load.
    invalid: bool,
    /// The frame of the last [`Self::update`] call, to notice a page that was hidden.
    last_frame: Option<u64>,
    /// When the last load ran, in Unix seconds.
    last_loaded: Option<i64>,
    /// The login characters and their revisions at the last load.
    revisions: Vec<(u64, u64)>,
}

impl View {
    /// Reloads the points when the range, the price, the parts, the characters or the clock make
    /// the loaded copy stale. The page may be drawn more than once in a frame, so only the first
    /// call of a frame loads.
    pub fn update(&mut self, cx: &mut Shared, choices: &Choices, frame: u64) {
        let now = (cx.clock)();
        let revisions = login_revisions(cx);
        let hidden = self
            .last_frame
            .is_some_and(|last| frame > last.saturating_add(1));
        let aged = self
            .last_loaded
            .is_none_or(|last| now.saturating_sub(last) >= REFRESH_SECS);
        let due = self.shown.is_none()
            || self.invalid
            || self
                .shown
                .as_ref()
                .is_some_and(|shown| &shown.choices != choices)
            || self.revisions != revisions
            || hidden
            || aged;
        self.last_frame = Some(frame);
        if !due {
            return;
        }

        self.invalid = false;
        self.revisions = revisions;
        self.last_loaded = Some(now);
        let left_out = BTreeSet::new();
        let loaded = load(
            &mut cx.store,
            cx.sde.as_ref(),
            choices.range.from(now),
            now,
            &choices.shown,
            choices.show_plex,
            &left_out,
        );
        match loaded {
            Ok(loaded) => {
                if let Some(error) = &loaded.error {
                    cx.notices.push(Notice::error(error.clone()));
                }
                let summary =
                    match summarise(&loaded.points, loaded.previous_at_latest_prices.as_ref()) {
                        Ok(summary) => summary,
                        Err(SummaryError::Overflow) => {
                            cx.notices.push(Notice::error(
                                "Could not add up net worth: the sum is too large",
                            ));
                            None
                        }
                    };
                self.shown = Some(Shown {
                    loaded,
                    summary,
                    choices: choices.clone(),
                });
            }
            Err(text) => {
                cx.notices.push(Notice::error(text.clone()));
                self.shown = Some(Shown {
                    loaded: Loaded {
                        points: Vec::new(),
                        previous_at_latest_prices: None,
                        error: Some(text),
                    },
                    summary: None,
                    choices: choices.clone(),
                });
            }
        }
    }

    /// Asks the next [`Self::update`] to read the range again, after a record or a price changed.
    pub fn invalidate(&mut self) {
        self.invalid = true;
    }

    /// The loaded points, when the panel has read some.
    pub fn shown(&self) -> Option<&Shown> {
        self.shown.as_ref()
    }
}

/// The login characters with their revisions, in the order the login lists them.
fn login_revisions(cx: &Shared) -> Vec<(u64, u64)> {
    cx.session
        .characters()
        .iter()
        .map(|character| (character.id, cx.session.revision(character.id)))
        .collect()
}

/// Draws the total and the changes of the last point.
pub fn show_headline(ui: &mut Ui, cx: &Shared, view: &View, choices: &Choices) {
    let Some(shown) = view.shown() else {
        return;
    };
    let Some(last) = shown.loaded.points.last() else {
        ui.colored_label(
            cx.style.mute,
            "No net worth points yet. Refresh your characters to record the first one.",
        );
        return;
    };

    let total = format!("Net worth {} ISK", last.total.format_isk_short());
    ui.label(RichText::new(total).size(HEADLINE_SIZE))
        .on_hover_text(format!("{} ISK", last.total.format_isk()));
    ui.colored_label(
        cx.style.mute,
        basis_text(price_text(choices.price), last.source),
    );

    if let Some(summary) = shown.summary {
        if let Some(since) = summary.since {
            let day = since_day(&since, last);
            ui.label(format!(
                "{} since {day}",
                change_text(since.change.amount, since.change.percent)
            ));
            ui.colored_label(
                cx.style.mute,
                format!(
                    "Price moves {}, everything else {}",
                    since.price_moves.format_isk_short_signed(),
                    since.everything_else.format_isk_short_signed(),
                ),
            );
        }
        if let Some(over_range) = summary.over_range {
            ui.label(format!(
                "{} over {}",
                change_text(over_range.change.amount, over_range.change.percent),
                choices.range.over_text(),
            ));
        }
    }

    if !last.unpriced.is_empty() {
        let count = last.unpriced.len();
        let text = format!("{count} item types have no price at this basis and are not counted.");
        let hover = unpriced_hover(cx, &last.unpriced);
        let response = ui.colored_label(cx.style.stale, text);
        if !hover.is_empty() {
            response.on_hover_text(hover);
        }
    }
}

/// A signed change with its percentage in brackets, when the earlier total gave one.
fn change_text(amount: mop_core::money::Money, percent: Option<i64>) -> String {
    match percent {
        Some(percent) => format!(
            "{} ({})",
            amount.format_isk_short_signed(),
            percent_text(percent, true)
        ),
        None => amount.format_isk_short_signed(),
    }
}

/// The day text after "since": `yesterday` when the earlier point is the last one's day before,
/// else the short day.
fn since_day(since: &crate::net_worth::summary::Since, last: &ValuedPoint) -> String {
    if since.from_day == day_of(last.value.at).saturating_sub(1) {
        "yesterday".to_owned()
    } else {
        short_day(since.from_day)
    }
}

/// `hundredths` as a percentage with two decimals, in hundredths of a percent: 7,272 is `72.72%`.
/// `signed` puts a `+` on positive values that are not zero.
pub(super) fn percent_text(hundredths: i64, signed: bool) -> String {
    let sign = if hundredths < 0 {
        "-"
    } else if signed && hundredths > 0 {
        "+"
    } else {
        ""
    };
    let magnitude = hundredths.unsigned_abs();
    format!("{sign}{}.{:02}%", magnitude / 100, magnitude % 100)
}

/// The price source after the rule's name: `Forge sell, 5% price · 9 Sep 01:30` for a copy,
/// `Forge sell, 5% price · 9 Sep` for a history day, else the name alone.
pub(super) fn basis_text(label: &str, source: PriceSource) -> String {
    match source {
        PriceSource::Copy {
            published_at: Some(at),
        } => match short_moment(at) {
            Some((day, time)) => format!("{label} · {day} {time}"),
            None => label.to_owned(),
        },
        PriceSource::History {
            newest_day: Some(day),
        } => format!("{label} · {}", short_day(day)),
        PriceSource::Copy { published_at: None } | PriceSource::History { newest_day: None } => {
            label.to_owned()
        }
    }
}

/// One line per unpriced type: `<name> × <units>`, or `Type <id>` when the game data has no name.
fn unpriced_hover(cx: &Shared, unpriced: &std::collections::BTreeMap<TypeId, u64>) -> String {
    let mut lines = Vec::with_capacity(unpriced.len());
    for (type_id, units) in unpriced {
        let name = cx
            .sde
            .as_ref()
            .and_then(|sde| sde.item(*type_id).ok().flatten())
            .map_or_else(|| format!("Type {type_id}"), |item| item.name);
        lines.push(format!("{name} \u{d7} {units}"));
    }
    lines.join("\n")
}

/// A day number as `9 Sep`.
fn short_day(day: i64) -> String {
    short_moment(day.saturating_mul(86_400)).map_or_else(|| day.to_string(), |(date, _time)| date)
}

/// UTC seconds as `9 Sep 01:30` and its date alone; the year is left out.
fn short_moment(seconds: i64) -> Option<(String, String)> {
    let text = utc_text(seconds);
    let (date, time) = text.split_once(' ')?;
    let mut parts = date.split('-');
    parts.next()?;
    let month = month_name(parts.next()?)?;
    let day: i64 = parts.next()?.parse().ok()?;
    Some((format!("{day} {month}"), time.to_owned()))
}

/// The EVE style month name of a two-digit month, or `None` when the number is not a month.
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
    use super::*;

    #[test]
    fn percent_text_keeps_two_decimals_and_signs_only_when_asked() {
        assert_eq!(percent_text(-906, true), "-9.06%");
        assert_eq!(percent_text(0, true), "0.00%");
        assert_eq!(percent_text(5, false), "0.05%");
        assert_eq!(percent_text(7_272, true), "+72.72%");
        assert_eq!(percent_text(-1, true), "-0.01%");
        assert_eq!(percent_text(758, true), "+7.58%");
    }

    #[test]
    fn basis_text_names_the_copy_time_or_the_history_day() {
        let label = "Forge sell, 5% price";
        assert_eq!(
            basis_text(
                label,
                PriceSource::Copy {
                    published_at: Some(999_999_000)
                }
            ),
            "Forge sell, 5% price · 9 Sep 01:30"
        );
        assert_eq!(
            basis_text(
                label,
                PriceSource::History {
                    newest_day: Some(11_567)
                }
            ),
            "Forge sell, 5% price · 2 Sep"
        );
        assert_eq!(
            basis_text(label, PriceSource::Copy { published_at: None }),
            label
        );
        assert_eq!(
            basis_text(label, PriceSource::History { newest_day: None }),
            label
        );
    }

    #[test]
    fn a_moment_before_1970_still_has_a_date() {
        assert_eq!(
            short_moment(0),
            Some(("1 Jan".to_owned(), "00:00".to_owned()))
        );
    }

    #[test]
    fn a_fresh_view_has_nothing_and_invalidate_asks_for_a_read() {
        let mut view = View::default();
        assert!(view.shown().is_none());
        view.invalidate();
        assert!(view.invalid);
    }
}

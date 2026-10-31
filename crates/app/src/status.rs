//! The status line at the bottom of the window: the activity or idle text, then the notices.
//!
//! Activity (a background update) shows first. Otherwise the first notice shows, with
//! ` (1 of <n>)` when there are several. With no notice, the idle text shows, followed by each
//! login character's login state or data age.

use egui::{
    Align, Color32, CornerRadius, FontId, Layout, Response, RichText, Sense, Stroke, StrokeKind,
    Ui, WidgetInfo, WidgetType, vec2,
};
use mop_esi::character_data::CharacterData;
use mop_esi::characters::Login;

use crate::build::prices::age_text;
use crate::session::Session;
use crate::theme::Style;

pub const IDLE_TEXT: &str = "No market data yet";
/// Longer notices are cut to this many characters; the full text shows on hover.
const CUT_AT: usize = 120;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Notice,
    Error,
}

/// A button on the status line. A notice's own button also removes the notice; `Refresh all` does
/// not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Downloads and imports build `build` in the background.
    Update { build: u64 },
    /// Refreshes every login character and The Forge prices.
    RefreshAll,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub level: Level,
    pub text: String,
    pub action: Option<Action>,
}

impl Notice {
    pub fn info(text: impl Into<String>) -> Self {
        Self {
            level: Level::Notice,
            text: text.into(),
            action: None,
        }
    }

    pub fn error(text: impl Into<String>) -> Self {
        Self {
            level: Level::Error,
            text: text.into(),
            action: None,
        }
    }

    pub fn with_action(mut self, action: Action) -> Self {
        self.action = Some(action);
        self
    }
}

/// Draws the status line. Returns the action whose button was pressed, if any.
pub fn show(
    ui: &mut Ui,
    style: &Style,
    notices: &mut Vec<Notice>,
    activity: Option<&str>,
    idle: &str,
    characters: &str,
) -> Option<Action> {
    let mut action = None;
    let mut notice_action = false;
    let mut dismiss = false;
    let bar = egui::Panel::bottom("status")
        .exact_size(style.sizes.status_height)
        .resizable(false)
        .show_separator_line(false)
        .frame(bar_frame(style))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 18.0;
            ui.horizontal(|ui| {
                if let Some(text) = activity {
                    ui.label(bar_text(text, style.mute));
                } else if let Some(first) = notices.first() {
                    let (label, color) = match first.level {
                        Level::Notice => ("NOTICE", style.mute),
                        Level::Error => ("ERROR", style.neg),
                    };
                    ui.label(bar_text(label, color));
                    let count = if notices.len() > 1 {
                        format!(" (1 of {})", notices.len())
                    } else {
                        String::new()
                    };
                    ui.label(bar_text(format!("{}{count}", cut(&first.text)), style.mute))
                        .on_hover_text(first.text.as_str());
                    if let Some(pending) = first.action
                        && bar_button(ui, style, button_text(pending)).clicked()
                    {
                        action = Some(pending);
                        notice_action = true;
                    }
                    if bar_button(ui, style, "Dismiss").clicked() {
                        dismiss = true;
                    }
                } else {
                    ui.label(bar_text(idle, style.mute));
                    if !characters.is_empty() {
                        ui.label(bar_text("·", style.mute));
                        ui.label(bar_text(characters, style.mute));
                    }
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if bar_button(ui, style, "Refresh all").clicked() {
                        action = Some(Action::RefreshAll);
                    }
                });
            });
        });
    ui.painter().hline(
        bar.response.rect.x_range(),
        bar.response.rect.top(),
        Stroke::new(1.0, style.line),
    );
    if (dismiss || notice_action) && !notices.is_empty() {
        notices.remove(0);
    }
    action
}

/// The login characters' line: `<name>: login expired` for an expired login, otherwise
/// `<name>: <age>` when its data holds any kind. Empty when there is nothing to say.
pub(crate) fn characters_text(session: &Session, now: i64) -> String {
    session
        .characters()
        .iter()
        .filter_map(|character| match character.login {
            Login::Expired => Some(format!("{}: login expired", character.name)),
            Login::Ok => session
                .character_data(character.id)
                .and_then(oldest_of)
                .map(|fetched_at| {
                    format!(
                        "{}: {}",
                        character.name,
                        age_text(now.saturating_sub(fetched_at))
                    )
                }),
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

/// The oldest fetch time among one character's kinds of data.
fn oldest_of(held: &CharacterData) -> Option<i64> {
    let assets = held.assets.as_ref().map(|fetched| fetched.fetched_at);
    let orders = held.orders.as_ref().map(|fetched| fetched.fetched_at);
    let jobs = held.jobs.as_ref().map(|fetched| fetched.fetched_at);
    let wallet = held.wallet.as_ref().map(|fetched| fetched.fetched_at);
    [assets, orders, jobs, wallet].into_iter().flatten().min()
}

/// The bar's frame: the panel colour, 12 px at the sides, nothing above or below.
fn bar_frame(style: &Style) -> egui::Frame {
    egui::Frame::new()
        .fill(style.panel)
        .inner_margin(egui::Margin {
            left: 12,
            right: 12,
            top: 0,
            bottom: 0,
        })
}

/// One piece of bar text, 13 px in `color`.
fn bar_text(text: impl Into<String>, color: Color32) -> RichText {
    RichText::new(text).size(13.0).color(color)
}

/// One bar button: 22 px tall, 10 px side padding, a 6 px radius, no fill, the line's outline and
/// the text colour. Drawn in a scope so the bar's spacing stays put.
fn bar_button(ui: &mut Ui, style: &Style, text: &str) -> Response {
    ui.scope(|ui| {
        let galley =
            ui.painter()
                .layout_no_wrap(text.to_owned(), FontId::proportional(13.0), style.text);
        let size = vec2(galley.size().x + 2.0 * 10.0, 22.0);
        let (rect, response) = ui.allocate_exact_size(size, Sense::click());
        ui.painter().rect(
            rect,
            CornerRadius::same(6),
            Color32::TRANSPARENT,
            Stroke::new(1.0, style.line),
            StrokeKind::Inside,
        );
        ui.painter()
            .galley(rect.center() - galley.size() * 0.5, galley, style.text);
        response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), text));
        response
    })
    .inner
}

fn button_text(action: Action) -> &'static str {
    match action {
        Action::Update { .. } => "Update",
        Action::RefreshAll => "Refresh all",
    }
}

/// The text cut to [`CUT_AT`] characters, with `…` when it was cut.
fn cut(text: &str) -> String {
    if text.chars().count() <= CUT_AT {
        return text.to_owned();
    }
    let head: String = text.chars().take(CUT_AT).collect();
    format!("{head}…")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use mop_core::money::Money;
    use mop_esi::character_data::{CharacterData, Fetched};
    use mop_esi::characters::{Character, Login};
    use mop_esi::places::StructureNames;

    use egui::Margin;
    use egui::vec2;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use super::{Action, IDLE_TEXT, Notice, bar_frame, characters_text, show};
    use crate::session::eve::NoEve;
    use crate::session::{Saved, Session};
    use crate::theme::Style;

    const NOW: i64 = 1_000_000;

    fn character(id: u64, name: &str, login: Login) -> Character {
        Character {
            id,
            name: name.to_owned(),
            scopes: BTreeSet::new(),
            added_at: 0,
            login,
        }
    }

    fn fetched<T>(data: T, fetched_at: i64) -> Fetched<T> {
        Fetched {
            data,
            fetched_at,
            last_modified: None,
            expires: None,
        }
    }

    fn wallet_data(character_id: u64, fetched_at: i64) -> CharacterData {
        let mut data = CharacterData::new(character_id);
        data.wallet = Some(fetched(Money::ZERO, fetched_at));
        data
    }

    fn session(characters: Vec<Character>, data: Vec<CharacterData>) -> Session {
        let mut session = Session::new(Box::new(NoEve));
        session.set_saved(Saved {
            characters,
            data,
            structures: StructureNames::new(),
        });
        session
    }

    #[test]
    fn an_ok_login_shows_its_age_and_an_expired_one_says_so() {
        let session = session(
            vec![
                character(1, "Main", Login::Ok),
                character(2, "Alt", Login::Expired),
            ],
            vec![wallet_data(1, NOW - 7_200)],
        );
        assert_eq!(
            characters_text(&session, NOW),
            "Main: 2h · Alt: login expired"
        );
    }

    #[test]
    fn the_oldest_kind_decides_the_age() {
        let mut data = wallet_data(1, NOW - 10_800);
        data.orders = Some(fetched(Vec::new(), NOW - 3_600));
        let session = session(vec![character(1, "Main", Login::Ok)], vec![data]);
        assert_eq!(characters_text(&session, NOW), "Main: 3h");
    }

    #[test]
    fn an_expired_login_without_data_says_so_and_an_ok_one_without_data_is_left_out() {
        let session = session(
            vec![
                character(1, "Alt", Login::Expired),
                character(2, "Third", Login::Ok),
            ],
            Vec::new(),
        );
        assert_eq!(characters_text(&session, NOW), "Alt: login expired");
    }

    #[test]
    fn no_characters_says_nothing_and_a_future_fetch_is_zero() {
        assert_eq!(characters_text(&session(Vec::new(), Vec::new()), NOW), "");
        let session = session(
            vec![character(1, "Main", Login::Ok)],
            vec![wallet_data(1, NOW + 60)],
        );
        assert_eq!(characters_text(&session, NOW), "Main: 0s");
    }

    /// The bar's state: its notices and the action the last frame returned.
    struct Bar {
        notices: Vec<Notice>,
        action: Option<Action>,
        bottom: f32,
    }

    fn bar(notices: Vec<Notice>) -> Harness<'static, Bar> {
        let style = Style::eve_dense();
        let state = Bar {
            notices,
            action: None,
            bottom: 0.0,
        };
        Harness::builder()
            .with_size(vec2(800.0, 200.0))
            .build_ui_state(
                move |ui, bar: &mut Bar| {
                    style.apply(ui.ctx());
                    bar.bottom = ui.max_rect().bottom();
                    let Bar {
                        notices, action, ..
                    } = bar;
                    if let Some(next) = show(ui, &style, notices, None, IDLE_TEXT, "") {
                        *action = Some(next);
                    }
                },
                state,
            )
    }

    fn click(harness: &mut Harness<'_, Bar>, label: &str) {
        harness.get_by_label(label).click();
        harness.run_steps(3);
    }

    #[test]
    fn the_bar_frame_is_the_panel_with_12_px_sides() {
        let style = Style::eve_dense();
        let frame = bar_frame(&style);
        assert_eq!(frame.fill, style.panel);
        assert_eq!(
            frame.inner_margin,
            Margin {
                left: 12,
                right: 12,
                top: 0,
                bottom: 0,
            }
        );
    }

    #[test]
    fn the_refresh_button_is_compact_inside_the_bar_and_refreshes() {
        let size = Style::eve_dense().sizes.status_height;
        let mut harness = bar(Vec::new());
        harness.run_steps(2);
        let rect = harness.get_by_label("Refresh all").rect();
        assert_eq!(rect.height(), 22.0);
        let bottom = harness.state().bottom;
        assert!(rect.top() >= bottom - size, "{rect:?} below {bottom}");
        assert!(rect.bottom() <= bottom, "{rect:?} below {bottom}");
        click(&mut harness, "Refresh all");
        assert_eq!(harness.state().action, Some(Action::RefreshAll));
    }

    #[test]
    fn an_error_shows_its_level_and_text_and_dismiss_removes_it() {
        let mut harness = bar(vec![Notice::error("Prices could not be read")]);
        harness.run_steps(2);
        assert!(harness.query_by_label("ERROR").is_some());
        assert!(harness.query_by_label("Prices could not be read").is_some());
        click(&mut harness, "Dismiss");
        assert!(harness.state().notices.is_empty());
    }
}
